use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Result, anyhow};
use rust_protocol::{
    RequestId, TranslationFailureKind, TranslationFailureReport, TranslationProviderEntry,
    TranslationProviderStatus, TranslationWorkerEvent,
};
use rust_translation_engine::TranslationCancelled;

use crate::{
    Asked, EventWriter, WorkerState,
    providers::{Providers, ProvidersSnapshot},
    recover_lock, translate_request,
};

struct Job {
    state: Arc<Mutex<WorkerState>>,
    providers: Option<ProvidersSnapshot>,
    writer: EventWriter,
    asked: Asked,
    generation: u64,
}

struct QueueState<T> {
    pending: Option<T>,
    shutdown: bool,
    #[cfg(test)]
    blocker: Option<(SyncSender<()>, Receiver<()>)>,
}

impl<T> Default for QueueState<T> {
    fn default() -> Self {
        Self {
            pending: None,
            shutdown: false,
            #[cfg(test)]
            blocker: None,
        }
    }
}

impl<T> QueueState<T> {
    fn replace_pending(&mut self, job: T) -> Option<T> {
        self.pending.replace(job)
    }

    fn take_pending(&mut self) -> Option<T> {
        self.pending.take()
    }
}

enum ProviderCommand {
    Translate {
        state: Arc<Mutex<WorkerState>>,
        writer: EventWriter,
        asked: Asked,
        generation: u64,
    },
    Cancel(RequestId),
    CancelAll,
    SetEntries {
        entries: Vec<TranslationProviderEntry>,
        reply: SyncSender<Vec<TranslationProviderStatus>>,
    },
    SetCredential {
        provider_id: String,
        secret: Option<String>,
        reply: SyncSender<(Result<(), String>, Vec<TranslationProviderStatus>)>,
    },
    Statuses {
        reply: SyncSender<(Option<String>, Vec<TranslationProviderStatus>)>,
    },
    Adopt(ProvidersSnapshot),
    #[cfg(test)]
    BlockNetwork {
        entered: SyncSender<()>,
        release: Receiver<()>,
    },
    Shutdown,
}

/// Owns every mutable part of the provider configuration on one control actor.
/// A second, fixed thread executes at most one immutable request snapshot at a
/// time. Consequently credential/status commands never wait for network I/O,
/// no request creates an OS thread, and user work remains bounded to one active
/// request plus one replaceable pending request.
pub(crate) struct ProviderActorHandle {
    generation: Arc<AtomicU64>,
    current_request: Arc<Mutex<Option<RequestId>>>,
    commands: SyncSender<ProviderCommand>,
    actor_thread: Option<JoinHandle<()>>,
    network_thread: Option<JoinHandle<()>>,
    actor_finished: Receiver<()>,
    network_finished: Receiver<()>,
}

impl ProviderActorHandle {
    pub(crate) fn start(providers: Providers) -> Result<Self> {
        let generation = Arc::new(AtomicU64::new(0));
        let current_request = Arc::new(Mutex::new(None));
        let queue = Arc::new((Mutex::new(QueueState::default()), Condvar::new()));
        let (commands, command_receiver) = mpsc::sync_channel(64);
        let (actor_finished_sender, actor_finished) = mpsc::sync_channel(1);
        let (network_finished_sender, network_finished) = mpsc::sync_channel(1);

        let actor_queue = Arc::clone(&queue);
        let actor_thread = thread::Builder::new()
            .name("translation-provider-control".to_owned())
            .spawn(move || {
                control_loop(providers, command_receiver, actor_queue);
                let _ = actor_finished_sender.send(());
            })?;

        let worker_generation = Arc::clone(&generation);
        let worker_current = Arc::clone(&current_request);
        let worker_queue = Arc::clone(&queue);
        let worker_commands = commands.clone();
        let network_thread = thread::Builder::new()
            .name("translation-provider-network".to_owned())
            .spawn(move || {
                worker_loop(
                    worker_queue,
                    worker_generation,
                    worker_current,
                    worker_commands,
                );
                let _ = network_finished_sender.send(());
            })?;

        Ok(Self {
            generation,
            current_request,
            commands,
            actor_thread: Some(actor_thread),
            network_thread: Some(network_thread),
            actor_finished,
            network_finished,
        })
    }

    pub(crate) fn submit(
        &self,
        state: Arc<Mutex<WorkerState>>,
        writer: EventWriter,
        asked: Asked,
    ) -> Result<()> {
        let request_id = asked.request_id();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *recover_lock(&self.current_request) = Some(request_id);
        self.commands
            .send(ProviderCommand::Translate {
                state,
                writer,
                asked,
                generation,
            })
            .map_err(|_| anyhow!("translation provider actor is shutting down"))
    }

    pub(crate) fn set_entries(
        &self,
        entries: &[TranslationProviderEntry],
    ) -> Vec<TranslationProviderStatus> {
        let (reply, answer) = mpsc::sync_channel(1);
        if self
            .commands
            .send(ProviderCommand::SetEntries {
                entries: entries.to_vec(),
                reply,
            })
            .is_err()
        {
            return Vec::new();
        }
        answer.recv().unwrap_or_default()
    }

    pub(crate) fn set_credential(
        &self,
        provider_id: &str,
        secret: Option<&str>,
    ) -> (Result<(), String>, Vec<TranslationProviderStatus>) {
        let (reply, answer) = mpsc::sync_channel(1);
        if self
            .commands
            .send(ProviderCommand::SetCredential {
                provider_id: provider_id.to_owned(),
                secret: secret.map(str::to_owned),
                reply,
            })
            .is_err()
        {
            return (
                Err("translation provider actor is shutting down".to_owned()),
                Vec::new(),
            );
        }
        answer.recv().unwrap_or_else(|_| {
            (
                Err("translation provider actor did not answer".to_owned()),
                Vec::new(),
            )
        })
    }

    pub(crate) fn statuses(&self) -> (Option<String>, Vec<TranslationProviderStatus>) {
        let (reply, answer) = mpsc::sync_channel(1);
        if self
            .commands
            .send(ProviderCommand::Statuses { reply })
            .is_err()
        {
            return (
                Some("translation provider actor is shutting down".to_owned()),
                Vec::new(),
            );
        }
        answer.recv().unwrap_or_else(|_| {
            (
                Some("translation provider actor did not answer".to_owned()),
                Vec::new(),
            )
        })
    }

    pub(crate) fn cancel(&self, request_id: RequestId) {
        if *recover_lock(&self.current_request) != Some(request_id) {
            return;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(ProviderCommand::Cancel(request_id));
    }

    pub(crate) fn cancel_all(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(ProviderCommand::CancelAll);
    }

    /// Requests shutdown and waits only for a short cooperative window. A
    /// blocking HTTP call cannot be interrupted safely by std/reqwest; if it
    /// does not return, dropping its JoinHandle deliberately detaches it and
    /// process teardown terminates it instead of hanging shutdown.
    pub(crate) fn shutdown(mut self) -> bool {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(ProviderCommand::Shutdown);
        let actor_stopped = join_with_deadline(
            self.actor_thread.take(),
            &self.actor_finished,
            Duration::from_millis(250),
        );
        let network_stopped = join_with_deadline(
            self.network_thread.take(),
            &self.network_finished,
            Duration::from_millis(250),
        );
        actor_stopped && network_stopped
    }
}

fn control_loop(
    mut providers: Providers,
    commands: Receiver<ProviderCommand>,
    queue: Arc<(Mutex<QueueState<Job>>, Condvar)>,
) {
    while let Ok(command) = commands.recv() {
        match command {
            ProviderCommand::Translate {
                state,
                writer,
                asked,
                generation,
            } => {
                let provider_snapshot = providers.routing_required().then(|| providers.snapshot());
                let (lock, wake) = &*queue;
                let mut queue = recover_lock(lock);
                if queue.shutdown {
                    let _ = writer.send(&TranslationWorkerEvent::Cancelled {
                        request_id: asked.request_id(),
                    });
                    continue;
                }
                if let Some(replaced) = queue.replace_pending(Job {
                    state,
                    providers: provider_snapshot,
                    writer: writer.clone(),
                    asked,
                    generation,
                }) {
                    let _ = replaced.writer.send(&TranslationWorkerEvent::Cancelled {
                        request_id: replaced.asked.request_id(),
                    });
                }
                wake.notify_one();
            }
            ProviderCommand::Cancel(request_id) => {
                let (lock, _) = &*queue;
                let mut queue = recover_lock(lock);
                if queue
                    .pending
                    .as_ref()
                    .is_some_and(|job| job.asked.request_id() == request_id)
                    && let Some(job) = queue.take_pending()
                {
                    let _ = job
                        .writer
                        .send(&TranslationWorkerEvent::Cancelled { request_id });
                }
            }
            ProviderCommand::CancelAll => {
                let (lock, _) = &*queue;
                if let Some(job) = recover_lock(lock).take_pending() {
                    let _ = job.writer.send(&TranslationWorkerEvent::Cancelled {
                        request_id: job.asked.request_id(),
                    });
                }
            }
            ProviderCommand::SetEntries { entries, reply } => {
                providers.set_entries(&entries);
                let _ = reply.send(providers.statuses());
            }
            ProviderCommand::SetCredential {
                provider_id,
                secret,
                reply,
            } => {
                let outcome = providers.set_credential(&provider_id, secret.as_deref());
                let statuses = providers.statuses();
                let _ = reply.send((outcome, statuses));
            }
            ProviderCommand::Statuses { reply } => {
                let recovery = providers.recovery_message();
                let _ = reply.send((recovery, providers.statuses()));
            }
            ProviderCommand::Adopt(snapshot) => providers.adopt(snapshot),
            #[cfg(test)]
            ProviderCommand::BlockNetwork { entered, release } => {
                let (lock, wake) = &*queue;
                recover_lock(lock).blocker = Some((entered, release));
                wake.notify_one();
            }
            ProviderCommand::Shutdown => {
                let (lock, wake) = &*queue;
                let mut queue = recover_lock(lock);
                queue.shutdown = true;
                if let Some(job) = queue.take_pending() {
                    let _ = job.writer.send(&TranslationWorkerEvent::Cancelled {
                        request_id: job.asked.request_id(),
                    });
                }
                wake.notify_one();
                return;
            }
        }
    }
}

fn worker_loop(
    queue: Arc<(Mutex<QueueState<Job>>, Condvar)>,
    generation: Arc<AtomicU64>,
    current_request: Arc<Mutex<Option<RequestId>>>,
    commands: SyncSender<ProviderCommand>,
) {
    loop {
        let job = {
            let (lock, wake) = &*queue;
            let mut state = recover_lock(lock);
            while state.pending.is_none() && !state.shutdown && {
                #[cfg(test)]
                {
                    state.blocker.is_none()
                }
                #[cfg(not(test))]
                {
                    true
                }
            } {
                state = wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.shutdown {
                return;
            }
            #[cfg(test)]
            if let Some((entered, release)) = state.blocker.take() {
                drop(state);
                let _ = entered.send(());
                let _ = release.recv();
                continue;
            }
            state.take_pending().expect("pending job was checked")
        };
        run_job(job, &generation, &current_request, &commands);
    }
}

fn run_job(
    job: Job,
    generation: &AtomicU64,
    current_request: &Mutex<Option<RequestId>>,
    commands: &SyncSender<ProviderCommand>,
) {
    let Job {
        state,
        mut providers,
        writer,
        asked,
        generation: job_generation,
    } = job;
    let request_id = asked.request_id();
    let result = translate_request(&state, providers.as_mut(), &writer, asked, || {
        generation_is_current(generation, job_generation)
    });
    if let Some(snapshot) = providers {
        let _ = commands.send(ProviderCommand::Adopt(snapshot));
    }
    let cancelled = !generation_is_current(generation, job_generation)
        || result
            .as_ref()
            .err()
            .is_some_and(|error| error.downcast_ref::<TranslationCancelled>().is_some());
    if cancelled {
        let _ = writer.send(&TranslationWorkerEvent::Cancelled { request_id });
    } else if let Err(error) = result {
        let failure = error
            .downcast_ref::<super::TranslationRequestFailure>()
            .map(|failure| failure.0.clone())
            .unwrap_or_else(|| {
                let has_model = recover_lock(&state).selected_tier.is_some();
                TranslationFailureReport {
                    kind: if has_model {
                        TranslationFailureKind::Internal
                    } else {
                        TranslationFailureKind::LocalModelMissing
                    },
                    provider_id: None,
                    retry_after_ms: None,
                    required_catalog_id: None,
                    detail: format!("{error:#}"),
                    attempts: Vec::new(),
                }
            });
        let _ = writer.send(&TranslationWorkerEvent::RequestFailed {
            request_id,
            failure,
        });
    }
    let mut active = recover_lock(current_request);
    if *active == Some(request_id) {
        *active = None;
    }
}

fn join_with_deadline(
    thread: Option<JoinHandle<()>>,
    finished: &Receiver<()>,
    timeout: Duration,
) -> bool {
    let Some(thread) = thread else {
        return true;
    };
    if finished.recv_timeout(timeout).is_err() {
        drop(thread);
        return false;
    }
    let _ = thread.join();
    true
}

fn generation_is_current(generation: &AtomicU64, job_generation: u64) -> bool {
    generation.load(Ordering::Acquire) == job_generation
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::{
        ProviderActorHandle, ProviderCommand, QueueState, generation_is_current, join_with_deadline,
    };
    use crate::providers::Providers;

    #[test]
    fn the_request_queue_has_exactly_one_pending_slot() {
        let mut queue = QueueState::<u64>::default();

        assert_eq!(queue.replace_pending(1), None);
        assert_eq!(queue.replace_pending(2), Some(1));
        assert_eq!(queue.replace_pending(3), Some(2));
        assert_eq!(queue.take_pending(), Some(3));
        assert_eq!(queue.take_pending(), None);
        assert!(!queue.shutdown);
    }

    #[test]
    fn a_superseded_generation_can_never_publish_as_current() {
        let generation = AtomicU64::new(7);
        assert!(generation_is_current(&generation, 7));

        generation.fetch_add(1, Ordering::AcqRel);

        assert!(!generation_is_current(&generation, 7));
        assert!(generation_is_current(&generation, 8));
    }

    #[test]
    fn provider_control_answers_while_the_network_thread_is_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let actor = ProviderActorHandle::start(Providers::new(
            directory.path().join("translation-credentials.dat"),
        ))
        .unwrap();
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release, release_receiver) = mpsc::sync_channel(1);
        actor
            .commands
            .send(ProviderCommand::BlockNetwork {
                entered: entered_sender,
                release: release_receiver,
            })
            .unwrap();
        entered
            .recv_timeout(Duration::from_secs(1))
            .expect("network thread entered the blocked operation");

        let started = Instant::now();
        let (_, statuses) = actor.statuses();
        assert!(!statuses.is_empty());
        assert!(started.elapsed() < Duration::from_millis(100));

        let started = Instant::now();
        let (stored, statuses) = actor.set_credential("deepl", Some("test-key-123456789"));
        assert!(stored.is_ok());
        assert!(
            statuses
                .iter()
                .any(|status| status.id == "deepl" && status.has_credential)
        );
        assert!(started.elapsed() < Duration::from_millis(100));

        release.send(()).unwrap();
        assert!(actor.shutdown());
    }

    #[test]
    fn shutdown_does_not_wait_for_a_blocked_network_thread() {
        let (_finished_sender, finished) = mpsc::sync_channel(1);
        let worker = thread::spawn(|| thread::sleep(Duration::from_secs(2)));
        let started = Instant::now();

        assert!(!join_with_deadline(
            Some(worker),
            &finished,
            Duration::from_millis(20)
        ));
        assert!(started.elapsed() < Duration::from_millis(150));
    }

    #[test]
    fn the_real_actor_has_a_bounded_shutdown_while_network_work_is_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let actor = ProviderActorHandle::start(Providers::new(
            directory.path().join("translation-credentials.dat"),
        ))
        .unwrap();
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release, release_receiver) = mpsc::sync_channel(1);
        actor
            .commands
            .send(ProviderCommand::BlockNetwork {
                entered: entered_sender,
                release: release_receiver,
            })
            .unwrap();
        entered
            .recv_timeout(Duration::from_secs(1))
            .expect("network thread entered the blocked operation");

        let started = Instant::now();
        assert!(
            !actor.shutdown(),
            "the detached network operation is still blocked"
        );
        assert!(started.elapsed() < Duration::from_millis(600));

        // Let the detached test thread observe the shutdown flag and exit. A
        // real process teardown does the equivalent for an uninterruptible
        // blocking HTTP call.
        let _ = release.send(());
    }
}
