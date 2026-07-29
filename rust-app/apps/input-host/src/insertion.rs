use std::{
    io,
    sync::mpsc::{self, RecvTimeoutError, Sender},
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
        let (startup, startup_result) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("input-insertion-worker".to_owned())
            .spawn(move || {
                let apartment = match clipboard::ClipboardApartment::initialize() {
                    Ok(apartment) => {
                        let _ = startup.send(Ok(()));
                        apartment
                    }
                    Err(error) => {
                        let _ = startup.send(Err(error.to_string()));
                        return;
                    }
                };
                let _apartment = apartment;
                let mut pending_restore: Option<ScheduledClipboardRestore> = None;
                loop {
                    let command = if let Some(pending) = pending_restore.as_ref() {
                        let remaining = pending.deadline.saturating_duration_since(Instant::now());
                        match receiver.recv_timeout(remaining) {
                            Ok(command) => Some(command),
                            Err(RecvTimeoutError::Timeout) => {
                                settle_clipboard_restore(&events, pending_restore.take());
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => None,
                        }
                    } else {
                        receiver.recv().ok()
                    };
                    let Some(command) = command else {
                        settle_clipboard_restore(&events, pending_restore.take());
                        break;
                    };
                    // A new operation must observe the user's original clipboard, not
                    // the temporary payload from a preceding paste.
                    settle_clipboard_restore(&events, pending_restore.take());
                    match command {
                        InsertionCommand::Insert {
                            transcript,
                            target,
                            add_trailing_space,
                        } => {
                            match insert_transcript(
                                &events,
                                transcript.session_id,
                                target,
                                &transcript.text,
                                add_trailing_space,
                            ) {
                                Ok(restore) => pending_restore = restore,
                                Err(error) => {
                                    crate::diagnostic_line(&format!(
                                        "insertion action=insert result=failed session={} target_pid={} error={error}",
                                        transcript.session_id, target.process_id
                                    ));
                                    let _ = events.send(InputHostEvent::Failed {
                                        session_id: Some(transcript.session_id),
                                        code: "insert_transcript".to_owned(),
                                        message: error,
                                    });
                                }
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
                        } => match replace_selection(&events, request_id, target, &text) {
                            Ok(restore) => pending_restore = restore,
                            Err(error) => {
                                let _ = events.send(InputHostEvent::Failed {
                                    session_id: None,
                                    code: "replace_selection".to_owned(),
                                    message: error,
                                });
                            }
                        },
                        InsertionCommand::Shutdown => {
                            settle_clipboard_restore(&events, pending_restore.take());
                            break;
                        }
                    }
                }
            })?;
        match startup_result.recv() {
            Ok(Ok(())) => Ok(Self { commands }),
            Ok(Err(message)) => Err(io::Error::other(message)),
            Err(error) => Err(io::Error::other(format!(
                "insertion worker startup channel closed: {error}"
            ))),
        }
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

struct ScheduledClipboardRestore {
    pending: clipboard::PendingClipboardRestore,
    deadline: Instant,
    session_id: Option<SessionId>,
}

#[derive(Clone, Copy)]
struct InsertionTiming {
    started: Instant,
    method_started: Instant,
    release_wait_ms: u64,
}

impl ScheduledClipboardRestore {
    fn new(pending: clipboard::PendingClipboardRestore, session_id: Option<SessionId>) -> Self {
        Self {
            pending,
            deadline: Instant::now() + CLIPBOARD_RESTORE_DELAY,
            session_id,
        }
    }
}

fn settle_clipboard_restore(
    events: &Sender<InputHostEvent>,
    scheduled: Option<ScheduledClipboardRestore>,
) {
    let Some(scheduled) = scheduled else {
        return;
    };
    if let Err(error) = scheduled.pending.restore() {
        crate::diagnostic_line(&format!("clipboard_restore result=failed error={error}"));
    } else if let Some(session_id) = scheduled.session_id {
        let _ = events.send(InputHostEvent::ClipboardRestored { session_id });
    }
}

fn insert_transcript(
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    target_snapshot: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<Option<ScheduledClipboardRestore>, String> {
    let started = Instant::now();
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err(
            "physical modifier keys did not become idle; transcript remains in history".to_owned(),
        );
    }
    let timing = InsertionTiming {
        started,
        method_started: Instant::now(),
        release_wait_ms: elapsed_ms(started),
    };
    let paste_shortcut = target::paste_shortcut(target_snapshot);

    match clipboard::dispatch_paste(
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut(paste_shortcut).map_err(|_| clipboard::ClipboardError::Paste),
        text,
        add_trailing_space,
    ) {
        Ok(pending_restore) => {
            send_dispatched(
                events,
                session_id,
                InsertionMethod::ClipboardPaste,
                target_snapshot,
                text.chars().count(),
                timing,
            );
            Ok(Some(ScheduledClipboardRestore::new(
                pending_restore,
                Some(session_id),
            )))
        }
        Err(clipboard_error) if inject::is_foreground(target_snapshot) => {
            inject::insert_text(target_snapshot, text, add_trailing_space).map_err(
                |injection_error| {
                    format!("{clipboard_error}; Unicode fallback failed: {injection_error}")
                },
            )?;
            send_dispatched(
                events,
                session_id,
                InsertionMethod::UnicodeInput,
                target_snapshot,
                text.chars().count(),
                timing,
            );
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn replace_selection(
    events: &Sender<InputHostEvent>,
    request_id: RequestId,
    target_snapshot: TargetSnapshot,
    text: &str,
) -> Result<Option<ScheduledClipboardRestore>, String> {
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err("physical modifier keys did not become idle".to_owned());
    }
    let paste_shortcut = target::paste_shortcut(target_snapshot);

    match clipboard::dispatch_paste(
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut(paste_shortcut).map_err(|_| clipboard::ClipboardError::Paste),
        text,
        false,
    ) {
        Ok(pending_restore) => {
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            Ok(Some(ScheduledClipboardRestore::new(pending_restore, None)))
        }
        Err(clipboard_error) if inject::is_foreground(target_snapshot) => {
            inject::replace_selection(target_snapshot, text).map_err(|injection_error| {
                format!("{clipboard_error}; Unicode fallback failed: {injection_error}")
            })?;
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn send_dispatched(
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    method: InsertionMethod,
    target: TargetSnapshot,
    chars: usize,
    timing: InsertionTiming,
) {
    let method_ms = elapsed_ms(timing.method_started);
    let total_ms = elapsed_ms(timing.started);
    crate::diagnostic_line(&format!(
        "insertion action=insert result=dispatched session={session_id} method={method:?} \
target_pid={} chars={chars} release_wait_ms={} method_ms={method_ms} total_ms={total_ms}",
        target.process_id, timing.release_wait_ms,
    ));
    let _ = events.send(InputHostEvent::InsertionDispatched {
        session_id,
        method,
        dispatch_duration_ms: total_ms,
    });
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
