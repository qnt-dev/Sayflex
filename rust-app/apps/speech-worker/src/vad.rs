//! Silence and noise must not become text.
//!
//! Whisper was trained on subtitles, so it turns a silent or noisy recording
//! into "Thank you." or "Продолжение следует...". The tools built on it handle
//! this the same way, and SayFlex now does too:
//!
//! 1. Voice activity detection before decoding. whisper.cpp, faster-whisper
//!    and WhisperX all use the Silero VAD model for this. A recording without
//!    speech never reaches Whisper.
//! 2. OpenAI's own rule after decoding: a segment whose no-speech probability
//!    is above 0.6 and whose average token log-probability is below -1.0 is
//!    silence, not speech (`whisper/transcribe.py`).

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

/// Silero VAD v6.2.0 converted for whisper.cpp, MIT licensed.
/// <https://huggingface.co/ggml-org/whisper-vad>, revision
/// 9ffd54a1e1ee413ddf265af9913beaf518d1639b.
pub const VAD_MODEL_FILE: &str = "ggml-silero-v6.2.0.bin";
const VAD_MODEL_BYTES: u64 = 885_098;
const VAD_MODEL_SHA256: &str = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";
/// The installer places the model here, beside the speech workers.
const VAD_MODEL_DIRECTORY: &str = "speech-vad";

/// Less detected speech than this is treated as no speech at all. Short enough
/// for a one-syllable answer ("да", "yes").
pub const MIN_SPEECH_MS: u64 = 120;

/// OpenAI's thresholds for a segment that is silence, not speech.
const NO_SPEECH_PROBABILITY: f32 = 0.6;
const LOW_AVERAGE_LOG_PROBABILITY: f32 = -1.0;

/// Returned when a recording holds no speech. Callers treat it as an ordinary
/// outcome, not as a failure of the engine.
#[derive(Debug, thiserror::Error)]
#[error("no speech was recognized")]
pub struct NoSpeech;

pub struct SpeechGate {
    context: WhisperVadContext,
}

impl SpeechGate {
    /// The bundled model next to the running executable, if it is installed.
    pub fn bundled_model_path() -> Option<PathBuf> {
        let path = std::env::current_exe()
            .ok()?
            .parent()?
            .join(VAD_MODEL_DIRECTORY)
            .join(VAD_MODEL_FILE);
        path.is_file().then_some(path)
    }

    /// Loads the model after checking that it is exactly the pinned file.
    pub fn load(path: &Path) -> Result<Self> {
        verify_model(path)?;
        let mut parameters = WhisperVadContextParams::new();
        // A few milliseconds of CPU per recording; never compete with the
        // GPU backend for device memory.
        parameters.set_n_threads(2);
        parameters.set_use_gpu(false);
        let path = path
            .to_str()
            .context("VAD model path is not valid Unicode")?;
        let context = WhisperVadContext::new(path, parameters)
            .map_err(|error| anyhow::anyhow!("could not load the VAD model: {error}"))?;
        Ok(Self { context })
    }

    /// Milliseconds of speech in 16 kHz mono audio.
    pub fn speech_ms(&mut self, audio: &[f32]) -> Result<u64> {
        let mut parameters = WhisperVadParams::new();
        parameters.set_threshold(0.5);
        parameters.set_min_speech_duration(100);
        parameters.set_min_silence_duration(100);
        parameters.set_speech_pad(30);
        let segments = self
            .context
            .segments_from_samples(parameters, audio)
            .map_err(|error| anyhow::anyhow!("voice activity detection failed: {error}"))?;
        // Timestamps are in centiseconds.
        let total_cs: f32 = segments
            .map(|segment| (segment.end - segment.start).max(0.0))
            .sum();
        Ok((total_cs * 10.0).round() as u64)
    }
}

fn verify_model(path: &Path) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("could not open the VAD model {}", path.display()))?;
    let length = file.metadata()?.len();
    if length != VAD_MODEL_BYTES {
        bail!("the VAD model has {length} bytes, expected {VAD_MODEL_BYTES}");
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != VAD_MODEL_SHA256 {
        bail!("the VAD model does not match its pinned SHA-256");
    }
    Ok(())
}

/// OpenAI's rule for a decoded segment that is really silence.
pub fn is_silence_segment(no_speech_probability: f32, average_log_probability: f32) -> bool {
    no_speech_probability > NO_SPEECH_PROBABILITY
        && average_log_probability < LOW_AVERAGE_LOG_PROBABILITY
}

#[cfg(test)]
mod tests {
    use super::{SpeechGate, VAD_MODEL_FILE, is_silence_segment};

    #[test]
    fn silence_needs_both_a_high_no_speech_probability_and_low_confidence() {
        assert!(is_silence_segment(0.9, -1.5));
        assert!(!is_silence_segment(0.9, -0.4), "confident text is kept");
        assert!(!is_silence_segment(0.3, -1.5), "likely speech is kept");
        assert!(!is_silence_segment(0.6, -1.5), "the threshold is exclusive");
    }

    #[test]
    fn a_wrong_model_file_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(VAD_MODEL_FILE);
        std::fs::write(&path, b"not a model").unwrap();
        let error = SpeechGate::load(&path).err().unwrap();
        assert!(error.to_string().contains("bytes"), "{error:#}");
    }

    /// Runs the real model when SAYFLEX_VAD_MODEL points at the pinned file.
    #[test]
    #[ignore = "needs the pinned Silero model"]
    fn the_pinned_model_hears_speech_like_sound_and_not_silence() {
        let path = std::env::var_os("SAYFLEX_VAD_MODEL").expect("set SAYFLEX_VAD_MODEL");
        let mut gate = SpeechGate::load(std::path::Path::new(&path)).unwrap();
        let silence = vec![0.0_f32; 16_000 * 2];
        assert_eq!(gate.speech_ms(&silence).unwrap(), 0);
        if let Some(sample) = std::env::var_os("SAYFLEX_VAD_SPEECH_WAV") {
            let mut reader = hound::WavReader::open(sample).unwrap();
            let spec = reader.spec();
            assert_eq!(spec.sample_rate, 16_000);
            let samples = reader
                .samples::<i16>()
                .map(|sample| f32::from(sample.unwrap()) / 32_768.0)
                .collect::<Vec<_>>();
            assert!(gate.speech_ms(&samples).unwrap() > super::MIN_SPEECH_MS);
        }
    }
}
