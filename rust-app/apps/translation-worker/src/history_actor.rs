use std::{
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use anyhow::Result;
use rust_protocol::{TranslationHistoryEntry, TranslationWorkerEvent};

use crate::{EventWriter, history::TranslationHistory};

enum HistoryCommand {
    Recent { limit: usize, writer: EventWriter },
    SetEnabled(bool),
    Append(TranslationHistoryEntry),
    Clear { writer: EventWriter },
    Shutdown,
}

/// Owns translation-history state and disk I/O independently of the resident
/// translation engine. Every public operation only enqueues work, so history
/// reads and persistence can never delay Cancel or other worker controls.
#[derive(Clone)]
pub(crate) struct TranslationHistoryActor {
    commands: Sender<HistoryCommand>,
}

impl TranslationHistoryActor {
    pub(crate) fn start(path: PathBuf) -> Result<(Self, JoinHandle<()>)> {
        let (commands, requests) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("translation-history".to_owned())
            .spawn(move || {
                let mut history = TranslationHistory::load(path);
                let mut enabled = true;
                for command in requests {
                    match command {
                        HistoryCommand::Recent { limit, writer } => {
                            let _ = writer.send(&TranslationWorkerEvent::History {
                                entries: history.recent(limit),
                            });
                        }
                        HistoryCommand::SetEnabled(value) => enabled = value,
                        HistoryCommand::Append(entry) if enabled => history.push(entry),
                        HistoryCommand::Append(_) => {}
                        HistoryCommand::Clear { writer } => {
                            history.clear();
                            let _ = writer.send(&TranslationWorkerEvent::History {
                                entries: Vec::new(),
                            });
                        }
                        HistoryCommand::Shutdown => break,
                    }
                }
            })?;
        Ok((Self { commands }, thread))
    }

    pub(crate) fn recent(&self, limit: usize, writer: EventWriter) {
        let _ = self.commands.send(HistoryCommand::Recent { limit, writer });
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let _ = self.commands.send(HistoryCommand::SetEnabled(enabled));
    }

    pub(crate) fn append(&self, entry: TranslationHistoryEntry) {
        let _ = self.commands.send(HistoryCommand::Append(entry));
    }

    pub(crate) fn clear(&self, writer: EventWriter) {
        let _ = self.commands.send(HistoryCommand::Clear { writer });
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.commands.send(HistoryCommand::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use rust_protocol::TranslationWorkerEvent;

    use super::TranslationHistoryActor;
    use crate::EventWriter;

    #[test]
    fn history_read_completes_while_the_engine_owner_is_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let (actor, actor_thread) =
            TranslationHistoryActor::start(directory.path().join("translation-history.json"))
                .unwrap();
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

        actor.recent(10, writer);
        assert!(matches!(
            received.recv_timeout(Duration::from_millis(100)),
            Ok(TranslationWorkerEvent::History { entries }) if entries.is_empty()
        ));

        release.send(()).unwrap();
        blocker.join().unwrap();
        actor.shutdown();
        actor_thread.join().unwrap();
    }
}
