use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use rust_protocol::{
    DictationLanguage, SessionId, SpeechOutputMode, SpeechWorkerEvent, Transcript,
    TranscriptionLatency,
};

use crate::{
    audio::RecordedAudio,
    engine::{AbortGeneration, EngineTranscript, WhisperEngine},
    history_actor::HistoryCommand,
};

const MAX_PREVIEW_COMPLETION_TAIL_MS: u64 = 1_200;
const MIN_SPECULATIVE_AUDIO_MS: u64 = 650;
const MIN_FINAL_TAIL_AUDIO_MS: u64 = 180;

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
    transcript: EngineTranscript,
}

enum InferenceCommand {
    Load {
        catalog_id: String,
        path: PathBuf,
        selected: bool,
    },
    SetLanguage(DictationLanguage),
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
    },
    /// Releases the local model and its backend allocations. Cloud-only mode
    /// must not keep VRAM or RAM occupied by an engine it cannot route to.
    Unload {
        only_catalog_id: Option<String>,
        acknowledged: Option<SyncSender<Option<String>>>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct InferenceHandle {
    commands: Sender<InferenceCommand>,
    generation: Arc<AtomicU64>,
    active_session: Arc<Mutex<Option<SessionId>>>,
    candidate: Arc<Mutex<Option<PreviewCandidate>>>,
    preview_in_flight: Arc<AtomicBool>,
    stable_source_sample_count: Arc<AtomicUsize>,
}

impl InferenceHandle {
    pub fn start(
        events: mpsc::Sender<SpeechWorkerEvent>,
        history: mpsc::Sender<HistoryCommand>,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(1));
        let actor_generation = Arc::clone(&generation);
        let preview_in_flight = Arc::new(AtomicBool::new(false));
        let actor_preview_in_flight = Arc::clone(&preview_in_flight);
        let stable_source_sample_count = Arc::new(AtomicUsize::new(0));
        let actor_stable_source_sample_count = Arc::clone(&stable_source_sample_count);
        thread::Builder::new()
            .name("speech-inference".to_owned())
            .spawn(move || {
                inference_loop(
                    receiver,
                    actor_generation,
                    actor_preview_in_flight,
                    actor_stable_source_sample_count,
                    events,
                    history,
                )
            })?;
        Ok(Self {
            commands,
            generation,
            active_session: Arc::new(Mutex::new(None)),
            candidate: Arc::new(Mutex::new(None)),
            preview_in_flight,
            stable_source_sample_count,
        })
    }

    pub fn load_model(&self, catalog_id: String, path: PathBuf, selected: bool) {
        self.reset_segmented_session();
        if let Err(error) = self.commands.send(InferenceCommand::Load {
            catalog_id,
            path,
            selected,
        }) {
            eprintln!("could not queue speech model load: {error}");
        }
    }

    pub fn unload_model(&self) {
        self.reset_segmented_session();
        if let Err(error) = self.commands.send(InferenceCommand::Unload {
            only_catalog_id: None,
            acknowledged: None,
        }) {
            eprintln!("could not queue speech model unload: {error}");
        }
    }

    /// Waits until the inference actor has dropped `catalog_id`, if that is the
    /// model it currently owns. Returning `None` means another model (or no
    /// model) was resident, so deleting this catalog entry is still safe.
    pub fn unload_model_if_and_wait(&self, catalog_id: &str) -> Result<Option<String>, String> {
        self.reset_segmented_session();
        let (acknowledged, answer) = mpsc::sync_channel(0);
        self.commands
            .send(InferenceCommand::Unload {
                only_catalog_id: Some(catalog_id.to_owned()),
                acknowledged: Some(acknowledged),
            })
            .map_err(|error| format!("could not queue speech model unload: {error}"))?;
        answer
            .recv_timeout(Duration::from_secs(30))
            .map_err(|error| format!("speech model unload was not acknowledged: {error}"))
    }

    #[must_use]
    pub fn has_active_session(&self) -> bool {
        recover_lock(&self.active_session).is_some()
    }

    pub fn set_language(&self, language: DictationLanguage) {
        self.reset_segmented_session();
        if let Err(error) = self.commands.send(InferenceCommand::SetLanguage(language)) {
            eprintln!("could not queue speech language change: {error}");
        }
    }

    pub fn begin_session(&self, session_id: SessionId) {
        self.reset_segmented_session();
        *recover_lock(&self.active_session) = Some(session_id);
    }

    pub fn cancel_session(&self, session_id: SessionId) {
        let mut active = recover_lock(&self.active_session);
        if *active == Some(session_id) {
            *active = None;
            drop(active);
            self.reset_segmented_session();
        }
    }

    #[must_use]
    pub fn preview_in_flight(&self) -> bool {
        self.preview_in_flight.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn stable_source_sample_count(&self) -> usize {
        self.stable_source_sample_count.load(Ordering::Acquire)
    }

    /// Cancels a preview that no longer reaches the live audio edge. Already
    /// committed segments remain available as an authoritative stable prefix.
    pub fn cancel_stale_preview_for_new_audio(&self) {
        self.cancel_preview_generation();
    }

    pub fn request_preview(&self, recording: RecordedAudio, output_mode: SpeechOutputMode) -> bool {
        if recording.segment_duration_ms() < MIN_SPECULATIVE_AUDIO_MS
            || *recover_lock(&self.active_session) != Some(recording.session_id)
        {
            return false;
        }
        if self
            .preview_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let candidate = PreviewCandidate {
            session_id: recording.session_id,
            source_sample_count: recording.source_total_samples,
            sample_rate_hz: recording.sample_rate_hz,
            generation,
        };
        match self.commands.send(InferenceCommand::Preview {
            recording,
            output_mode,
            generation,
        }) {
            Ok(()) => {
                *recover_lock(&self.candidate) = Some(candidate);
                true
            }
            Err(_) => {
                self.preview_in_flight.store(false, Ordering::Release);
                false
            }
        }
    }

    pub fn finalize(
        &self,
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    ) {
        *recover_lock(&self.active_session) = None;
        let candidate = *recover_lock(&self.candidate);
        let allow_preview_completion = candidate.is_some_and(|candidate| {
            candidate.session_id == recording.session_id
                && candidate.generation == self.generation.load(Ordering::Acquire)
                && candidate.sample_rate_hz == recording.sample_rate_hz
                && quiet_tail_after(&recording, candidate.source_sample_count)
        });
        if !allow_preview_completion {
            self.cancel_preview_generation();
        }
        if let Err(error) = self.commands.send(InferenceCommand::Finalize {
            recording,
            output_mode,
            audio_finalize_ms,
            release_started,
        }) {
            eprintln!("could not queue final transcription: {error}");
        }
    }

    pub fn shutdown(&self) {
        self.reset_segmented_session();
        let _ = self.commands.send(InferenceCommand::Shutdown);
    }

    fn cancel_preview_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *recover_lock(&self.candidate) = None;
    }

    fn reset_segmented_session(&self) {
        self.cancel_preview_generation();
        self.stable_source_sample_count.store(0, Ordering::Release);
    }
}

fn inference_loop(
    commands: mpsc::Receiver<InferenceCommand>,
    generation: Arc<AtomicU64>,
    preview_in_flight: Arc<AtomicBool>,
    stable_source_sample_count: Arc<AtomicUsize>,
    events: mpsc::Sender<SpeechWorkerEvent>,
    history: mpsc::Sender<HistoryCommand>,
) {
    let mut engine = WhisperEngine::new();
    let mut loaded_catalog_id: Option<String> = None;
    let mut stable_prefix: Option<PreviewResult> = None;
    for command in commands {
        match command {
            InferenceCommand::Load {
                catalog_id,
                path,
                selected,
            } => {
                stable_prefix = None;
                match engine.prepare(&path) {
                    Ok(prepared) => {
                        let _ = events.send(SpeechWorkerEvent::ModelReady {
                            catalog_id: catalog_id.clone(),
                            path: path.display().to_string(),
                            backend: prepared.backend.to_owned(),
                            load_duration_ms: prepared.load_duration_ms,
                            warmup_duration_ms: prepared.warmup_duration_ms,
                        });
                        loaded_catalog_id = Some(catalog_id.clone());
                        let _ = events.send(SpeechWorkerEvent::LocalModelState {
                            catalog_id: loaded_catalog_id.clone(),
                            loaded: true,
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
            InferenceCommand::SetLanguage(language) => {
                stable_prefix = None;
                engine.set_language(language.locale());
            }
            InferenceCommand::Preview {
                recording,
                output_mode,
                generation: preview_generation,
            } => {
                let _preview_guard = PreviewInFlightGuard(&preview_in_flight);
                if generation.load(Ordering::Acquire) != preview_generation {
                    continue;
                }
                let existing_prefix = stable_prefix
                    .as_ref()
                    .filter(|preview| {
                        preview.session_id == recording.session_id
                            && preview.output_mode == output_mode
                            && preview.transcript.source_sample_count
                                == recording.source_start_sample
                    })
                    .map(|preview| preview.transcript.clone());
                let prompt = existing_prefix
                    .as_ref()
                    .map(|prefix| context_prompt(&prefix.text));
                let result = engine.transcribe_with_prompt(
                    &recording,
                    output_mode,
                    prompt.as_deref(),
                    Some(AbortGeneration::new(
                        Arc::clone(&generation),
                        preview_generation,
                    )),
                );
                if generation.load(Ordering::Acquire) != preview_generation {
                    continue;
                }
                match result {
                    Ok(preview) => {
                        if let Some(transcript) =
                            advance_stable_prefix(existing_prefix.as_ref(), &preview)
                        {
                            stable_source_sample_count
                                .store(transcript.source_sample_count, Ordering::Release);
                            stable_prefix = Some(PreviewResult {
                                session_id: recording.session_id,
                                output_mode,
                                transcript,
                            });
                        }
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
            } => {
                let final_queue_wait_ms =
                    elapsed_ms(release_started).saturating_sub(audio_finalize_ms);
                let stable_prefix = stable_prefix.take().filter(|preview| {
                    preview.session_id == recording.session_id
                        && preview.output_mode == output_mode
                        && preview.transcript.source_sample_count <= recording.source_total_samples
                });
                let (engine_result, speculative_reuse) = match stable_prefix.as_ref() {
                    Some(prefix) => (
                        finalize_from_stable_prefix(
                            &mut engine,
                            &recording,
                            output_mode,
                            &prefix.transcript,
                        ),
                        true,
                    ),
                    None => (engine.transcribe(&recording, output_mode, None), false),
                };
                stable_source_sample_count.store(0, Ordering::Release);
                match engine_result {
                    Ok(result) => {
                        let created_unix_ms = crate::unix_ms();
                        let transcript = Transcript {
                            session_id: recording.session_id,
                            text: result.text,
                            language: result.language,
                            audio_duration_ms: result.audio_duration_ms,
                            processing_duration_ms: result.inference_ms,
                            provider: None,
                            latency: TranscriptionLatency {
                                audio_finalize_ms,
                                preprocessing_ms: result.preprocessing_ms,
                                final_queue_wait_ms,
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
            InferenceCommand::Unload {
                only_catalog_id,
                acknowledged,
            } => {
                let matches = loaded_catalog_id.is_none()
                    || only_catalog_id
                        .as_ref()
                        .is_none_or(|expected| loaded_catalog_id.as_ref() == Some(expected));
                let unloaded = if matches {
                    stable_prefix = None;
                    engine = WhisperEngine::new();
                    let unloaded = loaded_catalog_id.take();
                    let _ = events.send(SpeechWorkerEvent::LocalModelState {
                        catalog_id: unloaded.clone(),
                        loaded: false,
                    });
                    unloaded
                } else {
                    None
                };
                if let Some(acknowledged) = acknowledged {
                    let _ = acknowledged.send(unloaded);
                }
            }
            InferenceCommand::Shutdown => break,
        }
    }
}

fn advance_stable_prefix(
    existing: Option<&EngineTranscript>,
    preview: &EngineTranscript,
) -> Option<EngineTranscript> {
    // A streaming Whisper hypothesis is allowed to revise its final segment.
    // Commit only segments that precede it, then decode that final segment again
    // after key release with the complete acoustic context.
    let provisional = preview.segments.last()?;
    let boundary = provisional.source_start_sample;
    let completed = preview
        .segments
        .iter()
        .take(preview.segments.len().saturating_sub(1))
        .filter(|segment| {
            segment.source_start_sample < boundary && segment.source_end_sample <= boundary
        })
        .collect::<Vec<_>>();
    if completed.is_empty() {
        return existing.cloned();
    }

    let completed_text = completed.iter().fold(String::new(), |text, segment| {
        join_transcript_text(&text, &segment.text)
    });
    let text = existing.map_or_else(
        || completed_text.clone(),
        |prefix| join_transcript_text(&prefix.text, &completed_text),
    );
    Some(EngineTranscript {
        text,
        language: if preview.language == "und" {
            existing.map_or_else(|| "und".to_owned(), |prefix| prefix.language.clone())
        } else {
            preview.language.clone()
        },
        audio_duration_ms: preview.audio_duration_ms,
        source_sample_count: boundary,
        preprocessing_ms: preview.preprocessing_ms,
        inference_ms: preview.inference_ms,
        segments: Vec::new(),
    })
}

fn finalize_from_stable_prefix(
    engine: &mut WhisperEngine,
    recording: &RecordedAudio,
    output_mode: SpeechOutputMode,
    prefix: &EngineTranscript,
) -> anyhow::Result<EngineTranscript> {
    if prefix.source_sample_count >= recording.source_total_samples {
        return engine.transcribe(recording, output_mode, None);
    }

    let Some(tail) = recording.segment_from_absolute(prefix.source_sample_count) else {
        return engine.transcribe(recording, output_mode, None);
    };
    // Whisper needs a small amount of audio to decode reliably. A sub-180 ms
    // spoken tail is rare, but re-running the authoritative recording is safer
    // than silently dropping a final syllable.
    if tail.segment_duration_ms() < MIN_FINAL_TAIL_AUDIO_MS {
        return engine.transcribe(recording, output_mode, None);
    }
    let prompt = context_prompt(&prefix.text);
    match engine.transcribe_with_prompt(&tail, output_mode, Some(&prompt), None) {
        Ok(segment) => Ok(merge_engine_transcripts(prefix, segment)),
        Err(error) => {
            eprintln!(
                "segmented final tail was not usable; falling back to authoritative pass: {error:#}"
            );
            engine.transcribe(recording, output_mode, None)
        }
    }
}

fn merge_engine_transcripts(
    prefix: &EngineTranscript,
    mut segment: EngineTranscript,
) -> EngineTranscript {
    segment.text = join_transcript_text(&prefix.text, &segment.text);
    if segment.language == "und" {
        segment.language.clone_from(&prefix.language);
    }
    segment
}

fn context_prompt(text: &str) -> String {
    const MAX_PROMPT_CHARS: usize = 240;
    let mut reversed = text
        .chars()
        .rev()
        .take(MAX_PROMPT_CHARS)
        .collect::<Vec<_>>();
    reversed.reverse();
    reversed.into_iter().collect()
}

fn join_transcript_text(prefix: &str, segment: &str) -> String {
    let prefix = prefix.trim();
    let segment = segment.trim();
    if prefix.is_empty() {
        return segment.to_owned();
    }
    if segment.is_empty() {
        return prefix.to_owned();
    }

    let previous = prefix.chars().next_back();
    let next = segment.chars().next();
    let joins_without_space = next.is_some_and(is_attached_punctuation)
        || previous
            .zip(next)
            .is_some_and(|(left, right)| is_cjk_character(left) && is_cjk_character(right));
    if joins_without_space {
        format!("{prefix}{segment}")
    } else {
        format!("{prefix} {segment}")
    }
}

fn is_attached_punctuation(value: char) -> bool {
    matches!(
        value,
        '.' | ',' | '!' | '?' | ':' | ';' | ')' | ']' | '}' | '…' | '，' | '。' | '！' | '？'
    )
}

fn is_cjk_character(value: char) -> bool {
    matches!(
        value as u32,
        0x3040..=0x30ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
    )
}

struct PreviewInFlightGuard<'a>(&'a AtomicBool);

impl Drop for PreviewInFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn quiet_tail_after(recording: &RecordedAudio, source_sample_count: usize) -> bool {
    if source_sample_count < recording.source_start_sample
        || source_sample_count > recording.source_total_samples
    {
        return false;
    }
    let tail_samples = recording.source_total_samples - source_sample_count;
    let tail_ms = tail_samples as u64 * 1000 / u64::from(recording.sample_rate_hz.max(1));
    tail_ms <= MAX_PREVIEW_COMPLETION_TAIL_MS
        && !tail_contains_speech(recording, source_sample_count)
}

fn tail_contains_speech(recording: &RecordedAudio, source_sample_count: usize) -> bool {
    if source_sample_count < recording.source_start_sample
        || source_sample_count >= recording.source_total_samples
    {
        return false;
    }
    let local_source_sample_count = source_sample_count - recording.source_start_sample;
    let source = &recording.samples[..local_source_sample_count];
    let tail = &recording.samples[local_source_sample_count..];
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
    !tail_contains_speech(recording, recording.source_start_sample + split)
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
    use std::{sync::mpsc, time::Duration};

    use rust_protocol::{SessionId, SpeechWorkerEvent};

    use super::{
        InferenceHandle, advance_stable_prefix, context_prompt, has_trailing_silence,
        join_transcript_text, quiet_tail_after,
    };
    use crate::{
        audio::RecordedAudio,
        engine::{EngineSegment, EngineTranscript},
    };

    fn recording(samples: Vec<f32>, rate: u32) -> RecordedAudio {
        RecordedAudio {
            session_id: SessionId::new(),
            duration_ms: samples.len() as u64 * 1000 / u64::from(rate),
            source_start_sample: 0,
            source_total_samples: samples.len(),
            samples,
            sample_rate_hz: rate,
        }
    }

    #[test]
    fn unload_acknowledges_only_after_the_actor_reports_no_resident_model() {
        let (events, received_events) = mpsc::channel();
        let (history, _received_history) = mpsc::channel();
        let inference = InferenceHandle::start(events, history).unwrap();

        assert_eq!(
            inference.unload_model_if_and_wait("not-loaded").unwrap(),
            None
        );
        let event = received_events
            .recv_timeout(Duration::from_secs(1))
            .expect("the actor must report residency before acknowledging unload");
        assert_eq!(
            event,
            SpeechWorkerEvent::LocalModelState {
                catalog_id: None,
                loaded: false,
            }
        );
        inference.shutdown();
    }

    #[test]
    fn a_short_silent_tail_can_finish_an_in_flight_preview() {
        let mut samples = vec![0.08_f32; 16_000];
        samples.extend(vec![0.0; 4_000]);
        let audio = recording(samples, 16_000);
        assert!(has_trailing_silence(&audio, 200));
        assert!(quiet_tail_after(&audio, 16_000));
    }

    #[test]
    fn speech_in_tail_cancels_an_outdated_preview() {
        let mut samples = vec![0.08_f32; 16_000];
        samples.extend(vec![0.07; 2_000]);
        let audio = recording(samples, 16_000);
        assert!(!quiet_tail_after(&audio, 16_000));
    }

    #[test]
    fn preview_never_commits_its_final_segment() {
        let preview = transcript(
            "Первое предложение. Второе окончание.",
            32_000,
            vec![
                segment("Первое предложение.", 0, 16_000),
                segment("Второе окончание.", 16_000, 32_000),
            ],
        );

        let stable = advance_stable_prefix(None, &preview).expect("first segment is stable");
        assert_eq!(stable.text, "Первое предложение.");
        assert_eq!(stable.source_sample_count, 16_000);
    }

    #[test]
    fn a_single_segment_preview_remains_fully_provisional() {
        let preview = transcript(
            "Окончание ещё может измениться.",
            16_000,
            vec![segment("Окончание ещё может измениться.", 0, 16_000)],
        );

        assert!(advance_stable_prefix(None, &preview).is_none());
    }

    #[test]
    fn completed_segments_advance_an_existing_stable_prefix() {
        let existing = transcript("Уже проверено.", 16_000, Vec::new());
        let preview = transcript(
            "Новая стабильная часть. Последний фрагмент.",
            48_000,
            vec![
                segment("Новая стабильная часть.", 16_000, 32_000),
                segment("Последний фрагмент.", 32_000, 48_000),
            ],
        );

        let stable =
            advance_stable_prefix(Some(&existing), &preview).expect("prefix should advance");
        assert_eq!(stable.text, "Уже проверено. Новая стабильная часть.");
        assert_eq!(stable.source_sample_count, 32_000);
    }

    #[test]
    fn segmented_transcripts_join_without_damaging_punctuation_or_cjk() {
        assert_eq!(
            join_transcript_text("A completed phrase", "continues here."),
            "A completed phrase continues here."
        );
        assert_eq!(
            join_transcript_text("A completed phrase", ", with detail."),
            "A completed phrase, with detail."
        );
        assert_eq!(join_transcript_text("你好", "世界"), "你好世界");
    }

    #[test]
    fn context_prompt_is_bounded_to_recent_characters() {
        let source = format!("old {}", "x".repeat(400));
        let prompt = context_prompt(&source);
        assert_eq!(prompt.chars().count(), 240);
        assert!(prompt.chars().all(|character| character == 'x'));
    }

    fn transcript(
        text: &str,
        source_sample_count: usize,
        segments: Vec<EngineSegment>,
    ) -> EngineTranscript {
        EngineTranscript {
            text: text.to_owned(),
            language: "ru".to_owned(),
            audio_duration_ms: source_sample_count as u64 / 16,
            source_sample_count,
            preprocessing_ms: 1,
            inference_ms: 1,
            segments,
        }
    }

    fn segment(text: &str, start: usize, end: usize) -> EngineSegment {
        EngineSegment {
            text: text.to_owned(),
            source_start_sample: start,
            source_end_sample: end,
        }
    }
}
