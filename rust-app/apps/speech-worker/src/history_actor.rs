use std::{
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
};

use anyhow::Result;
use rust_protocol::{SpeechWorkerEvent, Transcript};

use crate::history::HistoryStore;

pub enum HistoryCommand {
    Append {
        transcript: Transcript,
        created_unix_ms: u64,
    },
    Latest {
        limit: usize,
    },
    SetEnabled {
        enabled: bool,
    },
    Clear,
    Shutdown,
}

pub fn start(path: PathBuf, events: Sender<SpeechWorkerEvent>) -> Result<Sender<HistoryCommand>> {
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
    thread::Builder::new()
        .name("speech-history-writer".to_owned())
        .spawn(move || {
            for command in receiver {
                match command {
                    HistoryCommand::Append {
                        transcript,
                        created_unix_ms,
                    } => {
                        if enabled && let Err(error) = store.append(transcript, created_unix_ms) {
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
                        if let Err(error) = store.clear() {
                            eprintln!("history clear failed: {error:#}");
                        }
                        let _ = events.send(SpeechWorkerEvent::History {
                            entries: Vec::new(),
                        });
                    }
                    HistoryCommand::Shutdown => break,
                }
            }
        })?;
    Ok(commands)
}
