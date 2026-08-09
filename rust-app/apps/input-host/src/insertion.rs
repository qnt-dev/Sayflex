use std::{
    borrow::Cow,
    io,
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

use rust_protocol::{
    InputHostEvent, InsertionDelivery, InsertionMethod, RequestId, SessionId, TargetSnapshot,
    Transcript,
};
use sha2::{Digest, Sha256};

use crate::{clipboard, hotkeys, inject, target, wezterm};

const INJECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(3);

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
                    Ok(apartment) => apartment,
                    Err(error) => {
                        let _ = startup.send(Err(error.to_string()));
                        return;
                    }
                };
                let _apartment = apartment;
                let mut clipboard_host = match clipboard::ClipboardPasteHost::create() {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = startup.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = startup.send(Ok(()));
                loop {
                    let command = receiver.recv().ok();
                    let Some(command) = command else {
                        break;
                    };
                    match command {
                        InsertionCommand::Insert {
                            transcript,
                            target,
                            add_trailing_space,
                        } => {
                            match insert_transcript(
                                &mut clipboard_host,
                                &events,
                                transcript.session_id,
                                target,
                                &transcript.text,
                                add_trailing_space,
                            ) {
                                Ok(()) => {}
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
                        } => match replace_selection(
                            &mut clipboard_host,
                            &events,
                            request_id,
                            target,
                            &text,
                        ) {
                            Ok(()) => {}
                            Err(error) => {
                                let _ = events.send(InputHostEvent::Failed {
                                    session_id: None,
                                    code: "replace_selection".to_owned(),
                                    message: error,
                                });
                            }
                        },
                        InsertionCommand::Shutdown => {
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

#[derive(Clone, Copy)]
struct InsertionTiming {
    started: Instant,
    method_started: Instant,
    release_wait_ms: u64,
}

fn insert_transcript(
    clipboard_host: &mut clipboard::ClipboardPasteHost,
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
    let timing = InsertionTiming {
        started,
        method_started: Instant::now(),
        release_wait_ms: elapsed_ms(started),
    };
    let payload = if add_trailing_space {
        Cow::Owned(format!("{text} "))
    } else {
        Cow::Borrowed(text)
    };
    if target::is_wezterm_target(target_snapshot) {
        match wezterm::send_text(target_snapshot, &payload) {
            Ok(()) => {
                send_dispatched(
                    events,
                    session_id,
                    InsertionMethod::WezTermCli,
                    InsertionDelivery::AdapterAcknowledged,
                    target_snapshot,
                    payload.chars().count(),
                    timing,
                );
                return Ok(());
            }
            Err(error) => {
                crate::diagnostic_line(&format!(
                    "insertion action=wezterm_adapter result=unicode_fallback session={session_id} \
target_pid={} error={error}",
                    target_snapshot.process_id
                ));
                if !inject::is_foreground(target_snapshot) {
                    return Err(format!(
                        "{error}; target changed before the Unicode fallback"
                    ));
                }
                inject::insert_text(target_snapshot, text, add_trailing_space).map_err(
                    |injection_error| {
                        format!("{error}; deterministic Unicode fallback failed: {injection_error}")
                    },
                )?;
                send_dispatched(
                    events,
                    session_id,
                    InsertionMethod::UnicodeInput,
                    InsertionDelivery::Unverified,
                    target_snapshot,
                    payload.chars().count(),
                    timing,
                );
                return Ok(());
            }
        }
    }
    let paste_shortcut = target::paste_shortcut(target_snapshot);
    crate::diagnostic_line(&format!(
        "clipboard_lease action=prepare session={session_id} target_pid={} target_hwnd={} foreground_hwnd={}",
        target_snapshot.process_id,
        target_snapshot.window_handle,
        inject::foreground_window_handle(),
    ));

    match clipboard::dispatch_paste(
        clipboard_host,
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut(paste_shortcut).map_err(|_| clipboard::ClipboardError::Paste),
        target_snapshot.process_id,
        text,
        add_trailing_space,
    ) {
        Ok(receipt) => {
            log_clipboard_lease(Some(session_id), None, &payload, target_snapshot, receipt);
            let delivery = match receipt.disposition {
                clipboard::ClipboardLeaseDisposition::RestoredAfterTargetRead => {
                    let _ = events.send(InputHostEvent::ClipboardRestored { session_id });
                    InsertionDelivery::ClipboardReadAcknowledged
                }
                clipboard::ClipboardLeaseDisposition::PreservedUnverified => {
                    InsertionDelivery::Unverified
                }
            };
            send_dispatched(
                events,
                session_id,
                InsertionMethod::ClipboardPaste,
                delivery,
                target_snapshot,
                text.chars().count(),
                timing,
            );
            Ok(())
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
                InsertionDelivery::Unverified,
                target_snapshot,
                text.chars().count(),
                timing,
            );
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn replace_selection(
    clipboard_host: &mut clipboard::ClipboardPasteHost,
    events: &Sender<InputHostEvent>,
    request_id: RequestId,
    target_snapshot: TargetSnapshot,
    text: &str,
) -> Result<(), String> {
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err("physical modifier keys did not become idle".to_owned());
    }
    let paste_shortcut = target::paste_shortcut(target_snapshot);

    match clipboard::dispatch_paste(
        clipboard_host,
        || inject::is_foreground(target_snapshot),
        || inject::paste_shortcut(paste_shortcut).map_err(|_| clipboard::ClipboardError::Paste),
        target_snapshot.process_id,
        text,
        false,
    ) {
        Ok(receipt) => {
            log_clipboard_lease(None, Some(request_id), text, target_snapshot, receipt);
            crate::diagnostic_line(&format!(
                "insertion action=replace_selection result=sent_unverified request={request_id} \
target_pid={} method=clipboard",
                target_snapshot.process_id
            ));
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            Ok(())
        }
        Err(clipboard_error) if inject::is_foreground(target_snapshot) => {
            inject::replace_selection(target_snapshot, text).map_err(|injection_error| {
                format!("{clipboard_error}; Unicode fallback failed: {injection_error}")
            })?;
            crate::diagnostic_line(&format!(
                "insertion action=replace_selection result=sent_unverified request={request_id} \
target_pid={} method=unicode",
                target_snapshot.process_id
            ));
            let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn log_clipboard_lease(
    session_id: Option<SessionId>,
    request_id: Option<RequestId>,
    payload: &str,
    target: TargetSnapshot,
    receipt: clipboard::ClipboardPasteReceipt,
) {
    let owner = session_id
        .map(|id| format!("session={id}"))
        .or_else(|| request_id.map(|id| format!("request={id}")))
        .unwrap_or_else(|| "owner=unknown".to_owned());
    crate::diagnostic_line(&format!(
        "clipboard_lease action=settle result={:?} {owner} target_pid={} chars={} \
payload_sha256={} original_sequence={} replacement_sequence={} settled_sequence={}",
        receipt.disposition,
        target.process_id,
        payload.chars().count(),
        payload_fingerprint(payload),
        receipt.original_sequence,
        receipt.replacement_sequence,
        receipt.settled_sequence,
    ));
}

fn payload_fingerprint(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn send_dispatched(
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    method: InsertionMethod,
    delivery: InsertionDelivery,
    target: TargetSnapshot,
    chars: usize,
    timing: InsertionTiming,
) {
    let method_ms = elapsed_ms(timing.method_started);
    let total_ms = elapsed_ms(timing.started);
    crate::diagnostic_line(&format!(
        "insertion action=insert result={delivery:?} session={session_id} method={method:?} \
target_pid={} chars={chars} release_wait_ms={} method_ms={method_ms} total_ms={total_ms}",
        target.process_id, timing.release_wait_ms,
    ));
    let _ = events.send(InputHostEvent::InsertionDispatched {
        session_id,
        method,
        delivery,
        dispatch_duration_ms: total_ms,
    });
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::payload_fingerprint;

    #[test]
    fn clipboard_payload_fingerprint_is_stable_and_non_plaintext() {
        let fingerprint = payload_fingerprint("SayFlex");

        assert_eq!(fingerprint, "cbb691b3059acace");
        assert_eq!(fingerprint.len(), 16);
        assert!(!fingerprint.contains("SayFlex"));
    }

    #[test]
    fn clipboard_payload_fingerprint_covers_trailing_space() {
        assert_ne!(
            payload_fingerprint("dictation"),
            payload_fingerprint("dictation ")
        );
    }
}
