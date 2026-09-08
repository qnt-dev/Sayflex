use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Version 15 adds the ordered list of transcription providers: the local
// engine and any network services the user has keys for.
// All packaged processes must agree on this schema before exchanging commands.
/// The shape of every message in this file, as one number.
///
/// It goes up whenever the shape changes — a field added, a variant removed, a
/// meaning altered — and not once per release. Version 16 was used twice: an
/// event was added under it and then removed under it again, so two
/// incompatible schemas both introduced themselves as 16 and shook hands
/// happily. A desktop and a worker from those two builds would have agreed they
/// understood each other and then quietly disagreed about where a translation
/// came from.
pub const PROTOCOL_VERSION: u32 = 17;
pub const AUDIO_SPECTRUM_BANDS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(Uuid);

impl RequestId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSnapshot {
    pub window_handle: u64,
    pub process_id: u32,
    pub thread_id: u32,
    pub keyboard_layout: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationCommand {
    Begin {
        session_id: SessionId,
        started_unix_ms: u64,
    },
    End {
        session_id: SessionId,
    },
    Cancel {
        session_id: SessionId,
    },
    GetTranscript {
        session_id: SessionId,
    },
    GetHistory {
        limit: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub session_id: SessionId,
    pub text: String,
    pub language: String,
    pub audio_duration_ms: u64,
    /// Kept for history/protocol compatibility. This is the actual Whisper pass only.
    pub processing_duration_ms: u64,
    /// Which transcription provider produced this text, when it was not the
    /// local engine. Recorded so history can say where the words came from,
    /// and so a user can tell which dictations left the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default)]
    pub latency: TranscriptionLatency,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionLatency {
    /// Time spent stopping capture and taking ownership of the recorded buffer.
    pub audio_finalize_ms: u64,
    /// Resampling and other deterministic audio preparation.
    pub preprocessing_ms: u64,
    /// Time between audio finalization and the final inference pass beginning.
    #[serde(default)]
    pub final_queue_wait_ms: u64,
    /// Time spent inside the speech model for the result that was returned.
    pub inference_ms: u64,
    /// End command receipt to completed transcript event.
    pub after_release_ms: u64,
    /// True when a correctness-guarded result prepared during trailing silence was reused.
    pub speculative_reuse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub transcript: Transcript,
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSummary {
    pub catalog_id: String,
    pub display_name: String,
    pub description: String,
    pub size_bytes: u64,
    pub installed: bool,
    pub selected: bool,
    pub recommended: bool,
    pub supports_speech_translation: bool,
}

/// One row of the user's ordered transcription list, as it travels between the
/// desktop and the speech worker.
///
/// The identifier is a string rather than an enum so a list built by a newer
/// build stays readable: an unknown row keeps its place and is never selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionProviderEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Cloudflare account identifier, or Azure region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// What the worker knows about one row, for Settings to display.
///
/// A key never travels in this direction: `credential_hint` is a masked tail,
/// enough to recognise which key is stored and useless for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionProviderStatus {
    pub id: String,
    pub position: usize,
    /// The row's own switch, independent of whether it can run right now.
    pub enabled: bool,
    pub available: bool,
    /// Why it is unavailable, in words meant for the user. Empty when it is.
    pub detail: String,
    pub has_credential: bool,
    #[serde(default)]
    pub credential_hint: Option<String>,
}

/// One row of the user's ordered translation list.
///
/// Deliberately not the transcription list, and deliberately not a reuse of
/// [`TranscriptionProviderEntry`]. A key for Azure Translator is not a key for
/// Azure Speech, the free allowances are counted separately, and somebody who
/// is willing to send audio to a service is not thereby willing to send the
/// contents of their documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationProviderEntry {
    pub id: String,
    pub enabled: bool,
    /// The Azure resource's region. Nothing else needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// What the worker knows about one translation row, for Settings to display.
///
/// A key never travels in this direction: `credential_hint` is a masked tail,
/// enough to recognise which key is stored and useless for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationProviderStatus {
    pub id: String,
    pub position: usize,
    /// The row's own switch, independent of whether it can run right now.
    pub enabled: bool,
    pub available: bool,
    /// Why it is unavailable, in words meant for the user. Empty when it is.
    pub detail: String,
    pub has_credential: bool,
    #[serde(default)]
    pub credential_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextActionIntent {
    Adaptive,
    Translate,
    CorrectLayout,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationLanguage {
    #[default]
    English,
    Russian,
}

impl TranslationLanguage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
        }
    }

    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::English => Self::Russian,
            Self::Russian => Self::English,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationTier {
    Tiny,
    Teacher,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationModelSummary {
    pub catalog_id: String,
    pub display_name: String,
    pub description: String,
    pub tier: TranslationTier,
    pub languages: Vec<TranslationLanguage>,
    pub size_bytes: u64,
    pub installed: bool,
    pub selected: bool,
    pub recommended: bool,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationResult {
    pub request_id: RequestId,
    pub source_text: String,
    pub translated_text: String,
    pub source_language: TranslationLanguage,
    pub target_language: TranslationLanguage,
    /// The local model that produced it. `None` when a service did, which is
    /// the case `provider` names.
    #[serde(default)]
    pub tier: Option<TranslationTier>,
    /// The service that produced it, when one did. `None` means the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub processing_duration_ms: u64,
    #[serde(default)]
    pub first_token_duration_ms: u64,
    #[serde(default)]
    pub backend_request_count: u32,
    #[serde(default)]
    pub tokenize_duration_ms: u64,
    #[serde(default)]
    pub chunk_count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionCaptureTiming {
    pub release_wait_ms: Option<u64>,
    pub uia_ms: Option<u64>,
    pub clipboard_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechOutputMode {
    #[default]
    OriginalLanguage,
    TranslateToEnglish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationEvent {
    Listening {
        session_id: SessionId,
    },
    Processing {
        session_id: SessionId,
    },
    Completed {
        transcript: Transcript,
        created_unix_ms: u64,
    },
    Cancelled {
        session_id: SessionId,
    },
    Failed {
        session_id: SessionId,
        code: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputHostEvent {
    Ready {
        protocol_version: u32,
    },
    DictationBegin {
        session_id: SessionId,
        started_unix_ms: u64,
        target: TargetSnapshot,
    },
    DictationEnd {
        session_id: SessionId,
        target: TargetSnapshot,
    },
    InsertLastRequested {
        target: TargetSnapshot,
    },
    SelectionCaptureStarted {
        request_id: RequestId,
        intent: TextActionIntent,
        target: TargetSnapshot,
        started_unix_ms: u64,
    },
    SelectionCaptured {
        request_id: RequestId,
        intent: TextActionIntent,
        target: TargetSnapshot,
        text: String,
        timing: SelectionCaptureTiming,
    },
    SelectionInputReleased {
        request_id: RequestId,
        release_wait_ms: u64,
    },
    SelectionInputReleaseFailed {
        request_id: RequestId,
        message: String,
    },
    SelectionCaptureFailed {
        request_id: RequestId,
        message: String,
    },
    InsertionCompleted {
        session_id: SessionId,
        method: InsertionMethod,
        delivery: InsertionDelivery,
        dispatch_duration_ms: u64,
    },
    Recovered {
        session_id: SessionId,
    },
    SelectionReplaced {
        request_id: RequestId,
    },
    TextCopied {
        request_id: RequestId,
    },
    Failed {
        #[serde(default)]
        session_id: Option<SessionId>,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertionMethod {
    /// Synchronous EM_REPLACESEL delivery to a standard Win32 edit control.
    NativeEditMessage,
    /// Delayed-rendered, privacy-marked clipboard transaction for targets
    /// without a synchronous native adapter.
    ClipboardTransaction,
    WezTermCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertionDelivery {
    /// A target-specific adapter processed the operation and returned success.
    Committed,
    /// The captured target process requested the exact delayed-render payload.
    /// This does not claim that arbitrary document contents can be read back.
    PayloadRead,
    /// The observable focused text changed by the exact immutable payload.
    Verified,
    /// The compatibility paste command was dispatched and its private payload
    /// remained available for a bounded lease, but no target-specific
    /// acknowledgement channel was available. This is not proof of delivery.
    Dispatched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputHostCommand {
    InsertTranscript {
        transcript: Transcript,
        target: TargetSnapshot,
        add_trailing_space: bool,
    },
    CopyTranscript {
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
    CancelSelection {
        request_id: RequestId,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranslationWorkerCommand {
    Initialize {
        selected_tier: Option<TranslationTier>,
        keep_warm: bool,
        history_enabled: bool,
    },
    EnsureModel {
        tier: TranslationTier,
    },
    /// Stops a pack or runtime installation in flight, keeping partial files.
    CancelModelInstall,
    SelectTier {
        tier: TranslationTier,
        keep_warm: bool,
    },
    Translate {
        request_id: RequestId,
        text: String,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
    },
    Cancel {
        request_id: RequestId,
    },
    /// Replaces the ordered translation list.
    SetTranslationProviders {
        providers: Vec<TranslationProviderEntry>,
    },
    /// Stores or clears the key for one translation provider. `None` clears it.
    SetTranslationProviderCredential {
        operation_id: RequestId,
        provider_id: String,
        secret: Option<String>,
    },
    GetTranslationProviders,
    GetHistory {
        limit: usize,
    },
    SetHistoryEnabled {
        enabled: bool,
    },
    ClearHistory,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationHistoryEntry {
    pub result: TranslationResult,
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranslationWorkerEvent {
    Ready {
        protocol_version: u32,
        selected_tier: Option<TranslationTier>,
        models: Vec<TranslationModelSummary>,
    },
    ModelDownloadProgress {
        catalog_id: String,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    ModelInstalled {
        catalog_id: String,
        tier: TranslationTier,
        path: String,
    },
    ModelReady {
        catalog_id: String,
        tier: TranslationTier,
        load_duration_ms: u64,
    },
    ModelSelected {
        tier: TranslationTier,
    },
    Partial {
        request_id: RequestId,
        translated_text: String,
        first_token_duration_ms: u64,
        tokenize_duration_ms: u64,
        chunk_index: u32,
        chunk_count: u32,
    },
    Completed {
        result: TranslationResult,
        created_unix_ms: u64,
    },
    Cancelled {
        request_id: RequestId,
    },
    History {
        entries: Vec<TranslationHistoryEntry>,
    },
    /// The ordered list with each row's availability, after any change.
    TranslationProviders {
        statuses: Vec<TranslationProviderStatus>,
    },
    /// The key named by this operation is in the store, or is not.
    CredentialSaved {
        operation_id: RequestId,
        provider_id: String,
        /// Empty when it worked. The store's own words when it did not.
        error: String,
    },
    Failed {
        request_id: Option<RequestId>,
        /// The model the failure belongs to, when it belongs to one.
        ///
        /// Without it a download that died of a network fault left its row
        /// offering a Cancel that cancelled nothing, because the desktop had to
        /// guess which install had just ended.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        catalog_id: Option<String>,
        code: String,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SpeechWorkerCommand {
    Initialize {
        selected_model_id: String,
        output_mode: SpeechOutputMode,
        history_enabled: bool,
        fast_release_previews: bool,
    },
    Begin {
        session_id: SessionId,
        started_unix_ms: u64,
    },
    End {
        session_id: SessionId,
    },
    Cancel {
        session_id: SessionId,
    },
    EnsureModel {
        catalog_id: String,
    },
    SelectModel {
        catalog_id: String,
    },
    /// Stops an installation in flight. The partial download is kept, so a
    /// later attempt resumes instead of starting again.
    CancelModelInstall,
    /// Replaces the ordered transcription list.
    SetTranscriptionProviders {
        providers: Vec<TranscriptionProviderEntry>,
    },
    /// Stores or clears the key for one provider. `None` clears it.
    SetProviderCredential {
        /// Names this particular save so its answer can be recognised.
        ///
        /// A successful `send` only means the pipe took the bytes. The store is
        /// written later, on another process, and it can fail — a profile
        /// copied from another Windows account cannot be decrypted, a disk can
        /// be full. Without an id the desktop had no way to tell whether the
        /// generic failure that came back was about this key, so it closed the
        /// form and threw away what the user had typed.
        operation_id: RequestId,
        provider_id: String,
        secret: Option<String>,
    },
    GetTranscriptionProviders,
    SetOutputMode {
        output_mode: SpeechOutputMode,
    },
    SetFastReleasePreviews {
        enabled: bool,
    },
    GetHistory {
        limit: usize,
    },
    SetHistoryEnabled {
        enabled: bool,
    },
    ClearHistory,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SpeechWorkerEvent {
    Ready {
        protocol_version: u32,
        selected_model_id: String,
        models: Vec<ModelSummary>,
    },
    ModelDownloadProgress {
        catalog_id: String,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    ModelInstalled {
        catalog_id: String,
        path: String,
    },
    ModelReady {
        catalog_id: String,
        path: String,
        backend: String,
        load_duration_ms: u64,
        warmup_duration_ms: u64,
    },
    ModelSelected {
        catalog_id: String,
    },
    Listening {
        session_id: SessionId,
    },
    AudioLevel {
        session_id: SessionId,
        peak: f32,
        spectrum: Vec<f32>,
    },
    Processing {
        session_id: SessionId,
    },
    Completed {
        transcript: Transcript,
        created_unix_ms: u64,
    },
    Cancelled {
        session_id: SessionId,
    },
    History {
        entries: Vec<HistoryEntry>,
    },
    /// The ordered list with each row's availability, after any change.
    TranscriptionProviders {
        statuses: Vec<TranscriptionProviderStatus>,
    },
    /// The key named by this operation is in the store, or is not.
    ///
    /// Sent for every `SetProviderCredential`, whichever way it went, so the
    /// form that is waiting always hears something.
    CredentialSaved {
        operation_id: RequestId,
        provider_id: String,
        /// Empty when it worked. The store's own words when it did not.
        error: String,
    },
    /// A recording was transcribed by a network service rather than locally.
    /// Carried separately from the transcript so history and the interface can
    /// show where the words came from.
    TranscribedRemotely {
        session_id: SessionId,
        provider_id: String,
    },
    Failed {
        session_id: Option<SessionId>,
        code: String,
        message: String,
        retryable: bool,
    },
}

/// Commands sent to the optional, long-lived Companion Orb renderer.
///
/// The renderer owns presentation only. Audio capture and session ownership
/// remain in the speech worker and desktop process respectively, so selecting
/// this visualizer never opens a second microphone stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CompanionRendererCommand {
    /// Selects or deselects the companion style. Selection makes the orb a
    /// persistent desktop presence even when no dictation is active.
    SetEnabled {
        enabled: bool,
    },
    /// Restores the user's preferred top-left position for the persistent
    /// companion surface. The renderer validates visibility before applying it.
    SetHomePosition {
        x: i32,
        y: i32,
    },
    Activate {
        session_id: SessionId,
        target_window_handle: u64,
    },
    AudioLevel {
        session_id: SessionId,
        peak: f32,
        spectrum: Vec<f32>,
    },
    Processing {
        session_id: SessionId,
    },
    Complete {
        session_id: SessionId,
    },
    Dismiss {
        session_id: Option<SessionId>,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CompanionRendererEvent {
    Ready {
        protocol_version: u32,
    },
    /// Emitted only after an intentional Ctrl+Alt drag completes.
    HomePositionChanged {
        x: i32,
        y: i32,
    },
    Failed {
        session_id: Option<SessionId>,
        code: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        CompanionRendererCommand, DictationCommand, InputHostEvent, InsertionDelivery,
        InsertionMethod, RequestId, SelectionCaptureTiming, SessionId, SpeechWorkerEvent,
        TargetSnapshot, TextActionIntent, Transcript, TranscriptionLatency, TranslationLanguage,
        TranslationResult, TranslationTier, TranslationWorkerEvent,
    };

    #[test]
    fn command_json_preserves_session_identity() {
        let session_id = SessionId::new();
        let command = DictationCommand::End { session_id };

        let json = serde_json::to_string(&command).unwrap();
        let decoded: DictationCommand = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, command);
    }

    #[test]
    fn companion_renderer_messages_preserve_session_scope() {
        let session_id = SessionId::new();
        let command = CompanionRendererCommand::AudioLevel {
            session_id,
            peak: 0.42,
            spectrum: vec![0.1, 0.4, 0.2],
        };

        let json = serde_json::to_string(&command).unwrap();
        let decoded = serde_json::from_str::<CompanionRendererCommand>(&json).unwrap();

        assert_eq!(decoded, command);

        let position = CompanionRendererCommand::SetHomePosition { x: -640, y: 320 };
        let json = serde_json::to_string(&position).unwrap();
        assert_eq!(
            serde_json::from_str::<CompanionRendererCommand>(&json).unwrap(),
            position
        );

        let changed = super::CompanionRendererEvent::HomePositionChanged { x: 120, y: 240 };
        let json = serde_json::to_string(&changed).unwrap();
        assert_eq!(
            serde_json::from_str::<super::CompanionRendererEvent>(&json).unwrap(),
            changed
        );
    }

    #[test]
    fn worker_messages_round_trip_without_losing_session_identity() {
        let session_id = SessionId::new();
        let input = InputHostEvent::DictationEnd {
            session_id,
            target: TargetSnapshot {
                window_handle: 42,
                process_id: 7,
                thread_id: 8,
                keyboard_layout: 9,
            },
        };
        let input_json = serde_json::to_string(&input).unwrap();
        assert_eq!(
            serde_json::from_str::<InputHostEvent>(&input_json).unwrap(),
            input
        );

        let insertion = InputHostEvent::InsertionCompleted {
            session_id,
            method: InsertionMethod::WezTermCli,
            delivery: InsertionDelivery::Committed,
            dispatch_duration_ms: 9,
        };
        let insertion_json = serde_json::to_string(&insertion).unwrap();
        assert_eq!(
            serde_json::from_str::<InputHostEvent>(&insertion_json).unwrap(),
            insertion
        );

        let speech = SpeechWorkerEvent::Completed {
            transcript: Transcript {
                session_id,
                text: "hello".to_owned(),
                language: "en".to_owned(),
                audio_duration_ms: 1200,
                processing_duration_ms: 300,
                provider: None,
                latency: super::TranscriptionLatency {
                    inference_ms: 300,
                    after_release_ms: 340,
                    ..Default::default()
                },
            },
            created_unix_ms: 123,
        };
        let speech_json = serde_json::to_string(&speech).unwrap();
        assert_eq!(
            serde_json::from_str::<SpeechWorkerEvent>(&speech_json).unwrap(),
            speech
        );
    }

    #[test]
    fn older_latency_payloads_default_the_new_queue_metric() {
        let latency: TranscriptionLatency = serde_json::from_str(
            r#"{
                "audio_finalize_ms": 12,
                "preprocessing_ms": 34,
                "inference_ms": 56,
                "after_release_ms": 102,
                "speculative_reuse": false
            }"#,
        )
        .unwrap();

        assert_eq!(latency.final_queue_wait_ms, 0);
        assert_eq!(latency.inference_ms, 56);
    }

    #[test]
    fn translation_messages_round_trip_without_losing_request_identity() {
        let request_id = RequestId::new();
        let event = TranslationWorkerEvent::Completed {
            result: TranslationResult {
                request_id,
                source_text: "hello".to_owned(),
                translated_text: "привет".to_owned(),
                source_language: TranslationLanguage::English,
                target_language: TranslationLanguage::Russian,
                tier: Some(TranslationTier::Tiny),
                provider: None,
                processing_duration_ms: 12,
                first_token_duration_ms: 4,
                backend_request_count: 1,
                tokenize_duration_ms: 0,
                chunk_count: 1,
            },
            created_unix_ms: 123,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<TranslationWorkerEvent>(&json).unwrap(),
            event
        );

        let selection = InputHostEvent::SelectionCaptured {
            request_id,
            intent: TextActionIntent::Adaptive,
            target: TargetSnapshot::default(),
            text: "hello".to_owned(),
            timing: SelectionCaptureTiming {
                release_wait_ms: Some(10),
                uia_ms: Some(3),
                clipboard_ms: None,
                total_ms: 10,
            },
        };
        let json = serde_json::to_string(&selection).unwrap();
        assert_eq!(
            serde_json::from_str::<InputHostEvent>(&json).unwrap(),
            selection
        );

        let capture_started = InputHostEvent::SelectionCaptureStarted {
            request_id,
            intent: TextActionIntent::Translate,
            target: TargetSnapshot::default(),
            started_unix_ms: 42,
        };
        let json = serde_json::to_string(&capture_started).unwrap();
        assert_eq!(
            serde_json::from_str::<InputHostEvent>(&json).unwrap(),
            capture_started
        );

        let input_released = InputHostEvent::SelectionInputReleased {
            request_id,
            release_wait_ms: 21,
        };
        let json = serde_json::to_string(&input_released).unwrap();
        assert_eq!(
            serde_json::from_str::<InputHostEvent>(&json).unwrap(),
            input_released
        );
    }
}

/// The name every message carries on the wire, from an exhaustive match.
///
/// Exhaustive on purpose. A list of strings sitting next to an enum drifts from
/// it silently — that is how protocol 16 came to describe two different
/// schemas. This does not compile until a new variant is named here, and the
/// test below does not pass until the version number moves with it.
#[must_use]
pub fn translation_event_name(event: &TranslationWorkerEvent) -> &'static str {
    match event {
        TranslationWorkerEvent::Ready { .. } => "ready",
        TranslationWorkerEvent::ModelDownloadProgress { .. } => "model_download_progress",
        TranslationWorkerEvent::ModelInstalled { .. } => "model_installed",
        TranslationWorkerEvent::ModelReady { .. } => "model_ready",
        TranslationWorkerEvent::ModelSelected { .. } => "model_selected",
        TranslationWorkerEvent::Partial { .. } => "partial",
        TranslationWorkerEvent::Completed { .. } => "completed",
        TranslationWorkerEvent::Cancelled { .. } => "cancelled",
        TranslationWorkerEvent::History { .. } => "history",
        TranslationWorkerEvent::TranslationProviders { .. } => "translation_providers",
        TranslationWorkerEvent::CredentialSaved { .. } => "credential_saved",
        TranslationWorkerEvent::Failed { .. } => "failed",
    }
}

/// The same, for what the speech worker sends.
#[must_use]
pub fn speech_event_name(event: &SpeechWorkerEvent) -> &'static str {
    match event {
        SpeechWorkerEvent::Ready { .. } => "ready",
        SpeechWorkerEvent::ModelDownloadProgress { .. } => "model_download_progress",
        SpeechWorkerEvent::ModelInstalled { .. } => "model_installed",
        SpeechWorkerEvent::ModelReady { .. } => "model_ready",
        SpeechWorkerEvent::ModelSelected { .. } => "model_selected",
        SpeechWorkerEvent::Listening { .. } => "listening",
        SpeechWorkerEvent::AudioLevel { .. } => "audio_level",
        SpeechWorkerEvent::Processing { .. } => "processing",
        SpeechWorkerEvent::Completed { .. } => "completed",
        SpeechWorkerEvent::Cancelled { .. } => "cancelled",
        SpeechWorkerEvent::History { .. } => "history",
        SpeechWorkerEvent::TranscriptionProviders { .. } => "transcription_providers",
        SpeechWorkerEvent::CredentialSaved { .. } => "credential_saved",
        SpeechWorkerEvent::TranscribedRemotely { .. } => "transcribed_remotely",
        SpeechWorkerEvent::Failed { .. } => "failed",
    }
}

#[cfg(test)]
mod wire_schema_tests {
    //! The shape of every message, pinned to the version number.
    //!
    //! Protocol 16 described two different schemas: an event was added under it
    //! and removed under it again, and both builds introduced themselves as 16.
    //! A list of variant names would not have caught that on its own — it has to
    //! be a list nobody can change without the number moving.
    //!
    //! So this is the whole wire surface: every variant of every message, with
    //! the fields it carries, written down as one fingerprint. Renaming a
    //! variant, adding or removing a field, or changing what serde calls it all
    //! fail here, and the only way to make them pass is to say which version the
    //! new shape belongs to.

    use serde_json::{Value, json};

    use super::*;

    /// One message's name and the fields it puts on the wire, sorted.
    fn shape(value: &Value) -> String {
        let name = value["type"].as_str().unwrap_or("<untagged>");
        let mut fields: Vec<&str> = value
            .get("payload")
            .and_then(Value::as_object)
            .map(|payload| payload.keys().map(String::as_str).collect())
            .unwrap_or_default();
        fields.sort_unstable();
        format!("{name}({})", fields.join(","))
    }

    fn fingerprint(messages: &[Value]) -> Vec<String> {
        let mut shapes: Vec<String> = messages.iter().map(shape).collect();
        shapes.sort();
        shapes
    }

    /// One of every translation event, filled in enough to serialise.
    fn translation_events() -> Vec<Value> {
        let request_id = RequestId::new();
        let result = TranslationResult {
            request_id,
            source_text: String::new(),
            translated_text: String::new(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            tier: None,
            provider: None,
            processing_duration_ms: 0,
            first_token_duration_ms: 0,
            backend_request_count: 0,
            tokenize_duration_ms: 0,
            chunk_count: 0,
        };
        [
            TranslationWorkerEvent::Ready {
                protocol_version: PROTOCOL_VERSION,
                selected_tier: None,
                models: Vec::new(),
            },
            TranslationWorkerEvent::ModelDownloadProgress {
                catalog_id: String::new(),
                downloaded_bytes: 0,
                total_bytes: 0,
            },
            TranslationWorkerEvent::ModelInstalled {
                catalog_id: String::new(),
                tier: TranslationTier::Tiny,
                path: String::new(),
            },
            TranslationWorkerEvent::ModelReady {
                catalog_id: String::new(),
                tier: TranslationTier::Tiny,
                load_duration_ms: 0,
            },
            TranslationWorkerEvent::ModelSelected {
                tier: TranslationTier::Tiny,
            },
            TranslationWorkerEvent::Partial {
                request_id,
                translated_text: String::new(),
                first_token_duration_ms: 0,
                tokenize_duration_ms: 0,
                chunk_index: 0,
                chunk_count: 0,
            },
            TranslationWorkerEvent::Completed {
                result,
                created_unix_ms: 0,
            },
            TranslationWorkerEvent::Cancelled { request_id },
            TranslationWorkerEvent::History {
                entries: Vec::new(),
            },
            TranslationWorkerEvent::TranslationProviders {
                statuses: Vec::new(),
            },
            TranslationWorkerEvent::CredentialSaved {
                operation_id: request_id,
                provider_id: String::new(),
                error: String::new(),
            },
            TranslationWorkerEvent::Failed {
                request_id: None,
                catalog_id: None,
                code: String::new(),
                message: String::new(),
                retryable: false,
            },
        ]
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect()
    }

    fn translation_commands() -> Vec<Value> {
        let request_id = RequestId::new();
        [
            TranslationWorkerCommand::Initialize {
                selected_tier: None,
                keep_warm: false,
                history_enabled: false,
            },
            TranslationWorkerCommand::EnsureModel {
                tier: TranslationTier::Tiny,
            },
            TranslationWorkerCommand::CancelModelInstall,
            TranslationWorkerCommand::SelectTier {
                tier: TranslationTier::Tiny,
                keep_warm: false,
            },
            TranslationWorkerCommand::Translate {
                request_id,
                text: String::new(),
                source_language: None,
                target_language: None,
            },
            TranslationWorkerCommand::Cancel { request_id },
            TranslationWorkerCommand::SetTranslationProviders {
                providers: Vec::new(),
            },
            TranslationWorkerCommand::SetTranslationProviderCredential {
                operation_id: request_id,
                provider_id: String::new(),
                secret: None,
            },
            TranslationWorkerCommand::GetTranslationProviders,
            TranslationWorkerCommand::GetHistory { limit: 0 },
            TranslationWorkerCommand::SetHistoryEnabled { enabled: false },
            TranslationWorkerCommand::ClearHistory,
            TranslationWorkerCommand::Shutdown,
        ]
        .iter()
        .map(|command| serde_json::to_value(command).unwrap())
        .collect()
    }

    #[test]
    fn the_translation_wire_shape_belongs_to_this_version() {
        assert_eq!(
            PROTOCOL_VERSION, 17,
            "the shapes below were written for protocol 17"
        );
        assert_eq!(
            fingerprint(&translation_events()),
            vec![
                "cancelled(request_id)",
                "completed(created_unix_ms,result)",
                "credential_saved(error,operation_id,provider_id)",
                "failed(code,message,request_id,retryable)",
                "history(entries)",
                "model_download_progress(catalog_id,downloaded_bytes,total_bytes)",
                "model_installed(catalog_id,path,tier)",
                "model_ready(catalog_id,load_duration_ms,tier)",
                "model_selected(tier)",
                "partial(chunk_count,chunk_index,first_token_duration_ms,request_id,\
tokenize_duration_ms,translated_text)",
                "ready(models,protocol_version,selected_tier)",
                "translation_providers(statuses)",
            ]
        );
        assert_eq!(
            fingerprint(&translation_commands()),
            vec![
                "cancel(request_id)",
                "cancel_model_install()",
                "clear_history()",
                "ensure_model(tier)",
                "get_history(limit)",
                "get_translation_providers()",
                "initialize(history_enabled,keep_warm,selected_tier)",
                "select_tier(keep_warm,tier)",
                "set_history_enabled(enabled)",
                "set_translation_provider_credential(operation_id,provider_id,secret)",
                "set_translation_providers(providers)",
                "shutdown()",
                "translate(request_id,source_language,target_language,text)",
            ]
        );
    }

    #[test]
    fn a_result_carries_the_fields_the_desktop_reads() {
        // The payload of a message matters as much as its name: `provider` is
        // what tells the window a translation came from a service rather than
        // from this machine, and it was added without the version moving.
        let result = json!({
            "request_id": RequestId::new(),
            "source_text": "",
            "translated_text": "",
            "source_language": "english",
            "target_language": "russian",
            "processing_duration_ms": 0,
        });
        let parsed: TranslationResult = serde_json::from_value(result).unwrap();

        assert_eq!(parsed.tier, None);
        assert_eq!(parsed.provider, None);
    }
}
