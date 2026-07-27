use std::{
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

use rust_protocol::{SessionId, SpeechOutputMode};

use crate::{
    audio::AudioSnapshotSource,
    inference::{InferenceHandle, has_trailing_silence},
};

const SCHEDULER_TICK: Duration = Duration::from_millis(45);
const FIRST_PREVIEW_AFTER: Duration = Duration::from_millis(650);
const MIN_NEW_AUDIO_MS: u64 = 420;
const PREVIEW_COOLDOWN: Duration = Duration::from_millis(850);
const REQUIRED_TRAILING_SILENCE_MS: u64 = 170;

enum SpeculationCommand {
    Begin {
        session_id: SessionId,
        output_mode: SpeechOutputMode,
    },
    Stop {
        session_id: SessionId,
    },
    Shutdown,
}

struct ActiveSession {
    session_id: SessionId,
    output_mode: SpeechOutputMode,
    started: Instant,
    last_preview_at: Option<Instant>,
    last_preview_samples: usize,
}

pub struct SpeculationScheduler {
    commands: Sender<SpeculationCommand>,
}

impl SpeculationScheduler {
    pub fn start(
        snapshots: AudioSnapshotSource,
        inference: InferenceHandle,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("speech-speculation-scheduler".to_owned())
            .spawn(move || {
                let mut active: Option<ActiveSession> = None;
                loop {
                    match receiver.recv_timeout(SCHEDULER_TICK) {
                        Ok(SpeculationCommand::Begin {
                            session_id,
                            output_mode,
                        }) => {
                            active = Some(ActiveSession {
                                session_id,
                                output_mode,
                                started: Instant::now(),
                                last_preview_at: None,
                                last_preview_samples: 0,
                            });
                        }
                        Ok(SpeculationCommand::Stop { session_id }) => {
                            if active
                                .as_ref()
                                .is_some_and(|session| session.session_id == session_id)
                            {
                                active = None;
                            }
                        }
                        Ok(SpeculationCommand::Shutdown)
                        | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }

                    let Some(session) = active.as_mut() else {
                        continue;
                    };
                    if session.started.elapsed() < FIRST_PREVIEW_AFTER
                        || session
                            .last_preview_at
                            .is_some_and(|last| last.elapsed() < PREVIEW_COOLDOWN)
                    {
                        continue;
                    }
                    let Some(recording) = snapshots.snapshot(session.session_id) else {
                        continue;
                    };
                    inference.invalidate_preview_if_speech_resumed(session.session_id, &recording);
                    let new_audio_ms = recording
                        .samples
                        .len()
                        .saturating_sub(session.last_preview_samples)
                        as u64
                        * 1000
                        / u64::from(recording.sample_rate_hz.max(1));
                    if new_audio_ms < MIN_NEW_AUDIO_MS
                        || !has_trailing_silence(&recording, REQUIRED_TRAILING_SILENCE_MS)
                    {
                        continue;
                    }
                    let sample_count = recording.samples.len();
                    if inference.request_preview(recording, session.output_mode) {
                        session.last_preview_at = Some(Instant::now());
                        session.last_preview_samples = sample_count;
                    }
                }
            })?;
        Ok(Self { commands })
    }

    pub fn begin(&self, session_id: SessionId, output_mode: SpeechOutputMode) {
        let _ = self.commands.send(SpeculationCommand::Begin {
            session_id,
            output_mode,
        });
    }

    pub fn stop(&self, session_id: SessionId) {
        let _ = self.commands.send(SpeculationCommand::Stop { session_id });
    }

    pub fn shutdown(&self) {
        let _ = self.commands.send(SpeculationCommand::Shutdown);
    }
}
