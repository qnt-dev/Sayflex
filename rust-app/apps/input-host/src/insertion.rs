use std::{
    collections::HashSet,
    io,
    ops::ControlFlow,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{clipboard, hotkeys, insertion_adapter, sta_dispatcher};
use rust_protocol::{
    InputHostEvent, InsertionDelivery, InsertionMethod, RequestId, SessionId, TargetSnapshot,
    Transcript,
};
use sha2::{Digest, Sha256};

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
    commands: sta_dispatcher::StaCommandSender<InsertionCommand>,
    pending_inserts: Arc<Mutex<HashSet<InsertionKey>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct InsertionKey {
    session_id: SessionId,
    target_process_id: u32,
    target_window_handle: u64,
}

impl InsertionKey {
    fn new(transcript: &Transcript, target: TargetSnapshot) -> Self {
        Self {
            session_id: transcript.session_id,
            target_process_id: target.process_id,
            target_window_handle: target.window_handle,
        }
    }
}

impl InsertionService {
    pub fn start(events: Sender<InputHostEvent>) -> std::io::Result<Self> {
        let (commands, dispatcher) = sta_dispatcher::StaDispatcher::channel();
        let pending_inserts = Arc::new(Mutex::new(HashSet::new()));
        let worker_pending_inserts = Arc::clone(&pending_inserts);
        let (startup, startup_result) = mpsc::sync_channel::<Result<(), String>>(1);
        thread::Builder::new()
            .name("input-insertion-worker".to_owned())
            .spawn(move || {
                if let Err(error) = dispatcher.bind_current_thread() {
                    let _ = startup.send(Err(error.to_string()));
                    return;
                }
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
                let run_result = dispatcher.run(|command| match command {
                        InsertionCommand::Insert {
                            transcript,
                            target,
                            add_trailing_space,
                        } => {
                            let key = InsertionKey::new(&transcript, target);
                            match insert_transcript(
                                &mut clipboard_host,
                                &events,
                                transcript.session_id,
                                target,
                                &transcript.text,
                                add_trailing_space,
                            ) {
                                Ok(()) => {}
                                Err(InsertFailure { code, message }) => {
                                    crate::diagnostic_line(&format!(
                                        "insertion action=insert result=failed session={} target_pid={} code={code} error={message}",
                                        transcript.session_id, target.process_id
                                    ));
                                    let _ = events.send(InputHostEvent::Failed {
                                        session_id: Some(transcript.session_id),
                                        code: code.to_owned(),
                                        message,
                                    });
                                }
                            }
                            worker_pending_inserts
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .remove(&key);
                            ControlFlow::Continue(())
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
                            ControlFlow::Continue(())
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
                            ControlFlow::Continue(())
                        }
                        InsertionCommand::ReplaceSelection {
                            request_id,
                            target,
                            text,
                        } => {
                            match replace_selection(
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
                            }
                            ControlFlow::Continue(())
                        }
                        InsertionCommand::Shutdown => ControlFlow::Break(()),
                    });
                if let Err(error) = run_result {
                    crate::diagnostic_line(&format!(
                        "insertion_worker lifecycle=failed stage=message_loop error={error}"
                    ));
                }
            })?;
        match startup_result.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                pending_inserts,
            }),
            Ok(Err(message)) => Err(io::Error::other(message)),
            Err(error) => Err(io::Error::other(format!(
                "insertion worker startup channel closed: {error}"
            ))),
        }
    }

    pub fn insert(&self, transcript: Transcript, target: TargetSnapshot, add_trailing_space: bool) {
        let key = InsertionKey::new(&transcript, target);
        let inserted = self
            .pending_inserts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key);
        if !inserted {
            crate::diagnostic_line(&format!(
                "insertion action=insert state=coalesced session={} target_pid={} target_hwnd={} trailing_space={}",
                transcript.session_id, target.process_id, target.window_handle, add_trailing_space,
            ));
            return;
        }
        if self
            .commands
            .send(InsertionCommand::Insert {
                transcript,
                target,
                add_trailing_space,
            })
            .is_err()
        {
            self.pending_inserts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&key);
        }
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

struct InsertionSession {
    session_id: SessionId,
    target: TargetSnapshot,
    payload: String,
    chars: usize,
}

impl InsertionSession {
    fn new(
        session_id: SessionId,
        target: TargetSnapshot,
        text: &str,
        add_trailing_space: bool,
    ) -> Self {
        let payload = if add_trailing_space {
            format!("{text} ")
        } else {
            text.to_owned()
        };
        let chars = payload.chars().count();
        Self {
            session_id,
            target,
            payload,
            chars,
        }
    }
}

/// Why a dictation was not inserted. The code tells the desktop which
/// explanation to show; the message stays in the diagnostic log.
struct InsertFailure {
    code: &'static str,
    message: String,
}

fn insert_transcript(
    clipboard_host: &mut clipboard::ClipboardPasteHost,
    events: &Sender<InputHostEvent>,
    session_id: SessionId,
    target_snapshot: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<(), InsertFailure> {
    let started = Instant::now();
    let session = InsertionSession::new(session_id, target_snapshot, text, add_trailing_space);
    crate::diagnostic_line(&format!(
        "insertion action=insert state=prepared session={} target_pid={} chars={} payload_sha256={}",
        session.session_id,
        session.target.process_id,
        session.chars,
        payload_fingerprint(&session.payload),
    ));
    if !hotkeys::wait_until_injection_safe(INJECTION_IDLE_TIMEOUT) {
        return Err(InsertFailure {
            code: "insert_modifiers_busy",
            message: "physical modifier keys did not become idle; transcript remains in history"
                .to_owned(),
        });
    }
    let timing = InsertionTiming {
        started,
        method_started: Instant::now(),
        release_wait_ms: elapsed_ms(started),
    };
    let mut announce_dispatch = || {
        let _ = events.send(InputHostEvent::InsertionDispatched {
            session_id: session.session_id,
        });
    };
    let receipt = match insertion_adapter::deliver(
        clipboard_host,
        session.target,
        &session.payload,
        &mut announce_dispatch,
    ) {
        Ok(receipt) => receipt,
        Err(failure) => {
            let foreground = if failure.state == insertion_adapter::DeliveryFailureState::Superseded
            {
                format!(" {}", crate::target::describe_foreground())
            } else {
                String::new()
            };
            crate::diagnostic_line(&format!(
                "insertion action=insert state={:?} session={} stage={} method={:?} target_pid={} target_hwnd={} chars={} payload_sha256={} error={}{foreground}",
                failure.state,
                session.session_id,
                failure.stage,
                failure.method,
                session.target.process_id,
                session.target.window_handle,
                session.chars,
                payload_fingerprint(&session.payload),
                failure.message,
            ));
            return Err(InsertFailure {
                code: match failure.state {
                    insertion_adapter::DeliveryFailureState::Superseded => "insert_target_changed",
                    insertion_adapter::DeliveryFailureState::Ambiguous => "insert_unconfirmed",
                    insertion_adapter::DeliveryFailureState::NotDelivered => "insert_not_delivered",
                },
                message: format!("{:?}: {}", failure.state, failure.message),
            });
        }
    };
    if let Some(clipboard_receipt) = receipt.clipboard_receipt {
        crate::diagnostic_line(&format!(
            "clipboard_transaction action=settle session={} result={:?} original_sequence={} replacement_sequence={} settled_sequence={} ansi_renders={} settled_owner_pid={}",
            session.session_id,
            clipboard_receipt.disposition,
            clipboard_receipt.original_sequence,
            clipboard_receipt.replacement_sequence,
            clipboard_receipt.settled_sequence,
            clipboard_receipt.ansi_renders,
            clipboard_receipt.settled_owner_process_id,
        ));
    }
    send_completed(
        events,
        session.session_id,
        receipt.method,
        receipt.delivery,
        session.target,
        session.chars,
        timing,
    );
    Ok(())
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
    let receipt = insertion_adapter::deliver(clipboard_host, target_snapshot, text, &mut || {})
        .map_err(
        |failure| {
            crate::diagnostic_line(&format!(
                "insertion action=replace_selection state={:?} request={request_id} target_pid={} method={:?} error={}",
                failure.state, target_snapshot.process_id, failure.method, failure.message,
            ));
            format!("{:?}: {}", failure.state, failure.message)
        },
    )?;
    if let Some(clipboard_receipt) = receipt.clipboard_receipt {
        crate::diagnostic_line(&format!(
            "clipboard_transaction action=settle request={request_id} result={:?} original_sequence={} replacement_sequence={} settled_sequence={}",
            clipboard_receipt.disposition,
            clipboard_receipt.original_sequence,
            clipboard_receipt.replacement_sequence,
            clipboard_receipt.settled_sequence,
        ));
    }
    crate::diagnostic_line(&format!(
        "insertion action=replace_selection state={:?} request={request_id} \
target_pid={} method={:?}",
        receipt.delivery, target_snapshot.process_id, receipt.method,
    ));
    let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
    Ok(())
}

fn send_completed(
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
    let _ = events.send(InputHostEvent::InsertionCompleted {
        session_id,
        method,
        delivery,
        dispatch_duration_ms: total_ms,
    });
}

fn payload_fingerprint(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{InsertionKey, payload_fingerprint};
    use rust_protocol::{SessionId, TargetSnapshot, Transcript, TranscriptionLatency};
    use std::collections::HashSet;

    fn transcript(session_id: SessionId) -> Transcript {
        Transcript {
            session_id,
            text: "payload".to_owned(),
            language: "ru".to_owned(),
            audio_duration_ms: 0,
            processing_duration_ms: 0,
            provider: None,
            latency: TranscriptionLatency::default(),
        }
    }

    #[test]
    fn payload_fingerprint_is_stable_and_does_not_log_plaintext() {
        let fingerprint = payload_fingerprint("SayFlex");
        assert_eq!(fingerprint, "cbb691b3059acace");
        assert_eq!(fingerprint.len(), 16);
        assert!(!fingerprint.contains("SayFlex"));
    }

    #[test]
    fn trailing_space_has_a_distinct_session_payload_identity() {
        assert_ne!(
            payload_fingerprint("dictation"),
            payload_fingerprint("dictation ")
        );
    }

    #[test]
    fn duplicate_insert_last_is_coalesced_only_while_the_same_delivery_is_pending() {
        let session_id = SessionId::new();
        let transcript = transcript(session_id);
        let target = TargetSnapshot {
            window_handle: 42,
            process_id: 7,
            ..TargetSnapshot::default()
        };
        let key = InsertionKey::new(&transcript, target);
        let mut pending = HashSet::new();
        assert!(pending.insert(key));
        assert!(!pending.insert(key));
        assert!(pending.remove(&key));
        assert!(pending.insert(key));
    }

    #[test]
    fn automatic_and_explicit_requests_for_one_session_are_coalesced_while_pending() {
        let session_id = SessionId::new();
        let transcript = transcript(session_id);
        let target = TargetSnapshot {
            window_handle: 42,
            process_id: 7,
            ..TargetSnapshot::default()
        };
        assert_eq!(
            InsertionKey::new(&transcript, target),
            InsertionKey::new(&transcript, target),
        );
    }
}
