use std::{
    io,
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

use crate::{clipboard, hotkeys, insertion_adapter};
use rust_protocol::{
    InputHostEvent, InsertionDelivery, InsertionMethod, RequestId, SessionId, TargetSnapshot,
    Transcript,
};

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
        let (startup, startup_result) = mpsc::sync_channel::<Result<(), String>>(1);
        thread::Builder::new()
            .name("input-insertion-worker".to_owned())
            .spawn(move || {
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
    let receipt = insertion_adapter::deliver(target_snapshot, text, add_trailing_space)?;
    let chars = text.chars().count() + usize::from(add_trailing_space);
    send_dispatched(
        events,
        session_id,
        receipt.method,
        receipt.delivery,
        target_snapshot,
        chars,
        timing,
    );
    Ok(())
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
    let receipt = insertion_adapter::deliver(target_snapshot, text, false)?;
    crate::diagnostic_line(&format!(
        "insertion action=replace_selection result={:?} request={request_id} \
target_pid={} method={:?} clipboard=untouched",
        receipt.delivery, target_snapshot.process_id, receipt.method,
    ));
    let _ = events.send(InputHostEvent::SelectionReplaced { request_id });
    Ok(())
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
