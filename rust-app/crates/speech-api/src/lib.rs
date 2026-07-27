use std::path::{Path, PathBuf};

use rust_protocol::{SessionId, Transcript};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechEngineKind {
    WhisperCpp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeBackend {
    Auto,
    Cpu,
    Cuda,
    Vulkan,
    Metal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    pub catalog_id: String,
    pub path: PathBuf,
    pub engine: SpeechEngineKind,
    pub compute_backend: ComputeBackend,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionRequest<'audio> {
    pub session_id: SessionId,
    pub samples: &'audio [f32],
    pub sample_rate_hz: u32,
    pub language: Option<&'audio str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionResult {
    pub transcript: Transcript,
    pub detected_language_probability: Option<f32>,
}

#[derive(Debug, Error)]
pub enum SpeechEngineError {
    #[error("model is incompatible with this speech engine")]
    IncompatibleModel,
    #[error("compute backend is unavailable: {0:?}")]
    BackendUnavailable(ComputeBackend),
    #[error("model loading failed: {0}")]
    ModelLoad(String),
    #[error("transcription failed: {0}")]
    Transcription(String),
}

pub trait SpeechEngine: Send {
    fn kind(&self) -> SpeechEngineKind;

    fn supported_compute_backends(&self) -> &[ComputeBackend];

    fn load_model(
        &mut self,
        catalog_id: &str,
        path: &Path,
        compute_backend: ComputeBackend,
    ) -> Result<LoadedModel, SpeechEngineError>;

    fn unload_model(&mut self);

    fn transcribe(
        &mut self,
        request: TranscriptionRequest<'_>,
    ) -> Result<TranscriptionResult, SpeechEngineError>;
}

pub trait SpeechEngineFactory: Send + Sync {
    fn create(&self, kind: SpeechEngineKind) -> Result<Box<dyn SpeechEngine>, SpeechEngineError>;
}
