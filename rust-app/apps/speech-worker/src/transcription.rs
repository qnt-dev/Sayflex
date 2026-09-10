//! Routing a finished recording to a transcription service.
//!
//! The user builds an ordered list in Settings; this actor walks it. Reaching
//! the local engine, or running out of services, hands the recording to the
//! inference actor exactly as before — so a list where the local engine is
//! first, which is what a fresh installation has, behaves identically to the
//! application before any of this existed.
//!
//! It runs on its own thread for the same reason installation does: a network
//! request takes seconds, and the worker must keep reading commands throughout.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use rust_app_core::credentials::CredentialStore;
use rust_cloud_speech::{
    Chain, HttpExecutor, LanguageHint, ProviderEntry, ProviderId, RouteOutcome, Router,
    TranscriptionRequest, Unavailable,
    audio::{UPLOAD_SAMPLE_RATE_HZ, encode_wav},
    http::DEFAULT_TIMEOUT,
    router::CancelSignal,
};
use rust_platform_windows::credentials::DpapiCipher;
use rust_protocol::{
    DictationLanguage, RequestId, SessionId, SpeechOutputMode, SpeechWorkerEvent, Transcript,
    TranscriptionLatency, TranscriptionProviderEntry, TranscriptionProviderStatus,
};

use crate::transcription_mailbox::{
    MailboxItem, SessionCancellationSet, TranscriptionMailbox, TranscriptionMailboxReceiver,
};
use crate::{audio::RecordedAudio, history_actor::HistoryCommand, inference::InferenceHandle};

pub enum TranscriptionCommand {
    /// Replaces the ordered list.
    SetProviders(Vec<TranscriptionProviderEntry>),
    /// Stores or clears one key.
    SetCredential {
        /// Named so the window that is waiting can recognise the answer. The
        /// write happens here, on another thread in another process, long
        /// after the command was put in the pipe.
        operation_id: RequestId,
        provider_id: String,
        secret: Option<String>,
    },
    /// Asks for the current list and its availability.
    ReportStatus,
    /// Replaces the explicit language hint. `Auto` becomes no hint at all.
    SetLanguage(DictationLanguage),
    /// A finished recording to transcribe.
    Finalize {
        recording: Box<RecordedAudio>,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct TranscriptionActor {
    commands: TranscriptionMailbox<TranscriptionCommand, TranscriptionCommand>,
    cancelled: SessionCancellationSet,
    events: Sender<SpeechWorkerEvent>,
    /// Read on the command loop for every recording, so the local path costs
    /// nothing extra: no audio is encoded and no thread is crossed unless a
    /// service is actually ahead of the local engine.
    prefers_cloud: Arc<AtomicBool>,
}

impl TranscriptionActor {
    pub fn send(&self, command: TranscriptionCommand) {
        if matches!(command, TranscriptionCommand::Finalize { .. }) {
            if let Some(TranscriptionCommand::Finalize { recording, .. }) =
                self.commands.send_latest(command)
            {
                let session_id = recording.session_id;
                self.cancelled.cancel(session_id);
                let _ = self
                    .events
                    .send(SpeechWorkerEvent::Cancelled { session_id });
                self.cancelled.complete(session_id);
            }
        } else if self.commands.send_control(command).is_err() {
            let _ = self.events.send(SpeechWorkerEvent::Failed {
                session_id: None,
                code: "transcription_control_overload".to_owned(),
                message: "The transcription control queue is full.".to_owned(),
                retryable: true,
            });
        }
    }

    pub fn prefers_cloud(&self) -> bool {
        self.prefers_cloud.load(Ordering::Acquire)
    }

    /// Tells the routing thread that this dictation no longer has a waiting
    /// user. Safe to call for a session it never saw.
    pub fn cancel(&self, session_id: SessionId) {
        self.cancelled.cancel(session_id);
    }
}

pub fn start(
    credentials_path: PathBuf,
    events: Sender<SpeechWorkerEvent>,
    history: Sender<HistoryCommand>,
    inference: InferenceHandle,
) -> Result<TranscriptionActor> {
    let (commands, requests) = TranscriptionMailbox::bounded(32);
    let prefers_cloud = Arc::new(AtomicBool::new(false));
    let worker_prefers_cloud = Arc::clone(&prefers_cloud);
    let cancelled = SessionCancellationSet::default();
    let worker_cancelled = cancelled.clone();
    let actor_events = events.clone();
    let network = NetworkWorker::start()?;
    let (network_result_sender, network_results) = mpsc::channel();
    thread::Builder::new()
        .name("speech-transcription-router".to_owned())
        .spawn(move || {
            let mut state = State {
                router: Router::new(Chain::default_order()),
                credentials: CredentialStore::open(credentials_path, DpapiCipher),
                executor: HttpExecutor::new(DEFAULT_TIMEOUT).ok(),
                events,
                history,
                inference,
                prefers_cloud: worker_prefers_cloud,
                cancelled: worker_cancelled,
                language: DictationLanguage::Auto,
                router_revision: 0,
                network,
                network_results,
                network_result_sender,
            };
            if let Some(message) = state.credentials.take_recovery_message() {
                let _ = state.events.send(SpeechWorkerEvent::Failed {
                    session_id: None,
                    code: "credentials_unreadable".to_owned(),
                    message,
                    retryable: false,
                });
            }
            while let Some(item) = requests.recv_next(Duration::from_millis(25)) {
                let command = match item {
                    MailboxItem::Control(command) | MailboxItem::Work(command) => command,
                };
                if matches!(command, TranscriptionCommand::Shutdown) {
                    break;
                }
                state.handle(command);
            }
            state.shutdown();
        })?;
    Ok(TranscriptionActor {
        commands,
        prefers_cloud,
        cancelled,
        events: actor_events,
    })
}

fn reported_language(fallback: Option<&LanguageHint>) -> String {
    fallback.map_or_else(|| "auto".to_owned(), |hint| hint.primary().to_owned())
}

struct NetworkLedger {
    revision: u64,
    ledger: rust_cloud_speech::chain::Ledger,
}

enum NetworkControl {
    Shutdown,
}

struct NetworkJob {
    router: Router,
    revision: u64,
    secrets: HashMap<String, String>,
    executor: Option<HttpExecutor>,
    recording: RecordedAudio,
    output_mode: SpeechOutputMode,
    audio_finalize_ms: u64,
    release_started: Instant,
    language: DictationLanguage,
    events: Sender<SpeechWorkerEvent>,
    history: Sender<HistoryCommand>,
    inference: InferenceHandle,
    cancelled: SessionCancellationSet,
    results: Sender<NetworkLedger>,
}

struct NetworkWorker {
    mailbox: TranscriptionMailbox<NetworkControl, NetworkJob>,
    thread: Option<JoinHandle<()>>,
    finished: Receiver<()>,
}

impl NetworkWorker {
    fn start() -> Result<Self> {
        let (mailbox, receiver) = TranscriptionMailbox::bounded(1);
        let (finished_sender, finished) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("speech-provider-network".to_owned())
            .spawn(move || {
                network_loop(receiver);
                let _ = finished_sender.send(());
            })?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
            finished,
        })
    }

    fn submit(&self, job: NetworkJob) -> Option<NetworkJob> {
        self.mailbox.send_latest(job)
    }

    fn shutdown(&mut self) -> bool {
        let _ = self.mailbox.send_control(NetworkControl::Shutdown);
        join_with_deadline(
            self.thread.take(),
            &self.finished,
            Duration::from_millis(250),
        )
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

fn network_loop(receiver: TranscriptionMailboxReceiver<NetworkControl, NetworkJob>) {
    while let Some(item) = receiver.recv_next(Duration::from_millis(25)) {
        match item {
            MailboxItem::Control(NetworkControl::Shutdown) => return,
            MailboxItem::Work(job) => job.run(),
        }
    }
}

impl NetworkJob {
    fn run(mut self) {
        let session_id = self.recording.session_id;
        let watcher = self.cancelled.watcher(session_id);
        if watcher.is_cancelled() {
            let _ = self
                .events
                .send(SpeechWorkerEvent::Cancelled { session_id });
            self.cancelled.complete(session_id);
            return;
        }

        let Some(executor) = self.executor.as_ref() else {
            let local_enabled = self
                .router
                .chain()
                .entries()
                .iter()
                .any(|entry| entry.enabled && entry.id.is_local());
            if local_enabled {
                self.inference.finalize(
                    self.recording,
                    self.output_mode,
                    self.audio_finalize_ms,
                    self.release_started,
                );
            } else {
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription_failed".to_owned(),
                    message:
                        "No network client could be created, and local speech is switched off."
                            .to_owned(),
                    retryable: true,
                });
            }
            self.cancelled.complete(session_id);
            return;
        };

        let wav = encode_wav(
            &crate::audio::resample_bandlimited(
                &self.recording.samples,
                self.recording.sample_rate_hz,
                UPLOAD_SAMPLE_RATE_HZ,
            ),
            UPLOAD_SAMPLE_RATE_HZ,
        );
        let request = TranscriptionRequest {
            audio_duration_ms: self.recording.duration_ms,
            upload_bytes: wav.len() as u64,
            translate_to_english: self.output_mode == SpeechOutputMode::TranslateToEnglish,
            language: self.language.locale().and_then(LanguageHint::new),
        };
        let started = Instant::now();
        let outcome = self.router.transcribe(
            &wav,
            &request,
            started,
            &|id: &ProviderId| self.secrets.get(id.as_str()).cloned(),
            executor,
            &watcher,
        );

        report_attempts(&outcome, &self.secrets);
        match outcome {
            RouteOutcome::Transcribed { provider, text, .. } => {
                let _ = self.events.send(SpeechWorkerEvent::TranscribedRemotely {
                    session_id,
                    provider_id: provider.to_string(),
                });
                let created_unix_ms = crate::unix_ms();
                let transcript = Transcript {
                    session_id,
                    text,
                    language: reported_language(request.language.as_ref()),
                    audio_duration_ms: self.recording.duration_ms,
                    processing_duration_ms: crate::elapsed_ms(started),
                    provider: Some(provider.to_string()),
                    latency: TranscriptionLatency {
                        audio_finalize_ms: self.audio_finalize_ms,
                        after_release_ms: crate::elapsed_ms(self.release_started),
                        ..TranscriptionLatency::default()
                    },
                };
                let _ = self.events.send(SpeechWorkerEvent::Completed {
                    transcript: transcript.clone(),
                    created_unix_ms,
                });
                let _ = self.history.send(HistoryCommand::Append {
                    transcript,
                    created_unix_ms,
                });
            }
            RouteOutcome::UseLocalEngine { .. } => self.inference.finalize(
                self.recording,
                self.output_mode,
                self.audio_finalize_ms,
                self.release_started,
            ),
            RouteOutcome::Cancelled { .. } => {
                let _ = self
                    .events
                    .send(SpeechWorkerEvent::Cancelled { session_id });
            }
            RouteOutcome::NoSpeech { .. } => {
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription".to_owned(),
                    message: "no speech was recognized".to_owned(),
                    retryable: true,
                });
            }
            RouteOutcome::Exhausted { ref attempts } => {
                let message = if attempts.is_empty() {
                    "Every transcription provider is switched off.".to_owned()
                } else {
                    attempts.last().map_or_else(
                        || "No provider could transcribe the recording.".to_owned(),
                        |attempt| format!("{}: {}", attempt.provider, attempt.reason()),
                    )
                };
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription_failed".to_owned(),
                    message,
                    retryable: true,
                });
            }
        }
        let _ = self.results.send(NetworkLedger {
            revision: self.revision,
            ledger: self.router.into_ledger(),
        });
        self.cancelled.complete(session_id);
    }
}

fn report_attempts(outcome: &RouteOutcome, secrets: &HashMap<String, String>) {
    for attempt in outcome_attempts(outcome) {
        let mut reason = attempt.reason();
        if let Some(secret) = secrets.get(attempt.provider.as_str()) {
            reason = rust_cloud_speech::redact_secret(&reason, secret);
        }
        eprintln!(
            "transcription provider={} result=not_used reason={reason}",
            attempt.provider
        );
    }
}

struct State {
    router: Router,
    credentials: CredentialStore<DpapiCipher>,
    /// `None` when a client could not be built at all; the local engine still
    /// works, which is the point of the list.
    executor: Option<HttpExecutor>,
    events: Sender<SpeechWorkerEvent>,
    history: Sender<HistoryCommand>,
    inference: InferenceHandle,
    prefers_cloud: Arc<AtomicBool>,
    cancelled: SessionCancellationSet,
    language: DictationLanguage,
    router_revision: u64,
    network: NetworkWorker,
    network_results: Receiver<NetworkLedger>,
    network_result_sender: Sender<NetworkLedger>,
}

impl State {
    /// Keeps the flag the command loop reads in step with the list, the stored
    /// keys and anything the ledger has learned.
    fn refresh_preference(&self) {
        // Deliberately a question about the configuration, not about the
        // ledger: a value that changed as cooldowns came and went would leave
        // the command loop taking the direct local path at the exact moment a
        // recovered service should have been tried again, with nothing to
        // recompute it afterwards.
        let routing_required = self
            .router
            .chain()
            .routing_required(|id| !id.needs_credential() || self.credentials.has(id.as_str()));
        self.prefers_cloud
            .store(routing_required, Ordering::Release);
    }

    fn handle(&mut self, command: TranscriptionCommand) {
        self.drain_network_results();
        match command {
            TranscriptionCommand::SetProviders(providers) => {
                let mut chain = Chain::new(providers.iter().map(to_entry).collect());
                // A build that knows a service the stored list has never seen
                // still offers it, switched off and last.
                chain.merge_known();
                *self.router.chain_mut() = chain;
                self.router_revision = self.router_revision.wrapping_add(1);
                self.refresh_preference();
                self.report_status();
            }
            TranscriptionCommand::SetCredential {
                operation_id,
                provider_id,
                secret,
            } => {
                let result = match secret {
                    Some(secret) => self.credentials.set(&provider_id, &secret),
                    None => self.credentials.remove(&provider_id),
                };
                // Answered either way, because a window is waiting on it and a
                // silence is indistinguishable from a success.
                let _ = self.events.send(SpeechWorkerEvent::CredentialSaved {
                    operation_id,
                    provider_id: provider_id.clone(),
                    error: result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                });
                match result {
                    Ok(()) => {
                        // A key the user has just corrected deserves another
                        // chance without waiting for a cooldown.
                        self.router
                            .ledger_mut()
                            .clear_credential_refusal(&ProviderId::new(&provider_id));
                        self.router_revision = self.router_revision.wrapping_add(1);
                    }
                    Err(error) => {
                        let _ = self.events.send(SpeechWorkerEvent::Failed {
                            session_id: None,
                            code: "credential_store".to_owned(),
                            message: format!("The key could not be stored: {error}"),
                            retryable: false,
                        });
                    }
                }
                self.refresh_preference();
                self.report_status();
            }
            TranscriptionCommand::ReportStatus => self.report_status(),
            TranscriptionCommand::SetLanguage(language) => {
                self.language = language;
                self.report_status();
            }
            TranscriptionCommand::Finalize {
                recording,
                output_mode,
                audio_finalize_ms,
                release_started,
            } => self.finalize(*recording, output_mode, audio_finalize_ms, release_started),
            TranscriptionCommand::Shutdown => {}
        }
    }

    fn finalize(
        &mut self,
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    ) {
        let routing_required = self
            .router
            .chain()
            .routing_required(|id| !id.needs_credential() || self.credentials.has(id.as_str()));
        if !routing_required {
            self.inference
                .finalize(recording, output_mode, audio_finalize_ms, release_started);
            return;
        }

        let secrets = self
            .credentials
            .providers()
            .into_iter()
            .filter_map(|id| {
                self.credentials
                    .secret(id)
                    .map(|secret| (id.to_owned(), secret.to_owned()))
            })
            .collect();
        let job = NetworkJob {
            router: self.router.clone(),
            revision: self.router_revision,
            secrets,
            executor: self.executor.clone(),
            recording,
            output_mode,
            audio_finalize_ms,
            release_started,
            language: self.language,
            events: self.events.clone(),
            history: self.history.clone(),
            inference: self.inference.clone(),
            cancelled: self.cancelled.clone(),
            results: self.network_result_sender.clone(),
        };
        if let Some(replaced) = self.network.submit(job) {
            let session_id = replaced.recording.session_id;
            self.cancelled.cancel(session_id);
            let _ = self
                .events
                .send(SpeechWorkerEvent::Cancelled { session_id });
            self.cancelled.complete(session_id);
        }
    }

    fn drain_network_results(&mut self) {
        while let Ok(result) = self.network_results.try_recv() {
            if result.revision == self.router_revision {
                self.router.replace_ledger(result.ledger);
            }
        }
    }

    fn shutdown(&mut self) {
        if !self.network.shutdown() {
            eprintln!(
                "speech provider network did not stop cooperatively; process teardown will finish it"
            );
        }
        self.drain_network_results();
    }

    fn report_status(&self) {
        let statuses = self
            .router
            .chain()
            .status(self.router.ledger(), Instant::now(), |id| {
                !id.needs_credential() || self.credentials.has(id.as_str())
            })
            .into_iter()
            .map(|status| TranscriptionProviderStatus {
                detail: describe(&status.unavailable),
                enabled: !matches!(status.unavailable, Some(Unavailable::Disabled)),
                available: status.is_available(),
                has_credential: self.credentials.has(status.id.as_str()),
                credential_hint: self.credentials.hint(status.id.as_str()),
                position: status.position,
                id: status.id.to_string(),
            })
            .collect();
        let _ = self
            .events
            .send(SpeechWorkerEvent::TranscriptionProviders { statuses });
    }
}

fn outcome_attempts(outcome: &RouteOutcome) -> &[rust_cloud_speech::Attempt] {
    match outcome {
        RouteOutcome::Transcribed { attempts, .. }
        | RouteOutcome::UseLocalEngine { attempts }
        | RouteOutcome::Cancelled { attempts }
        | RouteOutcome::NoSpeech { attempts }
        | RouteOutcome::Exhausted { attempts } => attempts,
    }
}

fn to_entry(entry: &TranscriptionProviderEntry) -> ProviderEntry {
    ProviderEntry {
        id: ProviderId::new(&entry.id),
        enabled: entry.enabled,
        endpoint: entry.endpoint.clone(),
        model: entry.model.clone(),
        account: entry.account.clone(),
    }
}

fn describe(unavailable: &Option<rust_cloud_speech::Unavailable>) -> String {
    use rust_cloud_speech::Unavailable;
    match unavailable {
        None => String::new(),
        Some(Unavailable::Incapable(reason)) => format!("Not for this recording: {reason}"),
        Some(Unavailable::Disabled) => "Switched off".to_owned(),
        Some(Unavailable::MissingCredential) => "No key stored".to_owned(),
        Some(Unavailable::Unsupported) => "Not supported by this version".to_owned(),
        Some(Unavailable::CredentialRefused(detail)) => format!("Key refused: {detail}"),
        Some(Unavailable::CoolingDown { remaining, reason }) => format!(
            "{reason}; available again in about {} min",
            remaining.as_secs().div_ceil(60)
        ),
    }
}

#[cfg(test)]
mod language_policy_tests {
    use rust_cloud_speech::LanguageHint;

    use super::reported_language;

    #[test]
    fn automatic_and_explicit_languages_are_reported_honestly() {
        let explicit = LanguageHint::new("ru-RU");

        assert_eq!(reported_language(None), "auto");
        assert_eq!(reported_language(explicit.as_ref()), "ru");
        assert_eq!(reported_language(explicit.as_ref()), "ru");
    }
}

#[cfg(test)]
mod network_shutdown_tests {
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::{NetworkControl, NetworkJob, NetworkWorker};
    use crate::transcription_mailbox::TranscriptionMailbox;

    #[test]
    fn the_real_network_owner_has_a_bounded_shutdown_when_http_is_stuck() {
        let (mailbox, _commands) = TranscriptionMailbox::<NetworkControl, NetworkJob>::bounded(1);
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release, release_receiver) = mpsc::sync_channel(1);
        let (finished_sender, finished) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let _ = entered_sender.send(());
            let _ = release_receiver.recv();
            let _ = finished_sender.send(());
        });
        let mut worker = NetworkWorker {
            mailbox,
            thread: Some(thread),
            finished,
        };
        entered
            .recv_timeout(Duration::from_secs(1))
            .expect("network operation started");

        let started = Instant::now();
        assert!(!worker.shutdown(), "the network call remains blocked");
        assert!(started.elapsed() < Duration::from_millis(600));

        // A real worker process exits here; release this synthetic operation so
        // the detached test thread does not outlive the test process.
        let _ = release.send(());
    }
}
