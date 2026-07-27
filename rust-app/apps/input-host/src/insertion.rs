use std::{
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

use rust_protocol::{
    InputHostEvent, InsertionMethod, RequestId, SessionId, TargetSnapshot, Transcript,
};

use crate::{clipboard, hotkeys, inject, target};

const INJECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(3);
// Some modern editors resolve Ctrl+V asynchronously. Keeping our replacement
// on the clipboard for this guarded window prevents them from reading the
// restored previous value. Sequence-number validation still guarantees that a
// newer clipboard change made by the user is never overwritten.
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(750);

enum InsertionCommand {
    Insert {
        transcript: Transcript,
        target: TargetSnapshot,
        add_trailing_space: bool,
    },
    Copy {
        transcript: Transcript,
    },
    CopyText {
        request_id: RequestId,
        text: String,
    },
    ReplaceSelection {
        request_id: RequestId,
        target: TargetSnapshot,
        text: String,
    },
    Shutdown,
}

pub struct InsertionService {
    commands: Sender<InsertionCommand>,
}

impl InsertionService {
    pub fn start(events: Sender<InputHostEvent>) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("input-insertion-worker".to_owned())
            .spawn(move || {
                for command in receiver {
                    match command {
                        InsertionCommand::Insert {
                            transcript,
                            target,
                            add_trailing_space,
                        } => {
                            if let Err(error) = insert_transcript(
                                &events,
                                transcript.session_id,
                                target,
                                &transcript.text,
                                add_trailing_space,
                            ) {
                                let _ = events.send(InputHostEvent::Failed {
                                    session_id: Some(transcript.session_id),
                                    code: "insert_transcript".to_owned(),
                                    message: error,
                                });
                            }
                        }
                        InsertionCommand::Copy { transcript } => {
                            match clipboard::set_text(&transcript.text) {
                                Ok(()) => {
                                    let _ = events.send(InputHostEvent::Recovered {
                                        session_id: transcript.session_id,
                                    });
                                }
                                Err(error) => {
                                    let _ = events.send(InputHostEvent::Failed {
                                        session_id: Some(transcript.session_id),
                                        code: "copy_transcript".to_owned(),
                                        message: error.to_string(),
                                    });
                                }
                            }
                        }
                        InsertionCommand::CopyText { request_id, text } => {
                            match clipboard::set_text(&text) {
                                Ok(()) => {
                                    let _ = events.send(InputHostEvent::TextCopied { request_id });
                                }
                                Err(error) => {
                                    let _ = events.send(InputHostEvent::Failed {
                                        session_id: None,
                                        code: "copy_text".to_owned(),
                                        message: error.to_string(),
                                    });
                                }
                            }
                        }
                        InsertionCommand::ReplaceSelection {
                            request_id,
                            target,
                            text,
                        } => {
                            if let Err(error) =
                                replace_selection(&events, request_id, target, &text)
                            {
                                let _ = events.send(InputHostEvent::Failed {
                                    session_id: None,
                                    code: "replace_selection".to_owned(),
                                    message: error,
                                });
                            }
                        }
                        InsertionCommand::Shutdown => break,
                    }
                }
            })?;
        Ok(Self { commands })
    }

    pub fn insert(&self, transcript: Transcript, target: TargetSnapshot, add_trailing_space: bool) {
        let _ = self.commands.send(InsertionCommand::Insert {
            transcript,
            target,
            add_trailing_space,
        });
    }

    pub fn copy(&self, transcript: Transcript) {
        let _ = self.commands.send(InsertionCommand::Copy { transcript });
    }

    pub fn copy_text(&self, request_id: RequestId, text: String) {
        let _ = self
            .commands
            .send(InsertionCommand::CopyText { request_id, text });
    }

    pub fn replace_selection(&self, request_id: RequestId, target: TargetSnapshot, text: String) {
        let _ = self.commands.send(InsertionCommand::ReplaceSelection {
            request_id,
            target,
            text,
        });
    }

    pub fn shutdown(&self) {
        let _ = self.commands.send(InsertionCommand::Shutdown);
    }
}

fn insert_transcript(
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    target_snapshot: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<(), String> {
    let started = Instant::now();
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err(
            "physical modifier keys did not become idle; transcript remains in history".to_owned(),
        );
    }

    if target::prefer_direct_unicode(target_snapshot) {
        inject::insert_text(target_snapshot, text, add_trailing_space)
            .map_err(|error| error.to_string())?;
        send_dispatched(events, session_id, InsertionMethod::UnicodeInput, started);
        return Ok(());
    }

    match clipboard::dispatch_paste(
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut().map_err(|_| clipboard::ClipboardError::Paste),
        text,
        add_trailing_space,
    ) {
        Ok(pending_restore) => {
            // Paste has already been placed in the target. Clipboard restoration is a
            // background durability concern and is intentionally outside visible latency.
            send_dispatched(events, session_id, InsertionMethod::ClipboardPaste, started);
            if let Err(error) = pending_restore.restore_after(CLIPBOARD_RESTORE_DELAY) {
                eprintln!("clipboard restoration failed for {session_id}: {error}");
            } else {
                let _ = events.send(InputHostEvent::ClipboardRestored { session_id });
            }
            Ok(())
        }
        Err(clipboard_error) if inject::is_foreground(target_snapshot) => {
            inject::insert_text(target_snapshot, text, add_trailing_space).map_err(
                |injection_error| {
                    format!("{clipboard_error}; Unicode fallback failed: {injection_error}")
                },
            )?;
            send_dispatched(events, session_id, InsertionMethod::UnicodeInput, started);
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn replace_selection(
    events: &Sender<InputHostEvent>,
    request_id: RequestId,
    target_snapshot: TargetSnapshot,
    text: &str,
) -> Result<(), String> {
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err("physical modifier keys did not become idle".to_owned());
    }

    if target::prefer_direct_unicode(target_snapshot) {
        inject::replace_selection(target_snapshot, text).map_err(|error| error.to_string())?;
        let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
        return Ok(());
    }

    match clipboard::dispatch_paste(
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut().map_err(|_| clipboard::ClipboardError::Paste),
        text,
        false,
    ) {
        Ok(pending_restore) => {
            // Replacement is complete as soon as paste is dispatched. Restore
            // the user's clipboard afterward without extending visible latency.
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            if let Err(error) = pending_restore.restore_after(CLIPBOARD_RESTORE_DELAY) {
                eprintln!("clipboard restoration failed for text action {request_id}: {error}");
            }
            Ok(())
        }
        Err(clipboard_error) if inject::is_foreground(target_snapshot) => {
            inject::replace_selection(target_snapshot, text).map_err(|injection_error| {
                format!("{clipboard_error}; Unicode fallback failed: {injection_error}")
            })?;
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn send_dispatched(
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    method: InsertionMethod,
    started: Instant,
) {
    let _ = events.send(InputHostEvent::InsertionDispatched {
        session_id,
        method,
        dispatch_duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    });
}
