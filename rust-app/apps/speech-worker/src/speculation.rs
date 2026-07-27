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
const EFFICIENT_MAX_PREVIEWS_PER_SESSION: u32 = 2;

enum SpeculationCommand {
    Begin {
        session_id: SessionId,
        output_mode: SpeechOutputMode,
        fast_release_previews: bool,
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
    preview_count: u32,
    fast_release_previews: bool,
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
                    let command = if active.is_some() {
                        receiver.recv_timeout(SCHEDULER_TICK)
                    } else {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match command {
                        Ok(SpeculationCommand::Begin {
                            session_id,
                            output_mode,
                            fast_release_previews,
                        }) => {
                            active = Some(ActiveSession {
                                session_id,
                                output_mode,
                                started: Instant::now(),
                                last_preview_at: None,
                                last_preview_samples: 0,
                                preview_count: 0,
                                fast_release_previews,
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
                    if !preview_budget_available(
                        session.fast_release_previews,
                        session.preview_count,
                    ) || session.started.elapsed() < FIRST_PREVIEW_AFTER
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
                        session.preview_count = session.preview_count.saturating_add(1);
                    }
                }
            })?;
        Ok(Self { commands })
    }

    pub fn begin(
        &self,
        session_id: SessionId,
        output_mode: SpeechOutputMode,
        fast_release_previews: bool,
    ) {
        let _ = self.commands.send(SpeculationCommand::Begin {
            session_id,
            output_mode,
            fast_release_previews,
        });
    }

    pub fn stop(&self, session_id: SessionId) {
        let _ = self.commands.send(SpeculationCommand::Stop { session_id });
    }

    pub fn shutdown(&self) {
        let _ = self.commands.send(SpeculationCommand::Shutdown);
    }
}

const fn preview_budget_available(fast_release_previews: bool, preview_count: u32) -> bool {
    fast_release_previews || preview_count < EFFICIENT_MAX_PREVIEWS_PER_SESSION
}

#[cfg(test)]
mod tests {
    use super::preview_budget_available;

    #[test]
    fn fastest_profile_never_silently_caps_release_preparation() {
        assert!(preview_budget_available(true, u32::MAX));
    }

    #[test]
    fn efficient_profile_has_an_explicit_two_preview_budget() {
        assert!(preview_budget_available(false, 0));
        assert!(preview_budget_available(false, 1));
        assert!(!preview_budget_available(false, 2));
    }
}
