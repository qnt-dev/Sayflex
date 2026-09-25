use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result, bail};
use rust_protocol::SpeechOutputMode;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    get_lang_str,
};

use crate::audio::{RecordedAudio, resample_bandlimited};

#[cfg(all(feature = "cuda", feature = "vulkan"))]
compile_error!("the speech worker must be built for exactly one GPU backend");

const WHISPER_SAMPLE_RATE: u32 = 16_000;
const WARMUP_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;

#[derive(Debug, Clone)]
pub struct PreparedModel {
    pub load_duration_ms: u64,
    pub warmup_duration_ms: u64,
    pub backend: &'static str,
}

#[derive(Debug, Clone)]
pub struct EngineTranscript {
    pub text: String,
    pub language: String,
    pub audio_duration_ms: u64,
    pub source_sample_count: usize,
    pub preprocessing_ms: u64,
    pub inference_ms: u64,
    pub segments: Vec<EngineSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSegment {
    pub text: String,
    pub source_start_sample: usize,
    pub source_end_sample: usize,
}

struct DecodedTranscript {
    text: String,
    language: String,
    segments: Vec<DecodedSegment>,
}

#[derive(Clone, Copy)]
struct WhisperDecodePolicy {
    no_timestamps: bool,
    single_segment: bool,
}

const fn whisper_decode_policy(single_segment: bool) -> WhisperDecodePolicy {
    WhisperDecodePolicy {
        // Only the warm-up probe is intentionally a timestamp-free single
        // segment. Real transcripts need segment boundaries so speculative
        // work can become a reusable stable prefix.
        no_timestamps: single_segment,
        single_segment,
    }
}

struct DecodedSegment {
    text: String,
    start_timestamp_cs: i64,
    end_timestamp_cs: i64,
}

pub struct AbortGeneration {
    current: Arc<AtomicU64>,
    expected: u64,
}

impl AbortGeneration {
    pub fn new(current: Arc<AtomicU64>, expected: u64) -> Self {
        Self { current, expected }
    }

    fn was_cancelled(&self) -> bool {
        self.current.load(Ordering::Acquire) != self.expected
    }
}

pub struct WhisperEngine {
    loaded_path: Option<PathBuf>,
    context: Option<WhisperContext>,
    state: Option<WhisperState>,
    direct_translation_supported: bool,
    language: Option<String>,
}

impl WhisperEngine {
    pub fn new() -> Self {
        Self {
            loaded_path: None,
            context: None,
            state: None,
            language: None,
            direct_translation_supported: false,
        }
    }

    pub fn set_language(&mut self, language: Option<&str>) {
        self.language =
            language.map(|language| language.split('-').next().unwrap_or(language).to_owned());
    }

    /// Loads the model and performs one silent pass. Once this returns, a user gesture never
    /// pays model I/O, CUDA context creation, or first-kernel initialization.
    pub fn prepare(&mut self, model_path: &Path) -> Result<PreparedModel> {
        if self.loaded_path.as_deref() == Some(model_path)
            && self.context.is_some()
            && self.state.is_some()
        {
            return Ok(PreparedModel {
                load_duration_ms: 0,
                warmup_duration_ms: 0,
                backend: backend_name(),
            });
        }

        let load_started = Instant::now();
        let model = model_path
            .to_str()
            .context("model path is not valid Unicode")?;
        let mut parameters = WhisperContextParameters::default();
        parameters
            .use_gpu(cfg!(any(feature = "cuda", feature = "vulkan")))
            .flash_attn(cfg!(feature = "cuda"));
        let context = WhisperContext::new_with_params(model, parameters)
            .context("could not load the Whisper model")?;
        let model_type = context
            .model_type_readable_str_lossy()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let direct_translation_supported = supports_direct_translation(
            &model_type,
            context.model_n_text_layer(),
            context.model_n_text_state(),
        );
        let state = context
            .create_state()
            .context("could not create reusable Whisper inference state")?;
        let load_duration_ms = elapsed_ms(load_started);

        self.context = Some(context);
        self.state = Some(state);
        self.loaded_path = Some(model_path.to_owned());
        self.direct_translation_supported = direct_translation_supported;

        let warmup_started = Instant::now();
        let silence = vec![0.0_f32; WARMUP_SAMPLES];
        // Model loading alone does not prove that the selected GPU contains
        // compatible kernels. A full silent pass is the backend capability
        // probe; no-speech is a valid result, while a compute error is not.
        self.run_whisper(
            &silence,
            SpeechOutputMode::OriginalLanguage,
            true,
            None,
            None,
        )
        .context("speech backend warm-up probe failed")?;

        Ok(PreparedModel {
            load_duration_ms,
            warmup_duration_ms: elapsed_ms(warmup_started),
            backend: backend_name(),
        })
    }

    pub fn transcribe(
        &mut self,
        recording: &RecordedAudio,
        output_mode: SpeechOutputMode,
        abort_generation: Option<AbortGeneration>,
    ) -> Result<EngineTranscript> {
        self.transcribe_with_prompt(recording, output_mode, None, abort_generation)
    }

    pub fn transcribe_with_prompt(
        &mut self,
        recording: &RecordedAudio,
        output_mode: SpeechOutputMode,
        initial_prompt: Option<&str>,
        abort_generation: Option<AbortGeneration>,
    ) -> Result<EngineTranscript> {
        if recording.segment_duration_ms() < 180 || recording.samples.is_empty() {
            bail!("recording is too short");
        }
        if recording
            .source_start_sample
            .saturating_add(recording.samples.len())
            != recording.source_total_samples
        {
            bail!("recording segment does not match its absolute source range");
        }
        if self.state.is_none() {
            bail!("Whisper model is not prepared");
        }
        if output_mode == SpeechOutputMode::TranslateToEnglish && !self.direct_translation_supported
        {
            bail!(
                "the selected Whisper Turbo model is optimized for transcription and does not \
                 support reliable speech translation"
            );
        }

        let preprocessing_started = Instant::now();
        let audio = resample_bandlimited(
            &recording.samples,
            recording.sample_rate_hz,
            WHISPER_SAMPLE_RATE,
        );
        let preprocessing_ms = elapsed_ms(preprocessing_started);
        let inference_started = Instant::now();
        let decoded =
            self.run_whisper(&audio, output_mode, false, initial_prompt, abort_generation)?;
        let segments = decoded
            .segments
            .into_iter()
            .map(|segment| EngineSegment {
                text: segment.text,
                source_start_sample: source_sample_at_timestamp(
                    recording,
                    segment.start_timestamp_cs,
                ),
                source_end_sample: source_sample_at_timestamp(recording, segment.end_timestamp_cs),
            })
            .collect();

        Ok(EngineTranscript {
            text: decoded.text,
            language: decoded.language,
            audio_duration_ms: recording.duration_ms,
            source_sample_count: recording.source_total_samples,
            preprocessing_ms,
            inference_ms: elapsed_ms(inference_started),
            segments,
        })
    }

    fn run_whisper(
        &mut self,
        audio: &[f32],
        output_mode: SpeechOutputMode,
        single_segment: bool,
        initial_prompt: Option<&str>,
        abort_generation: Option<AbortGeneration>,
    ) -> Result<DecodedTranscript> {
        let state = self
            .state
            .as_mut()
            .context("Whisper inference state is not prepared")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let decode_policy = whisper_decode_policy(single_segment);
        params.set_n_threads(inference_threads());
        params.set_language(self.language.as_deref());
        params.set_translate(output_mode == SpeechOutputMode::TranslateToEnglish);
        params.set_no_context(true);
        if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.is_empty()) {
            params.set_initial_prompt(prompt);
        }
        params.set_no_timestamps(decode_policy.no_timestamps);
        params.set_single_segment(decode_policy.single_segment);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        if let Some(abort) = abort_generation.as_ref() {
            // whisper_full() is synchronous. The stack reference remains valid for the complete
            // native call and avoids the allocation leak in whisper-rs 0.16's closure wrapper.
            unsafe {
                params.set_abort_callback(Some(abort_callback));
                params.set_abort_callback_user_data(
                    std::ptr::from_ref(abort).cast_mut().cast::<c_void>(),
                );
            }
        }

        state
            .full(params, audio)
            .context("Whisper inference failed")?;

        let mut raw_text = String::new();
        let mut segments = Vec::new();
        for segment in state.as_iter() {
            let raw_segment = segment
                .to_str()
                .context("Whisper returned invalid transcript text")?;
            raw_text.push_str(raw_segment);
            let text = normalize_transcript(raw_segment);
            if !text.is_empty() {
                segments.push(DecodedSegment {
                    text,
                    start_timestamp_cs: segment.start_timestamp(),
                    end_timestamp_cs: segment.end_timestamp(),
                });
            }
        }
        let text = normalize_transcript(&raw_text);
        if text.is_empty() {
            bail!("no speech was recognized");
        }
        let language = if output_mode == SpeechOutputMode::TranslateToEnglish {
            "en".to_owned()
        } else {
            get_lang_str(state.full_lang_id_from_state())
                .unwrap_or("und")
                .to_owned()
        };
        Ok(DecodedTranscript {
            text,
            language,
            segments,
        })
    }
}

fn source_sample_at_timestamp(recording: &RecordedAudio, timestamp_cs: i64) -> usize {
    let timestamp_cs = u64::try_from(timestamp_cs.max(0)).unwrap_or(0);
    let local_sample = timestamp_cs
        .saturating_mul(u64::from(recording.sample_rate_hz))
        .checked_div(100)
        .unwrap_or(0);
    let local_sample = usize::try_from(local_sample)
        .unwrap_or(usize::MAX)
        .min(recording.samples.len());
    recording.source_start_sample.saturating_add(local_sample)
}

unsafe extern "C" fn abort_callback(user_data: *mut c_void) -> bool {
    if user_data.is_null() {
        return false;
    }
    unsafe { &*user_data.cast::<AbortGeneration>() }.was_cancelled()
}

fn inference_threads() -> i32 {
    std::thread::available_parallelism()
        .map_or(8, usize::from)
        .clamp(4, 12) as i32
}

const fn backend_name() -> &'static str {
    if cfg!(feature = "cuda") {
        "cuda"
    } else if cfg!(feature = "vulkan") {
        "vulkan"
    } else {
        "cpu"
    }
}

fn supports_direct_translation(model_type: &str, text_layers: i32, text_state: i32) -> bool {
    let explicitly_turbo = model_type.to_ascii_lowercase().contains("turbo");
    let structurally_turbo = text_layers == 4 && text_state >= 1_000;
    !explicitly_turbo && !structurally_turbo
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn normalize_transcript(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use rust_protocol::SessionId;

    use super::{
        inference_threads, normalize_transcript, source_sample_at_timestamp,
        supports_direct_translation, whisper_decode_policy,
    };
    use crate::audio::RecordedAudio;

    #[test]
    fn transcript_whitespace_is_stable_for_insertion_and_history() {
        assert_eq!(normalize_transcript("  Hello,\n world!  "), "Hello, world!");
    }

    #[test]
    fn inference_thread_count_is_bounded_for_desktop_responsiveness() {
        assert!((4..=12).contains(&inference_threads()));
    }

    #[test]
    fn turbo_is_not_advertised_as_a_translation_model() {
        assert!(!supports_direct_translation("large v3 turbo", 4, 1280));
        assert!(!supports_direct_translation("large v3", 4, 1280));
        assert!(supports_direct_translation("large v3", 32, 1280));
        assert!(supports_direct_translation("medium", 24, 1024));
        assert!(supports_direct_translation("tiny", 4, 384));
    }

    #[test]
    fn segment_timestamps_map_to_absolute_source_samples() {
        let recording = RecordedAudio {
            session_id: SessionId::new(),
            samples: vec![0.0; 48_000],
            sample_rate_hz: 48_000,
            source_start_sample: 96_000,
            source_total_samples: 144_000,
            duration_ms: 3_000,
        };

        assert_eq!(source_sample_at_timestamp(&recording, 25), 108_000);
        assert_eq!(source_sample_at_timestamp(&recording, 100), 144_000);
        assert_eq!(source_sample_at_timestamp(&recording, 500), 144_000);
        assert_eq!(source_sample_at_timestamp(&recording, -10), 96_000);
    }

    #[test]
    fn reusable_transcription_keeps_segment_timestamps_enabled() {
        let policy = whisper_decode_policy(false);

        assert!(!policy.no_timestamps);
        assert!(!policy.single_segment);
    }
}
