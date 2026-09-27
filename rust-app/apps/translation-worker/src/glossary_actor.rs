use std::{
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use rust_protocol::{
    GlossaryEntry, GlossaryEntryDraft, GlossaryEntryId, GlossaryOrigin, RequestId, ScreenBlockId,
    SourceContext, TranslationLanguage, TranslationWorkerEvent,
};

use crate::{
    EventWriter,
    glossary::{GlossaryError, GlossaryStore},
};

struct WriteRequest {
    operation_id: RequestId,
    request_id: RequestId,
    block_id: ScreenBlockId,
    expected_revision: u64,
    entry: GlossaryEntryDraft,
    writer: EventWriter,
}

enum GlossaryCommand {
    Save(WriteRequest),
    List {
        operation_id: RequestId,
        query: Option<String>,
        limit: usize,
        writer: EventWriter,
    },
    Update {
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
        entry: GlossaryEntryDraft,
        writer: EventWriter,
    },
    Delete {
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
        writer: EventWriter,
    },
    #[allow(dead_code)] // Consumed by structured block translation in Task 11.
    Lookup {
        source_phrase: String,
        source_language: TranslationLanguage,
        target_language: TranslationLanguage,
        context: SourceContext,
        limit: usize,
        response: Sender<Vec<GlossaryEntry>>,
    },
    Shutdown,
}

/// Owns personal-glossary memory and filesystem I/O. Translation and Cancel
/// never acquire this state: commands enqueue bounded work to this actor.
#[derive(Clone)]
pub(crate) struct GlossaryActor {
    commands: Sender<GlossaryCommand>,
}

impl GlossaryActor {
    pub(crate) fn start(path: PathBuf) -> Result<(Self, JoinHandle<()>)> {
        let (commands, requests) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("translation-glossary".to_owned())
            .spawn(move || {
                let mut store = GlossaryStore::load(path.clone()).unwrap_or_else(|_| {
                    rust_app_core::diagnostics::log_line(
                        "translation_glossary startup=recovered_empty",
                    );
                    GlossaryStore::empty(path)
                });
                for command in requests {
                    match command {
                        GlossaryCommand::Save(request) => save(&mut store, request),
                        GlossaryCommand::List {
                            operation_id,
                            query,
                            limit,
                            writer,
                        } => {
                            let _ = writer.send(&TranslationWorkerEvent::GlossaryList {
                                operation_id,
                                entries: store.list(query.as_deref(), limit),
                            });
                        }
                        GlossaryCommand::Update {
                            operation_id,
                            entry_id,
                            entry,
                            writer,
                        } => match store.update(&entry_id, entry, unix_ms()) {
                            Ok(entry) => {
                                let _ = writer.send(&TranslationWorkerEvent::GlossarySaved {
                                    operation_id,
                                    entry,
                                });
                            }
                            Err(error) => send_failure(&writer, operation_id, &error),
                        },
                        GlossaryCommand::Delete {
                            operation_id,
                            entry_id,
                            writer,
                        } => match store.delete(&entry_id) {
                            Ok(()) => {
                                let _ = writer.send(&TranslationWorkerEvent::GlossaryDeleted {
                                    operation_id,
                                    entry_id,
                                });
                            }
                            Err(error) => send_failure(&writer, operation_id, &error),
                        },
                        GlossaryCommand::Lookup {
                            source_phrase,
                            source_language,
                            target_language,
                            context,
                            limit,
                            response,
                        } => {
                            let _ = response.send(store.lookup(
                                &source_phrase,
                                source_language,
                                target_language,
                                &context,
                                limit,
                            ));
                        }
                        GlossaryCommand::Shutdown => break,
                    }
                }
            })?;
        Ok((Self { commands }, thread))
    }

    pub(crate) fn confirm(
        &self,
        operation_id: RequestId,
        request_id: RequestId,
        block_id: ScreenBlockId,
        expected_revision: u64,
        mut entry: GlossaryEntryDraft,
        writer: EventWriter,
    ) {
        entry.origin = GlossaryOrigin::Confirmed;
        self.save(WriteRequest {
            operation_id,
            request_id,
            block_id,
            expected_revision,
            entry,
            writer,
        });
    }

    pub(crate) fn save_correction(
        &self,
        operation_id: RequestId,
        request_id: RequestId,
        block_id: ScreenBlockId,
        expected_revision: u64,
        mut entry: GlossaryEntryDraft,
        writer: EventWriter,
    ) {
        entry.origin = GlossaryOrigin::Corrected;
        self.save(WriteRequest {
            operation_id,
            request_id,
            block_id,
            expected_revision,
            entry,
            writer,
        });
    }

    fn save(&self, request: WriteRequest) {
        let _ = self.commands.send(GlossaryCommand::Save(request));
    }

    pub(crate) fn list(
        &self,
        operation_id: RequestId,
        query: Option<String>,
        limit: usize,
        writer: EventWriter,
    ) {
        let _ = self.commands.send(GlossaryCommand::List {
            operation_id,
            query,
            limit,
            writer,
        });
    }

    pub(crate) fn update(
        &self,
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
        entry: GlossaryEntryDraft,
        writer: EventWriter,
    ) {
        let _ = self.commands.send(GlossaryCommand::Update {
            operation_id,
            entry_id,
            entry,
            writer,
        });
    }

    pub(crate) fn delete(
        &self,
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
        writer: EventWriter,
    ) {
        let _ = self.commands.send(GlossaryCommand::Delete {
            operation_id,
            entry_id,
            writer,
        });
    }

    #[allow(dead_code)] // Consumed by structured block translation in Task 11.
    pub(crate) fn lookup(
        &self,
        source_phrase: String,
        source_language: TranslationLanguage,
        target_language: TranslationLanguage,
        context: SourceContext,
        limit: usize,
    ) -> Result<Vec<GlossaryEntry>> {
        let (response, received) = mpsc::channel();
        self.commands
            .send(GlossaryCommand::Lookup {
                source_phrase,
                source_language,
                target_language,
                context,
                limit,
                response,
            })
            .map_err(|_| anyhow!("personal glossary actor stopped"))?;
        received
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| anyhow!("personal glossary lookup timed out"))
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.commands.send(GlossaryCommand::Shutdown);
    }
}

fn save(store: &mut GlossaryStore, request: WriteRequest) {
    let WriteRequest {
        operation_id,
        request_id,
        block_id,
        expected_revision,
        entry,
        writer,
    } = request;
    match store.save(entry, unix_ms()) {
        Ok(entry) => {
            let _ = writer.send(&TranslationWorkerEvent::GlossarySaved {
                operation_id,
                entry,
            });
        }
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_glossary operation={operation_id} request={request_id} block_id_bytes={} revision={expected_revision} result=failed",
                block_id.as_str().len()
            ));
            send_failure(&writer, operation_id, &error);
        }
    }
}

fn send_failure(writer: &EventWriter, operation_id: RequestId, error: &GlossaryError) {
    let (code, message, retryable) = match error {
        GlossaryError::NotFound => (
            "glossary_not_found",
            "The personal glossary entry no longer exists.",
            false,
        ),
        GlossaryError::Invalid(_) | GlossaryError::Capacity => (
            "glossary_invalid",
            "The personal glossary entry could not be accepted.",
            false,
        ),
        GlossaryError::Io(_) | GlossaryError::Json(_) => (
            "glossary_persistence",
            "The personal glossary could not be saved. Try again.",
            true,
        ),
    };
    let _ = writer.send(&TranslationWorkerEvent::GlossaryFailed {
        operation_id,
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
    });
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use rust_protocol::{
        GlossaryEntryDraft, GlossaryOrigin, RequestId, ScreenBlockId, SourceContext,
        TranslationLanguage, TranslationWorkerEvent,
    };
    use tempfile::tempdir;

    use super::GlossaryActor;
    use crate::EventWriter;

    fn draft() -> GlossaryEntryDraft {
        GlossaryEntryDraft {
            source_phrase: "cooldown".to_owned(),
            approved_translation: "время перезарядки".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            example: "Reduces the cooldown".to_owned(),
            context: SourceContext::default(),
            origin: GlossaryOrigin::Confirmed,
        }
    }

    #[test]
    fn only_explicit_confirmation_or_correction_creates_an_entry() {
        let directory = tempdir().unwrap();
        let (actor, thread) = GlossaryActor::start(directory.path().join("glossary.json")).unwrap();
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };

        actor.list(RequestId::new(), None, 10, writer.clone());
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(1)),
            Ok(TranslationWorkerEvent::GlossaryList { entries, .. }) if entries.is_empty()
        ));

        actor.confirm(
            RequestId::new(),
            RequestId::new(),
            ScreenBlockId::from("block"),
            0,
            draft(),
            writer,
        );
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(1)),
            Ok(TranslationWorkerEvent::GlossarySaved { entry, .. })
                if entry.origin == GlossaryOrigin::Confirmed
        ));
        let matches = actor
            .lookup(
                "cooldown".to_owned(),
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                SourceContext::default(),
                5,
            )
            .unwrap();
        assert_eq!(matches.len(), 1);
        actor.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn failed_persistence_reports_retryable_error_and_does_not_take_caller_text() {
        let directory = tempdir().unwrap();
        let blocked_parent = directory.path().join("not-a-directory");
        fs::write(&blocked_parent, b"file").unwrap();
        let (actor, thread) = GlossaryActor::start(blocked_parent.join("glossary.json")).unwrap();
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let correction = draft();

        actor.save_correction(
            RequestId::new(),
            RequestId::new(),
            ScreenBlockId::from("block"),
            4,
            correction.clone(),
            writer,
        );

        assert!(matches!(
            received.recv_timeout(Duration::from_secs(1)),
            Ok(TranslationWorkerEvent::GlossaryFailed {
                retryable: true,
                ..
            })
        ));
        assert_eq!(correction.approved_translation, "время перезарядки");
        actor.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn glossary_list_does_not_wait_for_the_translation_engine_owner() {
        let directory = tempdir().unwrap();
        let (actor, actor_thread) =
            GlossaryActor::start(directory.path().join("glossary.json")).unwrap();
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let engine_owner = Arc::new(Mutex::new(()));
        let held_owner = Arc::clone(&engine_owner);
        let (locked, observed_locked) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let blocker = thread::spawn(move || {
            let _guard = held_owner.lock().unwrap();
            locked.send(()).unwrap();
            released.recv().unwrap();
        });
        observed_locked.recv().unwrap();

        actor.list(RequestId::new(), None, 10, writer);
        assert!(matches!(
            received.recv_timeout(Duration::from_millis(100)),
            Ok(TranslationWorkerEvent::GlossaryList { entries, .. }) if entries.is_empty()
        ));

        release.send(()).unwrap();
        blocker.join().unwrap();
        actor.shutdown();
        actor_thread.join().unwrap();
    }
}
