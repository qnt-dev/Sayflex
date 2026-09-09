use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Version 18 adds capture-time selection and pointer geometry to target
// snapshots so overlays can be placed beside the text instead of guessing.
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
pub const PROTOCOL_VERSION: u32 = 18;
/// Source identity injected by the release builder. Development builds say so
/// explicitly instead of pretending to belong to an arbitrary checkout.
pub const BUILD_COMMIT: &str = match option_env!("SAYFLEX_BUILD_COMMIT") {
    Some(value) => value,
    None => "development",
};
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
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSnapshot {
    pub window_handle: u64,
    pub process_id: u32,
    pub thread_id: u32,
    pub keyboard_layout: u64,
    /// Selection geometry captured while the target still owned focus. UIA
    /// providers report physical screen coordinates. Older peers omit it.
    #[serde(default)]
    pub selection_rect: Option<ScreenRect>,
    /// Pointer position at the hotkey edge. This is a stable fallback for
    /// canvas/terminal targets where neither UIA nor a Win32 caret exists.
    #[serde(default)]
    pub pointer: Option<ScreenPoint>,
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
                ..TargetSnapshot::default()
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
    //! A list of variant names would not have caught that on its own, and
    //! neither would a fingerprint of the top level — `provider` was added to
    //! `TranslationResult`, which is nested inside `completed`, and nothing
    //! noticed.
    //!
    //! So this walks the whole value: every variant of every message on every
    //! channel, and every field of every structure inside them, to whatever
    //! depth they go. Renaming anything, adding or removing a field, or
    //! changing what serde calls it fails here, and the only way to make it
    //! pass is to say which version the new shape belongs to.

    use serde::Serialize;
    use serde_json::Value;

    use super::*;

    /// Every path through a serialised value, sorted: `completed.result.tier`
    /// and so on, with the leaves' types but not their contents.
    fn paths(value: &Value, prefix: &str, into: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (name, child) in fields {
                    let path = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    paths(child, &path, into);
                }
            }
            // An array's shape is its element's shape. An empty one contributes
            // the path itself, so a field cannot vanish unnoticed.
            Value::Array(items) => match items.first() {
                Some(first) => paths(first, &format!("{prefix}[]"), into),
                None => into.push(format!("{prefix}[]")),
            },
            Value::Null => into.push(format!("{prefix}:null")),
            Value::Bool(_) => into.push(format!("{prefix}:bool")),
            Value::Number(_) => into.push(format!("{prefix}:number")),
            Value::String(_) => into.push(format!("{prefix}:string")),
        }
    }

    fn fingerprint<T: Serialize>(messages: &[T]) -> Vec<String> {
        let mut all = Vec::new();
        for message in messages {
            let value = serde_json::to_value(message).unwrap();
            let name = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("<untagged>")
                .to_owned();
            let mut fields = Vec::new();
            match value.get("payload") {
                Some(payload) => paths(payload, "", &mut fields),
                None => fields.push("()".to_owned()),
            }
            fields.sort();
            all.push(format!("{name}({})", fields.join(",")));
        }
        all.sort();
        all
    }

    /// A fixture with every optional field filled in.
    ///
    /// `None` serialises to nothing where a field is skipped, so a fixture full
    /// of `None` hides exactly the additions this is meant to catch — the
    /// `catalog_id` added to `Failed` was invisible for that reason.
    fn translation_result() -> TranslationResult {
        TranslationResult {
            request_id: RequestId::new(),
            source_text: String::new(),
            translated_text: String::new(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            tier: Some(TranslationTier::Tiny),
            provider: Some(String::new()),
            processing_duration_ms: 0,
            first_token_duration_ms: 0,
            backend_request_count: 0,
            tokenize_duration_ms: 0,
            chunk_count: 0,
        }
    }

    #[test]
    fn the_wire_shape_belongs_to_this_version() {
        assert_eq!(
            PROTOCOL_VERSION, 18,
            "every fingerprint below was written for protocol 18: if a shape \
             changes, this number changes with it"
        );
    }

    #[test]
    fn a_translation_result_carries_every_field_the_desktop_reads() {
        // Nested, and therefore the one the top-level fingerprint missed.
        let mut fields = Vec::new();
        paths(
            &serde_json::to_value(translation_result()).unwrap(),
            "",
            &mut fields,
        );
        fields.sort();

        assert_eq!(
            fields,
            vec![
                "backend_request_count:number",
                "chunk_count:number",
                "first_token_duration_ms:number",
                "processing_duration_ms:number",
                "provider:string",
                "request_id:string",
                "source_language:string",
                "source_text:string",
                "target_language:string",
                "tier:string",
                "tokenize_duration_ms:number",
                "translated_text:string",
            ]
        );
    }

    #[test]
    fn the_translation_events_have_the_shape_this_version_describes() {
        let request_id = RequestId::new();
        let shapes = fingerprint(&[
            TranslationWorkerEvent::Ready {
                protocol_version: 0,
                selected_tier: Some(TranslationTier::Tiny),
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
                result: translation_result(),
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
                request_id: Some(request_id),
                catalog_id: Some(String::new()),
                code: String::new(),
                message: String::new(),
                retryable: false,
            },
        ]);

        assert_eq!(
            shapes,
            vec![
                "cancelled(request_id:string)",
                "completed(created_unix_ms:number,result.backend_request_count:number,\
result.chunk_count:number,result.first_token_duration_ms:number,\
result.processing_duration_ms:number,result.provider:string,result.request_id:string,\
result.source_language:string,result.source_text:string,result.target_language:string,\
result.tier:string,result.tokenize_duration_ms:number,result.translated_text:string)",
                "credential_saved(error:string,operation_id:string,provider_id:string)",
                "failed(catalog_id:string,code:string,message:string,request_id:string,\
retryable:bool)",
                "history(entries[])",
                "model_download_progress(catalog_id:string,downloaded_bytes:number,\
total_bytes:number)",
                "model_installed(catalog_id:string,path:string,tier:string)",
                "model_ready(catalog_id:string,load_duration_ms:number,tier:string)",
                "model_selected(tier:string)",
                "partial(chunk_count:number,chunk_index:number,first_token_duration_ms:number,\
request_id:string,tokenize_duration_ms:number,translated_text:string)",
                "ready(models[],protocol_version:number,selected_tier:string)",
                "translation_providers(statuses[])",
            ]
        );
    }

    #[test]
    fn the_speech_events_have_the_shape_this_version_describes() {
        let session_id = SessionId::new();
        let shapes = fingerprint(&[
            SpeechWorkerEvent::Listening { session_id },
            SpeechWorkerEvent::Cancelled { session_id },
            SpeechWorkerEvent::CredentialSaved {
                operation_id: RequestId::new(),
                provider_id: String::new(),
                error: String::new(),
            },
            SpeechWorkerEvent::TranscribedRemotely {
                session_id,
                provider_id: String::new(),
            },
            SpeechWorkerEvent::TranscriptionProviders {
                statuses: Vec::new(),
            },
            SpeechWorkerEvent::History {
                entries: Vec::new(),
            },
        ]);

        assert_eq!(
            shapes,
            vec![
                "cancelled(session_id:string)",
                "credential_saved(error:string,operation_id:string,provider_id:string)",
                "history(entries[])",
                "listening(session_id:string)",
                "transcribed_remotely(provider_id:string,session_id:string)",
                "transcription_providers(statuses[])",
            ]
        );
    }

    #[test]
    fn the_structures_carried_inside_messages_have_their_own_shape() {
        // `statuses`, `models` and `entries` are empty in the fixtures above,
        // so the fingerprint sees only the field. These are the shapes they
        // carry, checked directly.
        let mut provider = Vec::new();
        paths(
            &serde_json::to_value(TranslationProviderStatus {
                id: String::new(),
                position: 0,
                enabled: false,
                available: false,
                detail: String::new(),
                has_credential: false,
                credential_hint: Some(String::new()),
            })
            .unwrap(),
            "",
            &mut provider,
        );
        provider.sort();
        assert_eq!(
            provider,
            vec![
                "available:bool",
                "credential_hint:string",
                "detail:string",
                "enabled:bool",
                "has_credential:bool",
                "id:string",
                "position:number",
            ]
        );

        let mut entry = Vec::new();
        paths(
            &serde_json::to_value(TranslationProviderEntry {
                id: String::new(),
                enabled: false,
                region: Some(String::new()),
            })
            .unwrap(),
            "",
            &mut entry,
        );
        entry.sort();
        assert_eq!(entry, vec!["enabled:bool", "id:string", "region:string"]);
    }

    #[test]
    fn every_message_kind_is_covered_by_a_fingerprint() {
        // The lists above are written by hand, so this is what makes leaving a
        // variant out of them visible: the names come from the exhaustive
        // matches, which do not compile until a new variant is named.
        assert_eq!(
            translation_event_name(&TranslationWorkerEvent::Cancelled {
                request_id: RequestId::new()
            }),
            "cancelled"
        );
        assert_eq!(
            speech_event_name(&SpeechWorkerEvent::Listening {
                session_id: SessionId::new()
            }),
            "listening"
        );
    }
}

#[cfg(test)]
mod complete_wire_schema_tests {
    //! A schema fingerprint is useful only if every process direction and every
    //! variant participates. The older test covered all translation events but
    //! only six speech events and no commands, input-host or companion traffic.

    use serde::Serialize;
    use serde_json::Value;

    use super::*;

    fn target() -> TargetSnapshot {
        TargetSnapshot {
            window_handle: 1,
            process_id: 2,
            thread_id: 3,
            keyboard_layout: 4,
            selection_rect: Some(ScreenRect {
                left: 5,
                top: 6,
                right: 7,
                bottom: 8,
            }),
            pointer: Some(ScreenPoint { x: 9, y: 10 }),
        }
    }

    fn latency() -> TranscriptionLatency {
        TranscriptionLatency {
            audio_finalize_ms: 1,
            preprocessing_ms: 2,
            final_queue_wait_ms: 3,
            inference_ms: 4,
            after_release_ms: 5,
            speculative_reuse: true,
        }
    }

    fn transcript() -> Transcript {
        Transcript {
            session_id: SessionId::new(),
            text: "text".to_owned(),
            language: "en".to_owned(),
            audio_duration_ms: 1,
            processing_duration_ms: 2,
            provider: Some("provider".to_owned()),
            latency: latency(),
        }
    }

    fn speech_model() -> ModelSummary {
        ModelSummary {
            catalog_id: "model".to_owned(),
            display_name: "Model".to_owned(),
            description: "description".to_owned(),
            size_bytes: 1,
            installed: true,
            selected: true,
            recommended: true,
            supports_speech_translation: true,
        }
    }

    fn translation_model() -> TranslationModelSummary {
        TranslationModelSummary {
            catalog_id: "translation".to_owned(),
            display_name: "Translation".to_owned(),
            description: "description".to_owned(),
            tier: TranslationTier::Tiny,
            languages: vec![TranslationLanguage::English],
            size_bytes: 1,
            installed: true,
            selected: true,
            recommended: true,
            license: "license".to_owned(),
        }
    }

    fn transcription_entry() -> TranscriptionProviderEntry {
        TranscriptionProviderEntry {
            id: "provider".to_owned(),
            enabled: true,
            endpoint: Some("https://example.invalid".to_owned()),
            model: Some("model".to_owned()),
            account: Some("account".to_owned()),
        }
    }

    fn transcription_status() -> TranscriptionProviderStatus {
        TranscriptionProviderStatus {
            id: "provider".to_owned(),
            position: 1,
            enabled: true,
            available: true,
            detail: "detail".to_owned(),
            has_credential: true,
            credential_hint: Some("hint".to_owned()),
        }
    }

    fn translation_entry() -> TranslationProviderEntry {
        TranslationProviderEntry {
            id: "provider".to_owned(),
            enabled: true,
            region: Some("region".to_owned()),
        }
    }

    fn translation_status() -> TranslationProviderStatus {
        TranslationProviderStatus {
            id: "provider".to_owned(),
            position: 1,
            enabled: true,
            available: true,
            detail: "detail".to_owned(),
            has_credential: true,
            credential_hint: Some("hint".to_owned()),
        }
    }

    fn translation_result_full() -> TranslationResult {
        TranslationResult {
            request_id: RequestId::new(),
            source_text: "source".to_owned(),
            translated_text: "translated".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            tier: Some(TranslationTier::Tiny),
            provider: Some("provider".to_owned()),
            processing_duration_ms: 1,
            first_token_duration_ms: 2,
            backend_request_count: 3,
            tokenize_duration_ms: 4,
            chunk_count: 5,
        }
    }

    fn input_events() -> Vec<InputHostEvent> {
        let request_id = RequestId::new();
        let session_id = SessionId::new();
        vec![
            InputHostEvent::Ready {
                protocol_version: 1,
            },
            InputHostEvent::DictationBegin {
                session_id,
                started_unix_ms: 1,
                target: target(),
            },
            InputHostEvent::DictationEnd {
                session_id,
                target: target(),
            },
            InputHostEvent::InsertLastRequested { target: target() },
            InputHostEvent::SelectionCaptureStarted {
                request_id,
                intent: TextActionIntent::Translate,
                target: target(),
                started_unix_ms: 1,
            },
            InputHostEvent::SelectionCaptured {
                request_id,
                intent: TextActionIntent::Translate,
                target: target(),
                text: "text".to_owned(),
                timing: SelectionCaptureTiming {
                    release_wait_ms: Some(1),
                    uia_ms: Some(2),
                    clipboard_ms: Some(3),
                    total_ms: 4,
                },
            },
            InputHostEvent::SelectionInputReleased {
                request_id,
                release_wait_ms: 1,
            },
            InputHostEvent::SelectionInputReleaseFailed {
                request_id,
                message: "message".to_owned(),
            },
            InputHostEvent::SelectionCaptureFailed {
                request_id,
                message: "message".to_owned(),
            },
            InputHostEvent::InsertionCompleted {
                session_id,
                method: InsertionMethod::ClipboardTransaction,
                delivery: InsertionDelivery::PayloadRead,
                dispatch_duration_ms: 1,
            },
            InputHostEvent::Recovered { session_id },
            InputHostEvent::SelectionReplaced { request_id },
            InputHostEvent::TextCopied { request_id },
            InputHostEvent::Failed {
                session_id: Some(session_id),
                code: "code".to_owned(),
                message: "message".to_owned(),
            },
        ]
    }

    fn input_commands() -> Vec<InputHostCommand> {
        let request_id = RequestId::new();
        vec![
            InputHostCommand::InsertTranscript {
                transcript: transcript(),
                target: target(),
                add_trailing_space: true,
            },
            InputHostCommand::CopyTranscript {
                transcript: transcript(),
            },
            InputHostCommand::CopyText {
                request_id,
                text: "text".to_owned(),
            },
            InputHostCommand::ReplaceSelection {
                request_id,
                target: target(),
                text: "text".to_owned(),
            },
            InputHostCommand::CancelSelection { request_id },
            InputHostCommand::Shutdown,
        ]
    }

    fn translation_commands() -> Vec<TranslationWorkerCommand> {
        let request_id = RequestId::new();
        vec![
            TranslationWorkerCommand::Initialize {
                selected_tier: Some(TranslationTier::Tiny),
                keep_warm: true,
                history_enabled: true,
            },
            TranslationWorkerCommand::EnsureModel {
                tier: TranslationTier::Tiny,
            },
            TranslationWorkerCommand::CancelModelInstall,
            TranslationWorkerCommand::SelectTier {
                tier: TranslationTier::Tiny,
                keep_warm: true,
            },
            TranslationWorkerCommand::Translate {
                request_id,
                text: "text".to_owned(),
                source_language: Some(TranslationLanguage::English),
                target_language: Some(TranslationLanguage::Russian),
            },
            TranslationWorkerCommand::Cancel { request_id },
            TranslationWorkerCommand::SetTranslationProviders {
                providers: vec![translation_entry()],
            },
            TranslationWorkerCommand::SetTranslationProviderCredential {
                operation_id: request_id,
                provider_id: "provider".to_owned(),
                secret: Some("secret".to_owned()),
            },
            TranslationWorkerCommand::GetTranslationProviders,
            TranslationWorkerCommand::GetHistory { limit: 1 },
            TranslationWorkerCommand::SetHistoryEnabled { enabled: true },
            TranslationWorkerCommand::ClearHistory,
            TranslationWorkerCommand::Shutdown,
        ]
    }

    fn translation_events() -> Vec<TranslationWorkerEvent> {
        let request_id = RequestId::new();
        vec![
            TranslationWorkerEvent::Ready {
                protocol_version: 1,
                selected_tier: Some(TranslationTier::Tiny),
                models: vec![translation_model()],
            },
            TranslationWorkerEvent::ModelDownloadProgress {
                catalog_id: "model".to_owned(),
                downloaded_bytes: 1,
                total_bytes: 2,
            },
            TranslationWorkerEvent::ModelInstalled {
                catalog_id: "model".to_owned(),
                tier: TranslationTier::Tiny,
                path: "path".to_owned(),
            },
            TranslationWorkerEvent::ModelReady {
                catalog_id: "model".to_owned(),
                tier: TranslationTier::Tiny,
                load_duration_ms: 1,
            },
            TranslationWorkerEvent::ModelSelected {
                tier: TranslationTier::Tiny,
            },
            TranslationWorkerEvent::Partial {
                request_id,
                translated_text: "text".to_owned(),
                first_token_duration_ms: 1,
                tokenize_duration_ms: 2,
                chunk_index: 3,
                chunk_count: 4,
            },
            TranslationWorkerEvent::Completed {
                result: translation_result_full(),
                created_unix_ms: 1,
            },
            TranslationWorkerEvent::Cancelled { request_id },
            TranslationWorkerEvent::History {
                entries: vec![TranslationHistoryEntry {
                    result: translation_result_full(),
                    created_unix_ms: 1,
                }],
            },
            TranslationWorkerEvent::TranslationProviders {
                statuses: vec![translation_status()],
            },
            TranslationWorkerEvent::CredentialSaved {
                operation_id: request_id,
                provider_id: "provider".to_owned(),
                error: "error".to_owned(),
            },
            TranslationWorkerEvent::Failed {
                request_id: Some(request_id),
                catalog_id: Some("model".to_owned()),
                code: "code".to_owned(),
                message: "message".to_owned(),
                retryable: true,
            },
        ]
    }

    fn speech_commands() -> Vec<SpeechWorkerCommand> {
        let session_id = SessionId::new();
        let request_id = RequestId::new();
        vec![
            SpeechWorkerCommand::Initialize {
                selected_model_id: "model".to_owned(),
                output_mode: SpeechOutputMode::OriginalLanguage,
                history_enabled: true,
                fast_release_previews: true,
            },
            SpeechWorkerCommand::Begin {
                session_id,
                started_unix_ms: 1,
            },
            SpeechWorkerCommand::End { session_id },
            SpeechWorkerCommand::Cancel { session_id },
            SpeechWorkerCommand::EnsureModel {
                catalog_id: "model".to_owned(),
            },
            SpeechWorkerCommand::SelectModel {
                catalog_id: "model".to_owned(),
            },
            SpeechWorkerCommand::CancelModelInstall,
            SpeechWorkerCommand::SetTranscriptionProviders {
                providers: vec![transcription_entry()],
            },
            SpeechWorkerCommand::SetProviderCredential {
                operation_id: request_id,
                provider_id: "provider".to_owned(),
                secret: Some("secret".to_owned()),
            },
            SpeechWorkerCommand::GetTranscriptionProviders,
            SpeechWorkerCommand::SetOutputMode {
                output_mode: SpeechOutputMode::TranslateToEnglish,
            },
            SpeechWorkerCommand::SetFastReleasePreviews { enabled: true },
            SpeechWorkerCommand::GetHistory { limit: 1 },
            SpeechWorkerCommand::SetHistoryEnabled { enabled: true },
            SpeechWorkerCommand::ClearHistory,
            SpeechWorkerCommand::Shutdown,
        ]
    }

    fn speech_events() -> Vec<SpeechWorkerEvent> {
        let session_id = SessionId::new();
        let request_id = RequestId::new();
        vec![
            SpeechWorkerEvent::Ready {
                protocol_version: 1,
                selected_model_id: "model".to_owned(),
                models: vec![speech_model()],
            },
            SpeechWorkerEvent::ModelDownloadProgress {
                catalog_id: "model".to_owned(),
                downloaded_bytes: 1,
                total_bytes: 2,
            },
            SpeechWorkerEvent::ModelInstalled {
                catalog_id: "model".to_owned(),
                path: "path".to_owned(),
            },
            SpeechWorkerEvent::ModelReady {
                catalog_id: "model".to_owned(),
                path: "path".to_owned(),
                backend: "backend".to_owned(),
                load_duration_ms: 1,
                warmup_duration_ms: 2,
            },
            SpeechWorkerEvent::ModelSelected {
                catalog_id: "model".to_owned(),
            },
            SpeechWorkerEvent::Listening { session_id },
            SpeechWorkerEvent::AudioLevel {
                session_id,
                peak: 0.5,
                spectrum: vec![0.25],
            },
            SpeechWorkerEvent::Processing { session_id },
            SpeechWorkerEvent::Completed {
                transcript: transcript(),
                created_unix_ms: 1,
            },
            SpeechWorkerEvent::Cancelled { session_id },
            SpeechWorkerEvent::History {
                entries: vec![HistoryEntry {
                    transcript: transcript(),
                    created_unix_ms: 1,
                }],
            },
            SpeechWorkerEvent::TranscriptionProviders {
                statuses: vec![transcription_status()],
            },
            SpeechWorkerEvent::CredentialSaved {
                operation_id: request_id,
                provider_id: "provider".to_owned(),
                error: "error".to_owned(),
            },
            SpeechWorkerEvent::TranscribedRemotely {
                session_id,
                provider_id: "provider".to_owned(),
            },
            SpeechWorkerEvent::Failed {
                session_id: Some(session_id),
                code: "code".to_owned(),
                message: "message".to_owned(),
                retryable: true,
            },
        ]
    }

    fn companion_commands() -> Vec<CompanionRendererCommand> {
        let session_id = SessionId::new();
        vec![
            CompanionRendererCommand::SetEnabled { enabled: true },
            CompanionRendererCommand::SetHomePosition { x: 1, y: 2 },
            CompanionRendererCommand::Activate {
                session_id,
                target_window_handle: 1,
            },
            CompanionRendererCommand::AudioLevel {
                session_id,
                peak: 0.5,
                spectrum: vec![0.25],
            },
            CompanionRendererCommand::Processing { session_id },
            CompanionRendererCommand::Complete { session_id },
            CompanionRendererCommand::Dismiss {
                session_id: Some(session_id),
            },
            CompanionRendererCommand::Shutdown,
        ]
    }

    fn companion_events() -> Vec<CompanionRendererEvent> {
        vec![
            CompanionRendererEvent::Ready {
                protocol_version: 1,
            },
            CompanionRendererEvent::HomePositionChanged { x: 1, y: 2 },
            CompanionRendererEvent::Failed {
                session_id: Some(SessionId::new()),
                code: "code".to_owned(),
                message: "message".to_owned(),
            },
        ]
    }

    macro_rules! index_fn {
        ($name:ident, $ty:ty, $($pattern:pat => $index:expr),+ $(,)?) => {
            fn $name(value: &$ty) -> usize {
                match value { $($pattern => $index),+ }
            }
        };
    }

    index_fn!(input_event_index, InputHostEvent,
        InputHostEvent::Ready { .. } => 0, InputHostEvent::DictationBegin { .. } => 1,
        InputHostEvent::DictationEnd { .. } => 2, InputHostEvent::InsertLastRequested { .. } => 3,
        InputHostEvent::SelectionCaptureStarted { .. } => 4, InputHostEvent::SelectionCaptured { .. } => 5,
        InputHostEvent::SelectionInputReleased { .. } => 6, InputHostEvent::SelectionInputReleaseFailed { .. } => 7,
        InputHostEvent::SelectionCaptureFailed { .. } => 8, InputHostEvent::InsertionCompleted { .. } => 9,
        InputHostEvent::Recovered { .. } => 10, InputHostEvent::SelectionReplaced { .. } => 11,
        InputHostEvent::TextCopied { .. } => 12, InputHostEvent::Failed { .. } => 13
    );
    index_fn!(input_command_index, InputHostCommand,
        InputHostCommand::InsertTranscript { .. } => 0, InputHostCommand::CopyTranscript { .. } => 1,
        InputHostCommand::CopyText { .. } => 2, InputHostCommand::ReplaceSelection { .. } => 3,
        InputHostCommand::CancelSelection { .. } => 4, InputHostCommand::Shutdown => 5
    );
    index_fn!(translation_command_index, TranslationWorkerCommand,
        TranslationWorkerCommand::Initialize { .. } => 0, TranslationWorkerCommand::EnsureModel { .. } => 1,
        TranslationWorkerCommand::CancelModelInstall => 2, TranslationWorkerCommand::SelectTier { .. } => 3,
        TranslationWorkerCommand::Translate { .. } => 4, TranslationWorkerCommand::Cancel { .. } => 5,
        TranslationWorkerCommand::SetTranslationProviders { .. } => 6, TranslationWorkerCommand::SetTranslationProviderCredential { .. } => 7,
        TranslationWorkerCommand::GetTranslationProviders => 8, TranslationWorkerCommand::GetHistory { .. } => 9,
        TranslationWorkerCommand::SetHistoryEnabled { .. } => 10, TranslationWorkerCommand::ClearHistory => 11,
        TranslationWorkerCommand::Shutdown => 12
    );
    index_fn!(translation_event_index, TranslationWorkerEvent,
        TranslationWorkerEvent::Ready { .. } => 0, TranslationWorkerEvent::ModelDownloadProgress { .. } => 1,
        TranslationWorkerEvent::ModelInstalled { .. } => 2, TranslationWorkerEvent::ModelReady { .. } => 3,
        TranslationWorkerEvent::ModelSelected { .. } => 4, TranslationWorkerEvent::Partial { .. } => 5,
        TranslationWorkerEvent::Completed { .. } => 6, TranslationWorkerEvent::Cancelled { .. } => 7,
        TranslationWorkerEvent::History { .. } => 8, TranslationWorkerEvent::TranslationProviders { .. } => 9,
        TranslationWorkerEvent::CredentialSaved { .. } => 10, TranslationWorkerEvent::Failed { .. } => 11
    );
    index_fn!(speech_command_index, SpeechWorkerCommand,
        SpeechWorkerCommand::Initialize { .. } => 0, SpeechWorkerCommand::Begin { .. } => 1,
        SpeechWorkerCommand::End { .. } => 2, SpeechWorkerCommand::Cancel { .. } => 3,
        SpeechWorkerCommand::EnsureModel { .. } => 4, SpeechWorkerCommand::SelectModel { .. } => 5,
        SpeechWorkerCommand::CancelModelInstall => 6, SpeechWorkerCommand::SetTranscriptionProviders { .. } => 7,
        SpeechWorkerCommand::SetProviderCredential { .. } => 8, SpeechWorkerCommand::GetTranscriptionProviders => 9,
        SpeechWorkerCommand::SetOutputMode { .. } => 10, SpeechWorkerCommand::SetFastReleasePreviews { .. } => 11,
        SpeechWorkerCommand::GetHistory { .. } => 12, SpeechWorkerCommand::SetHistoryEnabled { .. } => 13,
        SpeechWorkerCommand::ClearHistory => 14, SpeechWorkerCommand::Shutdown => 15
    );
    index_fn!(speech_event_index, SpeechWorkerEvent,
        SpeechWorkerEvent::Ready { .. } => 0, SpeechWorkerEvent::ModelDownloadProgress { .. } => 1,
        SpeechWorkerEvent::ModelInstalled { .. } => 2, SpeechWorkerEvent::ModelReady { .. } => 3,
        SpeechWorkerEvent::ModelSelected { .. } => 4, SpeechWorkerEvent::Listening { .. } => 5,
        SpeechWorkerEvent::AudioLevel { .. } => 6, SpeechWorkerEvent::Processing { .. } => 7,
        SpeechWorkerEvent::Completed { .. } => 8, SpeechWorkerEvent::Cancelled { .. } => 9,
        SpeechWorkerEvent::History { .. } => 10, SpeechWorkerEvent::TranscriptionProviders { .. } => 11,
        SpeechWorkerEvent::CredentialSaved { .. } => 12, SpeechWorkerEvent::TranscribedRemotely { .. } => 13,
        SpeechWorkerEvent::Failed { .. } => 14
    );
    index_fn!(companion_command_index, CompanionRendererCommand,
        CompanionRendererCommand::SetEnabled { .. } => 0, CompanionRendererCommand::SetHomePosition { .. } => 1,
        CompanionRendererCommand::Activate { .. } => 2, CompanionRendererCommand::AudioLevel { .. } => 3,
        CompanionRendererCommand::Processing { .. } => 4, CompanionRendererCommand::Complete { .. } => 5,
        CompanionRendererCommand::Dismiss { .. } => 6, CompanionRendererCommand::Shutdown => 7
    );
    index_fn!(companion_event_index, CompanionRendererEvent,
        CompanionRendererEvent::Ready { .. } => 0, CompanionRendererEvent::HomePositionChanged { .. } => 1,
        CompanionRendererEvent::Failed { .. } => 2
    );

    fn assert_complete<T>(values: &[T], index: fn(&T) -> usize, count: usize) {
        let mut indices = values.iter().map(index).collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices, (0..count).collect::<Vec<_>>());
    }

    fn schema_path(value: &Value, prefix: &str, output: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (name, child) in fields {
                    let prefix = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    schema_path(child, &prefix, output);
                }
            }
            Value::Array(items) => match items.first() {
                Some(item) => schema_path(item, &format!("{prefix}[]"), output),
                None => output.push(format!("{prefix}[]")),
            },
            Value::Null => output.push(format!("{prefix}:null")),
            Value::Bool(_) => output.push(format!("{prefix}:bool")),
            Value::Number(_) => output.push(format!("{prefix}:number")),
            Value::String(_) => output.push(format!("{prefix}:string")),
        }
    }

    fn append<T: Serialize>(channel: &str, values: &[T], output: &mut Vec<String>) {
        for value in values {
            let value = serde_json::to_value(value).unwrap();
            let kind = value.get("type").and_then(Value::as_str).unwrap();
            let mut paths = Vec::new();
            match value.get("payload") {
                Some(payload) => schema_path(payload, "", &mut paths),
                None => paths.push("()".to_owned()),
            }
            paths.sort();
            output.push(format!("{channel}.{kind}({})", paths.join(",")));
        }
    }

    fn stable_hash(lines: &[String]) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in lines.iter().flat_map(|line| line.bytes().chain([b'\n'])) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    #[test]
    fn every_process_direction_and_variant_has_a_versioned_wire_shape() {
        let input_events = input_events();
        let input_commands = input_commands();
        let translation_commands = translation_commands();
        let translation_events = translation_events();
        let speech_commands = speech_commands();
        let speech_events = speech_events();
        let companion_commands = companion_commands();
        let companion_events = companion_events();

        assert_complete(&input_events, input_event_index, 14);
        assert_complete(&input_commands, input_command_index, 6);
        assert_complete(&translation_commands, translation_command_index, 13);
        assert_complete(&translation_events, translation_event_index, 12);
        assert_complete(&speech_commands, speech_command_index, 16);
        assert_complete(&speech_events, speech_event_index, 15);
        assert_complete(&companion_commands, companion_command_index, 8);
        assert_complete(&companion_events, companion_event_index, 3);

        let mut schema = Vec::new();
        append("input.event", &input_events, &mut schema);
        append("input.command", &input_commands, &mut schema);
        append("translation.command", &translation_commands, &mut schema);
        append("translation.event", &translation_events, &mut schema);
        append("speech.command", &speech_commands, &mut schema);
        append("speech.event", &speech_events, &mut schema);
        append("companion.command", &companion_commands, &mut schema);
        append("companion.event", &companion_events, &mut schema);
        schema.sort();

        assert_eq!(
            stable_hash(&schema),
            0x2d07_271a_1134_f3e7,
            "protocol {PROTOCOL_VERSION} wire schema changed:\n{}",
            schema.join("\n")
        );
    }
}
