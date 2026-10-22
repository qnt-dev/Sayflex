use std::{
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use anyhow::Result;
use rust_protocol::{SpeechWorkerEvent, Transcript};

use crate::{history::HistoryStore, transcription_mailbox::SessionPublicationGuard};

pub enum HistoryCommand {
    Complete {
        transcript: Transcript,
        created_unix_ms: u64,
        publication: SessionPublicationGuard,
    },
    Latest {
        limit: usize,
    },
    SetEnabled {
        enabled: bool,
    },
    Clear,
    #[cfg(test)]
    PauseNextClear {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
    Shutdown,
}

/// Transfers terminal publication and persistence to the history actor so
/// completion is ordered with Clear. True means that actor owns the guard;
/// otherwise the caller must acknowledge the failure's terminal publication.
pub fn queue_completion(
    transcript: Transcript,
    created_unix_ms: u64,
    history: &Sender<HistoryCommand>,
    publication: &SessionPublicationGuard,
    events: &Sender<SpeechWorkerEvent>,
) -> bool {
    let session_id = transcript.session_id;
    if publication.is_cancelled() {
        let _ = events.send(SpeechWorkerEvent::Cancelled { session_id });
        return false;
    }
    if history
        .send(HistoryCommand::Complete {
            transcript,
            created_unix_ms,
            publication: publication.clone(),
        })
        .is_ok()
    {
        return true;
    }
    if !publication.try_publish(|| {
        let _ = events.send(SpeechWorkerEvent::Failed {
            session_id: Some(session_id),
            code: "history_unavailable".to_owned(),
            message: "The transcription history worker is unavailable.".to_owned(),
            retryable: true,
        });
    }) {
        let _ = events.send(SpeechWorkerEvent::Cancelled { session_id });
    }
    false
}

pub struct HistoryActor {
    commands: Sender<HistoryCommand>,
    thread: Option<JoinHandle<()>>,
}

impl HistoryActor {
    pub fn commands(&self) -> Sender<HistoryCommand> {
        self.commands.clone()
    }

    /// FIFO shutdown follows every command already accepted by the writer.
    /// Process exit is safe only after the final atomic history write finishes.
    pub fn shutdown_and_join(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let _ = self.commands.send(HistoryCommand::Shutdown);
        thread
            .join()
            .map_err(|_| anyhow::anyhow!("speech history writer panicked during shutdown"))
    }
}

impl Drop for HistoryActor {
    fn drop(&mut self) {
        // Startup errors and input errors must drain too, not detach the writer.
        if let Err(error) = self.finish() {
            eprintln!("history shutdown failed: {error:#}");
        }
    }
}

pub fn start(path: PathBuf, events: Sender<SpeechWorkerEvent>) -> Result<HistoryActor> {
    // Open synchronously so startup reports corrupt/unreadable history deterministically.
    let mut store = HistoryStore::open(path)?;
    if let Some(message) = store.take_recovery_message() {
        let _ = events.send(SpeechWorkerEvent::Failed {
            session_id: None,
            code: "history_recovered".to_owned(),
            message,
            retryable: true,
        });
    }
    let mut enabled = true;
    let (commands, receiver) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("speech-history-writer".to_owned())
        .spawn(move || {
            #[cfg(test)]
            let mut clear_pause: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)> = None;
            for command in receiver {
                match command {
                    HistoryCommand::Complete {
                        transcript,
                        created_unix_ms,
                        publication,
                    } => {
                        // Publish before this command's disk write, but after every
                        // earlier history command. Immediate shutdown still joins
                        // this append, while Cancel can win until publication.
                        let published = publication.try_publish(|| {
                            if let Some(provider_id) = &transcript.provider {
                                let _ = events.send(SpeechWorkerEvent::TranscribedRemotely {
                                    session_id: transcript.session_id,
                                    provider_id: provider_id.clone(),
                                });
                            }
                            let _ = events.send(SpeechWorkerEvent::Completed {
                                transcript: transcript.clone(),
                                created_unix_ms,
                            });
                        });
                        if !published {
                            let _ = events.send(SpeechWorkerEvent::Cancelled {
                                session_id: transcript.session_id,
                            });
                        }
                        publication.complete();
                        if published
                            && enabled
                            && let Err(error) = store.append(transcript, created_unix_ms)
                        {
                            eprintln!("history persistence failed: {error:#}");
                        }
                    }
                    HistoryCommand::Latest { limit } => {
                        let _ = events.send(SpeechWorkerEvent::History {
                            entries: store.latest(limit.min(250)),
                        });
                    }
                    HistoryCommand::SetEnabled { enabled: value } => {
                        enabled = value;
                    }
                    HistoryCommand::Clear => {
                        let error = store.clear().err().map(|error| {
                            eprintln!("history clear failed: {error:#}");
                            format!("{error:#}")
                        });
                        #[cfg(test)]
                        if let Some((entered, release)) = clear_pause.take() {
                            let _ = entered.send(());
                            let _ = release.recv();
                        }
                        let _ = events.send(SpeechWorkerEvent::History {
                            entries: store.latest(250),
                        });
                        let _ = events.send(SpeechWorkerEvent::HistoryCleared { error });
                    }
                    #[cfg(test)]
                    HistoryCommand::PauseNextClear { entered, release } => {
                        clear_pause = Some((entered, release));
                    }
                    HistoryCommand::Shutdown => break,
                }
            }
        })?;
    Ok(HistoryActor {
        commands,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, time::Duration};

    use rust_protocol::{HistoryEntry, SessionId, SpeechWorkerEvent, Transcript};
    use tempfile::tempdir;

    use super::{HistoryActor, HistoryCommand, queue_completion, start};
    use crate::transcription_mailbox::SessionCancellationSet;

    #[test]
    fn completion_queued_behind_active_clear_is_published_after_clear_answers() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let (events, received) = mpsc::channel();
        let actor = start(path.clone(), events.clone()).unwrap();
        let history = actor.commands();
        let release = pause_clear(&history);
        let transcript = transcript("new result");
        let publication = SessionCancellationSet::default().watcher(transcript.session_id);
        assert!(queue_completion(
            transcript,
            123,
            &history,
            &publication,
            &events
        ));
        release.send(()).unwrap();
        actor.shutdown_and_join().unwrap();
        let events: Vec<_> = received.try_iter().collect();
        assert!(
            matches!(events.as_slice(), [SpeechWorkerEvent::History { entries }, SpeechWorkerEvent::HistoryCleared { error: None }, SpeechWorkerEvent::Completed { .. }] if entries.is_empty()),
            "clear answers must precede the new result: {events:?}"
        );
        assert_eq!(
            crate::history::HistoryStore::open(path).unwrap().latest(1)[0]
                .transcript
                .text,
            "new result"
        );
    }

    #[test]
    fn unavailable_history_actor_returns_typed_failure_instead_of_completion() {
        let (history, receiver) = mpsc::channel();
        drop(receiver);
        let (events, received) = mpsc::channel();
        let transcript = transcript("unowned result");
        let publication = SessionCancellationSet::default().watcher(transcript.session_id);
        assert!(!queue_completion(
            transcript,
            123,
            &history,
            &publication,
            &events
        ));
        let events: Vec<_> = received.try_iter().collect();
        assert!(
            matches!(events.as_slice(), [SpeechWorkerEvent::Failed { code, retryable: true, .. }] if code == "history_unavailable"),
            "a dead actor cannot own a successful completion: {events:?}"
        );
    }

    #[test]
    fn cancellation_while_completion_waits_for_clear_suppresses_publication_and_history() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let (events, received) = mpsc::channel();
        let actor = start(path.clone(), events.clone()).unwrap();
        let history = actor.commands();
        let release = pause_clear(&history);
        let transcript = transcript("cancelled result");
        let session_id = transcript.session_id;
        let cancellations = SessionCancellationSet::default();
        let publication = cancellations.watcher(session_id);
        assert!(queue_completion(
            transcript,
            123,
            &history,
            &publication,
            &events
        ));
        cancellations.cancel(session_id);
        assert!(publication.is_cancelled());
        release.send(()).unwrap();
        actor.shutdown_and_join().unwrap();
        let events: Vec<_> = received.try_iter().collect();
        assert!(
            matches!(events.as_slice(), [SpeechWorkerEvent::History { .. }, SpeechWorkerEvent::HistoryCleared { .. }, SpeechWorkerEvent::Cancelled { session_id: cancelled }] if *cancelled == session_id),
            "cancellation must still win until actual terminal publication: {events:?}"
        );
        assert!(
            crate::history::HistoryStore::open(path)
                .unwrap()
                .latest(1)
                .is_empty()
        );
        assert!(
            !publication.is_cancelled(),
            "the terminal owner must acknowledge cancellation"
        );
    }

    fn pause_clear(history: &mpsc::Sender<HistoryCommand>) -> mpsc::Sender<()> {
        let (entered, received) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        history
            .send(HistoryCommand::PauseNextClear {
                entered,
                release: resume,
            })
            .unwrap();
        history.send(HistoryCommand::Clear).unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        release
    }

    fn transcript(text: &str) -> Transcript {
        Transcript {
            session_id: SessionId::new(),
            text: text.into(),
            language: "en".into(),
            audio_duration_ms: 10,
            processing_duration_ms: 1,
            provider: None,
            latency: Default::default(),
        }
    }

    #[test]
    fn shown_completion_survives_immediate_worker_shutdown() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let (events, received) = mpsc::channel();
        let actor = start(path.clone(), events.clone()).unwrap();
        let history = actor.commands();
        let transcript = Transcript {
            session_id: SessionId::new(),
            text: "already shown result".into(),
            language: "en".into(),
            audio_duration_ms: 10,
            processing_duration_ms: 1,
            provider: None,
            latency: Default::default(),
        };
        let publication = SessionCancellationSet::default().watcher(transcript.session_id);
        assert!(queue_completion(
            transcript,
            123,
            &history,
            &publication,
            &events
        ));
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            SpeechWorkerEvent::Completed { .. }
        ));
        actor.shutdown_and_join().unwrap();
        let persisted = crate::history::HistoryStore::open(path).unwrap().latest(1);
        assert_eq!(
            persisted.len(),
            1,
            "a displayed completion must already be queued before shutdown"
        );
        assert_eq!(persisted[0].transcript.text, "already shown result");
        assert_eq!(persisted[0].created_unix_ms, 123);
    }

    #[test]
    fn shutdown_drains_queued_appends_before_returning() {
        assert_drained(|actor| actor.shutdown_and_join().unwrap());
    }

    #[test]
    fn dropping_owner_on_early_return_also_drains_queued_appends() {
        assert_drained(drop);
    }

    fn assert_drained(shutdown: impl FnOnce(HistoryActor)) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let (events, received) = mpsc::channel();
        let actor = start(path.clone(), events.clone()).unwrap();
        let history = actor.commands();
        for index in 0..64 {
            let transcript = transcript(&format!("queued transcript {index}"));
            let publication = SessionCancellationSet::default().watcher(transcript.session_id);
            assert!(queue_completion(
                transcript,
                index,
                &history,
                &publication,
                &events
            ));
        }
        drop(events);
        shutdown(actor);
        let persisted: Vec<HistoryEntry> = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for _ in 0..64 {
            assert!(matches!(
                received.recv_timeout(Duration::from_secs(5)).unwrap(),
                SpeechWorkerEvent::Completed { .. }
            ));
        }
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert_eq!(
            persisted.len(),
            64,
            "shutdown must finish every earlier append"
        );
        assert_eq!(persisted[0].transcript.text, "queued transcript 63");
    }
}
