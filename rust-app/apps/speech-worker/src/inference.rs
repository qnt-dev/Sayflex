use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::Instant,
};

use rust_protocol::{
    SessionId, SpeechOutputMode, SpeechWorkerEvent, Transcript, TranscriptionLatency,
};

use crate::{
    audio::RecordedAudio,
    engine::{AbortGeneration, EngineTranscript, WhisperEngine},
    history_actor::HistoryCommand,
};

const MAX_REUSABLE_SILENCE_MS: u64 = 1_200;
const MIN_SPECULATIVE_AUDIO_MS: u64 = 650;

#[derive(Debug, Clone, Copy)]
struct PreviewCandidate {
    session_id: SessionId,
    source_sample_count: usize,
    sample_rate_hz: u32,
    generation: u64,
}

struct PreviewResult {
    session_id: SessionId,
    output_mode: SpeechOutputMode,
    generation: u64,
    transcript: EngineTranscript,
}

enum InferenceCommand {
    Load {
        catalog_id: String,
        path: PathBuf,
        selected: bool,
    },
    Preview {
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        generation: u64,
    },
    Finalize {
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
        allow_speculative: bool,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct InferenceHandle {
    commands: SyncSender<InferenceCommand>,
    generation: Arc<AtomicU64>,
    active_session: Arc<Mutex<Option<SessionId>>>,
    candidate: Arc<Mutex<Option<PreviewCandidate>>>,
}

impl InferenceHandle {
    pub fn start(
        events: mpsc::Sender<SpeechWorkerEvent>,
        history: mpsc::Sender<HistoryCommand>,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::sync_channel(4);
        let generation = Arc::new(AtomicU64::new(1));
        let actor_generation = Arc::clone(&generation);
        thread::Builder::new()
            .name("speech-inference".to_owned())
            .spawn(move || inference_loop(receiver, actor_generation, events, history))?;
        Ok(Self {
            commands,
            generation,
            active_session: Arc::new(Mutex::new(None)),
            candidate: Arc::new(Mutex::new(None)),
        })
    }

    pub fn load_model(&self, catalog_id: String, path: PathBuf, selected: bool) {
        self.invalidate();
        if let Err(error) = self.commands.send(InferenceCommand::Load {
            catalog_id,
            path,
            selected,
        }) {
            eprintln!("could not queue speech model load: {error}");
        }
    }

    pub fn begin_session(&self, session_id: SessionId) {
        self.invalidate();
        *self
            .active_session
            .lock()
            .expect("inference active-session mutex poisoned") = Some(session_id);
    }

    pub fn cancel_session(&self, session_id: SessionId) {
        let mut active = self
            .active_session
            .lock()
            .expect("inference active-session mutex poisoned");
        if *active == Some(session_id) {
            *active = None;
            drop(active);
            self.invalidate();
        }
    }

    pub fn request_preview(&self, recording: RecordedAudio, output_mode: SpeechOutputMode) -> bool {
        if recording.duration_ms < MIN_SPECULATIVE_AUDIO_MS
            || *self
                .active_session
                .lock()
                .expect("inference active-session mutex poisoned")
                != Some(recording.session_id)
        {
            return false;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let candidate = PreviewCandidate {
            session_id: recording.session_id,
            source_sample_count: recording.samples.len(),
            sample_rate_hz: recording.sample_rate_hz,
            generation,
        };
        match self.commands.try_send(InferenceCommand::Preview {
            recording,
            output_mode,
            generation,
        }) {
            Ok(()) => {
                *self
                    .candidate
                    .lock()
                    .expect("inference candidate mutex poisoned") = Some(candidate);
                true
            }
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn invalidate_preview_if_speech_resumed(
        &self,
        session_id: SessionId,
        recording: &RecordedAudio,
    ) {
        let candidate = *self
            .candidate
            .lock()
            .expect("inference candidate mutex poisoned");
        let Some(candidate) = candidate.filter(|candidate| candidate.session_id == session_id)
        else {
            return;
        };
        if tail_contains_speech(recording, candidate.source_sample_count) {
            self.invalidate();
        }
    }

    pub fn finalize(
        &self,
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    ) {
        *self
            .active_session
            .lock()
            .expect("inference active-session mutex poisoned") = None;
        let candidate = *self
            .candidate
            .lock()
            .expect("inference candidate mutex poisoned");
        let allow_speculative = candidate.is_some_and(|candidate| {
            candidate.session_id == recording.session_id
                && candidate.generation == self.generation.load(Ordering::Acquire)
                && candidate.sample_rate_hz == recording.sample_rate_hz
                && reusable_tail(&recording, candidate.source_sample_count)
        });
        if !allow_speculative {
            self.invalidate();
        }
        if let Err(error) = self.commands.send(InferenceCommand::Finalize {
            recording,
            output_mode,
            audio_finalize_ms,
            release_started,
            allow_speculative,
        }) {
            eprintln!("could not queue final transcription: {error}");
        }
    }

    pub fn shutdown(&self) {
        self.invalidate();
        let _ = self.commands.send(InferenceCommand::Shutdown);
    }

    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self
            .candidate
            .lock()
            .expect("inference candidate mutex poisoned") = None;
    }
}

fn inference_loop(
    commands: mpsc::Receiver<InferenceCommand>,
    generation: Arc<AtomicU64>,
    events: mpsc::Sender<SpeechWorkerEvent>,
    history: mpsc::Sender<HistoryCommand>,
) {
    let mut engine = WhisperEngine::new();
    let mut latest_preview: Option<PreviewResult> = None;
    for command in commands {
        match command {
            InferenceCommand::Load {
                catalog_id,
                path,
                selected,
            } => {
                latest_preview = None;
                match engine.prepare(&path) {
                    Ok(prepared) => {
                        let _ = events.send(SpeechWorkerEvent::ModelReady {
                            catalog_id: catalog_id.clone(),
                            path: path.display().to_string(),
                            backend: prepared.backend.to_owned(),
                            load_duration_ms: prepared.load_duration_ms,
                            warmup_duration_ms: prepared.warmup_duration_ms,
                        });
                        if selected {
                            let _ = events.send(SpeechWorkerEvent::ModelSelected { catalog_id });
                        }
                    }
                    Err(error) => {
                        let _ = events.send(SpeechWorkerEvent::Failed {
                            session_id: None,
                            code: "model_prepare".to_owned(),
                            message: format!("{error:#}"),
                            retryable: true,
                        });
                    }
                }
            }
            InferenceCommand::Preview {
                recording,
                output_mode,
                generation: preview_generation,
            } => {
                if generation.load(Ordering::Acquire) != preview_generation {
                    continue;
                }
                let result = engine.transcribe(
                    &recording,
                    output_mode,
                    true,
                    Some(AbortGeneration::new(
                        Arc::clone(&generation),
                        preview_generation,
                    )),
                );
                if generation.load(Ordering::Acquire) != preview_generation {
                    continue;
                }
                match result {
                    Ok(transcript) => {
                        latest_preview = Some(PreviewResult {
                            session_id: recording.session_id,
                            output_mode,
                            generation: preview_generation,
                            transcript,
                        });
                    }
                    Err(error) => {
                        eprintln!(
                            "speculative transcript was not usable for {}: {error:#}",
                            recording.session_id
                        );
                    }
                }
            }
            InferenceCommand::Finalize {
                recording,
                output_mode,
                audio_finalize_ms,
                release_started,
                allow_speculative,
            } => {
                let reusable = latest_preview.take().filter(|preview| {
                    allow_speculative
                        && preview.session_id == recording.session_id
                        && preview.output_mode == output_mode
                        && preview.generation == generation.load(Ordering::Acquire)
                        && preview.transcript.source_sample_count <= recording.samples.len()
                        && reusable_tail(&recording, preview.transcript.source_sample_count)
                });
                let (engine_result, speculative_reuse) = match reusable {
                    Some(preview) => (Ok(preview.transcript), true),
                    None => (
                        engine.transcribe(&recording, output_mode, false, None),
                        false,
                    ),
                };
                match engine_result {
                    Ok(result) => {
                        let created_unix_ms = crate::unix_ms();
                        let transcript = Transcript {
                            session_id: recording.session_id,
                            text: result.text,
                            language: result.language,
                            audio_duration_ms: result.audio_duration_ms,
                            processing_duration_ms: result.inference_ms,
                            latency: TranscriptionLatency {
                                audio_finalize_ms,
                                preprocessing_ms: result.preprocessing_ms,
                                inference_ms: result.inference_ms,
                                after_release_ms: elapsed_ms(release_started),
                                speculative_reuse,
                            },
                        };
                        // User-visible completion is deliberately ahead of durable history I/O.
                        let _ = events.send(SpeechWorkerEvent::Completed {
                            transcript: transcript.clone(),
                            created_unix_ms,
                        });
                        let _ = history.send(HistoryCommand::Append {
                            transcript,
                            created_unix_ms,
                        });
                    }
                    Err(error) => {
                        let _ = events.send(SpeechWorkerEvent::Failed {
                            session_id: Some(recording.session_id),
                            code: "transcription".to_owned(),
                            message: format!("{error:#}"),
                            retryable: true,
                        });
                    }
                }
            }
            InferenceCommand::Shutdown => break,
        }
    }
}

fn reusable_tail(recording: &RecordedAudio, source_sample_count: usize) -> bool {
    if source_sample_count > recording.samples.len() {
        return false;
    }
    let tail_samples = recording.samples.len() - source_sample_count;
    let tail_ms = tail_samples as u64 * 1000 / u64::from(recording.sample_rate_hz.max(1));
    tail_ms <= MAX_REUSABLE_SILENCE_MS && !tail_contains_speech(recording, source_sample_count)
}

fn tail_contains_speech(recording: &RecordedAudio, source_sample_count: usize) -> bool {
    if source_sample_count >= recording.samples.len() {
        return false;
    }
    let source = &recording.samples[..source_sample_count];
    let tail = &recording.samples[source_sample_count..];
    let reference_rms = rms(source);
    // This intentionally favors correctness over reuse. Noise can cause a full fallback;
    // quiet speech must never be discarded merely to hit a latency target.
    let threshold = (reference_rms * 0.18).clamp(0.006, 0.024);
    let block = (recording.sample_rate_hz as usize / 50).max(1);
    tail.chunks(block).any(|samples| rms(samples) > threshold)
}

pub fn has_trailing_silence(recording: &RecordedAudio, silence_ms: u64) -> bool {
    let silence_samples = (u64::from(recording.sample_rate_hz) * silence_ms / 1000) as usize;
    if recording.samples.len() <= silence_samples || silence_samples == 0 {
        return false;
    }
    let split = recording.samples.len() - silence_samples;
    !tail_contains_speech(recording, split)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use rust_protocol::SessionId;

    use super::{has_trailing_silence, reusable_tail};
    use crate::audio::RecordedAudio;

    fn recording(samples: Vec<f32>, rate: u32) -> RecordedAudio {
        RecordedAudio {
            session_id: SessionId::new(),
            duration_ms: samples.len() as u64 * 1000 / u64::from(rate),
            samples,
            sample_rate_hz: rate,
        }
    }

    #[test]
    fn speculative_result_is_reused_only_for_a_silent_short_tail() {
        let mut samples = vec![0.08_f32; 16_000];
        samples.extend(vec![0.0; 4_000]);
        let audio = recording(samples, 16_000);
        assert!(has_trailing_silence(&audio, 200));
        assert!(reusable_tail(&audio, 16_000));
    }

    #[test]
    fn speech_in_tail_forces_full_finalization() {
        let mut samples = vec![0.08_f32; 16_000];
        samples.extend(vec![0.07; 2_000]);
        let audio = recording(samples, 16_000);
        assert!(!reusable_tail(&audio, 16_000));
    }
}
