use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Version 22 adds atomic, editable hotkey configuration and acknowledgements.
// Version 23 adds worker-owned model removal, explicit local-engine residency
// events, and per-action hotkey application failures.
// Version 25 adds the screen-translation hotkey and local OCR worker protocol.
// Version 26 adds explicit in-memory model release commands for Resources.
// Version 30 adds contextual screen blocks, explicit block completion, and
// personal-glossary commands/events without changing the legacy text path.
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
pub const PROTOCOL_VERSION: u32 = 33;
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

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Uuid::parse_str(value).ok().map(Self)
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
    Ukrainian,
    Spanish,
    PortugueseBrazil,
    German,
    French,
    Italian,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    Hindi,
    Arabic,
}

impl TranslationLanguage {
    pub const ALL: [Self; 14] = [
        Self::English,
        Self::Russian,
        Self::Ukrainian,
        Self::Spanish,
        Self::PortugueseBrazil,
        Self::German,
        Self::French,
        Self::Italian,
        Self::ChineseSimplified,
        Self::ChineseTraditional,
        Self::Japanese,
        Self::Korean,
        Self::Hindi,
        Self::Arabic,
    ];

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
            Self::Ukrainian => "uk",
            Self::Spanish => "es",
            Self::PortugueseBrazil => "pt-BR",
            Self::German => "de",
            Self::French => "fr",
            Self::Italian => "it",
            Self::ChineseSimplified => "zh-Hans",
            Self::ChineseTraditional => "zh-Hant",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Hindi => "hi",
            Self::Arabic => "ar",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Russian",
            Self::Ukrainian => "Ukrainian",
            Self::Spanish => "Spanish",
            Self::PortugueseBrazil => "Portuguese (Brazil)",
            Self::German => "German",
            Self::French => "French",
            Self::Italian => "Italian",
            Self::ChineseSimplified => "Chinese (Simplified)",
            Self::ChineseTraditional => "Chinese (Traditional)",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::Hindi => "Hindi",
            Self::Arabic => "Arabic",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| {
            language.code().eq_ignore_ascii_case(code)
                || language
                    .code()
                    .split('-')
                    .next()
                    .is_some_and(|base| base.eq_ignore_ascii_case(code))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationLanguagePolicy {
    pub primary: TranslationLanguage,
    pub secondary: TranslationLanguage,
}

impl Default for TranslationLanguagePolicy {
    fn default() -> Self {
        Self {
            primary: TranslationLanguage::English,
            secondary: TranslationLanguage::Russian,
        }
    }
}

impl TranslationLanguagePolicy {
    #[must_use]
    pub fn target_for(self, source: TranslationLanguage) -> TranslationLanguage {
        if source == self.primary {
            self.secondary
        } else {
            self.primary
        }
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        self.primary != self.secondary
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationTier {
    Compact,
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
#[serde(rename_all = "snake_case")]
pub enum TranslationFailureKind {
    Credential,
    Misconfigured,
    QuotaExhausted,
    RateLimited,
    Timeout,
    Unavailable,
    UnsupportedRequest,
    Rejected,
    EmptyResponse,
    NoText,
    NoProviderAvailable,
    LocalModelMissing,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationAttemptReport {
    pub provider_id: String,
    pub kind: TranslationFailureKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Redacted provider detail for diagnostics. User-facing copy is projected
    /// from `kind`, never inferred by parsing this field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationFailureReport {
    pub kind: TranslationFailureKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Exact local package required to continue, when the failure is
    /// `LocalModelMissing`. The desktop can offer/install that package without
    /// parsing technical prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_catalog_id: Option<String>,
    /// Redacted technical context. The desktop renders localized copy from the
    /// typed fields and keeps this secondary.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TranslationAttemptReport>,
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
    /// Providers tried before the successful provider or local engine. This is
    /// empty for a direct success.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TranslationAttemptReport>,
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

/// What language the microphone is expected to hear.
///
/// This is deliberately independent of the Windows display language and of
/// text-translation direction. `Auto` lets capable providers and the local
/// Whisper model detect the recording; a named language is an explicit hint
/// from the person speaking and is forwarded only where the provider accepts
/// one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationLanguage {
    #[default]
    Auto,
    English,
    Russian,
    Ukrainian,
    Spanish,
    PortugueseBrazil,
    German,
    French,
    Italian,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    Hindi,
    Arabic,
}

impl DictationLanguage {
    pub const ALL: [Self; 15] = [
        Self::Auto,
        Self::English,
        Self::Russian,
        Self::Ukrainian,
        Self::Spanish,
        Self::PortugueseBrazil,
        Self::German,
        Self::French,
        Self::Italian,
        Self::ChineseSimplified,
        Self::ChineseTraditional,
        Self::Japanese,
        Self::Korean,
        Self::Hindi,
        Self::Arabic,
    ];

    #[must_use]
    pub const fn code(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::English => Some("en"),
            Self::Russian => Some("ru"),
            Self::Ukrainian => Some("uk"),
            Self::Spanish => Some("es"),
            Self::PortugueseBrazil => Some("pt-BR"),
            Self::German => Some("de"),
            Self::French => Some("fr"),
            Self::Italian => Some("it"),
            Self::ChineseSimplified => Some("zh-Hans"),
            Self::ChineseTraditional => Some("zh-Hant"),
            Self::Japanese => Some("ja"),
            Self::Korean => Some("ko"),
            Self::Hindi => Some("hi"),
            Self::Arabic => Some("ar"),
        }
    }

    /// Locale passed to services that require a concrete speech-recognition
    /// locale rather than a language family. OpenAI-compatible providers and
    /// the local Whisper engine take the primary subtag from this value.
    #[must_use]
    pub const fn locale(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::English => Some("en-US"),
            Self::Russian => Some("ru-RU"),
            Self::Ukrainian => Some("uk-UA"),
            Self::Spanish => Some("es-MX"),
            Self::PortugueseBrazil => Some("pt-BR"),
            Self::German => Some("de-DE"),
            Self::French => Some("fr-FR"),
            Self::Italian => Some("it-IT"),
            Self::ChineseSimplified => Some("zh-CN"),
            Self::ChineseTraditional => Some("zh-TW"),
            Self::Japanese => Some("ja-JP"),
            Self::Korean => Some("ko-KR"),
            Self::Hindi => Some("hi-IN"),
            Self::Arabic => Some("ar-SA"),
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto-detect",
            Self::English => "English",
            Self::Russian => "Russian",
            Self::Ukrainian => "Ukrainian",
            Self::Spanish => "Spanish",
            Self::PortugueseBrazil => "Portuguese (Brazil)",
            Self::German => "German",
            Self::French => "French",
            Self::Italian => "Italian",
            Self::ChineseSimplified => "Chinese (Simplified)",
            Self::ChineseTraditional => "Chinese (Traditional)",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::Hindi => "Hindi",
            Self::Arabic => "Arabic",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        if code.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        Self::ALL.into_iter().find(|language| {
            language.code().is_some_and(|known| {
                known.eq_ignore_ascii_case(code)
                    || known
                        .split('-')
                        .next()
                        .is_some_and(|base| base.eq_ignore_ascii_case(code))
            })
        })
    }
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
    ScreenCaptureRequested {
        request_id: RequestId,
    },
    ScreenCaptureCancelled {
        request_id: RequestId,
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
    HotkeysApplied {
        operation_id: RequestId,
        configuration: HotkeyConfiguration,
        /// Actions Windows could not register. Other actions in the same
        /// request are still applied and the returned configuration is the
        /// exact effective set, never the merely requested set.
        #[serde(default)]
        failures: Vec<HotkeyFailure>,
    },
    HotkeysRejected {
        operation_id: RequestId,
        action: Option<HotkeyAction>,
        reason: HotkeyRejection,
        message: String,
    },
    PauseChanged {
        paused: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    Dictate,
    InsertLast,
    CorrectLayout,
    Translate,
    ScreenTranslate,
    PauseResume,
}

impl HotkeyAction {
    pub const ALL: [Self; 6] = [
        Self::Dictate,
        Self::InsertLast,
        Self::CorrectLayout,
        Self::Translate,
        Self::ScreenTranslate,
        Self::PauseResume,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub windows: bool,
}

impl HotkeyModifiers {
    #[must_use]
    pub const fn control_alt() -> Self {
        Self {
            control: true,
            alt: true,
            shift: false,
            windows: false,
        }
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.control && !self.alt && !self.shift && !self.windows
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeySpec {
    pub modifiers: HotkeyModifiers,
    /// Windows virtual-key code. SayFlex is Windows-only for this release.
    pub virtual_key: u16,
}

impl HotkeySpec {
    #[must_use]
    pub const fn control_alt(virtual_key: u16) -> Self {
        Self {
            modifiers: HotkeyModifiers::control_alt(),
            virtual_key,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub spec: HotkeySpec,
    pub enabled: bool,
}

impl HotkeyBinding {
    #[must_use]
    pub const fn enabled(spec: HotkeySpec) -> Self {
        Self {
            spec,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBindings {
    pub dictate: HotkeyBinding,
    pub insert_last: HotkeyBinding,
    pub correct_layout: HotkeyBinding,
    pub translate: HotkeyBinding,
    #[serde(default = "default_screen_translate_binding")]
    pub screen_translate: HotkeyBinding,
    pub pause_resume: HotkeyBinding,
}

fn default_screen_translate_binding() -> HotkeyBinding {
    HotkeyBinding::enabled(HotkeySpec::control_alt(u16::from(b'G')))
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            dictate: HotkeyBinding::enabled(HotkeySpec::control_alt(0x20)),
            insert_last: HotkeyBinding::enabled(HotkeySpec::control_alt(u16::from(b'V'))),
            correct_layout: HotkeyBinding::enabled(HotkeySpec::control_alt(u16::from(b'K'))),
            translate: HotkeyBinding {
                spec: HotkeySpec::control_alt(u16::from(b'F')),
                enabled: false,
            },
            screen_translate: default_screen_translate_binding(),
            pause_resume: HotkeyBinding::enabled(HotkeySpec::control_alt(u16::from(b'P'))),
        }
    }
}

impl HotkeyBindings {
    #[must_use]
    pub const fn binding(&self, action: HotkeyAction) -> &HotkeyBinding {
        match action {
            HotkeyAction::Dictate => &self.dictate,
            HotkeyAction::InsertLast => &self.insert_last,
            HotkeyAction::CorrectLayout => &self.correct_layout,
            HotkeyAction::Translate => &self.translate,
            HotkeyAction::ScreenTranslate => &self.screen_translate,
            HotkeyAction::PauseResume => &self.pause_resume,
        }
    }

    #[must_use]
    pub fn binding_mut(&mut self, action: HotkeyAction) -> &mut HotkeyBinding {
        match action {
            HotkeyAction::Dictate => &mut self.dictate,
            HotkeyAction::InsertLast => &mut self.insert_last,
            HotkeyAction::CorrectLayout => &mut self.correct_layout,
            HotkeyAction::Translate => &mut self.translate,
            HotkeyAction::ScreenTranslate => &mut self.screen_translate,
            HotkeyAction::PauseResume => &mut self.pause_resume,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HotkeyConfiguration {
    #[serde(default)]
    pub bindings: HotkeyBindings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyRejection {
    Invalid,
    Duplicate,
    Busy,
    RegistrationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyFailure {
    pub action: HotkeyAction,
    pub reason: HotkeyRejection,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InputHostCommand {
    SetHotkeys {
        operation_id: RequestId,
        configuration: HotkeyConfiguration,
    },
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
    SetScreenCaptureMode {
        request_id: RequestId,
        active: bool,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub const MAX_SCREEN_BLOCKS: usize = 512;
pub const MAX_SCREEN_BLOCK_ID_BYTES: usize = 128;
pub const MAX_SCREEN_BLOCK_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_SCREEN_SOURCE_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_SOURCE_CONTEXT_FIELD_BYTES: usize = 16 * 1024;
pub const MAX_SOURCE_CONTEXT_NEARBY_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_GLOSSARY_FIELD_BYTES: usize = 16 * 1024;
pub const MAX_GLOSSARY_EXAMPLES: usize = 32;
pub const MAX_GLOSSARY_LIST_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScreenBlockId(String);

impl ScreenBlockId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ScreenBlockId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ScreenBlockId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GlossaryEntryId(String);

impl GlossaryEntryId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for GlossaryEntryId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for GlossaryEntryId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenTextBlock {
    pub block_id: ScreenBlockId,
    pub source_text: String,
    pub bounds: PhysicalRect,
    #[serde(default)]
    pub fragments: Vec<PhysicalRect>,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockCompletion {
    Completed,
    LimitReached,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationBackendCapabilities {
    #[serde(default)]
    pub structured_blocks: bool,
    #[serde(default)]
    pub context_instructions: bool,
    #[serde(default)]
    pub glossary_guidance: bool,
}

impl TranslationBackendCapabilities {
    #[must_use]
    pub const fn structured() -> Self {
        Self {
            structured_blocks: true,
            context_instructions: true,
            glossary_guidance: true,
        }
    }

    #[must_use]
    pub const fn limited() -> Self {
        Self {
            structured_blocks: false,
            context_instructions: false,
            glossary_guidance: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslatedScreenBlock {
    pub block_id: ScreenBlockId,
    pub translated_text: String,
    pub completion: BlockCompletion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossaryOrigin {
    Corrected,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryEntryDraft {
    pub source_phrase: String,
    pub approved_translation: String,
    pub source_language: TranslationLanguage,
    pub target_language: TranslationLanguage,
    pub example: String,
    pub context: SourceContext,
    pub origin: GlossaryOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub entry_id: GlossaryEntryId,
    pub source_phrase: String,
    pub approved_translation: String,
    pub source_language: TranslationLanguage,
    pub target_language: TranslationLanguage,
    pub examples: Vec<String>,
    pub context: SourceContext,
    pub origin: GlossaryOrigin,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationCommandValidationError {
    TooManyBlocks,
    EmptyBlockId,
    BlockIdTooLong,
    DuplicateBlockId,
    EmptyBlockText,
    BlockTextTooLong,
    TotalBlockTextTooLong,
    InvalidBlockBounds,
    InvalidConfidence,
    ContextFieldTooLong,
    EmptyGlossaryEntryId,
    GlossaryEntryIdTooLong,
    EmptyGlossaryField,
    GlossaryFieldTooLong,
    TooManyGlossaryExamples,
    GlossaryListLimitTooLarge,
}

impl fmt::Display for TranslationCommandValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooManyBlocks => "too many screen text blocks",
            Self::EmptyBlockId => "screen block id is empty",
            Self::BlockIdTooLong => "screen block id is too long",
            Self::DuplicateBlockId => "screen block id is duplicated",
            Self::EmptyBlockText => "screen block text is empty",
            Self::BlockTextTooLong => "screen block text is too long",
            Self::TotalBlockTextTooLong => "screen block text total is too long",
            Self::InvalidBlockBounds => "screen block bounds are empty",
            Self::InvalidConfidence => "screen block confidence is outside 0..=1000",
            Self::ContextFieldTooLong => "source context field is too long",
            Self::EmptyGlossaryEntryId => "glossary entry id is empty",
            Self::GlossaryEntryIdTooLong => "glossary entry id is too long",
            Self::EmptyGlossaryField => "required glossary field is empty",
            Self::GlossaryFieldTooLong => "glossary field is too long",
            Self::TooManyGlossaryExamples => "too many glossary examples",
            Self::GlossaryListLimitTooLarge => "glossary list limit is too large",
        })
    }
}

impl std::error::Error for TranslationCommandValidationError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrLanguagePolicy {
    Auto,
    Prefer(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrPixelFormat {
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub bounds: PhysicalRect,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFailureKind {
    ModelUnavailable,
    LanguageUnavailable,
    NoText,
    InvalidImage,
    Engine,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum OcrWorkerCommand {
    Recognize {
        request_id: RequestId,
        width: u32,
        height: u32,
        stride: u32,
        pixel_format: OcrPixelFormat,
        language_policy: OcrLanguagePolicy,
        rgba_base64: String,
    },
    Cancel {
        request_id: RequestId,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum OcrWorkerEvent {
    Ready {
        protocol_version: u32,
        engine: String,
    },
    Recognized {
        request_id: RequestId,
        lines: Vec<OcrLine>,
        language: Option<String>,
        processing_duration_ms: u64,
    },
    Failed {
        request_id: RequestId,
        kind: OcrFailureKind,
        message: String,
    },
    Cancelled {
        request_id: RequestId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranslationWorkerCommand {
    Initialize {
        /// Stable catalog identity. Older settings may omit it and provide only
        /// `selected_tier`, which remains a migration hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_catalog_id: Option<String>,
        selected_tier: Option<TranslationTier>,
        keep_warm: bool,
        history_enabled: bool,
        /// Whether the local translation route is enabled and may occupy
        /// memory. Cloud-only operation still retains the selected tier.
        prepare_local_model: bool,
    },
    EnsureModel {
        catalog_id: String,
    },
    /// Removes one catalog-owned local pack after its engine is no longer in
    /// use. The worker, not the desktop, owns the filesystem operation.
    RemoveModel {
        catalog_id: String,
    },
    /// Stops a pack or runtime installation in flight, keeping partial files.
    CancelModelInstall,
    SelectModel {
        catalog_id: String,
        keep_warm: bool,
    },
    Translate {
        request_id: RequestId,
        text: String,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
        language_policy: TranslationLanguagePolicy,
    },
    TranslateBlocks {
        request_id: RequestId,
        base_revision: u64,
        blocks: Vec<ScreenTextBlock>,
        context: SourceContext,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
        language_policy: TranslationLanguagePolicy,
    },
    SaveGlossaryEntry {
        operation_id: RequestId,
        request_id: RequestId,
        block_id: ScreenBlockId,
        expected_revision: u64,
        entry: GlossaryEntryDraft,
    },
    ListGlossary {
        operation_id: RequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        limit: usize,
    },
    UpdateGlossaryEntry {
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
        entry: GlossaryEntryDraft,
    },
    DeleteGlossaryEntry {
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
    },
    Cancel {
        request_id: RequestId,
    },
    /// Releases the resident local engine without deleting its selected model.
    /// The desktop may request the same model again on the next translation.
    UnloadLocalModel,
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

impl TranslationWorkerCommand {
    pub fn validate_limits(&self) -> Result<(), TranslationCommandValidationError> {
        match self {
            Self::TranslateBlocks {
                blocks, context, ..
            } => validate_screen_blocks(blocks, context),
            Self::SaveGlossaryEntry {
                block_id, entry, ..
            } => {
                validate_block_id(block_id)?;
                validate_glossary_draft(entry)
            }
            Self::ListGlossary { query, limit, .. } => {
                if *limit > MAX_GLOSSARY_LIST_LIMIT {
                    return Err(TranslationCommandValidationError::GlossaryListLimitTooLarge);
                }
                if query
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_GLOSSARY_FIELD_BYTES)
                {
                    return Err(TranslationCommandValidationError::GlossaryFieldTooLong);
                }
                Ok(())
            }
            Self::UpdateGlossaryEntry {
                entry_id, entry, ..
            } => {
                validate_glossary_entry_id(entry_id)?;
                validate_glossary_draft(entry)
            }
            Self::DeleteGlossaryEntry { entry_id, .. } => validate_glossary_entry_id(entry_id),
            _ => Ok(()),
        }
    }
}

fn validate_screen_blocks(
    blocks: &[ScreenTextBlock],
    context: &SourceContext,
) -> Result<(), TranslationCommandValidationError> {
    if blocks.len() > MAX_SCREEN_BLOCKS {
        return Err(TranslationCommandValidationError::TooManyBlocks);
    }
    let mut ids = std::collections::HashSet::with_capacity(blocks.len());
    let mut total_text_bytes = 0_usize;
    for block in blocks {
        validate_block_id(&block.block_id)?;
        if !ids.insert(block.block_id.as_str()) {
            return Err(TranslationCommandValidationError::DuplicateBlockId);
        }
        if block.source_text.trim().is_empty() {
            return Err(TranslationCommandValidationError::EmptyBlockText);
        }
        if block.source_text.len() > MAX_SCREEN_BLOCK_TEXT_BYTES {
            return Err(TranslationCommandValidationError::BlockTextTooLong);
        }
        total_text_bytes = total_text_bytes.saturating_add(block.source_text.len());
        if total_text_bytes > MAX_SCREEN_SOURCE_TEXT_BYTES {
            return Err(TranslationCommandValidationError::TotalBlockTextTooLong);
        }
        if block.bounds.width == 0 || block.bounds.height == 0 {
            return Err(TranslationCommandValidationError::InvalidBlockBounds);
        }
        if block.fragments.iter().any(|fragment| {
            fragment.width == 0
                || fragment.height == 0
                || !physical_rect_contains(block.bounds, *fragment)
        }) {
            return Err(TranslationCommandValidationError::InvalidBlockBounds);
        }
        if block.confidence_milli > 1_000 {
            return Err(TranslationCommandValidationError::InvalidConfidence);
        }
    }
    validate_source_context(context)
}

fn physical_rect_contains(outer: PhysicalRect, inner: PhysicalRect) -> bool {
    let outer_left = i64::from(outer.x);
    let outer_top = i64::from(outer.y);
    let outer_right = outer_left.saturating_add(i64::from(outer.width));
    let outer_bottom = outer_top.saturating_add(i64::from(outer.height));
    let inner_left = i64::from(inner.x);
    let inner_top = i64::from(inner.y);
    let inner_right = inner_left.saturating_add(i64::from(inner.width));
    let inner_bottom = inner_top.saturating_add(i64::from(inner.height));
    inner_left >= outer_left
        && inner_top >= outer_top
        && inner_right <= outer_right
        && inner_bottom <= outer_bottom
}

fn validate_block_id(block_id: &ScreenBlockId) -> Result<(), TranslationCommandValidationError> {
    if block_id.as_str().trim().is_empty() {
        return Err(TranslationCommandValidationError::EmptyBlockId);
    }
    if block_id.as_str().len() > MAX_SCREEN_BLOCK_ID_BYTES {
        return Err(TranslationCommandValidationError::BlockIdTooLong);
    }
    Ok(())
}

fn validate_glossary_entry_id(
    entry_id: &GlossaryEntryId,
) -> Result<(), TranslationCommandValidationError> {
    if entry_id.as_str().trim().is_empty() {
        return Err(TranslationCommandValidationError::EmptyGlossaryEntryId);
    }
    if entry_id.as_str().len() > MAX_SCREEN_BLOCK_ID_BYTES {
        return Err(TranslationCommandValidationError::GlossaryEntryIdTooLong);
    }
    Ok(())
}

fn validate_glossary_draft(
    entry: &GlossaryEntryDraft,
) -> Result<(), TranslationCommandValidationError> {
    for value in [
        entry.source_phrase.as_str(),
        entry.approved_translation.as_str(),
        entry.example.as_str(),
    ] {
        if value.trim().is_empty() {
            return Err(TranslationCommandValidationError::EmptyGlossaryField);
        }
        if value.len() > MAX_GLOSSARY_FIELD_BYTES {
            return Err(TranslationCommandValidationError::GlossaryFieldTooLong);
        }
    }
    validate_source_context(&entry.context)
}

fn validate_source_context(
    context: &SourceContext,
) -> Result<(), TranslationCommandValidationError> {
    let ordinary = [
        &context.application_id,
        &context.executable_name,
        &context.window_title,
        &context.site_url,
        &context.document_name,
        &context.domain,
    ];
    if ordinary
        .into_iter()
        .flatten()
        .any(|value| value.len() > MAX_SOURCE_CONTEXT_FIELD_BYTES)
        || context
            .nearby_text
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SOURCE_CONTEXT_NEARBY_TEXT_BYTES)
    {
        return Err(TranslationCommandValidationError::ContextFieldTooLong);
    }
    Ok(())
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
        selected_catalog_id: Option<String>,
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
        catalog_id: String,
        tier: TranslationTier,
    },
    /// What is resident in the local translation engine after the worker has
    /// completed a load or unload operation.
    LocalModelState {
        catalog_id: Option<String>,
        loaded: bool,
    },
    ModelRemoved {
        catalog_id: String,
        tier: TranslationTier,
        /// Logical bytes of files removed from SayFlex-owned paths. This is not
        /// a claim about physical allocation reclaimed by the filesystem.
        removed_bytes: u64,
        runtime_removed: bool,
        models: Vec<TranslationModelSummary>,
    },
    Partial {
        request_id: RequestId,
        translated_text: String,
        first_token_duration_ms: u64,
        tokenize_duration_ms: u64,
        chunk_index: u32,
        chunk_count: u32,
    },
    ScreenBlockPartial {
        request_id: RequestId,
        base_revision: u64,
        block_id: ScreenBlockId,
        translated_text: String,
    },
    ScreenBlocksCompleted {
        request_id: RequestId,
        base_revision: u64,
        blocks: Vec<TranslatedScreenBlock>,
        source_language: TranslationLanguage,
        target_language: TranslationLanguage,
        #[serde(default)]
        capabilities: TranslationBackendCapabilities,
        processing_duration_ms: u64,
    },
    GlossaryList {
        operation_id: RequestId,
        entries: Vec<GlossaryEntry>,
    },
    GlossarySaved {
        operation_id: RequestId,
        entry: GlossaryEntry,
    },
    GlossaryDeleted {
        operation_id: RequestId,
        entry_id: GlossaryEntryId,
    },
    GlossaryFailed {
        operation_id: RequestId,
        code: String,
        message: String,
        retryable: bool,
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
    /// The answer to `ClearHistory`. `error` is set when the history file
    /// could not be removed; the entries that remain are sent as `History`.
    HistoryCleared {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
    RequestFailed {
        request_id: RequestId,
        failure: TranslationFailureReport,
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
        dictation_language: DictationLanguage,
        history_enabled: bool,
        fast_release_previews: bool,
        /// Whether the local Whisper route is enabled and should occupy
        /// memory. A cloud-only chain starts without loading it.
        prepare_local_model: bool,
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
    /// Releases the resident local engine without deleting its selected model.
    /// The worker prepares it again when the next local dictation begins.
    UnloadLocalModel,
    EnsureModel {
        catalog_id: String,
    },
    SelectModel {
        catalog_id: String,
    },
    /// Removes one catalog-owned local model after its inference engine is no
    /// longer using the file.
    RemoveModel {
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
    SetDictationLanguage {
        language: DictationLanguage,
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
    /// What is resident in the local speech engine after the inference actor
    /// has completed a load or unload operation.
    LocalModelState {
        catalog_id: Option<String>,
        loaded: bool,
    },
    ModelRemoved {
        catalog_id: String,
        /// Logical bytes of files removed from SayFlex-owned paths. This is not
        /// a claim about physical allocation reclaimed by the filesystem.
        removed_bytes: u64,
        models: Vec<ModelSummary>,
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
    /// The answer to `ClearHistory`. `error` is set when the history file
    /// could not be removed; the entries that remain are sent as `History`.
    HistoryCleared {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
        BlockCompletion, CompanionRendererCommand, DictationCommand, DictationLanguage,
        GlossaryOrigin, HotkeyAction, HotkeyBindings, HotkeyConfiguration, HotkeySpec,
        InputHostCommand, InputHostEvent, InsertionDelivery, InsertionMethod, OcrLanguagePolicy,
        OcrPixelFormat, OcrWorkerCommand, OcrWorkerEvent, PhysicalRect, RequestId, ScreenBlockId,
        ScreenTextBlock, SelectionCaptureTiming, SessionId, SourceContext, SpeechWorkerCommand,
        SpeechWorkerEvent, TargetSnapshot, TextActionIntent, Transcript, TranscriptionLatency,
        TranslationAttemptReport, TranslationCommandValidationError, TranslationFailureKind,
        TranslationFailureReport, TranslationLanguage, TranslationLanguagePolicy,
        TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
    };

    fn round_trip<T>(value: T)
    where
        T: std::fmt::Debug + PartialEq + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value);
    }

    #[test]
    fn screen_translation_protocol_round_trips_with_default_hotkey() {
        let request_id = RequestId::new();

        assert_eq!(
            HotkeyBindings::default().screen_translate.spec,
            HotkeySpec::control_alt(u16::from(b'G'))
        );
        assert!(HotkeyBindings::default().screen_translate.enabled);
        assert_eq!(HotkeyAction::ALL.len(), 6);
        round_trip(InputHostEvent::ScreenCaptureRequested { request_id });
        round_trip(InputHostCommand::SetScreenCaptureMode {
            request_id,
            active: true,
        });
        round_trip(OcrWorkerCommand::Recognize {
            request_id,
            width: 2,
            height: 1,
            stride: 8,
            pixel_format: OcrPixelFormat::Rgba8,
            language_policy: OcrLanguagePolicy::Auto,
            rgba_base64: "AAAAAAAAAAA=".into(),
        });
        round_trip(OcrWorkerEvent::Cancelled { request_id });
    }

    #[test]
    fn resident_local_models_can_be_released_without_removing_their_files() {
        round_trip(SpeechWorkerCommand::UnloadLocalModel);
        round_trip(TranslationWorkerCommand::UnloadLocalModel);
    }

    #[test]
    fn compact_translation_tier_has_a_stable_wire_identity() {
        assert_eq!(
            serde_json::to_string(&TranslationTier::Compact).unwrap(),
            "\"compact\""
        );
        round_trip(TranslationTier::Compact);
    }

    #[test]
    fn legacy_hotkeys_enable_the_admitted_screen_translation_chord() {
        let mut legacy = serde_json::to_value(HotkeyBindings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("screen_translate");

        let migrated: HotkeyBindings = serde_json::from_value(legacy).unwrap();

        assert_eq!(
            migrated.screen_translate.spec,
            HotkeySpec::control_alt(u16::from(b'G'))
        );
        assert!(migrated.screen_translate.enabled);
    }

    #[test]
    fn command_json_preserves_session_identity() {
        let session_id = SessionId::new();
        let command = DictationCommand::End { session_id };

        let json = serde_json::to_string(&command).unwrap();
        let decoded: DictationCommand = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, command);
    }

    #[test]
    fn hotkey_configuration_round_trips_with_action_identity() {
        let operation_id = RequestId::new();
        let configuration = HotkeyConfiguration::default();
        let command = InputHostCommand::SetHotkeys {
            operation_id,
            configuration: configuration.clone(),
        };

        let json = serde_json::to_string(&command).unwrap();
        let decoded: InputHostCommand = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, command);
        assert_eq!(
            configuration
                .bindings
                .binding(HotkeyAction::Translate)
                .spec
                .virtual_key,
            u16::from(b'F')
        );
    }

    #[test]
    fn spoken_language_keeps_ui_codes_separate_from_service_locales() {
        assert_eq!(
            DictationLanguage::from_code("ru"),
            Some(DictationLanguage::Russian)
        );
        assert_eq!(DictationLanguage::Russian.code(), Some("ru"));
        assert_eq!(DictationLanguage::Russian.locale(), Some("ru-RU"));
        assert_eq!(
            DictationLanguage::from_code("zh-Hant"),
            Some(DictationLanguage::ChineseTraditional)
        );
        assert_eq!(DictationLanguage::Auto.locale(), None);
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
                attempts: Vec::new(),
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

    #[test]
    fn translation_attempt_reports_round_trip_as_typed_redacted_data() {
        let request_id = RequestId::new();
        let quota = TranslationAttemptReport {
            provider_id: "deepl".to_owned(),
            kind: TranslationFailureKind::QuotaExhausted,
            retry_after_ms: Some(3_600_000),
            detail: "allowance spent; key <redacted>".to_owned(),
        };
        let completed = TranslationWorkerEvent::Completed {
            result: TranslationResult {
                request_id,
                source_text: "hello".to_owned(),
                translated_text: "привет".to_owned(),
                source_language: TranslationLanguage::English,
                target_language: TranslationLanguage::Russian,
                tier: Some(TranslationTier::Tiny),
                provider: None,
                attempts: vec![quota.clone()],
                processing_duration_ms: 12,
                first_token_duration_ms: 4,
                backend_request_count: 1,
                tokenize_duration_ms: 0,
                chunk_count: 1,
            },
            created_unix_ms: 123,
        };
        let failed = TranslationWorkerEvent::RequestFailed {
            request_id,
            failure: TranslationFailureReport {
                kind: TranslationFailureKind::UnsupportedRequest,
                provider_id: Some("deepl".to_owned()),
                retry_after_ms: None,
                required_catalog_id: Some("firefox-de-en-v3".to_owned()),
                detail: "language pair is unavailable".to_owned(),
                attempts: vec![quota],
            },
        };

        for event in [completed, failed] {
            let json = serde_json::to_string(&event).unwrap();
            assert!(!json.contains("279a2e9d-83b3-c416-7e2d-f721593e42a0:fx"));
            assert_eq!(
                serde_json::from_str::<TranslationWorkerEvent>(&json).unwrap(),
                event
            );
        }
    }

    #[test]
    fn contextual_screen_command_has_bounded_stable_json() {
        let request_id = RequestId::new();
        let command = TranslationWorkerCommand::TranslateBlocks {
            request_id,
            base_revision: 7,
            blocks: vec![ScreenTextBlock {
                block_id: ScreenBlockId::from("block-1"),
                source_text: "Cooldown 2s".to_owned(),
                bounds: PhysicalRect {
                    x: -120,
                    y: 48,
                    width: 320,
                    height: 42,
                },
                fragments: vec![PhysicalRect {
                    x: -120,
                    y: 48,
                    width: 320,
                    height: 42,
                }],
                confidence_milli: 987,
            }],
            context: SourceContext {
                executable_name: Some("game.exe".to_owned()),
                domain: Some("dota-2".to_owned()),
                partial: true,
                ..SourceContext::default()
            },
            source_language: Some(TranslationLanguage::English),
            target_language: Some(TranslationLanguage::Russian),
            language_policy: TranslationLanguagePolicy::default(),
        };

        command.validate_limits().unwrap();
        let value = serde_json::to_value(&command).unwrap();
        assert_eq!(value["type"], "translate_blocks");
        assert_eq!(value["payload"]["blocks"][0]["block_id"], "block-1");
        assert_eq!(value["payload"]["blocks"][0]["bounds"]["x"], -120);
        assert_eq!(value["payload"]["context"]["partial"], true);
        assert!(value["payload"]["context"].get("site_url").is_none());
        assert!(value["payload"]["context"].get("nearby_text").is_none());
        assert_eq!(
            serde_json::from_value::<TranslationWorkerCommand>(value).unwrap(),
            command
        );
    }

    #[test]
    fn screen_block_fragments_round_trip_and_legacy_payloads_default_empty() {
        let value = serde_json::json!({
            "block_id": "screen-block-0000",
            "source_text": "DAMAGE TYPE: Magical",
            "bounds": { "x": 30, "y": 180, "width": 156, "height": 21 },
            "fragments": [
                { "x": 30, "y": 180, "width": 112, "height": 21 },
                { "x": 135, "y": 185, "width": 51, "height": 14 }
            ],
            "confidence_milli": 985
        });
        let block: ScreenTextBlock = serde_json::from_value(value.clone()).unwrap();

        assert_eq!(block.fragments.len(), 2);
        assert_eq!(block.fragments[1].x, 135);
        assert_eq!(serde_json::to_value(&block).unwrap(), value);

        let legacy: ScreenTextBlock = serde_json::from_value(serde_json::json!({
            "block_id": "screen-block-0000",
            "source_text": "legacy",
            "bounds": { "x": 4, "y": 5, "width": 6, "height": 7 },
            "confidence_milli": 900
        }))
        .unwrap();
        assert!(legacy.fragments.is_empty());
    }

    #[test]
    fn screen_block_validation_rejects_fragments_outside_the_row_bounds() {
        let command = TranslationWorkerCommand::TranslateBlocks {
            request_id: RequestId::new(),
            base_revision: 0,
            blocks: vec![ScreenTextBlock {
                block_id: ScreenBlockId::from("screen-block-0000"),
                source_text: "Damage".to_owned(),
                bounds: PhysicalRect {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 20,
                },
                fragments: vec![PhysicalRect {
                    x: 200,
                    y: 20,
                    width: 10,
                    height: 10,
                }],
                confidence_milli: 900,
            }],
            context: SourceContext::default(),
            source_language: None,
            target_language: Some(TranslationLanguage::Russian),
            language_policy: TranslationLanguagePolicy::default(),
        };

        assert_eq!(
            command.validate_limits(),
            Err(TranslationCommandValidationError::InvalidBlockBounds)
        );
    }

    #[test]
    fn contextual_screen_validation_rejects_empty_and_duplicate_ids() {
        let block = |id: &str| ScreenTextBlock {
            block_id: ScreenBlockId::from(id),
            source_text: "text".to_owned(),
            bounds: PhysicalRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            fragments: Vec::new(),
            confidence_milli: 900,
        };
        let command = |blocks| TranslationWorkerCommand::TranslateBlocks {
            request_id: RequestId::new(),
            base_revision: 0,
            blocks,
            context: SourceContext::default(),
            source_language: None,
            target_language: Some(TranslationLanguage::Russian),
            language_policy: TranslationLanguagePolicy::default(),
        };

        assert_eq!(
            command(vec![block("  ")]).validate_limits(),
            Err(TranslationCommandValidationError::EmptyBlockId)
        );
        assert_eq!(
            command(vec![block("same"), block("same")]).validate_limits(),
            Err(TranslationCommandValidationError::DuplicateBlockId)
        );
    }

    #[test]
    fn block_completion_and_glossary_origin_have_exact_wire_names() {
        for (completion, expected) in [
            (BlockCompletion::Completed, "completed"),
            (BlockCompletion::LimitReached, "limit_reached"),
            (BlockCompletion::Cancelled, "cancelled"),
            (BlockCompletion::Failed, "failed"),
        ] {
            assert_eq!(
                serde_json::to_string(&completion).unwrap(),
                format!("\"{expected}\"")
            );
        }
        assert_eq!(
            serde_json::to_string(&GlossaryOrigin::Corrected).unwrap(),
            "\"corrected\""
        );
        assert_eq!(
            serde_json::to_string(&GlossaryOrigin::Confirmed).unwrap(),
            "\"confirmed\""
        );
    }

    #[test]
    fn protocol_extension_still_decodes_the_previous_translate_fixture() {
        let fixture = r#"{
            "type":"translate",
            "payload":{
                "request_id":"00000000-0000-0000-0000-000000000001",
                "text":"hello",
                "source_language":"english",
                "target_language":"russian",
                "language_policy":{"primary":"english","secondary":"russian"}
            }
        }"#;

        assert!(matches!(
            serde_json::from_str::<TranslationWorkerCommand>(fixture).unwrap(),
            TranslationWorkerCommand::Translate { text, .. } if text == "hello"
        ));
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
        TranslationWorkerEvent::LocalModelState { .. } => "local_model_state",
        TranslationWorkerEvent::ModelRemoved { .. } => "model_removed",
        TranslationWorkerEvent::Partial { .. } => "partial",
        TranslationWorkerEvent::ScreenBlockPartial { .. } => "screen_block_partial",
        TranslationWorkerEvent::ScreenBlocksCompleted { .. } => "screen_blocks_completed",
        TranslationWorkerEvent::GlossaryList { .. } => "glossary_list",
        TranslationWorkerEvent::GlossarySaved { .. } => "glossary_saved",
        TranslationWorkerEvent::GlossaryDeleted { .. } => "glossary_deleted",
        TranslationWorkerEvent::GlossaryFailed { .. } => "glossary_failed",
        TranslationWorkerEvent::Completed { .. } => "completed",
        TranslationWorkerEvent::Cancelled { .. } => "cancelled",
        TranslationWorkerEvent::History { .. } => "history",
        TranslationWorkerEvent::HistoryCleared { .. } => "history_cleared",
        TranslationWorkerEvent::TranslationProviders { .. } => "translation_providers",
        TranslationWorkerEvent::CredentialSaved { .. } => "credential_saved",
        TranslationWorkerEvent::RequestFailed { .. } => "request_failed",
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
        SpeechWorkerEvent::LocalModelState { .. } => "local_model_state",
        SpeechWorkerEvent::ModelRemoved { .. } => "model_removed",
        SpeechWorkerEvent::Listening { .. } => "listening",
        SpeechWorkerEvent::AudioLevel { .. } => "audio_level",
        SpeechWorkerEvent::Processing { .. } => "processing",
        SpeechWorkerEvent::Completed { .. } => "completed",
        SpeechWorkerEvent::Cancelled { .. } => "cancelled",
        SpeechWorkerEvent::History { .. } => "history",
        SpeechWorkerEvent::HistoryCleared { .. } => "history_cleared",
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
            attempts: vec![TranslationAttemptReport {
                provider_id: String::new(),
                kind: TranslationFailureKind::QuotaExhausted,
                retry_after_ms: Some(0),
                detail: "detail".to_owned(),
            }],
            processing_duration_ms: 0,
            first_token_duration_ms: 0,
            backend_request_count: 0,
            tokenize_duration_ms: 0,
            chunk_count: 0,
        }
    }

    fn source_context() -> SourceContext {
        SourceContext {
            application_id: Some(String::new()),
            executable_name: Some(String::new()),
            window_title: Some(String::new()),
            site_url: Some(String::new()),
            document_name: Some(String::new()),
            nearby_text: Some(String::new()),
            domain: Some(String::new()),
            partial: false,
        }
    }

    fn glossary_entry() -> GlossaryEntry {
        GlossaryEntry {
            entry_id: GlossaryEntryId::from("entry"),
            source_phrase: String::new(),
            approved_translation: String::new(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            examples: vec![String::new()],
            context: source_context(),
            origin: GlossaryOrigin::Confirmed,
            created_unix_ms: 0,
            updated_unix_ms: 0,
        }
    }

    #[test]
    fn the_wire_shape_belongs_to_this_version() {
        assert_eq!(
            PROTOCOL_VERSION, 33,
            "every fingerprint below was written for protocol 33: if a shape \
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
                "attempts[].detail:string",
                "attempts[].kind:string",
                "attempts[].provider_id:string",
                "attempts[].retry_after_ms:number",
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
                selected_catalog_id: Some(String::new()),
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
                catalog_id: String::new(),
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
            TranslationWorkerEvent::ScreenBlockPartial {
                request_id,
                base_revision: 0,
                block_id: ScreenBlockId::from("block"),
                translated_text: String::new(),
            },
            TranslationWorkerEvent::ScreenBlocksCompleted {
                request_id,
                base_revision: 0,
                blocks: vec![TranslatedScreenBlock {
                    block_id: ScreenBlockId::from("block"),
                    translated_text: String::new(),
                    completion: BlockCompletion::Completed,
                    failure_message: Some(String::new()),
                }],
                source_language: TranslationLanguage::English,
                target_language: TranslationLanguage::Russian,
                capabilities: TranslationBackendCapabilities::structured(),
                processing_duration_ms: 0,
            },
            TranslationWorkerEvent::GlossaryList {
                operation_id: request_id,
                entries: vec![glossary_entry()],
            },
            TranslationWorkerEvent::GlossarySaved {
                operation_id: request_id,
                entry: glossary_entry(),
            },
            TranslationWorkerEvent::GlossaryDeleted {
                operation_id: request_id,
                entry_id: GlossaryEntryId::from("entry"),
            },
            TranslationWorkerEvent::GlossaryFailed {
                operation_id: request_id,
                code: String::new(),
                message: String::new(),
                retryable: false,
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
            TranslationWorkerEvent::RequestFailed {
                request_id,
                failure: TranslationFailureReport {
                    kind: TranslationFailureKind::UnsupportedRequest,
                    provider_id: Some(String::new()),
                    retry_after_ms: Some(0),
                    required_catalog_id: Some(String::new()),
                    detail: "detail".to_owned(),
                    attempts: vec![TranslationAttemptReport {
                        provider_id: String::new(),
                        kind: TranslationFailureKind::QuotaExhausted,
                        retry_after_ms: Some(0),
                        detail: "detail".to_owned(),
                    }],
                },
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
                "completed(created_unix_ms:number,result.attempts[].detail:string,\
 result.attempts[].kind:string,result.attempts[].provider_id:string,\
 result.attempts[].retry_after_ms:number,result.backend_request_count:number,\
 result.chunk_count:number,result.first_token_duration_ms:number,\
result.processing_duration_ms:number,result.provider:string,result.request_id:string,\
result.source_language:string,result.source_text:string,result.target_language:string,\
result.tier:string,result.tokenize_duration_ms:number,result.translated_text:string)",
                "credential_saved(error:string,operation_id:string,provider_id:string)",
                "failed(catalog_id:string,code:string,message:string,request_id:string,\
retryable:bool)",
                "glossary_deleted(entry_id:string,operation_id:string)",
                "glossary_failed(code:string,message:string,operation_id:string,retryable:bool)",
                "glossary_list(entries[].approved_translation:string,\
entries[].context.application_id:string,entries[].context.document_name:string,\
entries[].context.domain:string,entries[].context.executable_name:string,\
entries[].context.nearby_text:string,entries[].context.partial:bool,\
entries[].context.site_url:string,entries[].context.window_title:string,\
entries[].created_unix_ms:number,entries[].entry_id:string,entries[].examples[]:string,\
entries[].origin:string,entries[].source_language:string,entries[].source_phrase:string,\
entries[].target_language:string,entries[].updated_unix_ms:number,operation_id:string)",
                "glossary_saved(entry.approved_translation:string,\
entry.context.application_id:string,entry.context.document_name:string,\
entry.context.domain:string,entry.context.executable_name:string,\
entry.context.nearby_text:string,entry.context.partial:bool,entry.context.site_url:string,\
entry.context.window_title:string,entry.created_unix_ms:number,entry.entry_id:string,\
entry.examples[]:string,entry.origin:string,entry.source_language:string,\
entry.source_phrase:string,entry.target_language:string,entry.updated_unix_ms:number,\
operation_id:string)",
                "history(entries[])",
                "model_download_progress(catalog_id:string,downloaded_bytes:number,\
total_bytes:number)",
                "model_installed(catalog_id:string,path:string,tier:string)",
                "model_ready(catalog_id:string,load_duration_ms:number,tier:string)",
                "model_selected(catalog_id:string,tier:string)",
                "partial(chunk_count:number,chunk_index:number,first_token_duration_ms:number,\
request_id:string,tokenize_duration_ms:number,translated_text:string)",
                "ready(models[],protocol_version:number,selected_catalog_id:string,selected_tier:string)",
                "request_failed(failure.attempts[].detail:string,failure.attempts[].kind:string,\
 failure.attempts[].provider_id:string,failure.attempts[].retry_after_ms:number,\
 failure.detail:string,failure.kind:string,failure.provider_id:string,\
 failure.required_catalog_id:string,failure.retry_after_ms:number,request_id:string)",
                "screen_block_partial(base_revision:number,block_id:string,request_id:string,\
translated_text:string)",
                "screen_blocks_completed(base_revision:number,blocks[].block_id:string,\
 blocks[].completion:string,blocks[].failure_message:string,blocks[].translated_text:string,\
 capabilities.context_instructions:bool,capabilities.glossary_guidance:bool,\
 capabilities.structured_blocks:bool,processing_duration_ms:number,request_id:string,\
 source_language:string,target_language:string)",
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
            attempts: vec![TranslationAttemptReport {
                provider_id: "fallback".to_owned(),
                kind: TranslationFailureKind::RateLimited,
                retry_after_ms: Some(1_000),
                detail: "detail".to_owned(),
            }],
            processing_duration_ms: 1,
            first_token_duration_ms: 2,
            backend_request_count: 3,
            tokenize_duration_ms: 4,
            chunk_count: 5,
        }
    }

    fn source_context() -> SourceContext {
        SourceContext {
            application_id: Some("application".to_owned()),
            executable_name: Some("application.exe".to_owned()),
            window_title: Some("window".to_owned()),
            site_url: Some("https://example.invalid".to_owned()),
            document_name: Some("document".to_owned()),
            nearby_text: Some("nearby".to_owned()),
            domain: Some("domain".to_owned()),
            partial: true,
        }
    }

    fn screen_block() -> ScreenTextBlock {
        ScreenTextBlock {
            block_id: ScreenBlockId::from("block"),
            source_text: "source".to_owned(),
            bounds: PhysicalRect {
                x: -1,
                y: 2,
                width: 3,
                height: 4,
            },
            fragments: vec![PhysicalRect {
                x: -1,
                y: 2,
                width: 3,
                height: 4,
            }],
            confidence_milli: 900,
        }
    }

    fn glossary_draft() -> GlossaryEntryDraft {
        GlossaryEntryDraft {
            source_phrase: "source".to_owned(),
            approved_translation: "translated".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            example: "example".to_owned(),
            context: source_context(),
            origin: GlossaryOrigin::Corrected,
        }
    }

    fn glossary_entry() -> GlossaryEntry {
        GlossaryEntry {
            entry_id: GlossaryEntryId::from("entry"),
            source_phrase: "source".to_owned(),
            approved_translation: "translated".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            examples: vec!["example".to_owned()],
            context: source_context(),
            origin: GlossaryOrigin::Corrected,
            created_unix_ms: 1,
            updated_unix_ms: 2,
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
            InputHostEvent::ScreenCaptureRequested { request_id },
            InputHostEvent::ScreenCaptureCancelled { request_id },
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
            InputHostEvent::HotkeysApplied {
                operation_id: request_id,
                configuration: HotkeyConfiguration::default(),
                failures: vec![HotkeyFailure {
                    action: HotkeyAction::Translate,
                    reason: HotkeyRejection::RegistrationFailed,
                    message: "message".to_owned(),
                }],
            },
            InputHostEvent::HotkeysRejected {
                operation_id: request_id,
                action: Some(HotkeyAction::Translate),
                reason: HotkeyRejection::RegistrationFailed,
                message: "message".to_owned(),
            },
            InputHostEvent::PauseChanged { paused: true },
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
            InputHostCommand::SetHotkeys {
                operation_id: request_id,
                configuration: HotkeyConfiguration::default(),
            },
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
            InputHostCommand::SetScreenCaptureMode {
                request_id,
                active: true,
            },
            InputHostCommand::Shutdown,
        ]
    }

    fn ocr_commands() -> Vec<OcrWorkerCommand> {
        let request_id = RequestId::new();
        vec![
            OcrWorkerCommand::Recognize {
                request_id,
                width: 2,
                height: 1,
                stride: 8,
                pixel_format: OcrPixelFormat::Rgba8,
                language_policy: OcrLanguagePolicy::Prefer(vec!["en".to_owned()]),
                rgba_base64: "AAAAAAAAAAA=".to_owned(),
            },
            OcrWorkerCommand::Cancel { request_id },
            OcrWorkerCommand::Shutdown,
        ]
    }

    fn ocr_events() -> Vec<OcrWorkerEvent> {
        let request_id = RequestId::new();
        vec![
            OcrWorkerEvent::Ready {
                protocol_version: 1,
                engine: "engine".to_owned(),
            },
            OcrWorkerEvent::Recognized {
                request_id,
                lines: vec![OcrLine {
                    text: "text".to_owned(),
                    bounds: PhysicalRect {
                        x: 1,
                        y: 2,
                        width: 3,
                        height: 4,
                    },
                    confidence_milli: 900,
                }],
                language: Some("en".to_owned()),
                processing_duration_ms: 5,
            },
            OcrWorkerEvent::Failed {
                request_id,
                kind: OcrFailureKind::Engine,
                message: "message".to_owned(),
            },
            OcrWorkerEvent::Cancelled { request_id },
        ]
    }

    fn translation_commands() -> Vec<TranslationWorkerCommand> {
        let request_id = RequestId::new();
        vec![
            TranslationWorkerCommand::Initialize {
                selected_catalog_id: Some("qwen35-4b-q4".to_owned()),
                selected_tier: Some(TranslationTier::Tiny),
                keep_warm: true,
                history_enabled: true,
                prepare_local_model: true,
            },
            TranslationWorkerCommand::EnsureModel {
                catalog_id: "qwen35-4b-q4".to_owned(),
            },
            TranslationWorkerCommand::RemoveModel {
                catalog_id: "qwen35-4b-q4".to_owned(),
            },
            TranslationWorkerCommand::CancelModelInstall,
            TranslationWorkerCommand::SelectModel {
                catalog_id: "qwen35-4b-q4".to_owned(),
                keep_warm: true,
            },
            TranslationWorkerCommand::Translate {
                request_id,
                text: "text".to_owned(),
                source_language: Some(TranslationLanguage::English),
                target_language: Some(TranslationLanguage::Russian),
                language_policy: TranslationLanguagePolicy::default(),
            },
            TranslationWorkerCommand::TranslateBlocks {
                request_id,
                base_revision: 1,
                blocks: vec![screen_block()],
                context: source_context(),
                source_language: Some(TranslationLanguage::English),
                target_language: Some(TranslationLanguage::Russian),
                language_policy: TranslationLanguagePolicy::default(),
            },
            TranslationWorkerCommand::SaveGlossaryEntry {
                operation_id: request_id,
                request_id,
                block_id: ScreenBlockId::from("block"),
                expected_revision: 1,
                entry: glossary_draft(),
            },
            TranslationWorkerCommand::ListGlossary {
                operation_id: request_id,
                query: Some("query".to_owned()),
                limit: 10,
            },
            TranslationWorkerCommand::UpdateGlossaryEntry {
                operation_id: request_id,
                entry_id: GlossaryEntryId::from("entry"),
                entry: glossary_draft(),
            },
            TranslationWorkerCommand::DeleteGlossaryEntry {
                operation_id: request_id,
                entry_id: GlossaryEntryId::from("entry"),
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
            TranslationWorkerCommand::UnloadLocalModel,
            TranslationWorkerCommand::Shutdown,
        ]
    }

    fn translation_events() -> Vec<TranslationWorkerEvent> {
        let request_id = RequestId::new();
        vec![
            TranslationWorkerEvent::Ready {
                protocol_version: 1,
                selected_catalog_id: Some("qwen35-4b-q4".to_owned()),
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
                catalog_id: "model".to_owned(),
                tier: TranslationTier::Tiny,
            },
            TranslationWorkerEvent::LocalModelState {
                catalog_id: Some("model".to_owned()),
                loaded: true,
            },
            TranslationWorkerEvent::ModelRemoved {
                catalog_id: "model".to_owned(),
                tier: TranslationTier::Tiny,
                removed_bytes: 42,
                runtime_removed: true,
                models: vec![translation_model()],
            },
            TranslationWorkerEvent::Partial {
                request_id,
                translated_text: "text".to_owned(),
                first_token_duration_ms: 1,
                tokenize_duration_ms: 2,
                chunk_index: 3,
                chunk_count: 4,
            },
            TranslationWorkerEvent::ScreenBlockPartial {
                request_id,
                base_revision: 1,
                block_id: ScreenBlockId::from("block"),
                translated_text: "translated".to_owned(),
            },
            TranslationWorkerEvent::ScreenBlocksCompleted {
                request_id,
                base_revision: 1,
                blocks: vec![TranslatedScreenBlock {
                    block_id: ScreenBlockId::from("block"),
                    translated_text: "translated".to_owned(),
                    completion: BlockCompletion::Completed,
                    failure_message: Some("message".to_owned()),
                }],
                source_language: TranslationLanguage::English,
                target_language: TranslationLanguage::Russian,
                capabilities: TranslationBackendCapabilities::structured(),
                processing_duration_ms: 1,
            },
            TranslationWorkerEvent::GlossaryList {
                operation_id: request_id,
                entries: vec![glossary_entry()],
            },
            TranslationWorkerEvent::GlossarySaved {
                operation_id: request_id,
                entry: glossary_entry(),
            },
            TranslationWorkerEvent::GlossaryDeleted {
                operation_id: request_id,
                entry_id: GlossaryEntryId::from("entry"),
            },
            TranslationWorkerEvent::GlossaryFailed {
                operation_id: request_id,
                code: "code".to_owned(),
                message: "message".to_owned(),
                retryable: true,
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
            TranslationWorkerEvent::HistoryCleared {
                error: Some("history file is in use".to_owned()),
            },
            TranslationWorkerEvent::TranslationProviders {
                statuses: vec![translation_status()],
            },
            TranslationWorkerEvent::CredentialSaved {
                operation_id: request_id,
                provider_id: "provider".to_owned(),
                error: "error".to_owned(),
            },
            TranslationWorkerEvent::RequestFailed {
                request_id,
                failure: TranslationFailureReport {
                    kind: TranslationFailureKind::UnsupportedRequest,
                    provider_id: Some("provider".to_owned()),
                    retry_after_ms: Some(1_000),
                    required_catalog_id: Some("model".to_owned()),
                    detail: "detail".to_owned(),
                    attempts: vec![TranslationAttemptReport {
                        provider_id: "provider".to_owned(),
                        kind: TranslationFailureKind::UnsupportedRequest,
                        retry_after_ms: Some(1_000),
                        detail: "detail".to_owned(),
                    }],
                },
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
                dictation_language: DictationLanguage::Russian,
                history_enabled: true,
                fast_release_previews: true,
                prepare_local_model: true,
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
            SpeechWorkerCommand::RemoveModel {
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
            SpeechWorkerCommand::SetDictationLanguage {
                language: DictationLanguage::Ukrainian,
            },
            SpeechWorkerCommand::SetFastReleasePreviews { enabled: true },
            SpeechWorkerCommand::GetHistory { limit: 1 },
            SpeechWorkerCommand::SetHistoryEnabled { enabled: true },
            SpeechWorkerCommand::ClearHistory,
            SpeechWorkerCommand::UnloadLocalModel,
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
            SpeechWorkerEvent::LocalModelState {
                catalog_id: Some("model".to_owned()),
                loaded: true,
            },
            SpeechWorkerEvent::ModelRemoved {
                catalog_id: "model".to_owned(),
                removed_bytes: 42,
                models: vec![speech_model()],
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
            SpeechWorkerEvent::HistoryCleared {
                error: Some("history file is in use".to_owned()),
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
        InputHostEvent::ScreenCaptureRequested { .. } => 4, InputHostEvent::ScreenCaptureCancelled { .. } => 5,
        InputHostEvent::SelectionCaptureStarted { .. } => 6, InputHostEvent::SelectionCaptured { .. } => 7,
        InputHostEvent::SelectionInputReleased { .. } => 8, InputHostEvent::SelectionInputReleaseFailed { .. } => 9,
        InputHostEvent::SelectionCaptureFailed { .. } => 10, InputHostEvent::InsertionCompleted { .. } => 11,
        InputHostEvent::Recovered { .. } => 12, InputHostEvent::SelectionReplaced { .. } => 13,
        InputHostEvent::TextCopied { .. } => 14, InputHostEvent::HotkeysApplied { .. } => 15,
        InputHostEvent::HotkeysRejected { .. } => 16, InputHostEvent::PauseChanged { .. } => 17,
        InputHostEvent::Failed { .. } => 18
    );
    index_fn!(input_command_index, InputHostCommand,
        InputHostCommand::SetHotkeys { .. } => 0, InputHostCommand::InsertTranscript { .. } => 1,
        InputHostCommand::CopyTranscript { .. } => 2, InputHostCommand::CopyText { .. } => 3,
        InputHostCommand::ReplaceSelection { .. } => 4, InputHostCommand::CancelSelection { .. } => 5,
        InputHostCommand::SetScreenCaptureMode { .. } => 6, InputHostCommand::Shutdown => 7
    );
    index_fn!(ocr_command_index, OcrWorkerCommand,
        OcrWorkerCommand::Recognize { .. } => 0, OcrWorkerCommand::Cancel { .. } => 1,
        OcrWorkerCommand::Shutdown => 2
    );
    index_fn!(ocr_event_index, OcrWorkerEvent,
        OcrWorkerEvent::Ready { .. } => 0, OcrWorkerEvent::Recognized { .. } => 1,
        OcrWorkerEvent::Failed { .. } => 2, OcrWorkerEvent::Cancelled { .. } => 3
    );
    index_fn!(translation_command_index, TranslationWorkerCommand,
        TranslationWorkerCommand::Initialize { .. } => 0, TranslationWorkerCommand::EnsureModel { .. } => 1,
        TranslationWorkerCommand::RemoveModel { .. } => 2, TranslationWorkerCommand::CancelModelInstall => 3,
        TranslationWorkerCommand::SelectModel { .. } => 4, TranslationWorkerCommand::Translate { .. } => 5,
        TranslationWorkerCommand::TranslateBlocks { .. } => 6, TranslationWorkerCommand::SaveGlossaryEntry { .. } => 7,
        TranslationWorkerCommand::ListGlossary { .. } => 8, TranslationWorkerCommand::UpdateGlossaryEntry { .. } => 9,
        TranslationWorkerCommand::DeleteGlossaryEntry { .. } => 10, TranslationWorkerCommand::Cancel { .. } => 11,
        TranslationWorkerCommand::SetTranslationProviders { .. } => 12, TranslationWorkerCommand::SetTranslationProviderCredential { .. } => 13,
        TranslationWorkerCommand::GetTranslationProviders => 14, TranslationWorkerCommand::GetHistory { .. } => 15,
        TranslationWorkerCommand::SetHistoryEnabled { .. } => 16, TranslationWorkerCommand::ClearHistory => 17,
        TranslationWorkerCommand::UnloadLocalModel => 18, TranslationWorkerCommand::Shutdown => 19
    );
    index_fn!(translation_event_index, TranslationWorkerEvent,
        TranslationWorkerEvent::Ready { .. } => 0, TranslationWorkerEvent::ModelDownloadProgress { .. } => 1,
        TranslationWorkerEvent::ModelInstalled { .. } => 2, TranslationWorkerEvent::ModelReady { .. } => 3,
        TranslationWorkerEvent::ModelSelected { .. } => 4, TranslationWorkerEvent::LocalModelState { .. } => 5,
        TranslationWorkerEvent::ModelRemoved { .. } => 6, TranslationWorkerEvent::Partial { .. } => 7,
        TranslationWorkerEvent::ScreenBlockPartial { .. } => 8, TranslationWorkerEvent::ScreenBlocksCompleted { .. } => 9,
        TranslationWorkerEvent::GlossaryList { .. } => 10, TranslationWorkerEvent::GlossarySaved { .. } => 11,
        TranslationWorkerEvent::GlossaryDeleted { .. } => 12, TranslationWorkerEvent::GlossaryFailed { .. } => 13,
        TranslationWorkerEvent::Completed { .. } => 14, TranslationWorkerEvent::Cancelled { .. } => 15,
        TranslationWorkerEvent::History { .. } => 16, TranslationWorkerEvent::TranslationProviders { .. } => 17,
        TranslationWorkerEvent::CredentialSaved { .. } => 18, TranslationWorkerEvent::RequestFailed { .. } => 19,
        TranslationWorkerEvent::Failed { .. } => 20, TranslationWorkerEvent::HistoryCleared { .. } => 21
    );
    index_fn!(speech_command_index, SpeechWorkerCommand,
        SpeechWorkerCommand::Initialize { .. } => 0, SpeechWorkerCommand::Begin { .. } => 1,
        SpeechWorkerCommand::End { .. } => 2, SpeechWorkerCommand::Cancel { .. } => 3,
        SpeechWorkerCommand::EnsureModel { .. } => 4, SpeechWorkerCommand::SelectModel { .. } => 5,
        SpeechWorkerCommand::RemoveModel { .. } => 6, SpeechWorkerCommand::CancelModelInstall => 7,
        SpeechWorkerCommand::SetTranscriptionProviders { .. } => 8, SpeechWorkerCommand::SetProviderCredential { .. } => 9,
        SpeechWorkerCommand::GetTranscriptionProviders => 10, SpeechWorkerCommand::SetOutputMode { .. } => 11,
        SpeechWorkerCommand::SetDictationLanguage { .. } => 12, SpeechWorkerCommand::SetFastReleasePreviews { .. } => 13,
        SpeechWorkerCommand::GetHistory { .. } => 14, SpeechWorkerCommand::SetHistoryEnabled { .. } => 15,
        SpeechWorkerCommand::ClearHistory => 16, SpeechWorkerCommand::UnloadLocalModel => 17,
        SpeechWorkerCommand::Shutdown => 18
    );
    index_fn!(speech_event_index, SpeechWorkerEvent,
        SpeechWorkerEvent::Ready { .. } => 0, SpeechWorkerEvent::ModelDownloadProgress { .. } => 1,
        SpeechWorkerEvent::ModelInstalled { .. } => 2, SpeechWorkerEvent::ModelReady { .. } => 3,
        SpeechWorkerEvent::ModelSelected { .. } => 4, SpeechWorkerEvent::LocalModelState { .. } => 5,
        SpeechWorkerEvent::ModelRemoved { .. } => 6, SpeechWorkerEvent::Listening { .. } => 7,
        SpeechWorkerEvent::AudioLevel { .. } => 8, SpeechWorkerEvent::Processing { .. } => 9,
        SpeechWorkerEvent::Completed { .. } => 10, SpeechWorkerEvent::Cancelled { .. } => 11,
        SpeechWorkerEvent::History { .. } => 12, SpeechWorkerEvent::TranscriptionProviders { .. } => 13,
        SpeechWorkerEvent::CredentialSaved { .. } => 14, SpeechWorkerEvent::TranscribedRemotely { .. } => 15,
        SpeechWorkerEvent::Failed { .. } => 16, SpeechWorkerEvent::HistoryCleared { .. } => 17
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
    fn screen_block_capabilities_default_for_older_worker_events() {
        let json = r#"{"type":"screen_blocks_completed","payload":{"request_id":"00000000-0000-0000-0000-000000000001","base_revision":7,"blocks":[],"source_language":"english","target_language":"russian","processing_duration_ms":1}}"#;
        let event: TranslationWorkerEvent = serde_json::from_str(json).unwrap();
        let TranslationWorkerEvent::ScreenBlocksCompleted { capabilities, .. } = event else {
            panic!("wrong event variant");
        };
        assert_eq!(capabilities, TranslationBackendCapabilities::default());
    }

    #[test]
    fn every_process_direction_and_variant_has_a_versioned_wire_shape() {
        let input_events = input_events();
        let input_commands = input_commands();
        let ocr_commands = ocr_commands();
        let ocr_events = ocr_events();
        let translation_commands = translation_commands();
        let translation_events = translation_events();
        let speech_commands = speech_commands();
        let speech_events = speech_events();
        let companion_commands = companion_commands();
        let companion_events = companion_events();

        assert_complete(&input_events, input_event_index, 19);
        assert_complete(&input_commands, input_command_index, 8);
        assert_complete(&ocr_commands, ocr_command_index, 3);
        assert_complete(&ocr_events, ocr_event_index, 4);
        assert_complete(&translation_commands, translation_command_index, 20);
        assert_complete(&translation_events, translation_event_index, 22);
        assert_complete(&speech_commands, speech_command_index, 19);
        assert_complete(&speech_events, speech_event_index, 18);
        assert_complete(&companion_commands, companion_command_index, 8);
        assert_complete(&companion_events, companion_event_index, 3);

        let mut schema = Vec::new();
        append("input.event", &input_events, &mut schema);
        append("input.command", &input_commands, &mut schema);
        append("ocr.command", &ocr_commands, &mut schema);
        append("ocr.event", &ocr_events, &mut schema);
        append("translation.command", &translation_commands, &mut schema);
        append("translation.event", &translation_events, &mut schema);
        append("speech.command", &speech_commands, &mut schema);
        append("speech.event", &speech_events, &mut schema);
        append("companion.command", &companion_commands, &mut schema);
        append("companion.event", &companion_events, &mut schema);
        schema.sort();

        assert_eq!(
            stable_hash(&schema),
            0x1e8c_74dc_9ac8_7925,
            "protocol {PROTOCOL_VERSION} wire schema changed:\n{}",
            schema.join("\n")
        );
    }
}
