#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    env,
    error::Error,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "screen-translation")]
use base64::Engine as _;

use preview_modes::{
    indicator_presentation_probe_cycles, indicator_preview_slate_surface, indicator_preview_style,
    run_action_palette_preview, run_indicator_presentation_probe, run_indicator_preview,
    run_layout_preview, run_translation_presentation_probe, run_translation_preview,
    translation_presentation_probe_cycles, translation_preview_mode,
};
use providers_view::sync_providers;
use pulse_matrix::PulseMatrixPalette;
use rust_app_core::{
    BeginDisposition, EndDisposition, LayoutCandidate, SessionPhase, SessionRegistry,
    dictation::{CompletionDelivery, HistoryUpdate, plan_completion},
    hotkeys::format_hotkey,
    provider_drafts::ProviderDraftStore,
};
#[cfg(feature = "screen-translation")]
use rust_ocr_engine::reconstruct_text;
#[cfg(feature = "screen-translation")]
use rust_platform_windows::screen_capture::DesktopCaptureBackend as _;
use rust_protocol::{
    BUILD_COMMIT, CompanionRendererCommand, CompanionRendererEvent, DictationLanguage,
    HistoryEntry, HotkeyAction, HotkeyBindings, HotkeyConfiguration, HotkeyFailure,
    HotkeyRejection, InputHostCommand, InputHostEvent, ModelSummary, PROTOCOL_VERSION,
    PhysicalRect, RequestId, SessionId, SpeechOutputMode, SpeechWorkerCommand, SpeechWorkerEvent,
    TargetSnapshot, TextActionIntent, Transcript, TranscriptionProviderEntry,
    TranscriptionProviderStatus, TranslationFailureKind, TranslationFailureReport,
    TranslationHistoryEntry, TranslationLanguage, TranslationLanguagePolicy,
    TranslationModelSummary, TranslationProviderEntry, TranslationProviderStatus,
    TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
#[cfg(feature = "screen-translation")]
use rust_protocol::{
    GlossaryEntryDraft, GlossaryOrigin, OcrFailureKind, OcrLanguagePolicy, OcrPixelFormat,
    OcrWorkerCommand, OcrWorkerEvent, SourceContext,
};
#[cfg(all(feature = "inspect", feature = "screen-translation"))]
use screen_replay::{ReplayPhase, ReplayStep, ScreenReplayProbe};
#[cfg(feature = "screen-translation")]
use screen_result::{
    DocumentError, MIN_ADMITTED_OCR_CONFIDENCE_MILLI, ScreenTranslationDocument,
    screen_blocks_from_ocr,
};
#[cfg(feature = "screen-translation")]
use screen_result_window::{
    ScreenResultWindowRegistry, create_screen_result_window, get_or_create_screen_result_slot,
    pin_screen_result, present_screen_result, retire_screen_result_slot_by_handle,
};
#[cfg(feature = "screen-translation")]
use screen_translation::ScreenTranslationBegin;
#[cfg(feature = "screen-translation")]
use screen_translation::SelectionFailure;
use screen_translation::{
    DesktopScreenSelectionCoordinator, DesktopSelectionEnvironment, ScreenSelectionCoordinator,
    ScreenTranslationCancellation, ScreenTranslationCoordinator, SelectionTerminal,
};
#[cfg(feature = "screen-translation")]
use screenshot_store::{
    ScreenshotMetadata, ScreenshotRetention, ScreenshotStore, save_frame_permanent,
};
use serde::{Deserialize, Serialize};
use settings_document::{
    SavedSettings, apply_first_run_choice, data_root, load_settings, save_first_run_settings,
    save_settings,
};
use settings_view::{
    effective_hotkey_bindings, precreate_action_windows, prepend_history_row, set_indicator_status,
    set_runtime_status, set_status_problem, show_layout_chooser, sync_action_settings,
    sync_history, sync_hotkey_capture_status, sync_models, sync_output_mode,
    sync_translation_models, wire_action_palette, wire_chooser, wire_settings, wire_translator,
    wire_tray,
};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{
    ComponentHandle, Image, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode,
};
#[cfg(feature = "screen-translation")]
use source_context::ContextCollector;
#[cfg(feature = "screen-translation")]
use translation_editor::{
    EditorCommitScope, EditorDraftKey, EditorPhase, TranslationEditorState,
    host_editor_should_close,
};
use translation_view::{
    StableParagraphStream, TranslationPreviewRegistry, TranslationPreviewSlot,
    adaptive_text_panel_height, apply_translation_preview_layout,
    apply_translation_preview_pin_state, configure_slint_translation_resize,
    create_translation_preview_slot, precreate_translation_preview,
    refresh_cloaked_translation_backdrop, retire_translation_preview, set_translation_status,
    update_translation_preview_content, wire_translation_preview,
};
use visualizer::{
    BroadcastMicPalette, CircularSpectrumPalette, INDICATOR_FRAME_INTERVAL, IndicatorAnimation,
    RecordingSurface, VisualizerStyle, WritingSlateSurface, new_recording_indicators,
    new_settings_preview,
};
use worker_supervisor::{
    InitialWorkers, SessionEventRouter, StartupSessionGate, WorkerLifecycle, WorkerRecovery,
    Workers, spawn_initial_workers,
};

mod cuda_activation;
mod cuda_setup;
mod first_run;
mod flow_sphere;
#[cfg(feature = "screen-translation")]
mod glossary_view;
mod insertion_notice;
mod installer_locale;
mod localization;
mod overlay_presenter;
mod preview_modes;
mod providers_view;
mod pulse_matrix;
mod resource_view;
#[cfg(feature = "screen-translation")]
mod screen_geometry;
#[cfg(feature = "screen-translation")]
mod screen_inpaint;
#[cfg(all(feature = "inspect", feature = "screen-translation"))]
mod screen_replay;
#[cfg(feature = "screen-translation")]
mod screen_result;
#[cfg(feature = "screen-translation")]
mod screen_result_view;
#[cfg(feature = "screen-translation")]
mod screen_result_window;
mod screen_translation;
mod screen_translation_preview;
#[cfg(feature = "screen-translation")]
mod screenshot_store;
mod settings_document;
mod settings_preview;
mod settings_projection;
mod settings_view;
#[cfg(feature = "screen-translation")]
mod source_context;
mod speech_backend;
mod support_action;
#[cfg(feature = "screen-translation")]
mod translation_editor;
mod translation_view;
mod translator_status;
mod visualizer;
mod wheel_scroll;
mod worker_supervisor;

#[cfg(feature = "screen-translation")]
use localization::{ScreenTranslationOutcome, screen_translation_outcome_text};
use localization::{UiLocale, UiText, active_locale, render, text};
use overlay_presenter::{OverlayContract, OverlayPresenter};
use speech_backend::{SpeechBackend, SpeechBackendPreference};

slint::include_modules!();

fn state_text(state: &Rc<RefCell<AppState>>, message: UiText) -> &'static str {
    text(active_locale(state.borrow().ui_locale), message)
}

fn state_message(
    state: &Rc<RefCell<AppState>>,
    message: UiText,
    values: &[(&str, &str)],
) -> String {
    render(active_locale(state.borrow().ui_locale), message, values)
}

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const DEFAULT_TRANSLATION_BLUR_RADIUS: u32 = 3;
const MAX_TRANSLATION_BLUR_RADIUS: u32 = 8;
/// Visible and in-memory history depth. The speech worker keeps the same
/// number of entries on disk.
const HISTORY_RETAINED_ROWS: usize = 250;

fn record_translation_history(
    history: &mut Vec<TranslationHistoryEntry>,
    enabled: bool,
    result: &TranslationResult,
    created_unix_ms: u64,
) {
    if !enabled {
        return;
    }
    history.insert(
        0,
        TranslationHistoryEntry {
            result: result.clone(),
            created_unix_ms,
        },
    );
    history.truncate(HISTORY_RETAINED_ROWS);
}

/// The newest microphone frame, held outside the UI event queue.
static LATEST_AUDIO_LEVEL: Mutex<Option<AudioLevelFrame>> = Mutex::new(None);
/// Set while a wake-up for that frame is already queued for the UI thread.
static AUDIO_LEVEL_QUEUED: AtomicBool = AtomicBool::new(false);

struct AudioLevelFrame {
    instance_id: u64,
    session_id: SessionId,
    peak: f32,
    spectrum: Vec<f32>,
}

/// Keeps only the newest microphone frame and asks for at most one wake-up.
///
/// The worker emits about thirty-one frames a second, each carrying
/// thirty-two spectrum bands. Queueing every one of them puts the transcript
/// that ends a dictation behind a backlog of visualizer updates whenever the
/// UI thread stalls, and then replays frames that are already stale. Only the
/// newest frame can still be worth drawing.
fn record_audio_level(frame: AudioLevelFrame) -> Option<RuntimeEvent> {
    let instance_id = frame.instance_id;
    *LATEST_AUDIO_LEVEL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(frame);
    (!AUDIO_LEVEL_QUEUED.swap(true, Ordering::AcqRel))
        .then_some(RuntimeEvent::SpeechAudioLevel { instance_id })
}

fn take_audio_level() -> Option<AudioLevelFrame> {
    // Cleared first: a frame that arrives during the take below queues its own
    // wake-up rather than being dropped.
    AUDIO_LEVEL_QUEUED.store(false, Ordering::Release);
    LATEST_AUDIO_LEVEL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

enum RuntimeEvent {
    Input {
        instance_id: u64,
        event: InputHostEvent,
    },
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    ScreenReplayTick,
    Speech {
        instance_id: u64,
        event: SpeechWorkerEvent,
    },
    /// A coalesced microphone frame is waiting in `LATEST_AUDIO_LEVEL`.
    SpeechAudioLevel {
        instance_id: u64,
    },
    Translation {
        instance_id: u64,
        event: TranslationWorkerEvent,
    },
    #[cfg(feature = "screen-translation")]
    Ocr {
        instance_id: u64,
        event: OcrWorkerEvent,
    },
    Companion {
        instance_id: u64,
        event: CompanionRendererEvent,
    },
    DismissTranslationPreview(u64),
    TranslationPreviewPinChanged {
        window_handle: u64,
        pinned: bool,
    },
    ScrollTranslationPreview {
        window_handle: u64,
        delta: i32,
    },
    #[cfg(feature = "screen-translation")]
    ScreenBlockEditRequested {
        window_handle: u64,
        block_index: i32,
    },
    #[cfg(feature = "screen-translation")]
    ScreenViewModeChanged {
        window_handle: u64,
        mode: i32,
    },
    #[cfg(feature = "screen-translation")]
    ScreenCopyTextRequested {
        window_handle: u64,
    },
    #[cfg(feature = "screen-translation")]
    ScreenTranslationRetryRequested {
        window_handle: u64,
    },
    #[cfg(feature = "screen-translation")]
    ScreenResultPresentationFailed {
        request_id: RequestId,
        anchor: PhysicalRect,
        error: String,
        presentation: Option<overlay_presenter::PresentationTicket>,
        attachment_attempt: Option<u64>,
    },
    #[cfg(feature = "screen-translation")]
    ScreenSaveFileRequested {
        window_handle: u64,
    },
    #[cfg(feature = "screen-translation")]
    ScreenDeleteImageRequested {
        window_handle: u64,
    },
    #[cfg(feature = "screen-translation")]
    StoredScreenshotSaved(Result<(), String>),
    #[cfg(feature = "screen-translation")]
    StoredScreenshotDeleted {
        request_id: RequestId,
        result: Result<(), String>,
    },
    #[cfg(feature = "screen-translation")]
    ScreenEditorCommit,
    #[cfg(feature = "screen-translation")]
    ScreenEditorCancel,
    #[cfg(feature = "screen-translation")]
    ScreenEditorRetry,
    #[cfg(feature = "screen-translation")]
    GlossarySearchRequested(String),
    #[cfg(feature = "screen-translation")]
    GlossarySaveRequested {
        entry_id: String,
        source: String,
        translation: String,
    },
    #[cfg(feature = "screen-translation")]
    GlossaryDeleteRequested(String),
    #[cfg(feature = "screen-translation")]
    GlossaryRetryRequested,
    #[cfg(feature = "screen-translation")]
    ScreenshotRetentionChanged(bool),
    #[cfg(feature = "screen-translation")]
    DeleteStoredScreenshots,
    #[cfg(feature = "screen-translation")]
    StoredScreenshotsDeleted(Result<usize, String>),
    #[cfg(feature = "screen-translation")]
    OpenLatestScreenTranslation,
    #[cfg(feature = "screen-translation")]
    LatestScreenTranslationLoaded(
        Result<
            Option<(
                ScreenTranslationDocument,
                TranslationLanguage,
                TranslationLanguage,
            )>,
            String,
        >,
    ),
    #[cfg(feature = "screen-translation")]
    ScreenPixelsExpired {
        request_id: RequestId,
    },
    ConfirmQueuedModelInstall,
    ScreenSelectionPointer(rust_platform_windows::region_selection::PointerEvent),
    ScreenSelectionWatchdog {
        request_id: RequestId,
    },
    ScreenSelectionSurfacesReady {
        request_id: RequestId,
    },
    #[cfg(feature = "screen-translation")]
    ScreenCapturePrepared {
        request_id: RequestId,
        excluded_overlays: Vec<overlay_presenter::CaptureExcludedOverlay>,
        result: Result<PreparedScreenCapture, rust_platform_windows::screen_capture::CaptureError>,
    },
    #[cfg(feature = "screen-translation")]
    ScreenOcrPrepared {
        request_id: RequestId,
        rect: PhysicalRect,
        result: Result<PreparedScreenOcr, rust_platform_windows::screen_capture::CaptureError>,
    },
    #[cfg(feature = "screen-translation")]
    OcrWatchdog {
        request_id: RequestId,
        instance_id: u64,
    },
    SessionEnd(rust_platform_windows::SessionEndEvent),
    WorkerError {
        kind: WorkerKind,
        instance_id: u64,
        message: String,
    },
    WorkerExited {
        kind: WorkerKind,
        instance_id: u64,
        exit: rust_platform_windows::ProcessExit,
    },
}

#[cfg(feature = "screen-translation")]
struct PreparedScreenOcr {
    command: OcrWorkerCommand,
    crop: Arc<rust_platform_windows::screen_capture::RgbaFrame>,
}

#[cfg(feature = "screen-translation")]
struct PreparedScreenCapture {
    capture: rust_platform_windows::screen_capture::DesktopCapture,
    inventory: Option<rust_platform_windows::WindowInventory>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WorkerKind {
    Input,
    Speech,
    Translation,
    #[cfg(feature = "screen-translation")]
    Ocr,
    Companion,
}

type UiEventHandler = Box<dyn FnMut(RuntimeEvent)>;

impl RuntimeEvent {
    fn input(instance_id: u64, event: InputHostEvent) -> Self {
        Self::Input { instance_id, event }
    }

    fn translation(instance_id: u64, event: TranslationWorkerEvent) -> Self {
        Self::Translation { instance_id, event }
    }

    fn companion(instance_id: u64, event: CompanionRendererEvent) -> Self {
        Self::Companion { instance_id, event }
    }

    fn filter_current_worker(
        self,
        is_current: impl FnOnce(WorkerKind, u64) -> bool,
    ) -> Option<Self> {
        let (kind, instance_id) = match &self {
            Self::Input { instance_id, .. } => (WorkerKind::Input, *instance_id),
            Self::Speech { instance_id, .. } => (WorkerKind::Speech, *instance_id),
            Self::Translation { instance_id, .. } => (WorkerKind::Translation, *instance_id),
            Self::Companion { instance_id, .. } => (WorkerKind::Companion, *instance_id),
            #[cfg(feature = "screen-translation")]
            Self::Ocr { instance_id, .. } => (WorkerKind::Ocr, *instance_id),
            _ => return Some(self),
        };
        if is_current(kind, instance_id) {
            Some(self)
        } else {
            rust_app_core::diagnostics::log_line(&format!(
                "worker kind={kind:?} instance={instance_id} event=stale_ignored"
            ));
            None
        }
    }
}

#[cfg(test)]
#[path = "worker_event_regression_tests.rs"]
mod worker_event_regression_tests;

thread_local! {
    static UI_EVENT_HANDLER: RefCell<Option<UiEventHandler>> = RefCell::new(None);
}

#[derive(Clone)]
struct RuntimeUi {
    settings: slint::Weak<SettingsWindow>,
    recording_overlays: RecordingOverlays,
    chooser: slint::Weak<SelectionChooser>,
    chooser_presenter: OverlayPresenter,
    action_palette: slint::Weak<ActionPalette>,
    action_palette_presenter: OverlayPresenter,
    translation_previews: Rc<RefCell<TranslationPreviewRegistry>>,
    #[cfg(feature = "screen-translation")]
    screen_result_windows: Rc<RefCell<ScreenResultWindowRegistry>>,
    translator: slint::Weak<TranslatorWindow>,
    state: Rc<RefCell<AppState>>,
    workers: Rc<Workers>,
    indicator_animation: Rc<RefCell<IndicatorAnimation>>,
    insertion_notice: Rc<insertion_notice::InsertionNoticeController>,
    screen_selection: Rc<RefCell<DesktopScreenSelectionCoordinator>>,
    screen_translation: Rc<RefCell<ScreenTranslationCoordinator>>,
    #[cfg(feature = "screen-translation")]
    active_screen_editor: Rc<RefCell<Option<ActiveScreenEditor>>>,
    #[cfg(feature = "screen-translation")]
    glossary_view: Rc<RefCell<glossary_view::GlossaryViewState>>,
    #[cfg(feature = "screen-translation")]
    screen_capture_backend:
        Arc<rust_platform_windows::screen_capture::WindowsDesktopCaptureBackend>,
    #[cfg(feature = "screen-translation")]
    screenshot_store: Option<Arc<ScreenshotStore>>,
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    screen_replay: Option<Rc<RefCell<ScreenReplayProbe>>>,
}

#[cfg(feature = "screen-translation")]
struct ActiveScreenEditor {
    state: TranslationEditorState,
    source_language: TranslationLanguage,
    target_language: TranslationLanguage,
    context: SourceContext,
    host_window_handle: u64,
    block_index: i32,
    canonical_revision: u64,
    last_applied_translation: String,
}

/// The recording HUD deliberately owns two native windows. A Winit/DWM
/// surface's alpha mode is chosen when that window is created, so a single
/// HWND must never be recycled between the legacy panel visualizers and the
/// per-pixel-transparent visualizers. Doing so is what made Circular, Flow
/// Sphere and Writing Slate sometimes inherit a dark rectangle.
#[derive(Clone)]
struct RecordingOverlay {
    indicator: Rc<RecordingIndicator>,
    presenter: OverlayPresenter,
    /// Fixed when the HWND is created. Never change a Winit/DWM surface from
    /// panel composition to per-pixel transparency (or back) at runtime.
    transparent: bool,
}

#[derive(Clone)]
struct RecordingOverlays {
    panel: RecordingOverlay,
    transparent: RecordingOverlay,
}

impl RecordingOverlays {
    fn for_style(&self, style: VisualizerStyle) -> Option<&RecordingOverlay> {
        match style.recording_surface() {
            Some(RecordingSurface::Panel) => Some(&self.panel),
            Some(RecordingSurface::Transparent) => Some(&self.transparent),
            None => None,
        }
    }

    fn set_style(&self, style: VisualizerStyle) {
        for overlay in [&self.panel, &self.transparent] {
            overlay.indicator.set_visualizer_style(style.index());
        }
    }

    fn set_circular_palette(&self, palette: CircularSpectrumPalette) {
        for overlay in [&self.panel, &self.transparent] {
            overlay.indicator.set_circular_palette(palette.index());
        }
    }

    fn set_broadcast_mic_palette(&self, palette: BroadcastMicPalette) {
        for overlay in [&self.panel, &self.transparent] {
            overlay.indicator.set_broadcast_mic_palette(palette.index());
        }
    }

    fn set_writing_slate_surface(&self, surface: WritingSlateSurface) {
        for overlay in [&self.panel, &self.transparent] {
            overlay.indicator.set_writing_slate_surface(surface.index());
        }
    }

    fn retire_all(&self, reason: &str) {
        for overlay in [&self.panel, &self.transparent] {
            overlay.indicator.set_prepared(false);
            overlay.presenter.retire(overlay.indicator.window(), reason);
        }
    }

    fn retire_inactive_surface(&self, style: VisualizerStyle) {
        let inactive = match style.recording_surface() {
            Some(RecordingSurface::Panel) => Some(&self.transparent),
            Some(RecordingSurface::Transparent) => Some(&self.panel),
            None => None,
        };
        if let Some(inactive) = inactive {
            inactive.indicator.set_prepared(false);
            inactive
                .presenter
                .retire(inactive.indicator.window(), "visualizer_surface_switch");
        } else {
            self.retire_all("visualizer_disabled");
        }
    }
}

struct PendingSelection {
    request_id: RequestId,
    target: TargetSnapshot,
    candidates: Vec<LayoutCandidate>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TextActionMode {
    #[default]
    Adaptive,
    AlwaysPalette,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TranslationDelivery {
    #[default]
    Preview,
    Replace,
    Copy,
}

#[derive(Clone)]
struct PendingTextAction {
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    request_started_unix_ms: u64,
}

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "screen-translation"), allow(dead_code))]
enum TranslationOrigin {
    Selection {
        target: TargetSnapshot,
        delivery: TranslationDelivery,
    },
    #[allow(dead_code)] // Kept for older queued preview state during an in-place upgrade.
    ScreenRegion {
        anchor: PhysicalRect,
    },
    Translator,
}

struct PendingTranslation {
    origin: TranslationOrigin,
    source_text: String,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    language_policy: TranslationLanguagePolicy,
    started: Instant,
    request_started_unix_ms: u64,
    first_partial_shown: bool,
    stable_paragraphs: Option<StableParagraphStream>,
}

struct QueuedModelTranslation {
    catalog_id: String,
    request_id: RequestId,
    pending: PendingTranslation,
}

#[derive(Clone, Copy)]
pub(crate) enum QueuedModelMatch {
    Request(RequestId),
    Translator,
}

fn take_matching_queued_model_translation(
    queued: &mut Option<QueuedModelTranslation>,
    expected: QueuedModelMatch,
) -> Option<QueuedModelTranslation> {
    let matches = queued.as_ref().is_some_and(|candidate| match expected {
        QueuedModelMatch::Request(request_id) => candidate.request_id == request_id,
        QueuedModelMatch::Translator => {
            matches!(candidate.pending.origin, TranslationOrigin::Translator)
        }
    });
    matches.then(|| queued.take()).flatten()
}

pub(crate) fn discard_matching_queued_model_translation(
    state: &Rc<RefCell<AppState>>,
    expected: QueuedModelMatch,
) -> Option<RequestId> {
    let mut app = state.borrow_mut();
    let queued =
        take_matching_queued_model_translation(&mut app.queued_model_translation, expected)?;
    app.selection_release_gates.remove(&queued.request_id);
    Some(queued.request_id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactInstallOffer {
    catalog_id: String,
    display_name: String,
    size_bytes: u64,
}

fn take_pending_translation_install_offer(
    install_offer: Option<CompactInstallOffer>,
    pending: &mut Option<PendingTranslation>,
) -> Option<(CompactInstallOffer, PendingTranslation)> {
    let offer = install_offer?;
    let pending_translation = pending.take()?;
    Some((offer, pending_translation))
}

fn required_compact_install(
    failure: &TranslationFailureReport,
    models: &[TranslationModelSummary],
    install_already_queued: bool,
) -> Option<CompactInstallOffer> {
    if install_already_queued || failure.kind != TranslationFailureKind::LocalModelMissing {
        return None;
    }
    let catalog_id = failure.required_catalog_id.as_deref()?;
    models
        .iter()
        .find(|model| {
            model.catalog_id == catalog_id
                && model.tier == TranslationTier::Compact
                && !model.installed
        })
        .map(|model| CompactInstallOffer {
            catalog_id: model.catalog_id.clone(),
            display_name: model.display_name.clone(),
            size_bytes: model.size_bytes,
        })
}

fn pending_translation_command(
    request_id: RequestId,
    pending: &PendingTranslation,
) -> TranslationWorkerCommand {
    TranslationWorkerCommand::Translate {
        request_id,
        text: pending.source_text.clone(),
        source_language: pending.source_language,
        target_language: pending.target_language,
        language_policy: pending.language_policy,
    }
}

fn confirm_queued_model_install(runtime: &RuntimeUi) {
    let Some((catalog_id, request_id, origin, source_text)) = runtime
        .state
        .borrow()
        .queued_model_translation
        .as_ref()
        .map(|queued| {
            (
                queued.catalog_id.clone(),
                queued.request_id,
                queued.pending.origin,
                queued.pending.source_text.clone(),
            )
        })
    else {
        return;
    };

    if let Err(error) = runtime
        .workers
        .translation
        .send(&TranslationWorkerCommand::EnsureModel {
            catalog_id: catalog_id.clone(),
        })
    {
        runtime.state.borrow_mut().queued_model_translation = None;
        runtime
            .state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        let message = state_message(
            &runtime.state,
            UiText::CouldNotStartTranslation,
            &[("error", &error.to_string())],
        );
        match origin {
            TranslationOrigin::Translator => {
                if let Some(translator) = runtime.translator.upgrade() {
                    translator.set_model_offer_visible(false);
                    translator.set_processing(false);
                    translator.set_presentation_state(4);
                    translator.set_notice_headline(message.clone().into());
                    translator.set_notice_detail("".into());
                    translator.set_status_text(message.clone().into());
                }
            }
            TranslationOrigin::Selection { target, .. } => {
                show_selection_translation_preview(
                    request_id,
                    target,
                    &source_text,
                    &message,
                    &runtime.translation_previews,
                    true,
                );
            }
            TranslationOrigin::ScreenRegion { anchor } => {
                show_screen_translation_preview(
                    request_id,
                    anchor,
                    &source_text,
                    &message,
                    &runtime.translation_previews,
                    true,
                );
            }
        }
        set_status_problem(&runtime.settings, &message);
        return;
    }

    let loading = state_text(&runtime.state, UiText::LocalTranslationLoading);
    match origin {
        TranslationOrigin::Translator => {
            if let Some(translator) = runtime.translator.upgrade() {
                translator.set_model_offer_visible(false);
                translator.set_model_offer_action("".into());
                translator.set_processing(true);
                translator.set_presentation_state(1);
                translator.set_notice_headline("".into());
                translator.set_notice_detail("".into());
                translator.set_status_text(loading.into());
            }
        }
        TranslationOrigin::Selection {
            target,
            delivery: TranslationDelivery::Preview,
        } => show_selection_translation_preview(
            request_id,
            target,
            &source_text,
            loading,
            &runtime.translation_previews,
            false,
        ),
        TranslationOrigin::Selection { .. } => {}
        TranslationOrigin::ScreenRegion { anchor } => show_screen_translation_preview(
            request_id,
            anchor,
            &source_text,
            loading,
            &runtime.translation_previews,
            false,
        ),
    }
    set_indicator_status(&runtime.settings, loading);
    rust_app_core::diagnostics::log_line(&format!(
        "translation_pipeline request={request_id} stage=installing_required_pack catalog_id={catalog_id} consent=explicit"
    ));
}

#[derive(Debug, PartialEq, Eq)]
enum SelectionReleaseGate {
    Pending,
    Released,
    Failed(String),
    AwaitingReplacement {
        target: TargetSnapshot,
        text: String,
    },
}

struct SelectionTranslationRequest {
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    delivery: TranslationDelivery,
    request_started_unix_ms: u64,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
}

#[cfg(feature = "screen-translation")]
struct ScreenTranslationRequest {
    request_id: RequestId,
    anchor: PhysicalRect,
    request_started_unix_ms: u64,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
}

struct PendingInsertLast {
    session_id: Option<SessionId>,
    target: TargetSnapshot,
}

enum InsertLastResolution {
    Insert(Transcript),
    Await(SessionId),
    LoadHistory,
    Unavailable(&'static str),
}

struct AppState {
    registry: SessionRegistry,
    targets: HashMap<SessionId, TargetSnapshot>,
    transcripts: HashMap<SessionId, Transcript>,
    latest_history_session: Option<SessionId>,
    history: Vec<HistoryEntry>,
    history_loaded: bool,
    models: Vec<ModelSummary>,
    selected_model_id: String,
    speech_loaded_model_id: Option<String>,
    speech_backend_preference: SpeechBackendPreference,
    speech_backend_active: Option<SpeechBackend>,
    output_mode: SpeechOutputMode,
    dictation_language: DictationLanguage,
    fast_release_previews: bool,
    /// Type dictation instead of pasting it through the clipboard.
    avoid_clipboard_insertion: bool,
    visualizer_style: VisualizerStyle,
    circular_spectrum_palette: CircularSpectrumPalette,
    broadcast_mic_palette: BroadcastMicPalette,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    pulse_matrix_palette: PulseMatrixPalette,
    writing_slate_surface: WritingSlateSurface,
    companion_home_position: Option<(i32, i32)>,
    input_ready: bool,
    /// The chords the user chose. This is what is saved and validated.
    hotkeys: HotkeyBindings,
    /// What Windows actually registered for the last acknowledged request.
    /// A chord another application holds stays chosen in `hotkeys` and is
    /// reported through `hotkey_errors`; it is never written back as the
    /// user's choice.
    registered_hotkeys: HotkeyBindings,
    pending_hotkeys: Option<(RequestId, HotkeyBindings)>,
    hotkey_errors: HashMap<HotkeyAction, HotkeyRejection>,
    input_paused: bool,
    /// The speech process has completed its protocol handshake. This is
    /// deliberately separate from `speech_ready`: a cloud-first route can
    /// record as soon as the process is alive, without waiting for a local
    /// Whisper model that route will never reach.
    speech_worker_ready: bool,
    speech_ready: bool,
    /// A user-requested memory release keeps the installed selection. The
    /// next local dictation may therefore start the worker's lazy reload.
    speech_reload_on_next_use: bool,
    speech_unloading: bool,
    indicator_session: Option<SessionId>,
    active_visualizer_style: Option<VisualizerStyle>,
    release_started: HashMap<SessionId, Instant>,
    pending_insert_last: Option<PendingInsertLast>,
    pending_selection: Option<PendingSelection>,
    translation_models: Vec<TranslationModelSummary>,
    selected_translation_tier: Option<TranslationTier>,
    selected_translation_model_id: Option<String>,
    translation_loaded_model_id: Option<String>,
    /// The translation process has completed its protocol handshake. Cloud
    /// text providers depend on this, not on whether a local model is warm.
    translation_worker_ready: bool,
    translation_ready: bool,
    translation_reload_on_next_use: bool,
    translation_unloading: bool,
    translation_enabled: bool,
    layout_enabled: bool,
    translation_keep_warm: bool,
    translation_blur_radius: u32,
    translation_primary_language: TranslationLanguage,
    translation_secondary_language: TranslationLanguage,
    text_action_mode: TextActionMode,
    translation_delivery: TranslationDelivery,
    history_enabled: bool,
    screenshot_retention_enabled: bool,
    screenshot_retention_hours: u32,
    screenshot_retention_max_bytes: u64,
    ui_locale: UiLocale,
    applied_installer_language_id: Option<String>,
    pending_text_action: Option<PendingTextAction>,
    pending_selection_captures: HashMap<RequestId, u64>,
    selection_release_gates: HashMap<RequestId, SelectionReleaseGate>,
    pending_translations: HashMap<RequestId, PendingTranslation>,
    /// A "Clear history" request waiting for both workers to answer.
    history_clear: Option<HistoryClear>,
    /// Copy requests sent from a screen result window, by the window that
    /// asked, so the input host's confirmation can mark that window copied.
    screen_copy_requests: HashMap<RequestId, u64>,
    /// A user request that discovered its exact compact language pack was not
    /// installed. The installer owns the model operation; this keeps the one
    /// original request so it can resume without making the user click twice.
    queued_model_translation: Option<QueuedModelTranslation>,
    queued_selection_translation: Option<SelectionTranslationRequest>,
    #[cfg(feature = "screen-translation")]
    queued_screen_translation: Option<ScreenTranslationRequest>,
    translation_history: Vec<TranslationHistoryEntry>,
    /// Installs in flight, by catalog id. The workers report progress for
    /// exactly as long as a download runs, so an entry here is the same fact
    /// as "this row is downloading" and the two cannot disagree.
    downloads: HashMap<String, settings_projection::Download>,
    removing_models: HashSet<String>,
    // Invariant: an id is in `downloads` only between a progress report and the
    // next thing the worker says about that id. `finish_download` is the only
    // way out, and every terminal event calls it.
    /// The ordered transcription list as the worker reports it, for Settings.
    transcription_providers: Vec<TranscriptionProviderStatus>,
    /// The same list as configured and persisted, which is what is sent to the
    /// worker and written back to settings.
    transcription_chain: Vec<TranscriptionProviderEntry>,
    /// The translation list, kept apart from the transcription one all the way
    /// down: a key for Azure Translator is not a key for Azure Speech, and
    /// somebody willing to send audio is not thereby willing to send documents.
    translation_providers: Vec<TranslationProviderStatus>,
    translation_chain: Vec<TranslationProviderEntry>,
    /// Key saves that have been sent and not yet answered.
    ///
    /// A successful `send` puts bytes in a pipe; the store is written later, in
    /// another process, and can fail. Until this existed the window closed the
    /// form, threw away what the user had typed, and wrote the second field to
    /// disk on the strength of the pipe having accepted the message.
    pending_credentials: HashMap<RequestId, PendingCredential>,
    /// Unsaved provider text belongs to Rust, not to a repeated Slint row.
    /// Reordering or refreshing provider status may rebuild that row.
    provider_drafts: ProviderDraftStore,
}

/// One key save the window is waiting to hear about.
#[derive(Debug, Clone)]
pub(crate) struct PendingCredential {
    /// Correlation id retained after the map entry is removed by a timeout.
    pub(crate) operation_id: RequestId,
    /// The row it belongs to, prefixed with its list.
    pub(crate) row_id: String,
    /// The second field, held here until the key is known to be stored. It is
    /// settings rather than a secret, and writing it before the key landed left
    /// rows configured for a service they had no key for.
    pub(crate) extra: Option<String>,
    /// What to call the service when telling the user how it went.
    pub(crate) display_name: String,
    /// Whether this operation removes the key rather than storing one. A
    /// removal must not take the row's region or endpoint with it.
    pub(crate) forget: bool,
    /// When this was sent. A worker that dies mid-write answers nothing, and a
    /// form that waits forever is a form holding a key nobody will store.
    pub(crate) sent: Instant,
}

impl AppState {
    fn new(
        selected_model_id: String,
        output_mode: SpeechOutputMode,
        speech_backend_active: SpeechBackend,
        saved_settings: &SavedSettings,
    ) -> Self {
        Self {
            registry: SessionRegistry::default(),
            targets: HashMap::new(),
            transcripts: HashMap::new(),
            latest_history_session: None,
            history: Vec::new(),
            history_loaded: false,
            models: Vec::new(),
            selected_model_id,
            speech_loaded_model_id: None,
            speech_backend_preference: saved_settings.speech_backend_preference,
            speech_backend_active: Some(speech_backend_active),
            output_mode,
            dictation_language: saved_settings.dictation_language,
            fast_release_previews: saved_settings.fast_release_previews,
            avoid_clipboard_insertion: saved_settings.avoid_clipboard_insertion,
            visualizer_style: saved_settings.visualizer_style,
            circular_spectrum_palette: saved_settings.circular_spectrum_palette,
            broadcast_mic_palette: saved_settings.broadcast_mic_palette,
            flow_sphere_palette: saved_settings.flow_sphere_palette,
            pulse_matrix_palette: saved_settings.pulse_matrix_palette,
            writing_slate_surface: saved_settings.writing_slate_surface,
            companion_home_position: saved_settings.companion_home_position,
            input_ready: false,
            hotkeys: saved_settings.hotkeys.clone(),
            registered_hotkeys: saved_settings.hotkeys.clone(),
            pending_hotkeys: None,
            hotkey_errors: HashMap::new(),
            input_paused: false,
            speech_worker_ready: false,
            speech_ready: false,
            speech_reload_on_next_use: false,
            speech_unloading: false,
            indicator_session: None,
            active_visualizer_style: None,
            release_started: HashMap::new(),
            pending_insert_last: None,
            pending_selection: None,
            translation_models: Vec::new(),
            selected_translation_tier: saved_settings.selected_translation_tier,
            selected_translation_model_id: saved_settings.selected_translation_model_id.clone(),
            translation_loaded_model_id: None,
            translation_worker_ready: false,
            translation_ready: false,
            translation_reload_on_next_use: false,
            translation_unloading: false,
            translation_enabled: saved_settings.translation_enabled,
            layout_enabled: saved_settings.layout_enabled,
            translation_keep_warm: saved_settings.translation_keep_warm,
            translation_blur_radius: saved_settings.translation_blur_radius,
            translation_primary_language: saved_settings.translation_primary_language,
            translation_secondary_language: saved_settings.translation_secondary_language,
            text_action_mode: saved_settings.text_action_mode,
            translation_delivery: saved_settings.translation_delivery,
            pending_text_action: None,
            pending_selection_captures: HashMap::new(),
            selection_release_gates: HashMap::new(),
            pending_translations: HashMap::new(),
            history_clear: None,
            screen_copy_requests: HashMap::new(),
            queued_model_translation: None,
            queued_selection_translation: None,
            #[cfg(feature = "screen-translation")]
            queued_screen_translation: None,
            translation_history: Vec::new(),
            downloads: HashMap::new(),
            removing_models: HashSet::new(),
            transcription_providers: Vec::new(),
            transcription_chain: saved_settings.transcription_providers.clone(),
            translation_providers: Vec::new(),
            translation_chain: saved_settings.translation_providers.clone(),
            pending_credentials: HashMap::new(),
            provider_drafts: ProviderDraftStore::default(),
            history_enabled: saved_settings.history_enabled,
            screenshot_retention_enabled: saved_settings.screenshot_retention_enabled,
            screenshot_retention_hours: saved_settings.screenshot_retention_hours,
            screenshot_retention_max_bytes: saved_settings.screenshot_retention_max_bytes,
            ui_locale: saved_settings.ui_locale,
            applied_installer_language_id: saved_settings.applied_installer_language_id.clone(),
        }
    }

    fn first_transcription_provider(&self) -> Option<&TranscriptionProviderStatus> {
        self.transcription_providers
            .iter()
            .filter(|status| status.enabled && status.available)
            .min_by_key(|status| status.position)
    }

    fn local_speech_enabled(&self) -> bool {
        self.transcription_chain.is_empty()
            || self
                .transcription_chain
                .iter()
                .any(|entry| entry.id == "local" && entry.enabled)
    }

    /// Whether at least one enabled transcription route can honour
    /// `TranslateToEnglish`.
    ///
    /// This is a property of the whole configured route, not just of the
    /// selected local Whisper model. Groq and OpenAI-compatible endpoints can
    /// translate even when the local model is deliberately disabled; Azure,
    /// Cloudflare and Deepgram only transcribe and are skipped for that
    /// request shape by the worker.
    fn speech_translation_supported(&self) -> bool {
        let local_supports = self
            .models
            .iter()
            .find(|model| model.catalog_id == self.selected_model_id)
            .is_some_and(|model| model.supports_speech_translation);
        if self.transcription_chain.is_empty() {
            return local_supports;
        }
        self.transcription_chain
            .iter()
            .filter(|entry| entry.enabled)
            .any(|entry| match entry.id.as_str() {
                "groq" | "custom" => true,
                "local" => local_supports,
                _ => false,
            })
    }

    fn local_translation_enabled(&self) -> bool {
        self.translation_chain.is_empty()
            || self
                .translation_chain
                .iter()
                .any(|entry| entry.id == "local" && entry.enabled)
    }

    /// Whether the configured transcription route can accept a recording now.
    /// Cloud-only operation depends on the worker handshake; local operation
    /// additionally depends on a loaded model.
    fn speech_path_ready(&self) -> bool {
        match self.first_transcription_provider() {
            Some(status) if status.id == "local" => self.speech_ready,
            Some(_) => self.speech_worker_ready,
            None => self.transcription_providers.is_empty() && self.speech_ready,
        }
    }

    fn speech_path_can_start(&self) -> bool {
        if self.speech_path_ready() {
            return true;
        }
        let local_route = match self.first_transcription_provider() {
            Some(status) => status.id == "local",
            None => self.transcription_providers.is_empty(),
        };
        local_route
            && self.speech_worker_ready
            && self.speech_reload_on_next_use
            && !self.speech_unloading
    }

    fn speech_waits_for_local(&self) -> bool {
        match self.first_transcription_provider() {
            Some(status) => status.id == "local" && !self.speech_ready,
            None => self.transcription_providers.is_empty() && !self.speech_ready,
        }
    }

    /// Whether the first translation row that can actually run is ready now.
    /// A cloud-only order must not be blocked on a local model it will never
    /// reach; a local-first order still waits for that model to be warm.
    fn translation_path_ready(&self) -> bool {
        match self
            .translation_providers
            .iter()
            .filter(|status| status.enabled && status.available)
            .min_by_key(|status| status.position)
        {
            Some(status) if status.id == "local" => self.translation_ready,
            Some(_) => self.translation_worker_ready,
            None => self.translation_providers.is_empty() && self.translation_ready,
        }
    }

    fn translation_path_can_start(&self) -> bool {
        if self.translation_path_ready() {
            return true;
        }
        let local_route = match self
            .translation_providers
            .iter()
            .filter(|status| status.enabled && status.available)
            .min_by_key(|status| status.position)
        {
            Some(status) => status.id == "local",
            None => self.translation_providers.is_empty(),
        };
        local_route
            && self.translation_worker_ready
            && self.translation_reload_on_next_use
            && !self.translation_unloading
    }

    fn translation_waits_for_local(&self) -> bool {
        match self
            .translation_providers
            .iter()
            .filter(|status| status.enabled && status.available)
            .min_by_key(|status| status.position)
        {
            Some(status) => status.id == "local" && !self.translation_ready,
            None => self.translation_providers.is_empty() && !self.translation_ready,
        }
    }

    fn latest_transcript(&self) -> Option<&Transcript> {
        self.registry
            .latest_transcript()
            .or_else(|| self.history.first().map(|entry| &entry.transcript))
            .or_else(|| {
                self.latest_history_session
                    .and_then(|session_id| self.transcripts.get(&session_id))
            })
    }

    fn request_insert_last(&mut self, target: TargetSnapshot) -> InsertLastResolution {
        if let Some(session_id) = self.registry.latest_session() {
            return match self.registry.phase(session_id) {
                Some(SessionPhase::Completed) => self
                    .registry
                    .transcript(session_id)
                    .or_else(|| self.transcripts.get(&session_id))
                    .cloned()
                    .map_or_else(
                        || self.resolve_latest_completed(target),
                        InsertLastResolution::Insert,
                    ),
                Some(SessionPhase::Listening | SessionPhase::Processing) => {
                    self.pending_insert_last = Some(PendingInsertLast {
                        session_id: Some(session_id),
                        target,
                    });
                    InsertLastResolution::Await(session_id)
                }
                Some(SessionPhase::Cancelled | SessionPhase::Failed) | None => {
                    self.resolve_latest_completed(target)
                }
            };
        }

        self.resolve_latest_completed(target)
    }

    fn resolve_latest_completed(&mut self, target: TargetSnapshot) -> InsertLastResolution {
        if let Some(transcript) = self.latest_transcript().cloned() {
            return InsertLastResolution::Insert(transcript);
        }
        if self.history_loaded {
            return InsertLastResolution::Unavailable("There is no transcript to insert yet.");
        }

        self.pending_insert_last = Some(PendingInsertLast {
            session_id: None,
            target,
        });
        InsertLastResolution::LoadHistory
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|argument| argument == "--protocol-version") {
        println!("{PROTOCOL_VERSION}");
        return Ok(());
    }
    if env::args().any(|argument| argument == "--build-id") {
        println!("{BUILD_COMMIT}");
        return Ok(());
    }
    let replay_requested = env::args().any(|argument| argument == "--screen-replay");
    #[cfg(not(all(feature = "inspect", feature = "screen-translation")))]
    if replay_requested {
        return Err(std::io::Error::other(
            "--screen-replay requires inspect and screen-translation features",
        )
        .into());
    }
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    let screen_replay = replay_requested
        .then(ScreenReplayProbe::load_from_environment)
        .transpose()
        .map_err(std::io::Error::other)?
        .map(|probe| Rc::new(RefCell::new(probe)));
    rust_platform_windows::configure_process_error_mode();
    rust_platform_windows::configure_safe_dll_search()?;
    // Before any window: the shell reads the application's identity when the
    // first one appears, and the taskbar's icon follows that identity rather
    // than the window.
    if let Err(error) = rust_platform_windows::name_this_application_to_the_shell() {
        eprintln!("{error}");
    }
    let startup_gate = Arc::new(StartupSessionGate::new());
    let session_event_router = Arc::new(SessionEventRouter::new(Arc::clone(&startup_gate)));
    let session_router_for_monitor = Arc::clone(&session_event_router);
    let _session_end_monitor = rust_platform_windows::SessionEndMonitor::start(move |event| {
        session_router_for_monitor.receive(event);
    })
    .map_err(|error| format!("Windows session-end monitoring is unavailable: {error}"))?;
    configure_ui_backend()?;
    if let Some(cycles) = indicator_presentation_probe_cycles() {
        return run_indicator_presentation_probe(cycles);
    }
    if let Some(cycles) = translation_presentation_probe_cycles() {
        return run_translation_presentation_probe(cycles);
    }
    if env::args().any(|argument| argument == "--staging-probe") {
        return preview_modes::run_staging_probe();
    }
    if let Some(style) = indicator_preview_style() {
        let live_microphone = env::args().any(|argument| {
            argument == "--indicator-preview-live"
                || argument.starts_with("--indicator-preview-live=")
        });
        return run_indicator_preview(style, live_microphone, indicator_preview_slate_surface());
    }
    if env::args().any(|argument| argument == "--action-preview") {
        return run_action_palette_preview();
    }
    if let Some(long_text) = translation_preview_mode() {
        return run_translation_preview(long_text);
    }
    if env::args().any(|argument| argument == "--layout-preview") {
        return run_layout_preview();
    }
    if let Some(download_recommended) = settings_preview::requested_first_run_state() {
        return settings_preview::run_first_run_preview(download_recommended);
    }
    if settings_preview::support_wallet_preview_requested() {
        return settings_preview::run_support_wallet_preview();
    }
    if env::args().any(|argument| {
        argument.starts_with("--settings-preview") || argument == "--settings-scroll-comparison"
    }) {
        return settings_preview::run_settings_preview();
    }
    if settings_preview::requested_translator_state().is_some() {
        return settings_preview::run_translator_preview();
    }
    if settings_preview::requested_screen_translation_preview_state().is_some() {
        return settings_preview::run_screen_translation_preview();
    }

    if let Err(error) = rust_app_core::product::migrate_legacy_data_root() {
        eprintln!("SayFlex data migration is deferred: {error}");
    }
    let _ = rust_app_core::diagnostics::initialize(&data_root().join("logs"), "desktop");
    rust_app_core::lifecycle::install_panic_hook();
    let instance_name = format!("Local\\{}", rust_app_core::product::desktop_app_id());
    let _single_instance = rust_platform_windows::SingleInstanceGuard::acquire(&instance_name)?;
    let mut run_session =
        rust_app_core::lifecycle::RunSession::begin(&data_root(), env!("CARGO_PKG_VERSION"))?;
    let settings = SettingsWindow::new()?;
    if rust_app_core::product::is_test_build() {
        settings.set_window_title("SayFlex Test".into());
    }
    settings.set_app_version(if rust_app_core::product::is_test_build() {
        concat!(env!("CARGO_PKG_VERSION"), " · NVIDIA Test").into()
    } else {
        env!("CARGO_PKG_VERSION").into()
    });
    let support = SupportWindow::new()?;
    support_action::configure_support_window(&support)?;
    let loaded_settings = load_settings();
    let mut saved_settings = loaded_settings.settings;
    #[cfg(feature = "screen-translation")]
    let screenshot_store = {
        let retention = ScreenshotRetention {
            enabled: saved_settings.screenshot_retention_enabled,
            ttl: std::time::Duration::from_secs(
                u64::from(saved_settings.screenshot_retention_hours).saturating_mul(60 * 60),
            ),
            max_bytes: saved_settings.screenshot_retention_max_bytes,
        };
        match ScreenshotStore::open_default(retention) {
            Ok(store) => {
                let store = Arc::new(store);
                let cleanup_store = Arc::clone(&store);
                let _ = std::thread::Builder::new()
                    .name("screen-snapshot-cleanup".to_owned())
                    .spawn(move || {
                        let report = cleanup_store.cleanup(unix_ms());
                        rust_app_core::diagnostics::log_line(&format!(
                            "screen_snapshot_cleanup removed_entries={} removed_orphans={} errors={}",
                            report.removed_entries, report.removed_orphans, report.error_count
                        ));
                    });
                Some(store)
            }
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_snapshot_store result=unavailable error={error}"
                ));
                None
            }
        }
    };
    let system_locale = rust_platform_windows::user_locale_name();
    slint::select_bundled_translation(saved_settings.ui_locale.resolved_code(&system_locale))?;
    if loaded_settings.needs_first_run_choice {
        let Some(choice) = first_run::request_first_run_choice()? else {
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=startup_aborted reason=first_run_not_confirmed",
            );
            run_session.finish("first_run_not_confirmed");
            return Ok(());
        };
        saved_settings = apply_first_run_choice(saved_settings, choice);
        save_first_run_settings(&saved_settings)?;
    }
    settings.set_today(
        rust_platform_windows::local_date_line_for_locale(
            active_locale(saved_settings.ui_locale).code(),
        )
        .into(),
    );
    let (panel_indicator, transparent_indicator, indicator_animation) = new_recording_indicators()?;
    let settings_preview_animation = new_settings_preview(&settings);
    let panel_indicator_presenter = OverlayPresenter::install_always_renderable(
        panel_indicator.window(),
        "recording_panel_overlay",
        OverlayContract::ClickThrough,
        None,
    )
    .map_err(|error| format!("recording overlay initialization failed: {error}"))?;
    let transparent_indicator_presenter = OverlayPresenter::install_always_renderable(
        transparent_indicator.window(),
        "recording_transparent_overlay",
        OverlayContract::ClickThrough,
        None,
    )
    .map_err(|error| format!("transparent recording overlay initialization failed: {error}"))?;
    let recording_overlays = RecordingOverlays {
        panel: RecordingOverlay {
            indicator: Rc::new(panel_indicator),
            presenter: panel_indicator_presenter,
            transparent: false,
        },
        transparent: RecordingOverlay {
            indicator: Rc::new(transparent_indicator),
            presenter: transparent_indicator_presenter,
            transparent: true,
        },
    };
    indicator_animation
        .borrow_mut()
        .set_style(saved_settings.visualizer_style);
    indicator_animation
        .borrow_mut()
        .set_flow_sphere_palette(saved_settings.flow_sphere_palette);
    indicator_animation
        .borrow_mut()
        .set_pulse_matrix_palette(saved_settings.pulse_matrix_palette);
    recording_overlays.set_style(saved_settings.visualizer_style);
    recording_overlays.set_circular_palette(saved_settings.circular_spectrum_palette);
    recording_overlays.set_broadcast_mic_palette(saved_settings.broadcast_mic_palette);
    recording_overlays.set_writing_slate_surface(saved_settings.writing_slate_surface);
    let chooser = SelectionChooser::new()?;
    let action_palette = ActionPalette::new()?;
    let translation_preview_slot =
        create_translation_preview_slot(saved_settings.translation_blur_radius)
            .map_err(|error| format!("translation preview initialization failed: {error}"))?;
    #[cfg(feature = "screen-translation")]
    let active_screen_editor = Rc::new(RefCell::new(None));
    #[cfg(feature = "screen-translation")]
    let glossary_view = Rc::new(RefCell::new(glossary_view::GlossaryViewState::default()));
    let chooser_presenter = OverlayPresenter::install_always_renderable(
        chooser.window(),
        "selection_chooser_overlay",
        OverlayContract::Interactive,
        None,
    )
    .map_err(|error| format!("selection chooser initialization failed: {error}"))?;
    let action_palette_presenter = OverlayPresenter::install_always_renderable(
        action_palette.window(),
        "action_palette_overlay",
        OverlayContract::Interactive,
        None,
    )
    .map_err(|error| format!("action palette initialization failed: {error}"))?;
    let translation_previews = Rc::new(RefCell::new(TranslationPreviewRegistry {
        current: translation_preview_slot.clone(),
        current_request: None,
        pinned: Vec::new(),
        blur_radius: saved_settings.translation_blur_radius,
    }));
    #[cfg(feature = "screen-translation")]
    let screen_result_windows = Rc::new(RefCell::new(ScreenResultWindowRegistry::default()));
    let translator = TranslatorWindow::new()?;
    let tray = AppTray::new()?;
    precreate_indicator(
        &recording_overlays.panel.indicator,
        &recording_overlays.panel.presenter,
        recording_overlays.panel.transparent,
    );
    precreate_indicator(
        &recording_overlays.transparent.indicator,
        &recording_overlays.transparent.presenter,
        recording_overlays.transparent.transparent,
    );
    precreate_action_windows(
        &chooser,
        &chooser_presenter,
        &action_palette,
        &action_palette_presenter,
    );
    precreate_translation_preview(&translation_preview_slot);

    let Some(initial_workers) =
        spawn_initial_workers(&startup_gate, saved_settings.speech_backend_preference)?
    else {
        rust_app_core::diagnostics::log_line(
            "desktop lifecycle=startup_aborted_session_end_confirmed",
        );
        run_session.finish("startup_aborted_session_end_confirmed");
        return Ok(());
    };
    let InitialWorkers {
        job: worker_job,
        input_child,
        input,
        speech_child,
        speech,
        speech_backends,
        translation_child,
        translation,
    } = initial_workers;
    let workers = Rc::new(Workers {
        _job: worker_job,
        input,
        speech,
        translation,
        #[cfg(feature = "screen-translation")]
        ocr: RefCell::new(None),
        input_child: RefCell::new(input_child),
        speech_child: RefCell::new(speech_child),
        speech_backends: RefCell::new(speech_backends),
        translation_child: RefCell::new(translation_child),
        #[cfg(feature = "screen-translation")]
        ocr_child: RefCell::new(None),
        #[cfg(feature = "screen-translation")]
        ocr_request: RefCell::new(worker_supervisor::OcrRequestOwner::default()),
        companion: RefCell::new(None),
        companion_child: RefCell::new(None),
        lifecycle: Cell::new(WorkerLifecycle::Running),
        restart_attempts: RefCell::new(HashMap::new()),
        deferred_exits: RefCell::new(HashMap::new()),
    });
    let selected_model_id = saved_settings
        .selected_model_id
        .clone()
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let openai_route_enabled = saved_settings
        .transcription_providers
        .iter()
        .any(|provider| provider.enabled && matches!(provider.id.as_str(), "groq" | "custom"));
    let output_mode = if selected_model_id == DEFAULT_MODEL
        && saved_settings.output_mode == SpeechOutputMode::TranslateToEnglish
        && !openai_route_enabled
    {
        SpeechOutputMode::OriginalLanguage
    } else {
        saved_settings.output_mode
    };
    let state = Rc::new(RefCell::new(AppState::new(
        selected_model_id.clone(),
        output_mode,
        workers.active_speech_backend(),
        &saved_settings,
    )));
    if saved_settings.visualizer_style.uses_external_renderer()
        && let Err(error) = workers.ensure_companion().and_then(|()| {
            workers.send_companion(&CompanionRendererCommand::SetEnabled { enabled: true })
        })
    {
        rust_app_core::diagnostics::log_line(&format!(
            "companion_renderer lifecycle=start_failed error={error}"
        ));
    }
    sync_output_mode(&settings.as_weak(), &state);
    sync_action_settings(&settings.as_weak(), &state);

    workers.speech.send(&SpeechWorkerCommand::Initialize {
        selected_model_id,
        output_mode,
        dictation_language: saved_settings.dictation_language,
        history_enabled: saved_settings.history_enabled,
        fast_release_previews: saved_settings.fast_release_previews,
        prepare_local_model: state.borrow().local_speech_enabled(),
    })?;
    // An empty list means the user has never arranged one; the worker keeps
    // its own default of the local engine first and every service switched off.
    if !saved_settings.transcription_providers.is_empty() {
        workers
            .speech
            .send(&SpeechWorkerCommand::SetTranscriptionProviders {
                providers: saved_settings.transcription_providers.clone(),
            })?;
    }
    workers
        .speech
        .send(&SpeechWorkerCommand::GetTranscriptionProviders)?;
    workers
        .translation
        .send(&TranslationWorkerCommand::Initialize {
            selected_catalog_id: saved_settings.selected_translation_model_id.clone(),
            selected_tier: saved_settings.selected_translation_tier,
            keep_warm: saved_settings.translation_keep_warm,
            history_enabled: saved_settings.history_enabled,
            prepare_local_model: state.borrow().local_translation_enabled(),
        })?;
    if !saved_settings.translation_providers.is_empty() {
        workers
            .translation
            .send(&TranslationWorkerCommand::SetTranslationProviders {
                providers: saved_settings.translation_providers.clone(),
            })?;
    }
    workers
        .translation
        .send(&TranslationWorkerCommand::GetTranslationProviders)?;

    // A key save that never gets an answer is released with a reason rather
    // than left holding what the user typed. Checked on a slow timer, because
    // the deadline is ten seconds and the map is almost always empty.
    let credential_watch = Timer::default();
    {
        let settings = settings.as_weak();
        let state = Rc::clone(&state);
        let workers = Rc::clone(&workers);
        credential_watch.start(
            TimerMode::Repeated,
            std::time::Duration::from_secs(2),
            move || {
                providers_view::expire_pending_credentials(
                    &settings,
                    &state,
                    &workers,
                    "no answer in time",
                );
            },
        );
    }

    {
        let workers = Rc::clone(&workers);
        support_action::wire_support_with(&settings, &support, move |address| {
            let _ = workers.input.send(&InputHostCommand::CopyText {
                request_id: RequestId::new(),
                text: address.to_owned(),
            });
        });
    }
    wire_settings(
        &settings,
        &translator,
        &translation_previews,
        &state,
        &workers,
        &recording_overlays,
        &indicator_animation,
    );
    let _cuda_setup = {
        let settings_weak = settings.as_weak();
        let activation_state = Rc::clone(&state);
        let activation_workers = Rc::clone(&workers);
        cuda_setup::wire_cuda_setup(&settings, Rc::clone(&state), move || {
            cuda_activation::activate(&settings_weak, &activation_state, &activation_workers)
        })
    };
    #[cfg(feature = "screen-translation")]
    {
        settings_view::wire_contextual_translation_settings(&settings);
        settings.set_screenshot_retention_enabled(saved_settings.screenshot_retention_enabled);
        settings_view::sync_glossary(&settings.as_weak(), &glossary_view.borrow());
    }
    wire_tray(&tray, &settings, &translator, &state, &workers);
    wire_chooser(&chooser, &chooser_presenter, &state, &workers);
    wire_action_palette(
        &action_palette,
        &action_palette_presenter,
        &translation_previews,
        &chooser,
        &chooser_presenter,
        &settings,
        &state,
        &workers,
    );
    wire_translator(&translator, &state, &workers);
    let screen_selection = Rc::new(RefCell::new(ScreenSelectionCoordinator::new(
        DesktopSelectionEnvironment::new(Rc::clone(&workers)),
    )));
    let screen_translation = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
    let resource_timer =
        resource_view::wire_resources(&settings, &state, &workers, &screen_translation);
    #[cfg(feature = "screen-translation")]
    let screenshot_cleanup_timer = {
        let timer = Timer::default();
        let cleanup_store = screenshot_store.clone();
        let cleanup_screen_translation = Rc::clone(&screen_translation);
        let cleanup_running = Arc::new(AtomicBool::new(false));
        timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_secs(15 * 60),
            move || {
                let now = unix_ms();
                expire_screen_translation_pixels(
                    &cleanup_screen_translation,
                    now,
                    |request_id| {
                        dispatch_runtime_event(RuntimeEvent::ScreenPixelsExpired { request_id });
                    },
                );
                let Some(store) = cleanup_store.as_ref().map(Arc::clone) else {
                    return;
                };
                if cleanup_running.swap(true, Ordering::AcqRel) {
                    return;
                }
                let running_for_thread = Arc::clone(&cleanup_running);
                if std::thread::Builder::new()
                    .name("screen-snapshot-cleanup".to_owned())
                    .spawn(move || {
                        let report = store.cleanup(now);
                        rust_app_core::diagnostics::log_line(&format!(
                            "screen_snapshot_cleanup removed_entries={} removed_orphans={} errors={}",
                            report.removed_entries, report.removed_orphans, report.error_count
                        ));
                        for request_id in report.removed_request_ids {
                            dispatch_runtime_event(RuntimeEvent::ScreenPixelsExpired { request_id });
                        }
                        running_for_thread.store(false, Ordering::Release);
                    })
                    .is_err()
                {
                    cleanup_running.store(false, Ordering::Release);
                }
            },
        );
        timer
    };
    #[cfg(feature = "screen-translation")]
    let screen_capture_backend =
        Arc::new(rust_platform_windows::screen_capture::WindowsDesktopCaptureBackend::default());
    let insertion_notice = insertion_notice::InsertionNoticeController::new(&workers)?;
    install_runtime_handler(RuntimeUi {
        settings: settings.as_weak(),
        recording_overlays: recording_overlays.clone(),
        chooser: chooser.as_weak(),
        chooser_presenter: chooser_presenter.clone(),
        action_palette: action_palette.as_weak(),
        action_palette_presenter: action_palette_presenter.clone(),
        translation_previews: Rc::clone(&translation_previews),
        #[cfg(feature = "screen-translation")]
        screen_result_windows: Rc::clone(&screen_result_windows),
        translator: translator.as_weak(),
        state: Rc::clone(&state),
        workers: Rc::clone(&workers),
        indicator_animation: Rc::clone(&indicator_animation),
        insertion_notice,
        screen_selection,
        screen_translation,
        #[cfg(feature = "screen-translation")]
        active_screen_editor,
        #[cfg(feature = "screen-translation")]
        glossary_view: Rc::clone(&glossary_view),
        #[cfg(feature = "screen-translation")]
        screen_capture_backend,
        #[cfg(feature = "screen-translation")]
        screenshot_store: {
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if screen_replay.is_some() {
                None
            } else {
                screenshot_store.clone()
            }
            #[cfg(not(all(feature = "inspect", feature = "screen-translation")))]
            screenshot_store.clone()
        },
        #[cfg(all(feature = "inspect", feature = "screen-translation"))]
        screen_replay: screen_replay.clone(),
    });
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    let screen_replay_timer = screen_replay.as_ref().map(|_| {
        let timer = Timer::default();
        timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            || dispatch_runtime_event(RuntimeEvent::ScreenReplayTick),
        );
        timer
    });
    #[cfg(feature = "screen-translation")]
    {
        let operation_id = glossary_view.borrow_mut().begin_list(None);
        if let Err(error) = workers
            .translation
            .send(&TranslationWorkerCommand::ListGlossary {
                operation_id,
                query: None,
                limit: 500,
            })
        {
            rust_app_core::diagnostics::log_line(&format!(
                "glossary list=start_failed error={error}"
            ));
        }
    }
    session_event_router.attach_runtime();
    let _popup_dismiss_monitor = match rust_platform_windows::PopupDismissMonitor::start(
        |window_handle| {
            dispatch_runtime_event(RuntimeEvent::DismissTranslationPreview(window_handle));
        },
        |window_handle, pinned| {
            dispatch_runtime_event(RuntimeEvent::TranslationPreviewPinChanged {
                window_handle,
                pinned,
            });
        },
        |window_handle, delta| {
            dispatch_runtime_event(RuntimeEvent::ScrollTranslationPreview {
                window_handle,
                delta,
            });
        },
    ) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            eprintln!("translation popup click-away is unavailable: {error}");
            None
        }
    };
    let animation_timer = start_indicator_animation_with_settings_preview(
        vec![
            recording_overlays.panel.indicator.as_weak(),
            recording_overlays.transparent.indicator.as_weak(),
        ],
        &indicator_animation,
        settings.as_weak(),
        &settings_preview_animation,
    );

    if !env::args().any(|argument| argument == "--background") {
        settings.show()?;
        configure_standard_app_window(&settings, "Settings");
        visualizer::wake_animation();
    }
    settings.on_animation_wanted(visualizer::wake_animation);
    // A second launch (the shortcut, the Start menu) shows these settings.
    let _activation = {
        let settings = settings.as_weak();
        rust_platform_windows::listen_for_activation(&instance_name, move || {
            let settings = settings.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(settings) = settings.upgrade() else {
                    return;
                };
                rust_app_core::diagnostics::log_line(
                    "desktop lifecycle=activated_by_second_launch",
                );
                let _ = settings.show();
                configure_standard_app_window(&settings, "Settings");
                visualizer::wake_animation();
                if let Some(handle) = slint_window_handle(settings.window()) {
                    rust_platform_windows::activate_window(handle);
                }
            });
        })
        .map_err(|error| {
            rust_app_core::diagnostics::log_line(&format!(
                "desktop lifecycle=activation_listener_failed error={error}"
            ));
        })
        .ok()
    };
    tray.show()?;
    slint::run_event_loop()?;
    #[cfg(feature = "screen-translation")]
    screen_result_windows.borrow_mut().retire_all("shutdown");
    animation_timer.stop();
    resource_timer.stop();
    #[cfg(feature = "screen-translation")]
    screenshot_cleanup_timer.stop();
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    if let Some(timer) = screen_replay_timer.as_ref() {
        timer.stop();
    }
    UI_EVENT_HANDLER.with(|handler| {
        handler.borrow_mut().take();
    });
    workers.shutdown();
    run_session.finish("event_loop_returned");
    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
    if let Some(replay) = screen_replay {
        match replay.borrow_mut().final_result.take() {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(std::io::Error::other(error).into()),
            None => {
                return Err(std::io::Error::other("screen replay exited without a receipt").into());
            }
        }
    }
    Ok(())
}

fn configure_ui_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()
}

fn dispatch_runtime_event(event: RuntimeEvent) {
    if let Err(error) = slint::invoke_from_event_loop(move || {
        UI_EVENT_HANDLER.with(|handler| {
            if let Some(handler) = handler.borrow_mut().as_mut() {
                handler(event);
            }
        });
    }) {
        eprintln!("could not dispatch worker event to UI: {error}");
    }
}

fn install_runtime_handler(runtime: RuntimeUi) {
    UI_EVENT_HANDLER.with(|handler| {
        *handler.borrow_mut() = Some(Box::new(move |event| {
            handle_runtime_event(event, &runtime);
        }));
    });
}

#[cfg(all(feature = "inspect", feature = "screen-translation"))]
fn drive_screen_replay(runtime: &RuntimeUi) {
    enum Action {
        Start(RequestId),
        Dismiss(u64),
        OpenStored,
    }
    let Some(probe) = runtime.screen_replay.as_ref() else {
        return;
    };
    let mut action = None;
    {
        let mut probe = probe.borrow_mut();
        if probe.final_result.is_some() {
            return;
        }
        if probe.started.elapsed() > std::time::Duration::from_secs(600) {
            probe.finish(Err("replay exceeded its 10-minute deadline".into()));
            return;
        }
        let Some(step) = probe.step() else {
            probe.finish(Ok(()));
            return;
        };
        let step_number = probe.index + 1;
        match probe.phase {
            ReplayPhase::Ready => {
                let (ready, provider, selected_model, selected_tier, local_loaded) = {
                    let state = runtime.state.borrow();
                    let provider = state
                        .translation_providers
                        .iter()
                        .filter(|status| status.enabled && status.available)
                        .min_by_key(|status| status.position)
                        .map(|status| status.id.clone())
                        .unwrap_or_else(|| "local".to_owned());
                    (
                        state.translation_path_ready(),
                        provider,
                        state.selected_translation_model_id.clone(),
                        state.selected_translation_tier,
                        state.translation_ready,
                    )
                };
                if !ready {
                    return;
                }
                let loaded_model = probe.loaded_model_id.clone();
                if provider != "local"
                    || selected_model.is_none()
                    || !local_loaded
                    || loaded_model != selected_model
                {
                    probe.finish(Err(format!(
                        "replay requires selected, loaded local model; provider={provider} model={selected_model:?} loaded_model={loaded_model:?} loaded={local_loaded}"
                    )));
                    return;
                }
                if probe.index == 0 {
                    probe.record(format!("provider={provider} selected_model={selected_model:?} loaded_model={loaded_model:?} tier={selected_tier:?}"));
                }
                let request_id = RequestId::new();
                probe.request_id = Some(request_id);
                probe.completed = false;
                probe.translation_dispatched = false;
                probe.cancel_ack = false;
                probe.failure_kind = None;
                probe.failure = None;
                probe.phase = ReplayPhase::Awaiting(Instant::now());
                probe.record(format!(
                    "step={step_number} kind={step:?} request={request_id} started"
                ));
                action = Some(Action::Start(request_id));
            }
            ReplayPhase::Awaiting(since) => {
                let request_id = probe.request_id.expect("active replay request");
                if since.elapsed() > std::time::Duration::from_secs(120) {
                    probe.finish(Err(format!(
                        "step={step_number} request={request_id} timed out"
                    )));
                    return;
                }
                match step {
                    ReplayStep::Normal => {
                        if let Some(failure) = probe.failure.as_ref() {
                            let failure = failure.clone();
                            probe.finish(Err(format!(
                                "normal request={request_id} failed: {failure}"
                            )));
                            return;
                        }
                        let slot = runtime
                            .screen_result_windows
                            .borrow()
                            .slot_for_request(request_id);
                        if probe.completed
                            && probe.translation_dispatched
                            && let Some(slot) = slot
                            && slot.window.get_prepared()
                            && slot.handle() != 0
                        {
                            probe.record(format!(
                                "step={step_number} request={request_id} result_presented hwnd={}",
                                slot.handle()
                            ));
                            probe.phase = ReplayPhase::HoldingResult {
                                since: Instant::now(),
                                handle: slot.handle(),
                            };
                        }
                    }
                    ReplayStep::SelectionTimeout => {
                        let slot = runtime
                            .translation_previews
                            .borrow()
                            .slot_for_request(request_id);
                        if let Some(slot) = slot
                            && slot.preview.get_prepared()
                            && slint_window_handle(slot.preview.window())
                                .is_some_and(|handle| slot.presenter.is_presented_at(handle))
                        {
                            probe.record(format!("step={step_number} request={request_id} selection_timeout_presented"));
                            probe.phase = ReplayPhase::RestoredAfterTimeout(Instant::now());
                            action = Some(Action::OpenStored);
                        }
                    }
                    ReplayStep::WorkerFailure => {
                        if probe.completed {
                            probe.finish(Err(format!(
                                "worker-failure request={request_id} unexpectedly completed"
                            )));
                            return;
                        }
                        let slot = runtime
                            .translation_previews
                            .borrow()
                            .slot_for_request(request_id);
                        if probe.translation_dispatched
                            && probe.failure_kind
                                == Some(TranslationFailureKind::UnsupportedRequest)
                            && let Some(slot) = slot
                            && slot.preview.get_prepared()
                        {
                            let handle = slint_window_handle(slot.preview.window()).unwrap_or(0);
                            if handle != 0 && slot.presenter.is_presented_at(handle) {
                                probe.record(format!("step={step_number} request={request_id} failure_presented hwnd={handle}"));
                                probe.phase = ReplayPhase::Closing(Instant::now());
                                action = Some(Action::Dismiss(handle));
                            }
                        }
                    }
                    ReplayStep::CancelAfterDispatch => {
                        if probe.completed {
                            probe.finish(Err(format!(
                                "cancel request={request_id} completed before dismissal"
                            )));
                            return;
                        }
                        let slot = runtime
                            .translation_previews
                            .borrow()
                            .slot_for_request(request_id);
                        if let Some(failure) = probe.failure.clone() {
                            probe.finish(Err(format!(
                                "cancel request={request_id} failed before dismissal: {failure}"
                            )));
                            return;
                        }
                        if probe.translation_dispatched
                            && let Some(slot) = slot
                        {
                            let handle = slint_window_handle(slot.preview.window()).unwrap_or(0);
                            if handle != 0 {
                                probe.record(format!("step={step_number} request={request_id} dispatched_cancel hwnd={handle}"));
                                probe.phase = ReplayPhase::CancelClosing(Instant::now());
                                action = Some(Action::Dismiss(handle));
                            }
                        }
                    }
                }
            }
            ReplayPhase::RestoredAfterTimeout(since) => {
                let compact_active = runtime
                    .translation_previews
                    .borrow()
                    .current()
                    .presenter
                    .has_active_presentation();
                let restored = runtime
                    .screen_result_windows
                    .borrow()
                    .slot_for_request(probe.restored_request_id.expect("stored replay result"));
                if !compact_active
                    && let Some(restored) = restored
                    && restored.window.get_prepared()
                    && restored.handle() != 0
                {
                    let handle = restored.handle();
                    probe.record(format!("step={step_number} restored_result_presented hwnd={handle} compact_retired=true"));
                    probe.phase = ReplayPhase::HoldingResult {
                        since: Instant::now(),
                        handle,
                    };
                } else if since.elapsed() > std::time::Duration::from_secs(15) {
                    probe.finish(Err(
                        "stored result did not replace the stale compact preview".into(),
                    ));
                    return;
                }
            }
            ReplayPhase::HoldingResult { since, handle } => {
                let first_result_hold_ms = std::env::var("SAYFLEX_SCREEN_REPLAY_FIRST_HOLD_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(4_000)
                    .clamp(1_000, 120_000);
                let hold_ms = if probe.index == 0 {
                    first_result_hold_ms
                } else {
                    4_000
                };
                if since.elapsed() >= std::time::Duration::from_millis(hold_ms) {
                    probe.phase = ReplayPhase::Closing(Instant::now());
                    action = Some(Action::Dismiss(handle));
                }
            }
            ReplayPhase::Closing(since) | ReplayPhase::CancelClosing(since) => {
                let request_id = probe.request_id.expect("closing replay request");
                if step == ReplayStep::CancelAfterDispatch {
                    if probe.completed || probe.failure.is_some() {
                        probe.finish(Err(format!(
                            "cancel request={request_id} received late completion or failure"
                        )));
                        return;
                    }
                    if since.elapsed() > std::time::Duration::from_secs(15) && !probe.cancel_ack {
                        probe.finish(Err(format!("cancel request={request_id} received no matching worker Cancelled acknowledgment")));
                        return;
                    }
                }
                let result_request = if step == ReplayStep::SelectionTimeout {
                    probe.restored_request_id.expect("stored replay result")
                } else {
                    request_id
                };
                let result_gone = runtime
                    .screen_result_windows
                    .borrow()
                    .slot_for_request(result_request)
                    .is_none();
                let document_gone = runtime
                    .screen_translation
                    .borrow()
                    .result_snapshot(request_id)
                    .is_err()
                    && (result_request == request_id
                        || runtime
                            .screen_translation
                            .borrow()
                            .result_snapshot(result_request)
                            .is_err());
                let compact_gone = runtime
                    .translation_previews
                    .borrow()
                    .slot_for_request(request_id)
                    .is_none();
                if result_gone
                    && document_gone
                    && compact_gone
                    && since.elapsed() >= std::time::Duration::from_secs(1)
                    && (step != ReplayStep::CancelAfterDispatch || probe.cancel_ack)
                {
                    probe.record(format!("step={step_number} request={request_id} closed"));
                    probe.index += 1;
                    probe.request_id = None;
                    probe.phase = ReplayPhase::Ready;
                } else if since.elapsed() > std::time::Duration::from_secs(15) {
                    probe.finish(Err(format!("step={step_number} request={request_id} retained a result, document, or compact preview")));
                    return;
                }
            }
        }
    }
    match action {
        Some(Action::Start(request_id)) => handle_input_event(
            InputHostEvent::ScreenCaptureRequested { request_id },
            runtime,
        ),
        Some(Action::Dismiss(handle)) => {
            if probe.borrow().step() == Some(ReplayStep::CancelAfterDispatch) {
                let request_id = probe.borrow().request_id;
                if let Some(request_id) = request_id {
                    probe.borrow_mut().begin_cancel(request_id);
                }
            }
            handle_runtime_event(RuntimeEvent::DismissTranslationPreview(handle), runtime)
        }
        Some(Action::OpenStored) => {
            let restored = ScreenshotStore::open_default(ScreenshotRetention::default())
                .and_then(|store| store.load_latest_result(unix_ms()))
                .map_err(|error| error.to_string());
            match restored {
                Ok(Some(result)) => {
                    let restored_request = result.0.request_id();
                    probe.borrow_mut().restored_request_id = Some(restored_request);
                    finish_open_latest_screen_translation(Ok(Some(result)), runtime)
                }
                Ok(None) => probe
                    .borrow_mut()
                    .finish(Err("stored completed result is unavailable".into())),
                Err(error) => probe.borrow_mut().finish(Err(format!(
                    "could not load stored completed result: {error}"
                ))),
            }
        }
        None => {}
    }
}

#[cfg(feature = "screen-translation")]
fn sync_glossary_settings(runtime: &RuntimeUi) {
    settings_view::sync_glossary(&runtime.settings, &runtime.glossary_view.borrow());
}

#[cfg(feature = "screen-translation")]
fn request_glossary_list(query: String, runtime: &RuntimeUi) {
    let query = query.trim().to_owned();
    let operation_id = runtime
        .glossary_view
        .borrow_mut()
        .begin_list((!query.is_empty()).then(|| query.clone()));
    if let Err(error) = runtime
        .workers
        .translation
        .send(&TranslationWorkerCommand::ListGlossary {
            operation_id,
            query: (!query.is_empty()).then_some(query),
            limit: 500,
        })
    {
        set_status_problem(&runtime.settings, &error.to_string());
    }
}

#[cfg(feature = "screen-translation")]
fn update_glossary_entry(
    entry_id: String,
    source: String,
    translation: String,
    runtime: &RuntimeUi,
) {
    let source = source.trim().to_owned();
    let translation = translation.trim().to_owned();
    if source.is_empty() || translation.is_empty() {
        set_status_problem(
            &runtime.settings,
            "Glossary source and translation cannot be empty.",
        );
        return;
    }
    let entry_id = rust_protocol::GlossaryEntryId::from(entry_id);
    let Some(existing) = runtime
        .glossary_view
        .borrow()
        .entries()
        .iter()
        .find(|entry| entry.entry_id == entry_id)
        .cloned()
    else {
        set_status_problem(
            &runtime.settings,
            "This glossary entry is no longer available.",
        );
        return;
    };
    let draft = GlossaryEntryDraft {
        source_phrase: source,
        approved_translation: translation,
        source_language: existing.source_language,
        target_language: existing.target_language,
        example: existing.examples.first().cloned().unwrap_or_default(),
        context: existing.context,
        origin: existing.origin,
    };
    send_glossary_action(
        glossary_view::GlossaryAction::Update {
            entry_id,
            draft: Box::new(draft),
        },
        runtime,
    );
}

#[cfg(feature = "screen-translation")]
fn delete_glossary_entry(entry_id: String, runtime: &RuntimeUi) {
    send_glossary_action(
        glossary_view::GlossaryAction::Delete(rust_protocol::GlossaryEntryId::from(entry_id)),
        runtime,
    );
}

#[cfg(feature = "screen-translation")]
fn retry_glossary_action(runtime: &RuntimeUi) {
    let Some(pending) = runtime.glossary_view.borrow_mut().retry() else {
        return;
    };
    dispatch_glossary_action(pending.operation_id, pending.action, runtime);
}

#[cfg(feature = "screen-translation")]
fn send_glossary_action(action: glossary_view::GlossaryAction, runtime: &RuntimeUi) {
    let operation_id = runtime
        .glossary_view
        .borrow_mut()
        .begin_action(action.clone());
    dispatch_glossary_action(operation_id, action, runtime);
}

#[cfg(feature = "screen-translation")]
fn dispatch_glossary_action(
    operation_id: RequestId,
    action: glossary_view::GlossaryAction,
    runtime: &RuntimeUi,
) {
    let command = match action {
        glossary_view::GlossaryAction::Update { entry_id, draft } => {
            TranslationWorkerCommand::UpdateGlossaryEntry {
                operation_id,
                entry_id,
                entry: *draft,
            }
        }
        glossary_view::GlossaryAction::Delete(entry_id) => {
            TranslationWorkerCommand::DeleteGlossaryEntry {
                operation_id,
                entry_id,
            }
        }
    };
    if let Err(error) = runtime.workers.translation.send(&command) {
        runtime
            .glossary_view
            .borrow_mut()
            .apply_failure(operation_id, error.to_string(), true);
        sync_glossary_settings(runtime);
    }
}

#[cfg(feature = "screen-translation")]
fn update_screenshot_retention(enabled: bool, runtime: &RuntimeUi) {
    let retention = {
        let mut state = runtime.state.borrow_mut();
        state.screenshot_retention_enabled = enabled;
        let _ = save_settings(&state);
        ScreenshotRetention {
            enabled,
            ttl: std::time::Duration::from_secs(
                u64::from(state.screenshot_retention_hours).saturating_mul(60 * 60),
            ),
            max_bytes: state.screenshot_retention_max_bytes,
        }
    };
    if let Some(store) = &runtime.screenshot_store {
        store.set_retention(retention);
    }
    if let Some(settings) = runtime.settings.upgrade() {
        settings.set_screenshot_retention_enabled(enabled);
    }
}

#[cfg(feature = "screen-translation")]
fn delete_stored_screenshots(runtime: &RuntimeUi) {
    let Some(store) = runtime.screenshot_store.as_ref().map(Arc::clone) else {
        dispatch_runtime_event(RuntimeEvent::StoredScreenshotsDeleted(Ok(0)));
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("screen-snapshot-delete-all".to_owned())
        .spawn(move || {
            dispatch_runtime_event(RuntimeEvent::StoredScreenshotsDeleted(
                store.delete_all().map_err(|error| error.to_string()),
            ));
        })
    {
        dispatch_runtime_event(RuntimeEvent::StoredScreenshotsDeleted(Err(
            error.to_string()
        )));
    }
}

#[cfg(feature = "screen-translation")]
fn finish_delete_stored_screenshots(result: Result<usize, String>, runtime: &RuntimeUi) {
    match result {
        Ok(removed) => {
            expire_screen_translation_pixels(&runtime.screen_translation, u64::MAX, |request_id| {
                invalidate_screen_result_pixels(request_id, runtime)
            });
            rust_app_core::diagnostics::log_line(&format!(
                "screen_snapshot_delete_all removed={removed}"
            ));
        }
        Err(error) => set_status_problem(&runtime.settings, &error),
    }
}

fn start_indicator_animation(
    indicators: Vec<slint::Weak<RecordingIndicator>>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) -> Rc<Timer> {
    start_indicator_animation_with_optional_settings_preview(indicators, indicator_animation, None)
}

fn start_indicator_animation_with_settings_preview(
    indicators: Vec<slint::Weak<RecordingIndicator>>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
    settings: slint::Weak<SettingsWindow>,
    settings_preview_animation: &Rc<RefCell<IndicatorAnimation>>,
) -> Rc<Timer> {
    start_indicator_animation_with_optional_settings_preview(
        indicators,
        indicator_animation,
        Some((settings, Rc::clone(settings_preview_animation))),
    )
}

fn start_indicator_animation_with_optional_settings_preview(
    indicators: Vec<slint::Weak<RecordingIndicator>>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
    settings_preview: Option<(slint::Weak<SettingsWindow>, Rc<RefCell<IndicatorAnimation>>)>,
) -> Rc<Timer> {
    let timer = Rc::new(Timer::default());
    let indicator_animation = Rc::clone(indicator_animation);
    // One buffer, refilled. This ran sixty times a second on the same thread
    // that draws the interface and handles its events, and allocated two
    // vectors every time round: one for the upgraded handles and one holding
    // references into the first.
    let live = RefCell::new(Vec::with_capacity(indicators.len()));
    let sleeping_timer = Rc::downgrade(&timer);
    timer.start(TimerMode::Repeated, INDICATOR_FRAME_INTERVAL, move || {
        let mut live = live.borrow_mut();
        live.clear();
        live.extend(indicators.iter().filter_map(slint::Weak::upgrade));
        indicator_animation.borrow_mut().tick(&live);
        let mut preview_hidden = true;
        if let Some((settings, settings_preview_animation)) = &settings_preview
            && let Some(settings) = settings.upgrade()
        {
            let mut preview = settings_preview_animation.borrow_mut();
            preview.tick_settings_preview(&settings);
            preview_hidden = preview.is_hidden();
        }
        // Nothing on screen moves: sleep until a recording starts or the
        // visualizer page is shown (`visualizer::wake_animation`).
        if preview_hidden
            && indicator_animation.borrow().is_hidden()
            && let Some(timer) = sleeping_timer.upgrade()
        {
            timer.stop();
        }
    });
    let waking_timer = Rc::downgrade(&timer);
    visualizer::set_animation_waker(Rc::new(move || {
        if let Some(timer) = waking_timer.upgrade()
            && !timer.running()
        {
            timer.restart();
        }
    }));
    timer
}

fn handle_runtime_event(event: RuntimeEvent, runtime: &RuntimeUi) {
    let Some(event) = event.filter_current_worker(|kind, instance_id| {
        runtime.workers.is_current_instance(kind, instance_id)
    }) else {
        return;
    };
    match event {
        RuntimeEvent::Input { event, .. } => {
            handle_input_event(event, runtime);
        }
        #[cfg(all(feature = "inspect", feature = "screen-translation"))]
        RuntimeEvent::ScreenReplayTick => drive_screen_replay(runtime),
        RuntimeEvent::ScreenSelectionPointer(event) => {
            let terminal = runtime.screen_selection.borrow_mut().handle_pointer(event);
            if let Some(terminal) = terminal {
                handle_screen_selection_terminal(terminal, runtime);
            }
        }
        RuntimeEvent::ScreenSelectionWatchdog { request_id } => {
            let terminal = runtime
                .screen_selection
                .borrow_mut()
                .watchdog_expired(request_id);
            if let Some(terminal) = terminal {
                handle_screen_selection_terminal(terminal, runtime);
            }
        }
        RuntimeEvent::ScreenSelectionSurfacesReady { request_id } => {
            let terminal = runtime
                .screen_selection
                .borrow_mut()
                .surfaces_ready(request_id);
            if let Some(terminal) = terminal {
                handle_screen_selection_terminal(terminal, runtime);
            }
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenCapturePrepared {
            request_id,
            excluded_overlays,
            result,
        } => {
            runtime.screen_capture_backend.expire_reuse_frames();
            let capture_backend = Arc::clone(&runtime.screen_capture_backend);
            Timer::single_shot(
                rust_platform_windows::screen_capture::CAPTURE_REUSE_TTL
                    + std::time::Duration::from_millis(10),
                move || capture_backend.expire_reuse_frames(),
            );
            for surface in excluded_overlays {
                if let Err(error) = surface.restore() {
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=capture_exclusion_restore result=failed error={error}"
                    ));
                }
            }
            if !runtime.screen_translation.borrow().is_selecting(request_id) {
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation request={request_id} stage=capture result=stale_ignored"
                ));
                return;
            }
            let present_started = Instant::now();
            let terminal = match result {
                Ok(prepared) => {
                    if let Some(inventory) = prepared.inventory {
                        runtime
                            .screen_translation
                            .borrow_mut()
                            .store_inventory(request_id, inventory);
                    }
                    runtime
                        .screen_selection
                        .borrow_mut()
                        .begin_with_capture(request_id, prepared.capture)
                }
                Err(error) => Some(SelectionTerminal::Failed {
                    request_id,
                    error: SelectionFailure::Capture(error),
                }),
            };
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=overlay_present duration_ms={}",
                present_started.elapsed().as_millis()
            ));
            if let Some(terminal) = terminal {
                handle_screen_selection_terminal(terminal, runtime);
            }
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenOcrPrepared {
            request_id,
            rect,
            result,
        } => handle_prepared_screen_ocr(request_id, rect, result, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::OcrWatchdog {
            request_id,
            instance_id,
        } => match runtime
            .workers
            .terminate_stalled_ocr(request_id, instance_id)
        {
            Ok(true) => {
                runtime
                    .screen_translation
                    .borrow_mut()
                    .discard_pending(request_id);
                if let Some(anchor) = runtime.screen_translation.borrow_mut().complete(request_id) {
                    show_screen_translation_outcome(
                        request_id,
                        Some(anchor),
                        ScreenTranslationOutcome::WorkerFailed,
                        runtime,
                    );
                }
                rust_app_core::diagnostics::log_line(&format!(
                    "ocr_worker request={request_id} outcome=failed kind=timeout action=terminated"
                ));
            }
            Ok(false) => {}
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "ocr_worker request={request_id} outcome=failed kind=timeout action=terminate_failed error={error}"
                ));
                Timer::single_shot(std::time::Duration::from_secs(2), move || {
                    dispatch_runtime_event(RuntimeEvent::OcrWatchdog {
                        request_id,
                        instance_id,
                    });
                });
            }
        },
        RuntimeEvent::Speech { event, .. } => {
            handle_speech_event(
                event,
                &runtime.settings,
                &runtime.recording_overlays,
                &runtime.state,
                &runtime.workers,
                &runtime.indicator_animation,
            );
        }
        RuntimeEvent::SpeechAudioLevel { instance_id } => {
            let Some(frame) = take_audio_level() else {
                return;
            };
            if !runtime
                .workers
                .is_current_instance(WorkerKind::Speech, frame.instance_id)
                || frame.instance_id != instance_id
            {
                return;
            }
            apply_audio_level(
                frame.session_id,
                frame.peak,
                frame.spectrum,
                &runtime.state,
                &runtime.workers,
                &runtime.indicator_animation,
            );
        }
        RuntimeEvent::Translation { event, .. } => {
            handle_translation_event(event, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::Ocr { instance_id, event } => {
            handle_ocr_event(instance_id, event, runtime);
        }
        RuntimeEvent::Companion { event, .. } => match event {
            CompanionRendererEvent::Ready { protocol_version } => {
                if protocol_version == PROTOCOL_VERSION {
                    rust_app_core::diagnostics::log_line(
                        "companion_renderer lifecycle=ready protocol=matched",
                    );
                    let home_position = runtime.state.borrow().companion_home_position;
                    if let Some((x, y)) = home_position
                        && let Err(error) = runtime
                            .workers
                            .send_companion(&CompanionRendererCommand::SetHomePosition { x, y })
                    {
                        rust_app_core::diagnostics::log_line(&format!(
                            "companion_renderer home_position=restore_failed error={error}"
                        ));
                    }
                } else {
                    set_indicator_status(
                        &runtime.settings,
                        state_text(&runtime.state, UiText::CompanionProtocolMismatch),
                    );
                }
            }
            CompanionRendererEvent::HomePositionChanged { x, y } => {
                let mut state = runtime.state.borrow_mut();
                state.companion_home_position = Some((x, y));
                match save_settings(&state) {
                    Ok(()) => rust_app_core::diagnostics::log_line(&format!(
                        "companion_renderer home_position=saved x={x} y={y}"
                    )),
                    Err(error) => rust_app_core::diagnostics::log_line(&format!(
                        "companion_renderer home_position=save_failed error={error}"
                    )),
                }
            }
            CompanionRendererEvent::Failed {
                session_id,
                code,
                message,
            } => {
                rust_app_core::diagnostics::log_line(&format!(
                    "companion_renderer result=failed session={session_id:?} code={code} message={message}"
                ));
                set_indicator_status(
                    &runtime.settings,
                    &state_message(
                        &runtime.state,
                        UiText::CompanionOrbStatus,
                        &[("message", &message)],
                    ),
                );
            }
        },
        RuntimeEvent::DismissTranslationPreview(window_handle) => {
            #[cfg(feature = "screen-translation")]
            if let Some(request_id) = retire_screen_result_slot_by_handle(
                &runtime.screen_result_windows,
                window_handle,
                || {
                    let active_host = runtime
                        .active_screen_editor
                        .borrow()
                        .as_ref()
                        .map(|active| active.host_window_handle);
                    if host_editor_should_close(active_host, window_handle) {
                        close_screen_translation_editor(runtime, false);
                    }
                },
            ) {
                close_screen_translation_request(
                    &runtime.screen_translation,
                    request_id,
                    |cancellation| apply_screen_translation_cancellation(cancellation, runtime),
                );
                return;
            }
            let slot = runtime
                .translation_previews
                .borrow()
                .slot_by_handle(window_handle);
            if let Some(slot) = slot {
                let is_current = {
                    let previews = runtime.translation_previews.borrow();
                    slint_window_handle(previews.current.preview.window()) == Some(window_handle)
                };
                let current_request = is_current
                    .then(|| runtime.translation_previews.borrow().current_request)
                    .flatten();
                let request_id = current_request;
                retire_runtime_translation_preview(&slot, "click_away", runtime);
                if let Some(request_id) = request_id {
                    close_screen_translation_request(
                        &runtime.screen_translation,
                        request_id,
                        |cancellation| apply_screen_translation_cancellation(cancellation, runtime),
                    );
                }
                if let Some(request_id) = request_id
                    && discard_matching_queued_model_translation(
                        &runtime.state,
                        QueuedModelMatch::Request(request_id),
                    )
                    .is_some()
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_pipeline request={request_id} stage=required_pack_consent outcome=dismissed"
                    ));
                }
                if is_current {
                    runtime.translation_previews.borrow_mut().current_request = None;
                } else {
                    runtime
                        .translation_previews
                        .borrow_mut()
                        .pinned
                        .retain(|candidate| {
                            slint_window_handle(candidate.preview.window()) != Some(window_handle)
                        });
                }
            }
            rust_platform_windows::deactivate_popup_dismiss(window_handle);
        }
        RuntimeEvent::TranslationPreviewPinChanged {
            window_handle,
            pinned,
        } => {
            #[cfg(feature = "screen-translation")]
            if let Some(slot) = runtime
                .screen_result_windows
                .borrow()
                .slot_by_handle(window_handle)
            {
                let request_id = slot.request_id();
                if pinned
                    && !runtime
                        .screen_translation
                        .borrow_mut()
                        .pin_completed(request_id)
                {
                    slot.window.set_pinned(false);
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_result request={request_id} pin=ignored reason=processing"
                    ));
                    return;
                }
                if let Err(error) = pin_screen_result(&slot, pinned) {
                    slot.window.set_pinned(!pinned);
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_result request={request_id} pin=failed error={error}"
                    ));
                }
                return;
            }
            let slot = runtime
                .translation_previews
                .borrow()
                .slot_by_handle(window_handle);
            if let Some(slot) = slot {
                let is_current = {
                    let previews = runtime.translation_previews.borrow();
                    slint_window_handle(previews.current.preview.window()) == Some(window_handle)
                };
                rust_platform_windows::set_popup_pinned(window_handle, pinned);
                if pinned && is_current {
                    // Keep the immutable RefCell borrow out of the match
                    // scrutinee. Rust extends a scrutinee temporary through
                    // the whole match, which would overlap the mutable borrow
                    // used to atomically rotate current -> pinned below.
                    let blur_radius = runtime.translation_previews.borrow().blur_radius;
                    match create_translation_preview_slot(blur_radius) {
                        Ok(replacement) => {
                            if let Err(error) = apply_translation_preview_pin_state(
                                &slot.preview,
                                &slot.presenter,
                                true,
                            ) {
                                rust_app_core::diagnostics::log_line(&format!(
                                    "translation_preview pin_state=failed pinned=true error={error}"
                                ));
                            } else {
                                {
                                    let mut previews = runtime.translation_previews.borrow_mut();
                                    let old = std::mem::replace(
                                        &mut previews.current,
                                        replacement.clone(),
                                    );
                                    previews.current_request = None;
                                    previews.pinned.push(old);
                                }
                                precreate_translation_preview(&replacement);
                                rust_app_core::diagnostics::log_line(&format!(
                                    "translation_preview pin=detached hwnd={} pinned_count={}",
                                    window_handle,
                                    runtime.translation_previews.borrow().pinned.len()
                                ));
                            }
                        }
                        Err(error) => rust_app_core::diagnostics::log_line(&format!(
                            "translation_preview pin=failed stage=create_successor error={error}"
                        )),
                    }
                } else if let Err(error) =
                    apply_translation_preview_pin_state(&slot.preview, &slot.presenter, pinned)
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview pin_state=failed pinned={pinned} error={error}"
                    ));
                }
            }
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview pin=changed hwnd={window_handle} pinned={pinned}"
            ));
        }
        RuntimeEvent::ScrollTranslationPreview {
            window_handle,
            delta,
        } => {
            if let Some(slot) = runtime
                .translation_previews
                .borrow()
                .slot_by_handle(window_handle)
            {
                slot.preview.invoke_scroll_by(delta as f32);
            }
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenBlockEditRequested {
            window_handle,
            block_index,
        } => open_screen_translation_editor(window_handle, block_index, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenViewModeChanged {
            window_handle,
            mode,
        } => reflow_screen_result_view(window_handle, mode, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenCopyTextRequested { window_handle } => {
            copy_screen_translation(window_handle, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenTranslationRetryRequested { window_handle } => {
            retry_screen_translation(window_handle, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenResultPresentationFailed {
            request_id,
            anchor,
            error,
            presentation,
            attachment_attempt,
        } => {
            // Delayed failures must still identify the current failed attempt.
            if runtime
                .screen_result_windows
                .borrow()
                .slot_for_request(request_id)
                .is_some_and(|slot| match (presentation, attachment_attempt) {
                    (Some(ticket), _) => slot.is_current_presentation_failure(ticket),
                    (None, Some(attempt)) => slot.is_current_attachment_failure(attempt),
                    (None, None) => false,
                })
            {
                fail_screen_result_presentation(request_id, anchor, &error, runtime);
            }
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenSaveFileRequested { window_handle } => {
            save_screen_translation_file(window_handle, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenDeleteImageRequested { window_handle } => {
            delete_screen_translation_image(window_handle, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::StoredScreenshotSaved(result) => {
            if let Err(error) = result {
                set_status_problem(&runtime.settings, &error);
            }
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::StoredScreenshotDeleted { request_id, result } => {
            finish_delete_screen_translation_image(request_id, result, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenEditorCommit => commit_screen_translation_editor(false, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenEditorRetry => commit_screen_translation_editor(true, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenEditorCancel => close_screen_translation_editor(runtime, false),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::GlossarySearchRequested(query) => {
            request_glossary_list(query, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::GlossarySaveRequested {
            entry_id,
            source,
            translation,
        } => update_glossary_entry(entry_id, source, translation, runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::GlossaryDeleteRequested(entry_id) => {
            delete_glossary_entry(entry_id, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::GlossaryRetryRequested => retry_glossary_action(runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenshotRetentionChanged(enabled) => {
            update_screenshot_retention(enabled, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::DeleteStoredScreenshots => delete_stored_screenshots(runtime),
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::StoredScreenshotsDeleted(result) => {
            finish_delete_stored_screenshots(result, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::OpenLatestScreenTranslation => {
            open_latest_screen_translation(runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::LatestScreenTranslationLoaded(result) => {
            finish_open_latest_screen_translation(result, runtime);
        }
        #[cfg(feature = "screen-translation")]
        RuntimeEvent::ScreenPixelsExpired { request_id } => {
            invalidate_screen_result_pixels(request_id, runtime);
        }
        RuntimeEvent::ConfirmQueuedModelInstall => {
            confirm_queued_model_install(runtime);
        }
        RuntimeEvent::SessionEnd(event) => {
            handle_session_end(event, runtime);
        }
        RuntimeEvent::WorkerError {
            kind,
            instance_id,
            message,
        } => {
            if !runtime.workers.is_current_instance(kind, instance_id) {
                rust_app_core::diagnostics::log_line(&format!(
                    "worker kind={kind:?} instance={instance_id} event_stream=stale_invalid_data_ignored"
                ));
                return;
            }
            if kind == WorkerKind::Companion {
                rust_app_core::diagnostics::log_line(&format!(
                    "companion_renderer event_stream=invalid message={message}"
                ));
                set_indicator_status(&runtime.settings, &message);
                return;
            }
            #[cfg(feature = "screen-translation")]
            if kind == WorkerKind::Ocr {
                rust_app_core::diagnostics::log_line(&format!(
                    "ocr_worker instance={instance_id} event_stream=invalid message={message}"
                ));
                return;
            }
            if runtime
                .state
                .borrow()
                .active_visualizer_style
                .is_some_and(VisualizerStyle::uses_external_renderer)
            {
                let _ = runtime
                    .workers
                    .send_companion(&CompanionRendererCommand::Dismiss { session_id: None });
            }
            {
                let mut state = runtime.state.borrow_mut();
                state.indicator_session = None;
                state.active_visualizer_style = None;
            }
            // A worker that has fallen over owes answers it will never send.
            // A key save waiting on one of them would otherwise hold the form
            // open forever with no reason shown.
            // Only the operations this worker owed. A speech worker falling
            // over says nothing about a translation key in flight.
            let owner = match kind {
                WorkerKind::Speech => Some(providers_view::Kind::Speech),
                WorkerKind::Translation => Some(providers_view::Kind::Text),
                #[cfg(feature = "screen-translation")]
                WorkerKind::Ocr => None,
                _ => None,
            };
            if let Some(owner) = owner {
                providers_view::abandon_pending_credentials(
                    &runtime.settings,
                    &runtime.state,
                    owner,
                    "the worker stopped",
                );
            }
            runtime.indicator_animation.borrow_mut().stop();
            runtime.recording_overlays.retire_all("worker_error");
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status(state_text(&runtime.state, UiText::WorkerError).into());
            }
            set_status_problem(&runtime.settings, &message);
        }
        RuntimeEvent::WorkerExited {
            kind,
            instance_id,
            exit,
        } => {
            handle_worker_exit(kind, instance_id, exit, runtime);
        }
    }
}

fn retire_runtime_translation_preview(
    slot: &TranslationPreviewSlot,
    reason: &str,
    runtime: &RuntimeUi,
) {
    #[cfg(feature = "screen-translation")]
    if let Some(window_handle) = slint_window_handle(slot.preview.window()) {
        let active_host = runtime
            .active_screen_editor
            .borrow()
            .as_ref()
            .map(|active| active.host_window_handle);
        if host_editor_should_close(active_host, window_handle) {
            close_screen_translation_editor(runtime, false);
        }
    }
    retire_translation_preview(slot, reason);
}

#[cfg(feature = "screen-translation")]
fn take_terminal_compact_before_screen_result(
    previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    request_id: RequestId,
    is_active_work: impl Fn(RequestId) -> bool,
) -> Option<TranslationPreviewSlot> {
    let (slot, current_request) = {
        let previews = previews.borrow();
        (previews.current(), previews.current_request)
    };
    if slot.preview.get_pinned() {
        return None;
    }
    if current_request.is_some_and(|current| current != request_id && is_active_work(current)) {
        return None;
    }
    if let Some(current) = current_request {
        previews.borrow_mut().clear_current_request(current);
    }
    Some(slot)
}

#[cfg(feature = "screen-translation")]
fn take_superseded_screen_compact(
    previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    old_screen_request: Option<RequestId>,
    new_request: RequestId,
) -> Option<TranslationPreviewSlot> {
    let old_request = old_screen_request.filter(|old| *old != new_request)?;
    let slot = {
        let mut previews = previews.borrow_mut();
        if previews.current_request != Some(old_request) || previews.current.preview.get_pinned() {
            return None;
        }
        previews.clear_current_request(old_request);
        previews.current()
    };
    coordinator.borrow_mut().close(old_request);
    Some(slot)
}

#[cfg(feature = "screen-translation")]
fn retire_terminal_compact_before_screen_result(request_id: RequestId, runtime: &RuntimeUi) {
    let prior_request = runtime.translation_previews.borrow().current_request;
    let slot = take_terminal_compact_before_screen_result(
        &runtime.translation_previews,
        request_id,
        |current| {
            runtime
                .state
                .borrow()
                .pending_translations
                .contains_key(&current)
                || runtime
                    .screen_translation
                    .borrow()
                    .is_active_work_request(current)
        },
    );
    if let Some(slot) = slot {
        retire_runtime_translation_preview(&slot, "screen_result_ready", runtime);
        if let Some(prior_request) = prior_request.filter(|prior| *prior != request_id) {
            close_screen_translation_request(
                &runtime.screen_translation,
                prior_request,
                |cancellation| {
                    apply_screen_translation_cancellation(cancellation, runtime);
                },
            );
        }
    }
}

#[cfg(feature = "screen-translation")]
fn cloak_compact_previews_owned(
    previews: &Rc<RefCell<TranslationPreviewRegistry>>,
) -> Result<Vec<overlay_presenter::CaptureExcludedOverlay>, String> {
    previews.borrow().cloak_presented_for_capture()
}

#[cfg(feature = "screen-translation")]
fn open_screen_translation_editor(window_handle: u64, block_index: i32, runtime: &RuntimeUi) {
    if runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
        .is_some_and(|slot| slot.window.get_screen_translating())
    {
        return;
    }
    let existing = runtime
        .active_screen_editor
        .borrow()
        .as_ref()
        .map(|active| (active.host_window_handle, active.block_index));
    if existing == Some((window_handle, block_index)) {
        if let Some(slot) = runtime
            .screen_result_windows
            .borrow()
            .slot_by_handle(window_handle)
        {
            slot.window.set_screen_editor_visible(true);
        }
        return;
    }
    if existing.is_some() {
        close_screen_translation_editor(runtime, false);
    }
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
    else {
        return;
    };
    let request_id = slot.request_id();
    let Ok((snapshot, source_language, target_language)) = runtime
        .screen_translation
        .borrow()
        .result_snapshot(request_id)
    else {
        return;
    };
    let Ok(index) = usize::try_from(block_index) else {
        return;
    };
    let Some(block) = snapshot.blocks.get(index) else {
        return;
    };

    let state = TranslationEditorState::begin(
        EditorDraftKey {
            request_id,
            block_id: block.block_id.clone(),
            revision: snapshot.revision,
        },
        block.source.clone(),
        block.translation.clone(),
    );
    slot.window
        .set_screen_editor_original(block.source.clone().into());
    slot.window
        .set_screen_editor_block_source(block.source.clone().into());
    slot.window
        .set_screen_editor_baseline(block.translation.clone().into());
    slot.window
        .set_screen_editor_translation(block.translation.clone().into());
    slot.window
        .set_screen_editor_context(source_context_label(&snapshot.context).into());
    slot.window.set_screen_editor_direction(
        format!(
            "{} → {}",
            source_language.code().to_ascii_uppercase(),
            target_language.code().to_ascii_uppercase()
        )
        .into(),
    );
    slot.window.set_screen_save_error(false);
    slot.window.set_screen_editor_saving(false);

    slot.window.set_screen_selected_block(block_index);
    slot.window.set_screen_editor_visible(true);
    if let Some((x, y)) = screen_result_view::editor_position(&slot.window, index) {
        slot.window.set_screen_editor_panel_x(x);
        slot.window.set_screen_editor_panel_y(y);
    }
    *runtime.active_screen_editor.borrow_mut() = Some(ActiveScreenEditor {
        state,
        source_language,
        target_language,
        context: snapshot.context.clone(),
        host_window_handle: window_handle,
        block_index,
        canonical_revision: snapshot.revision,
        last_applied_translation: block.translation.clone(),
    });
    let _ = slot
        .window
        .window()
        .with_winit_window(|window| window.focus_window());
    slot.window.window().request_redraw();
}

#[cfg(feature = "screen-translation")]
fn save_screen_translation_edit(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    request_id: RequestId,
    block_id: &rust_protocol::ScreenBlockId,
    expected_revision: u64,
    text: String,
) -> Result<u64, DocumentError> {
    coordinator
        .borrow_mut()
        .save_user_edit(request_id, block_id, expected_revision, text)
}

#[cfg(feature = "screen-translation")]
fn apply_screen_translation_edit_for_scope(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    request_id: RequestId,
    block_id: &rust_protocol::ScreenBlockId,
    expected_revision: u64,
    scope: EditorCommitScope,
    text: String,
) -> Result<Option<u64>, DocumentError> {
    match scope {
        EditorCommitScope::WholeBlock => {
            save_screen_translation_edit(coordinator, request_id, block_id, expected_revision, text)
                .map(Some)
        }
        EditorCommitScope::GlossaryTerm => Ok(None),
    }
}

#[cfg(feature = "screen-translation")]
fn expire_screen_translation_pixels(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    now_unix_ms: u64,
    mut on_released: impl FnMut(RequestId),
) -> usize {
    let released = coordinator.borrow_mut().expire_pixels(now_unix_ms);
    let count = released.len();
    for request_id in released {
        on_released(request_id);
    }
    count
}

#[cfg(feature = "screen-translation")]
fn commit_screen_translation_editor(retry: bool, runtime: &RuntimeUi) {
    let Some(mut active) = runtime.active_screen_editor.borrow_mut().take() else {
        return;
    };
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(active.host_window_handle)
    else {
        active.state.cancel();
        return;
    };
    let edited = slot
        .window
        .get_screen_editor_translation()
        .trim()
        .to_owned();
    let source_phrase = slot.window.get_screen_editor_original().trim().to_owned();
    let scope = EditorCommitScope::for_source(active.state.source(), &source_phrase);
    if edited.is_empty() || source_phrase.is_empty() {
        slot.window.set_screen_save_error(true);
        slot.window.set_screen_editor_saving(false);
        *runtime.active_screen_editor.borrow_mut() = Some(active);
        return;
    }

    let retry_existing = retry
        && active.state.phase() == EditorPhase::SaveFailed
        && active.state.translation() == edited;
    if !retry_existing {
        active.state.set_translation(edited.clone());
    }
    let Some(operation_id) = (if retry_existing {
        active.state.begin_retry()
    } else {
        active.state.begin_commit()
    }) else {
        *runtime.active_screen_editor.borrow_mut() = Some(active);
        return;
    };

    if edited != active.last_applied_translation {
        match apply_screen_translation_edit_for_scope(
            &runtime.screen_translation,
            active.state.key().request_id,
            &active.state.key().block_id,
            active.canonical_revision,
            scope,
            edited.clone(),
        ) {
            Ok(Some(revision)) => {
                active.canonical_revision = revision;
                active.last_applied_translation.clone_from(&edited);
                let persisted_snapshot = runtime
                    .screen_translation
                    .borrow()
                    .result_snapshot(active.state.key().request_id)
                    .ok()
                    .map(|(snapshot, _, _)| snapshot);
                refresh_screen_result_preview(
                    active.state.key().request_id,
                    active.block_index,
                    runtime,
                );
                if let Some(snapshot) = persisted_snapshot.as_ref() {
                    persist_completed_screen_snapshot(
                        snapshot,
                        active.source_language,
                        active.target_language,
                        runtime.screenshot_store.as_ref(),
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                active.state.fail(operation_id, format!("{error:?}"), true);
                slot.window.set_screen_save_error(true);
                slot.window.set_screen_editor_saving(false);
                *runtime.active_screen_editor.borrow_mut() = Some(active);
                return;
            }
        }
    }

    // The editable phrase is the reusable glossary key. The complete OCR
    // block remains the example so a one-word correction is not detached
    // from the sentence in which the user approved it.
    let example = active.state.source().to_owned();
    let entry = GlossaryEntryDraft {
        source_phrase,
        approved_translation: active.state.translation().to_owned(),
        source_language: active.source_language,
        target_language: active.target_language,
        example,
        context: active.context.clone(),
        origin: if scope == EditorCommitScope::GlossaryTerm || active.state.changed() {
            GlossaryOrigin::Corrected
        } else {
            GlossaryOrigin::Confirmed
        },
    };
    let command = TranslationWorkerCommand::SaveGlossaryEntry {
        operation_id,
        request_id: active.state.key().request_id,
        block_id: active.state.key().block_id.clone(),
        expected_revision: active.canonical_revision,
        entry,
    };
    slot.window.set_screen_editor_saving(true);
    slot.window.set_screen_save_error(false);
    if let Err(error) = runtime.workers.translation.send(&command) {
        active.state.fail(operation_id, error.to_string(), true);
        slot.window.set_screen_editor_saving(false);
        slot.window.set_screen_save_error(true);
    }
    *runtime.active_screen_editor.borrow_mut() = Some(active);
}

#[cfg(feature = "screen-translation")]
fn close_screen_translation_editor(runtime: &RuntimeUi, saved: bool) {
    let Some(mut active) = runtime.active_screen_editor.borrow_mut().take() else {
        return;
    };
    if !saved {
        active.state.cancel();
    }
    let slot = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(active.host_window_handle);
    if let Some(slot) = slot {
        slot.window.set_screen_editor_visible(false);
        slot.window.set_screen_editor_saving(false);
        slot.window.set_screen_save_error(false);
        if saved {
            slot.window.set_screen_saved(true);
            let window = Rc::clone(&slot.window);
            Timer::single_shot(std::time::Duration::from_secs(2), move || {
                window.set_screen_saved(false);
            });
        }
    }
}

#[cfg(feature = "screen-translation")]
fn copy_screen_translation(window_handle: u64, runtime: &RuntimeUi) {
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
    else {
        return;
    };
    let request_id = slot.request_id();
    let copied = runtime.screen_translation.borrow().copy_text(request_id);
    match copied {
        Ok(text) => {
            let copy_request = RequestId::new();
            if let Err(error) = runtime.workers.input.send(&InputHostCommand::CopyText {
                request_id: copy_request,
                text,
            }) {
                set_status_problem(&runtime.settings, &error.to_string());
            } else {
                runtime
                    .state
                    .borrow_mut()
                    .screen_copy_requests
                    .insert(copy_request, window_handle);
                set_indicator_status(
                    &runtime.settings,
                    state_text(&runtime.state, UiText::CopiedToClipboard),
                );
            }
        }
        Err(error) => set_status_problem(&runtime.settings, &format!("{error:?}")),
    }
}

#[cfg(feature = "screen-translation")]
fn save_screen_translation_file(window_handle: u64, runtime: &RuntimeUi) {
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
    else {
        return;
    };
    if slot.window.get_screen_translating() {
        return;
    }
    let request_id = slot.request_id();
    let Ok((snapshot, _, _)) = runtime
        .screen_translation
        .borrow()
        .result_snapshot(request_id)
    else {
        return;
    };
    let Some(crop) = snapshot.crop else {
        set_status_problem(
            &runtime.settings,
            "The temporary screen capture is no longer available.",
        );
        return;
    };
    let suggested_name = format!("sayflex-screen-{request_id}.png");
    let destination =
        match rust_platform_windows::choose_png_save_path(Some(window_handle), &suggested_name, "")
        {
            Ok(Some(destination)) => destination,
            Ok(None) => return,
            Err(error) => {
                set_status_problem(&runtime.settings, &error.to_string());
                return;
            }
        };
    if let Err(error) = std::thread::Builder::new()
        .name("screen-snapshot-save".to_owned())
        .spawn(move || {
            dispatch_runtime_event(RuntimeEvent::StoredScreenshotSaved(
                save_frame_permanent(crop.as_ref(), &destination)
                    .map_err(|error| error.to_string()),
            ));
        })
    {
        set_status_problem(&runtime.settings, &error.to_string());
    }
}

#[cfg(feature = "screen-translation")]
fn delete_screen_translation_image(window_handle: u64, runtime: &RuntimeUi) {
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
    else {
        return;
    };
    if slot.window.get_screen_translating() {
        return;
    }
    let request_id = slot.request_id();
    let Some(store) = runtime.screenshot_store.as_ref().map(Arc::clone) else {
        finish_delete_screen_translation_image(request_id, Ok(()), runtime);
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("screen-snapshot-delete".to_owned())
        .spawn(move || {
            dispatch_runtime_event(RuntimeEvent::StoredScreenshotDeleted {
                request_id,
                result: store
                    .delete_now(request_id)
                    .map_err(|error| error.to_string()),
            });
        })
    {
        set_status_problem(&runtime.settings, &error.to_string());
    }
}

#[cfg(feature = "screen-translation")]
fn finish_delete_screen_translation_image(
    request_id: RequestId,
    result: Result<(), String>,
    runtime: &RuntimeUi,
) {
    if let Err(error) = result {
        set_status_problem(&runtime.settings, &error);
        return;
    }
    invalidate_screen_result_pixels(request_id, runtime);
}

#[cfg(feature = "screen-translation")]
fn open_latest_screen_translation(runtime: &RuntimeUi) {
    let Some(store) = runtime.screenshot_store.as_ref().map(Arc::clone) else {
        set_indicator_status(
            &runtime.settings,
            state_text(&runtime.state, UiText::NothingKeptYet),
        );
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("screen-snapshot-open-latest".to_owned())
        .spawn(move || {
            dispatch_runtime_event(RuntimeEvent::LatestScreenTranslationLoaded(
                store
                    .load_latest_result(unix_ms())
                    .map_err(|error| error.to_string()),
            ));
        })
    {
        set_status_problem(&runtime.settings, &error.to_string());
    }
}

#[cfg(feature = "screen-translation")]
fn finish_open_latest_screen_translation(
    result: Result<
        Option<(
            ScreenTranslationDocument,
            TranslationLanguage,
            TranslationLanguage,
        )>,
        String,
    >,
    runtime: &RuntimeUi,
) {
    let (document, source_language, target_language) = match result {
        Ok(Some(result)) => result,
        Ok(None) => {
            set_indicator_status(
                &runtime.settings,
                state_text(&runtime.state, UiText::NothingKeptYet),
            );
            return;
        }
        Err(error) => {
            set_status_problem(&runtime.settings, &error);
            return;
        }
    };
    let request_id = document.request_id();
    runtime.screen_translation.borrow_mut().restore_document(
        document,
        source_language,
        target_language,
    );
    let Ok((snapshot, _, _)) = runtime
        .screen_translation
        .borrow()
        .result_snapshot(request_id)
    else {
        set_status_problem(
            &runtime.settings,
            state_text(&runtime.state, UiText::ScreenTranslationRestoreFailed),
        );
        return;
    };

    show_screen_image_result(request_id, snapshot.crop_rect, &snapshot, false, runtime);
    if let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_for_request(request_id)
    {
        let _ = slot.window.window().show();
        let _ = slot
            .window
            .window()
            .with_winit_window(|window| window.focus_window());
        slot.window.window().request_redraw();
    }
}

#[cfg(feature = "screen-translation")]
fn invalidate_screen_result_pixels(request_id: RequestId, runtime: &RuntimeUi) {
    let editor_request = runtime
        .active_screen_editor
        .borrow()
        .as_ref()
        .map(|active| active.state.key().request_id);
    if editor_request == Some(request_id) {
        close_screen_translation_editor(runtime, false);
    }
    runtime
        .screen_translation
        .borrow_mut()
        .expire_request_pixels(request_id);
    runtime
        .screen_result_windows
        .borrow_mut()
        .retire(request_id, "pixels_expired");
}

#[cfg(feature = "screen-translation")]
fn refresh_screen_result_preview(request_id: RequestId, selected_block: i32, runtime: &RuntimeUi) {
    let Ok((snapshot, _, _)) = runtime
        .screen_translation
        .borrow()
        .result_snapshot(request_id)
    else {
        return;
    };
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_for_request(request_id)
    else {
        return;
    };
    let mode = slot.window.get_screen_view_mode();
    let editor_visible = slot.window.get_screen_editor_visible();
    if let Err(error) = screen_result_view::apply_snapshot(&slot.window, &snapshot) {
        rust_app_core::diagnostics::log_line(&format!(
            "screen_translation request={request_id} stage=result_refresh result=failed error={error}"
        ));
        return;
    }
    slot.window
        .set_screen_view_mode(screen_result_view::visible_ui_mode(
            mode,
            snapshot.crop.is_some(),
        ));
    slot.window.set_screen_selected_block(selected_block);
    slot.window
        .set_screen_editor_visible(screen_result_view::preserve_editor_on_snapshot_refresh(
            editor_visible,
            snapshot.crop.is_some(),
        ));
    slot.window.window().request_redraw();
}

#[cfg(feature = "screen-translation")]
fn reflow_screen_result_view(window_handle: u64, requested_mode: i32, runtime: &RuntimeUi) {
    let Some(slot) = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle)
    else {
        return;
    };
    let request_id = slot.request_id();
    let Ok(snapshot) = runtime
        .screen_translation
        .borrow()
        .display_snapshot(request_id)
    else {
        return;
    };
    let selected_block = slot.window.get_screen_selected_block();
    let editor_visible = slot.window.get_screen_editor_visible();
    slot.window
        .set_screen_view_mode(screen_result_view::visible_ui_mode(
            requested_mode,
            snapshot.crop.is_some(),
        ));
    if let Err(error) = screen_result_view::apply_snapshot(&slot.window, &snapshot) {
        rust_app_core::diagnostics::log_line(&format!(
            "screen_translation request={request_id} stage=view_reflow result=failed error={error}"
        ));
        return;
    }
    slot.window.set_screen_selected_block(selected_block);
    slot.window
        .set_screen_editor_visible(screen_result_view::preserve_editor_on_snapshot_refresh(
            editor_visible,
            snapshot.crop.is_some(),
        ));
    if editor_visible
        && selected_block >= 0
        && let Some((x, y)) =
            screen_result_view::editor_position(&slot.window, selected_block as usize)
    {
        slot.window.set_screen_editor_panel_x(x);
        slot.window.set_screen_editor_panel_y(y);
    }
    slot.window.window().request_redraw();
}

#[cfg(feature = "screen-translation")]
fn source_context_label(context: &SourceContext) -> String {
    let mut parts = Vec::new();
    for value in [
        context.domain.as_deref(),
        context.application_id.as_deref(),
        context.document_name.as_deref(),
        context.window_title.as_deref(),
        context.site_url.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let value = value.trim();
        if !value.is_empty() && !parts.contains(&value) {
            parts.push(value);
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" · ")
}

fn handle_session_end(event: rust_platform_windows::SessionEndEvent, runtime: &RuntimeUi) {
    match event {
        rust_platform_windows::SessionEndEvent::Query => {
            runtime.workers.begin_session_end();
        }
        rust_platform_windows::SessionEndEvent::Confirmed => {
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=exit_requested reason=session_end_confirmed",
            );
            runtime.workers.confirm_session_end();
            #[cfg(feature = "screen-translation")]
            runtime
                .screen_result_windows
                .borrow_mut()
                .retire_all("session_end");
            let _ = slint::quit_event_loop();
        }
        rust_platform_windows::SessionEndEvent::Cancelled => {
            let deferred = runtime.workers.cancel_session_end();
            for worker_exit in deferred {
                handle_worker_exit(
                    worker_exit.kind,
                    worker_exit.instance_id,
                    worker_exit.exit,
                    runtime,
                );
            }
        }
    }
}

/// Applies the translation worker's report of what is resident.
///
/// With keep-warm off the worker loads the model for each request and drops it
/// afterwards, so "not loaded" is its resting state, not a reason to refuse
/// the next translation: the model loads on demand.
fn apply_local_translation_model_state(
    app: &mut AppState,
    catalog_id: Option<String>,
    loaded: bool,
) {
    app.translation_loaded_model_id = loaded.then_some(catalog_id).flatten();
    let loads_on_demand = !app.translation_keep_warm && app.translation_worker_ready;
    app.translation_ready = (loaded || loads_on_demand) && app.local_translation_enabled();
    app.translation_unloading = false;
    if loaded {
        app.translation_reload_on_next_use = false;
    }
}

/// Which worker answered a "Clear history" request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryOwner {
    Speech,
    Translation,
}

/// Each worker owns one history file. "History cleared" may only be said once
/// both have answered and neither failed; a worker that could not remove its
/// file keeps its entries and says why.
#[derive(Debug, Default)]
struct HistoryClear {
    /// `None` while waiting; `Some(None)` cleared; `Some(Some(error))` failed.
    speech: Option<Option<String>>,
    translation: Option<Option<String>>,
}

impl HistoryClear {
    fn answer(&mut self, owner: HistoryOwner, error: Option<String>) {
        let slot = match owner {
            HistoryOwner::Speech => &mut self.speech,
            HistoryOwner::Translation => &mut self.translation,
        };
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    fn is_waiting_for(&self, owner: HistoryOwner) -> bool {
        match owner {
            HistoryOwner::Speech => self.speech.is_none(),
            HistoryOwner::Translation => self.translation.is_none(),
        }
    }

    /// `None` while an answer is outstanding; otherwise the joined failures.
    fn outcome(&self) -> Option<Result<(), String>> {
        let (Some(speech), Some(translation)) = (&self.speech, &self.translation) else {
            return None;
        };
        let failures = [speech, translation]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        Some(if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        })
    }
}

/// Records one worker's answer and, once both have answered, tells the user
/// what actually happened.
fn record_history_clear_answer(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    owner: HistoryOwner,
    error: Option<String>,
) {
    let outcome = {
        let mut state = state.borrow_mut();
        let Some(progress) = state.history_clear.as_mut() else {
            return;
        };
        let speech_cleared =
            owner == HistoryOwner::Speech && error.is_none() && progress.is_waiting_for(owner);
        progress.answer(owner, error);
        let outcome = progress.outcome();
        // Each acknowledgement owns its data. A later translation failure
        // must not retain deleted speech, or erase a newer speech completion.
        if speech_cleared {
            state.registry.clear_completed_history();
            state.transcripts.clear();
            state.latest_history_session = None;
            state.history_loaded = true;
            state.pending_insert_last = None;
        }
        if outcome.is_some() {
            state.history_clear = None;
        }
        outcome
    };
    sync_history(settings, state);
    let Some(outcome) = outcome else { return };
    match outcome {
        Ok(()) => set_indicator_status(settings, state_text(state, UiText::HistoryCleared)),
        Err(error) => {
            rust_app_core::diagnostics::log_line("history clear result=failed");
            set_status_problem(
                settings,
                &state_message(state, UiText::CouldNotClearHistory, &[("error", &error)]),
            );
        }
    }
}

/// Whether a worker's exit ends the dictation in progress.
const fn worker_exit_affects_dictation(kind: WorkerKind) -> bool {
    matches!(kind, WorkerKind::Input | WorkerKind::Speech)
}

/// Ends a translation request whose worker exited before answering.
fn fail_orphaned_translation(
    request_id: RequestId,
    pending: PendingTranslation,
    runtime: &RuntimeUi,
) {
    runtime
        .state
        .borrow_mut()
        .selection_release_gates
        .remove(&request_id);
    let message = state_text(&runtime.state, UiText::WorkerUnavailable);
    match pending.origin {
        TranslationOrigin::Translator => {
            if let Some(translator) = runtime.translator.upgrade() {
                translator.set_processing(false);
                translator.set_presentation_state(4);
                translator.set_notice_headline(message.into());
                translator.set_notice_detail("".into());
                translator.set_status_text(message.into());
            }
        }
        TranslationOrigin::Selection { target, .. } => {
            if runtime
                .translation_previews
                .borrow()
                .slot_for_request(request_id)
                .is_none()
            {
                runtime
                    .translation_previews
                    .borrow_mut()
                    .reserve_current(request_id);
            }
            show_selection_translation_preview(
                request_id,
                target,
                &pending.source_text,
                message,
                &runtime.translation_previews,
                true,
            );
        }
        // Screen requests are ended by the screen coordinator above.
        TranslationOrigin::ScreenRegion { .. } => {}
    }
    rust_app_core::diagnostics::log_line(&format!(
        "translation_pipeline request={request_id} stage=failed kind=WorkerExit"
    ));
}

/// The translator window shows one request at a time. A new submission
/// replaces whatever it was still waiting for: the older request leaves the
/// pending table, so its late cancel, failure or result cannot reset the
/// window while the new one runs.
fn supersede_translator_requests(state: &mut AppState) -> Vec<RequestId> {
    let superseded = state
        .pending_translations
        .iter()
        .filter(|(_, pending)| matches!(pending.origin, TranslationOrigin::Translator))
        .map(|(request_id, _)| *request_id)
        .collect::<Vec<_>>();
    for request_id in &superseded {
        state.pending_translations.remove(request_id);
    }
    superseded
}

fn handle_worker_exit(
    kind: WorkerKind,
    instance_id: u64,
    exit: rust_platform_windows::ProcessExit,
    runtime: &RuntimeUi,
) {
    if !runtime.workers.is_current_instance(kind, instance_id) {
        rust_app_core::diagnostics::log_line(&format!(
            "worker kind={kind:?} instance={instance_id} exit=stale"
        ));
        return;
    }
    let exit_code = exit
        .exit_code
        .map_or_else(|| "unknown".to_owned(), |code| format!("0x{code:08X}"));
    rust_app_core::diagnostics::log_line(&format!(
        "worker kind={kind:?} instance={instance_id} exit_code={exit_code} uptime_ms={}",
        exit.uptime.as_millis()
    ));

    // The companion process is presentation-only. Its failure must never
    // cancel audio capture, transcription, or insertion owned by other
    // processes. Recover it independently and leave the active session intact.
    if kind == WorkerKind::Companion {
        match runtime.workers.recover(kind, instance_id, exit) {
            Ok(WorkerRecovery::Restarted) => {
                let (enabled, active) = {
                    let state = runtime.state.borrow();
                    let enabled = state.visualizer_style.uses_external_renderer();
                    let active = state
                        .active_visualizer_style
                        .is_some_and(VisualizerStyle::uses_external_renderer)
                        .then(|| state.indicator_session)
                        .flatten()
                        .and_then(|session_id| {
                            state
                                .targets
                                .get(&session_id)
                                .copied()
                                .map(|target| (session_id, target.window_handle))
                        });
                    (enabled, active)
                };
                let _ = runtime
                    .workers
                    .send_companion(&CompanionRendererCommand::SetEnabled { enabled });
                if let Some((session_id, target_window_handle)) = active {
                    let _ = runtime
                        .workers
                        .send_companion(&CompanionRendererCommand::Activate {
                            session_id,
                            target_window_handle,
                        });
                }
                set_indicator_status(
                    &runtime.settings,
                    state_text(&runtime.state, UiText::CompanionOrbRestarted),
                );
            }
            Ok(WorkerRecovery::Deferred | WorkerRecovery::Stopping | WorkerRecovery::Stale) => {}
            Err(error) => set_status_problem(
                &runtime.settings,
                &state_message(
                    &runtime.state,
                    UiText::CompanionOrbUnavailable,
                    &[("error", &error.to_string())],
                ),
            ),
        }
        return;
    }

    #[cfg(feature = "screen-translation")]
    if kind == WorkerKind::Ocr {
        if let Some(request_id) = runtime.workers.abandon_active_ocr_request(instance_id) {
            runtime
                .screen_translation
                .borrow_mut()
                .discard_pending(request_id);
            if let Some(anchor) = runtime.screen_translation.borrow_mut().complete(request_id) {
                show_screen_translation_outcome(
                    request_id,
                    Some(anchor),
                    ScreenTranslationOutcome::WorkerFailed,
                    runtime,
                );
            }
            rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker request={request_id} outcome=failed kind=engine source=worker_exit replay=false"
            ));
        }
        match runtime.workers.recover(kind, instance_id, exit) {
            Ok(WorkerRecovery::Restarted) => {
                rust_app_core::diagnostics::log_line(
                    "ocr_worker recovery=restarted pending_request=not_replayed",
                );
            }
            Ok(WorkerRecovery::Deferred | WorkerRecovery::Stopping | WorkerRecovery::Stale) => {}
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker recovery=failed error={error}"
            )),
        }
        return;
    }

    // Only the input host and the speech worker take part in a dictation.
    // The translation worker never does — even translate-to-English is the
    // speech model's own task — so its exit must leave a recording, its
    // overlay and the healthy speech worker alone, as Companion and OCR do.
    let affects_dictation = worker_exit_affects_dictation(kind);
    if affects_dictation
        && runtime
            .state
            .borrow()
            .active_visualizer_style
            .is_some_and(VisualizerStyle::uses_external_renderer)
    {
        let _ = runtime
            .workers
            .send_companion(&CompanionRendererCommand::Dismiss { session_id: None });
    }

    #[cfg(feature = "screen-translation")]
    let failed_screen_translation = (kind == WorkerKind::Translation)
        .then(|| {
            runtime
                .screen_translation
                .borrow()
                .active_translation_source()
        })
        .flatten();
    #[cfg(not(feature = "screen-translation"))]
    let failed_screen_translation: Option<(RequestId, PhysicalRect, String)> = None;

    let (active_session, orphaned_translations) = {
        let mut state = runtime.state.borrow_mut();
        let mut orphaned = Vec::new();
        match kind {
            WorkerKind::Input => state.input_ready = false,
            WorkerKind::Speech => {
                state.speech_worker_ready = false;
                state.speech_ready = false;
            }
            WorkerKind::Translation => {
                state.translation_worker_ready = false;
                state.translation_ready = false;
                orphaned = state.pending_translations.drain().collect::<Vec<_>>();
            }
            #[cfg(feature = "screen-translation")]
            WorkerKind::Ocr => {}
            WorkerKind::Companion => {}
        }
        let active = if affects_dictation {
            let active = state.indicator_session.take();
            state.active_visualizer_style = None;
            if let Some(session_id) = active {
                let _ = state.registry.fail(session_id);
                state.targets.remove(&session_id);
                state.release_started.remove(&session_id);
            }
            active
        } else {
            None
        };
        (active, orphaned)
    };
    let history_owner = match kind {
        WorkerKind::Speech => Some(HistoryOwner::Speech),
        WorkerKind::Translation => Some(HistoryOwner::Translation),
        _ => None,
    };
    if let Some(owner) = history_owner
        && runtime
            .state
            .borrow()
            .history_clear
            .as_ref()
            .is_some_and(|progress| progress.is_waiting_for(owner))
    {
        record_history_clear_answer(
            &runtime.settings,
            &runtime.state,
            owner,
            Some("the worker stopped before confirming".to_owned()),
        );
    }
    // Requests the dead worker can no longer answer end visibly instead of
    // leaving the translator busy or a selection card waiting forever.
    for (request_id, pending) in orphaned_translations {
        fail_orphaned_translation(request_id, pending, runtime);
    }
    if let Some((request_id, anchor, _source_text)) = failed_screen_translation {
        runtime.screen_translation.borrow_mut().complete(request_id);
        #[cfg(feature = "screen-translation")]
        show_screen_translation_error(
            request_id,
            anchor,
            state_text(&runtime.state, UiText::WorkerUnavailable),
            runtime,
        );
        #[cfg(not(feature = "screen-translation"))]
        show_screen_translation_preview(
            request_id,
            anchor,
            &_source_text,
            state_text(&runtime.state, UiText::WorkerUnavailable),
            &runtime.translation_previews,
            true,
        );
    }
    if kind != WorkerKind::Speech
        && let Some(session_id) = active_session
    {
        let _ = runtime
            .workers
            .speech
            .send(&SpeechWorkerCommand::Cancel { session_id });
    }
    if affects_dictation {
        runtime.indicator_animation.borrow_mut().stop();
        runtime.recording_overlays.retire_all("worker_exit");
    }

    match runtime.workers.recover(kind, instance_id, exit) {
        Ok(WorkerRecovery::Restarted) => {
            let initialization = match kind {
                WorkerKind::Input => Ok(()),
                WorkerKind::Speech => {
                    let active = runtime.workers.active_speech_backend();
                    runtime.state.borrow_mut().speech_backend_active = Some(active);
                    sync_action_settings(&runtime.settings, &runtime.state);
                    let state = runtime.state.borrow();
                    runtime
                        .workers
                        .speech
                        .send(&SpeechWorkerCommand::Initialize {
                            selected_model_id: state.selected_model_id.clone(),
                            output_mode: state.output_mode,
                            dictation_language: state.dictation_language,
                            history_enabled: state.history_enabled,
                            fast_release_previews: state.fast_release_previews,
                            prepare_local_model: state.local_speech_enabled(),
                        })
                }
                WorkerKind::Translation => {
                    let state = runtime.state.borrow();
                    runtime
                        .workers
                        .translation
                        .send(&TranslationWorkerCommand::Initialize {
                            selected_catalog_id: state.selected_translation_model_id.clone(),
                            selected_tier: state.selected_translation_tier,
                            keep_warm: state.translation_keep_warm,
                            history_enabled: state.history_enabled,
                            prepare_local_model: state.local_translation_enabled(),
                        })
                }
                #[cfg(feature = "screen-translation")]
                WorkerKind::Ocr => Ok(()),
                WorkerKind::Companion => Ok(()),
            };
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status(
                    state_text(&runtime.state, UiText::RecoveringWorker).into(),
                );
            }
            // A restart that worked is progress; one that did not is trouble.
            // Reading that out of the sentence is what the old classifier got
            // wrong in both directions.
            match initialization {
                Ok(()) => set_indicator_status(
                    &runtime.settings,
                    state_text(&runtime.state, UiText::WorkerRestarted),
                ),
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "worker kind={kind:?} recovery=initialization_failed error={error}"
                    ));
                    set_status_problem(
                        &runtime.settings,
                        state_text(&runtime.state, UiText::WorkerRestartFailed),
                    );
                }
            }
        }
        Ok(WorkerRecovery::Deferred) => {
            rust_app_core::diagnostics::log_line(&format!(
                "worker kind={kind:?} instance={instance_id} recovery=deferred_session_end"
            ));
        }
        Ok(WorkerRecovery::Stopping) => {
            rust_app_core::diagnostics::log_line(&format!(
                "worker kind={kind:?} instance={instance_id} recovery=suppressed_stopping"
            ));
        }
        Ok(WorkerRecovery::Stale) => {}
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "worker kind={kind:?} recovery=failed error={error}"
            ));
            if kind == WorkerKind::Speech {
                runtime.state.borrow_mut().speech_backend_active = None;
                sync_action_settings(&runtime.settings, &runtime.state);
            }
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status(
                    state_text(&runtime.state, UiText::WorkerUnavailable).into(),
                );
            }
            set_status_problem(
                &runtime.settings,
                state_text(&runtime.state, UiText::WorkerUnavailable),
            );
        }
    }
}

fn close_screen_translation_request(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    request_id: RequestId,
    mut on_cancel: impl FnMut(ScreenTranslationCancellation),
) {
    let cancellation = coordinator.borrow_mut().close(request_id);
    if let Some(cancellation) = cancellation {
        on_cancel(cancellation);
    }
}

fn apply_screen_translation_cancellation(
    cancellation: ScreenTranslationCancellation,
    runtime: &RuntimeUi,
) {
    match cancellation {
        ScreenTranslationCancellation::Ocr(request_id) => {
            #[cfg(feature = "screen-translation")]
            {
                runtime
                    .screen_translation
                    .borrow_mut()
                    .discard_pending(request_id);
                let _ = runtime
                    .workers
                    .send_ocr(&OcrWorkerCommand::Cancel { request_id });
            }
            #[cfg(not(feature = "screen-translation"))]
            let _ = request_id;
        }
        ScreenTranslationCancellation::Translation(request_id) => {
            #[cfg(feature = "screen-translation")]
            {
                let cancelled_retry = runtime
                    .screen_result_windows
                    .borrow()
                    .slot_for_request(request_id)
                    .is_some_and(|slot| slot.window.get_screen_translating());
                if cancelled_retry {
                    runtime
                        .screen_result_windows
                        .borrow_mut()
                        .retire(request_id, "translation_cancelled");
                }
            }
            let _ = runtime
                .workers
                .translation
                .send(&TranslationWorkerCommand::Cancel { request_id });
            let mut state = runtime.state.borrow_mut();
            state.pending_translations.remove(&request_id);
            #[cfg(feature = "screen-translation")]
            if state
                .queued_screen_translation
                .as_ref()
                .is_some_and(|request| request.request_id == request_id)
            {
                state.queued_screen_translation = None;
            }
        }
    }
}

#[cfg(feature = "screen-translation")]
fn screen_outcome_for_capture_error(
    error: &rust_platform_windows::screen_capture::CaptureError,
) -> ScreenTranslationOutcome {
    match error {
        rust_platform_windows::screen_capture::CaptureError::ProtectedContent
        | rust_platform_windows::screen_capture::CaptureError::SecureDesktop => {
            ScreenTranslationOutcome::ProtectedContent
        }
        rust_platform_windows::screen_capture::CaptureError::EmptyRegion => {
            ScreenTranslationOutcome::EmptySelection
        }
        _ => ScreenTranslationOutcome::CaptureUnavailable,
    }
}

fn handle_screen_selection_terminal(terminal: SelectionTerminal, runtime: &RuntimeUi) {
    match terminal {
        SelectionTerminal::Selected {
            request_id,
            rect,
            capture,
        } => {
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=selected x={} y={} width={} height={}",
                rect.x, rect.y, rect.width, rect.height
            ));
            #[cfg(feature = "screen-translation")]
            {
                let context = runtime
                    .screen_translation
                    .borrow_mut()
                    .take_inventory(request_id)
                    .as_ref()
                    .map(|inventory| ContextCollector::from_inventory(inventory, rect))
                    .unwrap_or_else(ContextCollector::without_window);
                runtime
                    .screen_translation
                    .borrow_mut()
                    .store_context(request_id, context);
                dispatch_screen_capture_to_ocr(request_id, rect, capture, runtime);
            }
            #[cfg(not(feature = "screen-translation"))]
            {
                let _ = capture;
                runtime.screen_translation.borrow_mut().complete(request_id);
            }
        }
        SelectionTerminal::Cancelled { request_id } => {
            #[cfg(feature = "screen-translation")]
            runtime
                .screen_translation
                .borrow_mut()
                .discard_pending(request_id);
            if let Some(cancellation) = runtime.screen_translation.borrow_mut().close(request_id) {
                apply_screen_translation_cancellation(cancellation, runtime);
            }
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=cancelled"
            ));
        }
        SelectionTerminal::Failed { request_id, error } => {
            #[cfg(feature = "screen-translation")]
            runtime
                .screen_translation
                .borrow_mut()
                .discard_pending(request_id);
            runtime.screen_translation.borrow_mut().complete(request_id);
            #[cfg(feature = "screen-translation")]
            {
                let outcome = match &error {
                    SelectionFailure::Capture(error) => screen_outcome_for_capture_error(error),
                    SelectionFailure::EmptySelection => ScreenTranslationOutcome::EmptySelection,
                    SelectionFailure::Runtime(_) => ScreenTranslationOutcome::CaptureUnavailable,
                };
                show_screen_translation_outcome(request_id, None, outcome, runtime);
            }
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=failed error={error}"
            ));
        }
    }
}

#[cfg(feature = "screen-translation")]
fn dispatch_screen_capture_to_ocr(
    request_id: RequestId,
    rect: PhysicalRect,
    capture: rust_platform_windows::screen_capture::DesktopCapture,
    runtime: &RuntimeUi,
) {
    if !runtime
        .screen_translation
        .borrow_mut()
        .selected(request_id, rect)
    {
        rust_app_core::diagnostics::log_line(&format!(
            "screen_translation request={request_id} stage=selected result=stale_ignored"
        ));
        return;
    }
    runtime
        .translation_previews
        .borrow_mut()
        .reserve_current(request_id);
    show_screen_translation_preview(
        request_id,
        rect,
        "",
        state_text(&runtime.state, UiText::ReadingSelectedText),
        &runtime.translation_previews,
        false,
    );
    let language_policy = {
        let state = runtime.state.borrow();
        OcrLanguagePolicy::Prefer(vec![
            state.translation_primary_language.code().to_owned(),
            state.translation_secondary_language.code().to_owned(),
        ])
    };
    let spawn = std::thread::Builder::new()
        .name(format!("screen-ocr-prepare-{request_id}"))
        .spawn(move || {
            let prepare_started = Instant::now();
            let result = capture.crop(rect).map(|frame| {
                let command = OcrWorkerCommand::Recognize {
                    request_id,
                    width: frame.rect.width,
                    height: frame.rect.height,
                    stride: frame.stride,
                    pixel_format: OcrPixelFormat::Rgba8,
                    language_policy,
                    rgba_base64: base64::engine::general_purpose::STANDARD.encode(&frame.pixels),
                };
                PreparedScreenOcr {
                    command,
                    crop: Arc::new(frame),
                }
            });
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=ocr_prepare duration_ms={} result={}",
                prepare_started.elapsed().as_millis(),
                if result.is_ok() { "ready" } else { "failed" }
            ));
            dispatch_runtime_event(RuntimeEvent::ScreenOcrPrepared {
                request_id,
                rect,
                result,
            });
        });
    if let Err(error) = spawn {
        runtime.screen_translation.borrow_mut().complete(request_id);
        show_screen_translation_outcome(
            request_id,
            Some(rect),
            ScreenTranslationOutcome::WorkerFailed,
            runtime,
        );
        rust_app_core::diagnostics::log_line(&format!(
            "screen_translation request={request_id} stage=ocr_prepare result=thread_failed error={error}"
        ));
    }
}

#[cfg(feature = "screen-translation")]
fn handle_prepared_screen_ocr(
    request_id: RequestId,
    rect: PhysicalRect,
    result: Result<PreparedScreenOcr, rust_platform_windows::screen_capture::CaptureError>,
    runtime: &RuntimeUi,
) {
    if !runtime
        .screen_translation
        .borrow()
        .is_recognizing(request_id)
    {
        rust_app_core::diagnostics::log_line(&format!(
            "screen_translation request={request_id} stage=ocr_prepare result=stale_ignored"
        ));
        return;
    }
    let prepared = match result {
        Ok(prepared) => prepared,
        Err(error) => {
            runtime.screen_translation.borrow_mut().complete(request_id);
            let outcome = screen_outcome_for_capture_error(&error);
            show_screen_translation_outcome(request_id, Some(rect), outcome, runtime);
            return;
        }
    };
    if runtime.workers.send_ocr(&prepared.command).is_err() {
        runtime
            .screen_translation
            .borrow_mut()
            .discard_pending(request_id);
        runtime.screen_translation.borrow_mut().complete(request_id);
        show_screen_translation_outcome(
            request_id,
            Some(rect),
            ScreenTranslationOutcome::WorkerFailed,
            runtime,
        );
        return;
    }
    runtime
        .screen_translation
        .borrow_mut()
        .store_crop(request_id, prepared.crop);
    let Some(instance_id) = runtime.workers.current_ocr_instance_id() else {
        runtime
            .screen_translation
            .borrow_mut()
            .discard_pending(request_id);
        runtime.screen_translation.borrow_mut().complete(request_id);
        show_screen_translation_outcome(
            request_id,
            Some(rect),
            ScreenTranslationOutcome::WorkerFailed,
            runtime,
        );
        return;
    };
    Timer::single_shot(std::time::Duration::from_secs(30), move || {
        dispatch_runtime_event(RuntimeEvent::OcrWatchdog {
            request_id,
            instance_id,
        });
    });
    rust_app_core::diagnostics::log_line(&format!(
        "screen_translation request={request_id} stage=ocr_dispatched width={} height={}",
        rect.width, rect.height
    ));
    // `JsonWriter::send` serializes synchronously. The owned frame, crop, and
    // command drop here; only the transient pipe payload may remain until the
    // writer flushes it. Pixels never reach history, clipboard, diagnostics,
    // or a temporary file.
}

#[cfg(feature = "screen-translation")]
fn handle_ocr_event(instance_id: u64, event: OcrWorkerEvent, runtime: &RuntimeUi) {
    match event {
        OcrWorkerEvent::Ready {
            protocol_version,
            engine,
        } => {
            let matched = protocol_version == PROTOCOL_VERSION;
            rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker lifecycle=ready protocol_matched={matched} engine={engine}"
            ));
            if !matched {
                set_status_problem(
                    &runtime.settings,
                    state_text(&runtime.state, UiText::WorkerUnavailable),
                );
            }
        }
        OcrWorkerEvent::Recognized {
            request_id,
            lines,
            processing_duration_ms,
            ..
        } => {
            runtime
                .workers
                .complete_ocr_request(request_id, instance_id);
            let Some(anchor) = runtime
                .screen_translation
                .borrow_mut()
                .recognized(request_id)
            else {
                runtime
                    .screen_translation
                    .borrow_mut()
                    .discard_pending(request_id);
                rust_app_core::diagnostics::log_line(&format!(
                    "ocr_worker request={request_id} outcome=recognized result=stale_ignored"
                ));
                return;
            };
            rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker request={request_id} outcome=recognized lines={} duration_ms={processing_duration_ms}",
                lines.len()
            ));
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some(probe) = runtime.screen_replay.as_ref() {
                let mut probe = probe.borrow_mut();
                if probe.request_id == Some(request_id) {
                    probe.record(format!(
                        "request={request_id} real_ocr_lines={}",
                        lines.len()
                    ));
                }
            }
            match reconstruct_text(&lines, MIN_ADMITTED_OCR_CONFIDENCE_MILLI) {
                Ok(reconstructed) => {
                    let Some(crop) = runtime
                        .screen_translation
                        .borrow_mut()
                        .take_crop(request_id)
                    else {
                        runtime
                            .screen_translation
                            .borrow_mut()
                            .discard_pending(request_id);
                        runtime.screen_translation.borrow_mut().complete(request_id);
                        show_screen_translation_outcome(
                            request_id,
                            Some(anchor),
                            ScreenTranslationOutcome::WorkerFailed,
                            runtime,
                        );
                        return;
                    };
                    let expected_text = reconstructed.text;
                    let blocks = screen_blocks_from_ocr(reconstructed.elements);
                    let created_unix_ms = unix_ms();
                    let context = runtime
                        .screen_translation
                        .borrow_mut()
                        .take_context(request_id);
                    let context = ContextCollector::attach_nearby_text(context, &expected_text);
                    let retained_crop = Arc::clone(&crop);
                    let retention_ms = u64::from(runtime.state.borrow().screenshot_retention_hours)
                        .saturating_mul(60 * 60 * 1_000);
                    let document = match ScreenTranslationDocument::new(
                        request_id,
                        crop,
                        blocks,
                        context,
                        created_unix_ms,
                        created_unix_ms.saturating_add(retention_ms),
                    ) {
                        Ok(document) => document,
                        Err(error) => {
                            runtime.screen_translation.borrow_mut().complete(request_id);
                            show_screen_translation_outcome(
                                request_id,
                                Some(anchor),
                                ScreenTranslationOutcome::WorkerFailed,
                                runtime,
                            );
                            rust_app_core::diagnostics::log_line(&format!(
                                "screen_translation request={request_id} stage=document result=failed error={error:?}"
                            ));
                            return;
                        }
                    };
                    debug_assert_eq!(
                        document.context.nearby_text.as_deref(),
                        Some(expected_text.as_str())
                    );
                    let crop_rect = document.crop_rect();
                    let block_count = document.source_blocks().len();
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=document blocks={block_count} crop_width={} crop_height={} context_partial={} created_unix_ms={}",
                        crop_rect.width,
                        crop_rect.height,
                        document.context.partial,
                        document.created_unix_ms,
                    ));
                    if let Some(store) = runtime.screenshot_store.as_ref() {
                        let metadata = ScreenshotMetadata {
                            request_id,
                            revision: document.revision(),
                            crop_x: crop_rect.x,
                            crop_y: crop_rect.y,
                            width: crop_rect.width,
                            height: crop_rect.height,
                            blocks: document.source_blocks(),
                            translations: Vec::new(),
                            context: document.context.clone(),
                            source_language: None,
                            target_language: None,
                            created_unix_ms: document.created_unix_ms,
                            expires_unix_ms: document.expires_unix_ms,
                            crop_filename: String::new(),
                        };
                        let store = Arc::clone(store);
                        let _ = std::thread::Builder::new()
                            .name("screen-snapshot-write".to_owned())
                            .spawn(move || {
                                let result = store.persist_crop(metadata, retained_crop.as_ref());
                                match result {
                                    Ok(_) => {
                                        let report = store.cleanup(unix_ms());
                                        rust_app_core::diagnostics::log_line(&format!(
                                            "screen_snapshot request={request_id} result=stored cleanup_removed={} cleanup_errors={}",
                                            report.removed_entries + report.removed_orphans,
                                            report.error_count
                                        ));
                                        for removed_request_id in report.removed_request_ids {
                                            dispatch_runtime_event(
                                                RuntimeEvent::ScreenPixelsExpired {
                                                    request_id: removed_request_id,
                                                },
                                            );
                                        }
                                    }
                                    Err(error) => rust_app_core::diagnostics::log_line(&format!(
                                        "screen_snapshot request={request_id} result=failed error={error}"
                                    )),
                                }
                            });
                    }
                    runtime
                        .screen_translation
                        .borrow_mut()
                        .insert_document(document);
                    let (source_language, target_language) = {
                        #[cfg(all(feature = "inspect", feature = "screen-translation"))]
                        {
                            let intentional_failure =
                                runtime.screen_replay.as_ref().is_some_and(|probe| {
                                    let probe = probe.borrow();
                                    probe.request_id == Some(request_id)
                                        && probe.step() == Some(ReplayStep::WorkerFailure)
                                });
                            if intentional_failure {
                                (
                                    Some(TranslationLanguage::English),
                                    Some(TranslationLanguage::English),
                                )
                            } else {
                                (None, None)
                            }
                        }
                        #[cfg(not(all(feature = "inspect", feature = "screen-translation")))]
                        {
                            (None, None)
                        }
                    };
                    let translation_dispatched = begin_screen_translation(
                        ScreenTranslationRequest {
                            request_id,
                            anchor,
                            request_started_unix_ms: created_unix_ms,
                            source_language,
                            target_language,
                        },
                        &runtime.settings,
                        &runtime.translation_previews,
                        &runtime.state,
                        &runtime.workers,
                        &runtime.screen_translation,
                    );
                    #[cfg(not(all(feature = "inspect", feature = "screen-translation")))]
                    let _ = translation_dispatched;
                    #[cfg(all(feature = "inspect", feature = "screen-translation"))]
                    if let Some(probe) = runtime.screen_replay.as_ref() {
                        let mut probe = probe.borrow_mut();
                        if probe.request_id == Some(request_id) {
                            probe.translation_dispatched = translation_dispatched;
                            probe.record(format!("request={request_id} translate_blocks_send_accepted={translation_dispatched}"));
                        }
                    }
                }
                Err(_) => {
                    runtime
                        .screen_translation
                        .borrow_mut()
                        .discard_pending(request_id);
                    runtime.screen_translation.borrow_mut().complete(request_id);
                    show_screen_translation_outcome(
                        request_id,
                        Some(anchor),
                        ScreenTranslationOutcome::NoText,
                        runtime,
                    );
                }
            }
        }
        OcrWorkerEvent::Failed {
            request_id, kind, ..
        } => {
            runtime
                .workers
                .complete_ocr_request(request_id, instance_id);
            runtime
                .screen_translation
                .borrow_mut()
                .discard_pending(request_id);
            let Some(anchor) = runtime.screen_translation.borrow_mut().complete(request_id) else {
                return;
            };
            let outcome = match kind {
                OcrFailureKind::ModelUnavailable => ScreenTranslationOutcome::ModelUnavailable,
                OcrFailureKind::LanguageUnavailable => {
                    ScreenTranslationOutcome::LanguageUnavailable
                }
                OcrFailureKind::NoText => ScreenTranslationOutcome::NoText,
                OcrFailureKind::InvalidImage => ScreenTranslationOutcome::CaptureUnavailable,
                OcrFailureKind::Engine => ScreenTranslationOutcome::WorkerFailed,
                OcrFailureKind::Cancelled => ScreenTranslationOutcome::Cancelled,
            };
            show_screen_translation_outcome(request_id, Some(anchor), outcome, runtime);
            rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker request={request_id} outcome=failed kind={kind:?}"
            ));
        }
        OcrWorkerEvent::Cancelled { request_id } => {
            runtime
                .workers
                .complete_ocr_request(request_id, instance_id);
            runtime
                .screen_translation
                .borrow_mut()
                .discard_pending(request_id);
            if let Some(anchor) = runtime.screen_translation.borrow_mut().complete(request_id) {
                show_screen_translation_outcome(
                    request_id,
                    Some(anchor),
                    ScreenTranslationOutcome::Cancelled,
                    runtime,
                );
            }
            rust_app_core::diagnostics::log_line(&format!(
                "ocr_worker request={request_id} outcome=cancelled"
            ));
        }
    }
}

#[cfg(feature = "screen-translation")]
fn show_screen_translation_outcome(
    request_id: RequestId,
    anchor: Option<PhysicalRect>,
    outcome: ScreenTranslationOutcome,
    runtime: &RuntimeUi,
) {
    let message = screen_translation_outcome_text(runtime.state.borrow().ui_locale, outcome);
    if runtime
        .translation_previews
        .borrow()
        .slot_for_request(request_id)
        .is_none()
    {
        runtime
            .translation_previews
            .borrow_mut()
            .reserve_current(request_id);
    }
    show_translation_preview_at(
        request_id,
        anchor.map(TranslationPreviewAnchor::ScreenRegion),
        "",
        message,
        &runtime.translation_previews,
        true,
    );
    set_status_problem(&runtime.settings, message);
}

#[cfg(feature = "screen-translation")]
fn begin_screen_translation(
    request: ScreenTranslationRequest,
    settings: &slint::Weak<SettingsWindow>,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    screen_translation: &Rc<RefCell<ScreenTranslationCoordinator>>,
) -> bool {
    let ScreenTranslationRequest {
        request_id,
        anchor,
        request_started_unix_ms,
        source_language,
        target_language,
    } = request;
    let Some((base_revision, blocks, context)) =
        screen_translation.borrow().translation_input(request_id)
    else {
        screen_translation.borrow_mut().complete(request_id);
        let message = state_message(
            state,
            UiText::CouldNotStartTranslation,
            &[("error", "screen result unavailable")],
        );
        show_screen_translation_preview(
            request_id,
            anchor,
            "",
            &message,
            translation_previews,
            true,
        );
        return false;
    };
    let text = blocks
        .iter()
        .map(|block| block.source_text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let (
        enabled,
        selected_tier,
        selected_catalog_id,
        path_ready,
        waits_for_local,
        reload_on_next_use,
        keep_warm,
    ) = {
        let state = state.borrow();
        (
            state.translation_enabled,
            state.selected_translation_tier,
            state.selected_translation_model_id.clone(),
            state.translation_path_can_start(),
            state.translation_waits_for_local(),
            state.translation_reload_on_next_use,
            state.translation_keep_warm,
        )
    };
    if !enabled {
        screen_translation.borrow_mut().complete(request_id);
        show_screen_translation_preview(
            request_id,
            anchor,
            &text,
            state_text(state, UiText::TranslationDisabled),
            translation_previews,
            true,
        );
        return false;
    }
    if !(path_ready || selected_tier.is_some() && waits_for_local) {
        screen_translation.borrow_mut().complete(request_id);
        show_screen_translation_preview(
            request_id,
            anchor,
            &text,
            state_text(state, UiText::ChooseTranslationRoute),
            translation_previews,
            true,
        );
        return false;
    }
    if !path_ready {
        state.borrow_mut().queued_screen_translation = Some(ScreenTranslationRequest {
            request_id,
            anchor,
            request_started_unix_ms,
            source_language,
            target_language,
        });
        if reload_on_next_use
            && let Some(catalog_id) = selected_catalog_id
            && let Err(error) = workers
                .translation
                .send(&TranslationWorkerCommand::SelectModel {
                    catalog_id,
                    keep_warm,
                })
        {
            state.borrow_mut().queued_screen_translation = None;
            screen_translation.borrow_mut().complete(request_id);
            let message = state_message(
                state,
                UiText::CouldNotStartTranslation,
                &[("error", &error.to_string())],
            );
            show_screen_translation_preview(
                request_id,
                anchor,
                &text,
                &message,
                translation_previews,
                true,
            );
            return false;
        }
        show_screen_translation_preview(
            request_id,
            anchor,
            &text,
            state_text(state, UiText::LocalTranslationLoading),
            translation_previews,
            false,
        );
        return false;
    }

    let policy = {
        let state = state.borrow();
        TranslationLanguagePolicy {
            primary: state.translation_primary_language,
            secondary: state.translation_secondary_language,
        }
    };
    show_screen_translation_preview(request_id, anchor, &text, "", translation_previews, false);
    let command = TranslationWorkerCommand::TranslateBlocks {
        request_id,
        base_revision,
        blocks,
        context,
        source_language,
        target_language,
        language_policy: policy,
    };
    if let Err(error) = workers.translation.send(&command) {
        screen_translation.borrow_mut().complete(request_id);
        let message = state_message(
            state,
            UiText::CouldNotStartTranslation,
            &[("error", &error.to_string())],
        );
        show_screen_translation_preview(
            request_id,
            anchor,
            "",
            &message,
            translation_previews,
            true,
        );
        false
    } else {
        set_indicator_status(settings, state_text(state, UiText::TranslatingNow));
        true
    }
}

fn handle_input_event(event: InputHostEvent, runtime: &RuntimeUi) {
    let settings = &runtime.settings;
    let recording_overlays = &runtime.recording_overlays;
    let chooser = &runtime.chooser;
    let chooser_presenter = &runtime.chooser_presenter;
    let action_palette = &runtime.action_palette;
    let action_palette_presenter = &runtime.action_palette_presenter;
    let translation_previews = &runtime.translation_previews;
    let state = &runtime.state;
    let workers = &runtime.workers;
    let indicator_animation = &runtime.indicator_animation;

    match event {
        InputHostEvent::Ready { protocol_version } => {
            if protocol_version == PROTOCOL_VERSION {
                state.borrow_mut().input_ready = true;
                let operation_id = RequestId::new();
                let bindings = effective_hotkey_bindings(state.borrow().hotkeys.clone());
                state.borrow_mut().pending_hotkeys = Some((operation_id, bindings.clone()));
                if let Err(error) = workers.input.send(&InputHostCommand::SetHotkeys {
                    operation_id,
                    configuration: HotkeyConfiguration { bindings },
                }) {
                    state.borrow_mut().pending_hotkeys = None;
                    let error = error.to_string();
                    set_status_problem(
                        settings,
                        &state_message(state, UiText::CouldNotApplyShortcuts, &[("error", &error)]),
                    );
                    return;
                }
                let avoid_clipboard = state.borrow().avoid_clipboard_insertion;
                let _ = workers
                    .input
                    .send(&InputHostCommand::SetInsertionPolicy { avoid_clipboard });
                set_runtime_status(settings, state_text(state, UiText::InputReady));
            } else {
                state.borrow_mut().input_ready = false;
                set_runtime_status(settings, state_text(state, UiText::WorkerMismatch));
                set_indicator_status(settings, state_text(state, UiText::InputProtocolMismatch));
            }
        }
        #[cfg(feature = "screen-translation")]
        InputHostEvent::ScreenCaptureRequested { request_id } => {
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=requested"
            ));
            let old_screen_request = {
                let old = runtime.translation_previews.borrow().current_request;
                old.filter(|old| {
                    let coordinator = runtime.screen_translation.borrow();
                    coordinator.anchor(*old).is_some() || coordinator.source_text(*old).is_some()
                })
            };
            let begin = runtime.screen_translation.borrow_mut().begin(request_id);
            let ScreenTranslationBegin::Started(cancellation) = begin else {
                let ScreenTranslationBegin::AlreadySelecting { active_request_id } = begin else {
                    unreachable!()
                };
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation request={request_id} stage=requested result=already_selecting_ignored active_request={active_request_id}"
                ));
                return;
            };
            if let Some(store) = runtime.screenshot_store.as_ref() {
                store.register_capture(request_id);
            }
            let retired = runtime
                .screen_result_windows
                .borrow_mut()
                .retire_unpinned("superseded");
            if runtime
                .active_screen_editor
                .borrow()
                .as_ref()
                .is_some_and(|active| retired.contains(&active.state.key().request_id))
            {
                close_screen_translation_editor(runtime, false);
            }
            for retired_request in retired {
                runtime
                    .screen_translation
                    .borrow_mut()
                    .close(retired_request);
            }
            if let Some(slot) = take_superseded_screen_compact(
                &runtime.translation_previews,
                &runtime.screen_translation,
                old_screen_request,
                request_id,
            ) {
                retire_runtime_translation_preview(&slot, "new_screen_translation", runtime);
            }
            if let Some(cancellation) = cancellation {
                apply_screen_translation_cancellation(cancellation, runtime);
            }
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some((rect, capture, timeout)) =
                runtime.screen_replay.as_ref().and_then(|probe| {
                    let probe = probe.borrow();
                    (probe.request_id == Some(request_id)).then(|| {
                        (
                            probe.rect,
                            probe.capture.clone(),
                            probe.step() == Some(ReplayStep::SelectionTimeout),
                        )
                    })
                })
            {
                let terminal = if timeout {
                    SelectionTerminal::Failed {
                        request_id,
                        error: SelectionFailure::Runtime("inspect replay selection timeout".into()),
                    }
                } else {
                    SelectionTerminal::Selected {
                        request_id,
                        rect,
                        capture,
                    }
                };
                handle_screen_selection_terminal(terminal, runtime);
                return;
            }
            let compact_exclusion = cloak_compact_previews_owned(&runtime.translation_previews);
            let mut excluded_overlays = match compact_exclusion {
                Ok(surfaces) => surfaces,
                Err(error) => {
                    runtime.screen_translation.borrow_mut().complete(request_id);
                    show_screen_translation_outcome(
                        request_id,
                        None,
                        ScreenTranslationOutcome::CaptureUnavailable,
                        runtime,
                    );
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=capture_exclusion result=failed error={error}"
                    ));
                    return;
                }
            };
            match runtime
                .screen_result_windows
                .borrow()
                .cloak_presented_for_capture()
            {
                Ok(mut surfaces) => excluded_overlays.append(&mut surfaces),
                Err(error) => {
                    for surface in &excluded_overlays {
                        let _ = surface.restore();
                    }
                    runtime.screen_translation.borrow_mut().complete(request_id);
                    show_screen_translation_outcome(
                        request_id,
                        None,
                        ScreenTranslationOutcome::CaptureUnavailable,
                        runtime,
                    );
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=screen_result_exclusion result=failed error={error}"
                    ));
                    return;
                }
            }
            let excluded_on_spawn_failure = excluded_overlays.clone();
            let capture_backend = Arc::clone(&runtime.screen_capture_backend);
            let spawn = std::thread::Builder::new()
                .name(format!("screen-capture-{request_id}"))
                .spawn(move || {
                    let capture_started = Instant::now();
                    let result = capture_backend.capture().map(|capture| {
                        let context_started = Instant::now();
                        let inventory = rust_platform_windows::WindowInventory::capture(
                            capture.generation(),
                            capture.bounds(),
                            Instant::now() + std::time::Duration::from_millis(120),
                        );
                        match &inventory {
                            Ok(inventory) => rust_app_core::diagnostics::log_line(&format!(
                                "screen_translation request={request_id} stage=window_snapshot duration_ms={} windows={} generation={} result=ready",
                                context_started.elapsed().as_millis(),
                                inventory.len(),
                                inventory.generation(),
                            )),
                            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                                "screen_translation request={request_id} stage=window_snapshot duration_ms={} result=unavailable error_kind={}",
                                context_started.elapsed().as_millis(),
                                match error {
                                    rust_platform_windows::WindowSnapshotError::DeadlineExceeded => "deadline",
                                    rust_platform_windows::WindowSnapshotError::Enumeration(_) => "enumeration",
                                },
                            )),
                        }
                        PreparedScreenCapture {
                            capture,
                            inventory: inventory.ok(),
                        }
                    });
                    match &result {
                        Ok(prepared) => rust_app_core::diagnostics::log_line(&format!(
                            "screen_translation request={request_id} stage=capture duration_ms={} backend={} outputs={}",
                            capture_started.elapsed().as_millis(),
                            match prepared.capture.backend() {
                                rust_platform_windows::screen_capture::CaptureBackendKind::Primary => "dxgi",
                                rust_platform_windows::screen_capture::CaptureBackendKind::Fallback => "gdi",
                                rust_platform_windows::screen_capture::CaptureBackendKind::Unknown => "unknown",
                            },
                            prepared.capture.output_bounds().len()
                        )),
                        Err(error) => rust_app_core::diagnostics::log_line(&format!(
                            "screen_translation request={request_id} stage=capture duration_ms={} result=failed error={error}",
                            capture_started.elapsed().as_millis()
                        )),
                    }
                    dispatch_runtime_event(RuntimeEvent::ScreenCapturePrepared {
                        request_id,
                        excluded_overlays,
                        result,
                    });
                });
            if let Err(error) = spawn {
                for surface in excluded_on_spawn_failure {
                    let _ = surface.restore();
                }
                runtime.screen_translation.borrow_mut().complete(request_id);
                show_screen_translation_outcome(
                    request_id,
                    None,
                    ScreenTranslationOutcome::CaptureUnavailable,
                    runtime,
                );
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation request={request_id} stage=capture result=thread_failed error={error}"
                ));
            }
        }
        #[cfg(not(feature = "screen-translation"))]
        InputHostEvent::ScreenCaptureRequested { request_id } => {
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation request={request_id} stage=requested result=feature_unavailable"
            ));
        }
        InputHostEvent::ScreenCaptureCancelled { request_id } => {
            let terminal = runtime.screen_selection.borrow_mut().cancel(request_id);
            if let Some(terminal) = terminal {
                handle_screen_selection_terminal(terminal, runtime);
            } else {
                runtime.screen_translation.borrow_mut().close(request_id);
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation request={request_id} stage=stale_cancel_ignored"
                ));
            }
        }
        InputHostEvent::HotkeysApplied {
            operation_id,
            configuration,
            failures,
        } => {
            let applied = {
                let mut state = state.borrow_mut();
                if commit_hotkeys_if_pending(
                    &mut state,
                    operation_id,
                    configuration.bindings,
                    &failures,
                ) {
                    let _ = save_settings(&state);
                    true
                } else {
                    false
                }
            };
            if applied {
                if let Some(settings) = settings.upgrade() {
                    settings.set_hotkey_capture_index(-1);
                    settings.set_hotkey_capture_label("".into());
                    settings.set_hotkey_capture_value("".into());
                    settings.set_hotkey_capture_valid(false);
                }
                sync_action_settings(settings, state);
                if let Some(failure) = failures.first() {
                    sync_hotkey_capture_status(settings, state, Some(failure.reason));
                    rust_app_core::diagnostics::log_line(&format!(
                        "hotkeys action={:?} result=rejected reason={:?} detail={}",
                        failure.action, failure.reason, failure.message
                    ));
                } else {
                    set_indicator_status(settings, state_text(state, UiText::ShortcutsUpdated));
                }
            }
        }
        InputHostEvent::HotkeysRejected {
            operation_id,
            action,
            reason,
            message,
        } => {
            let rejected = {
                let mut state = state.borrow_mut();
                reject_hotkeys_if_pending(&mut state, operation_id, action, reason)
            };
            if rejected {
                if let Some(settings) = settings.upgrade()
                    && settings.get_hotkey_capture_index() >= 0
                {
                    settings.set_hotkey_capture_valid(false);
                }
                sync_action_settings(settings, state);
                sync_hotkey_capture_status(settings, state, Some(reason));
                rust_app_core::diagnostics::log_line(&format!(
                    "hotkeys action={action:?} result=rejected reason={reason:?} detail={message}"
                ));
            }
        }
        InputHostEvent::PauseChanged { paused } => {
            state.borrow_mut().input_paused = paused;
            sync_action_settings(settings, state);
            set_indicator_status(
                settings,
                if paused {
                    state_text(state, UiText::InputPaused)
                } else {
                    state_text(state, UiText::InputResumed)
                },
            );
        }
        InputHostEvent::DictationBegin {
            session_id,
            started_unix_ms,
            target,
        } => {
            if !state.borrow().input_ready {
                set_status_problem(settings, state_text(state, UiText::InputWorkerNotReady));
                return;
            }
            let (path_ready, worker_ready, waits_for_local) = {
                let state = state.borrow();
                (
                    state.speech_path_can_start(),
                    state.speech_worker_ready,
                    state.speech_waits_for_local(),
                )
            };
            if !path_ready {
                let message = if !worker_ready {
                    UiText::SpeechServiceStarting
                } else if waits_for_local {
                    UiText::SpeechModelPreparing
                } else {
                    UiText::NoTranscriptionProviderReady
                };
                set_indicator_status(settings, state_text(state, message));
                return;
            }
            if state.borrow().indicator_session.is_some() {
                set_indicator_status(
                    settings,
                    state_text(state, UiText::FinishingPreviousDictation),
                );
                return;
            }
            let begin = state
                .borrow_mut()
                .registry
                .begin(session_id, started_unix_ms);
            match begin {
                Ok(BeginDisposition::Started) => {}
                Ok(BeginDisposition::AlreadyKnown) => return,
                Err(_) => {
                    let _ = workers
                        .speech
                        .send(&SpeechWorkerCommand::Cancel { session_id });
                    return;
                }
            }
            {
                let mut state = state.borrow_mut();
                state.targets.insert(session_id, target);
                state.indicator_session = Some(session_id);
                state.active_visualizer_style = Some(state.visualizer_style);
            }
            if let Err(error) = workers.speech.send(&SpeechWorkerCommand::Begin {
                session_id,
                started_unix_ms,
            }) {
                let mut state = state.borrow_mut();
                let message = render(
                    active_locale(state.ui_locale),
                    UiText::CouldNotStartRecording,
                    &[("error", &error.to_string())],
                );
                let _ = state.registry.fail(session_id);
                state.targets.remove(&session_id);
                state.indicator_session = None;
                state.active_visualizer_style = None;
                set_status_problem(settings, &message);
                return;
            }
            let active_style = state.borrow().active_visualizer_style.unwrap_or_default();
            indicator_animation.borrow_mut().set_style(active_style);
            if active_style == VisualizerStyle::Off {
                indicator_animation.borrow_mut().stop();
            } else {
                indicator_animation.borrow_mut().begin_listening();
            }
            recording_overlays.set_style(active_style);
            if active_style.uses_external_renderer() {
                if let Err(error) = workers.send_companion(&CompanionRendererCommand::Activate {
                    session_id,
                    target_window_handle: target.window_handle,
                }) {
                    rust_app_core::diagnostics::log_line(&format!(
                        "companion_renderer transition=failed stage=activate session={session_id} error={error}"
                    ));
                    set_indicator_status(
                        settings,
                        &state_message(
                            state,
                            UiText::ListeningCompanionUnavailable,
                            &[("error", &error.to_string())],
                        ),
                    );
                }
            } else {
                recording_overlays.retire_inactive_surface(active_style);
                if let Some(overlay) = recording_overlays.for_style(active_style) {
                    overlay.indicator.set_processing(false);
                    overlay.indicator.set_signal_opacity(0.54);
                    overlay.indicator.set_disc_opacity(1.0);
                    show_indicator(
                        &overlay.indicator,
                        &overlay.presenter,
                        overlay.transparent,
                        Some(target.window_handle),
                    );
                }
            }
            set_indicator_status(settings, state_text(state, UiText::Listening));
        }
        InputHostEvent::DictationEnd {
            session_id,
            target: _,
        } => {
            state
                .borrow_mut()
                .release_started
                .insert(session_id, Instant::now());
            let disposition = state.borrow_mut().registry.end(session_id);
            if disposition == Ok(EndDisposition::StartProcessing)
                && state.borrow().indicator_session == Some(session_id)
            {
                if let Err(error) = workers
                    .speech
                    .send(&SpeechWorkerCommand::End { session_id })
                {
                    let mut state = state.borrow_mut();
                    let message = render(
                        active_locale(state.ui_locale),
                        UiText::CouldNotFinishRecording,
                        &[("error", &error.to_string())],
                    );
                    let companion_active = state
                        .active_visualizer_style
                        .is_some_and(VisualizerStyle::uses_external_renderer);
                    let _ = state.registry.fail(session_id);
                    state.targets.remove(&session_id);
                    state.indicator_session = None;
                    state.active_visualizer_style = None;
                    if companion_active {
                        let _ = workers.send_companion(&CompanionRendererCommand::Dismiss {
                            session_id: Some(session_id),
                        });
                    }
                    indicator_animation.borrow_mut().stop();
                    recording_overlays.retire_all("speech_end_send_failed");
                    set_status_problem(settings, &message);
                    return;
                }
                if state.borrow().active_visualizer_style != Some(VisualizerStyle::Off) {
                    indicator_animation.borrow_mut().begin_processing();
                }
                if state
                    .borrow()
                    .active_visualizer_style
                    .is_some_and(VisualizerStyle::uses_external_renderer)
                {
                    let _ = workers
                        .send_companion(&CompanionRendererCommand::Processing { session_id });
                }
                if let Some(active_style) = state.borrow().active_visualizer_style
                    && !active_style.uses_external_renderer()
                    && let Some(overlay) = recording_overlays.for_style(active_style)
                {
                    let indicator = &overlay.indicator;
                    indicator.set_processing(true);
                    indicator.set_signal_opacity(0.72);
                    indicator.set_disc_opacity(0.82);
                }
                set_indicator_status(settings, state_text(state, UiText::ProcessingLocally));
            }
        }
        InputHostEvent::InsertLastRequested { target } => {
            let resolution = state.borrow_mut().request_insert_last(target);
            match resolution {
                InsertLastResolution::Insert(transcript) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=insert session={} target_pid={}",
                        transcript.session_id, target.process_id
                    ));
                    let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                        transcript,
                        target,
                        add_trailing_space: false,
                    });
                }
                InsertLastResolution::Await(session_id) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=await session={session_id} target_pid={}",
                        target.process_id
                    ));
                    set_indicator_status(
                        settings,
                        state_text(state, UiText::DictationStillProcessing),
                    );
                }
                InsertLastResolution::LoadHistory => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=load_history target_pid={}",
                        target.process_id
                    ));
                    let _ = workers.speech.send(&SpeechWorkerCommand::GetHistory {
                        limit: HISTORY_LIMIT,
                    });
                    set_indicator_status(
                        settings,
                        state_text(state, UiText::LoadingLatestTranscript),
                    );
                }
                InsertLastResolution::Unavailable(_message) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=unavailable target_pid={}",
                        target.process_id
                    ));
                    set_indicator_status(settings, state_text(state, UiText::NoTranscriptToInsert));
                }
            }
        }
        InputHostEvent::SelectionCaptureStarted {
            request_id,
            intent,
            target: _,
            started_unix_ms,
        } => {
            let mut app = state.borrow_mut();
            app.pending_selection_captures
                .insert(request_id, started_unix_ms);
            app.selection_release_gates
                .insert(request_id, SelectionReleaseGate::Pending);
            drop(app);
            if intent == TextActionIntent::Translate {
                let slot = translation_previews.borrow().current();
                if !slot.preview.get_pinned() {
                    retire_runtime_translation_preview(&slot, "new_translation", runtime);
                    translation_previews.borrow_mut().current_request = None;
                }
            }
            set_indicator_status(settings, state_text(state, UiText::ReadingSelectedText));
        }
        InputHostEvent::SelectionCaptured {
            request_id,
            intent,
            target,
            text,
            timing,
        } => {
            let request_started_unix_ms = {
                let mut app = state.borrow_mut();
                if timing.release_wait_ms.is_some() {
                    app.selection_release_gates
                        .insert(request_id, SelectionReleaseGate::Released);
                }
                app.pending_selection_captures
                    .remove(&request_id)
                    .unwrap_or_else(unix_ms)
            };
            rust_app_core::diagnostics::log_line(&format!(
                "translation_pipeline request={} stage=selection_captured release_wait_ms={:?} \
uia_ms={:?} clipboard_ms={:?} total_ms={}",
                request_id,
                timing.release_wait_ms,
                timing.uia_ms,
                timing.clipboard_ms,
                timing.total_ms,
            ));
            handle_text_action(
                request_id,
                intent,
                target,
                text,
                request_started_unix_ms,
                settings,
                chooser,
                chooser_presenter,
                action_palette,
                action_palette_presenter,
                translation_previews,
                state,
                workers,
            );
        }
        InputHostEvent::SelectionInputReleased {
            request_id,
            release_wait_ms,
        } => {
            let deferred = {
                let mut app = state.borrow_mut();
                let (next, deferred) = release_gate_after_input_release(
                    app.selection_release_gates.remove(&request_id),
                );
                if let Some(next) = next {
                    app.selection_release_gates.insert(request_id, next);
                }
                deferred
            };
            rust_app_core::diagnostics::log_line(&format!(
                "translation_pipeline request={} stage=input_released release_wait_ms={}",
                request_id, release_wait_ms,
            ));
            if let Some((target, text)) = deferred {
                let _ = workers.input.send(&InputHostCommand::ReplaceSelection {
                    request_id,
                    target,
                    text,
                });
            }
        }
        InputHostEvent::SelectionInputReleaseFailed {
            request_id,
            message,
        } => {
            let had_deferred_replacement = {
                let mut app = state.borrow_mut();
                let (next, deferred) = release_gate_after_input_release_failure(
                    app.selection_release_gates.remove(&request_id),
                    message.clone(),
                );
                if let Some(next) = next {
                    app.selection_release_gates.insert(request_id, next);
                }
                deferred
            };
            rust_app_core::diagnostics::log_line(&format!(
                "translation_pipeline request={} stage=input_release_failed",
                request_id,
            ));
            if had_deferred_replacement {
                set_indicator_status(
                    settings,
                    &state_message(state, UiText::TranslationReadyBut, &[("message", &message)]),
                );
            }
        }
        InputHostEvent::SelectionCaptureFailed {
            request_id,
            message,
        } => {
            let mut app = state.borrow_mut();
            app.pending_selection_captures.remove(&request_id);
            app.selection_release_gates.remove(&request_id);
            drop(app);
            set_indicator_status(
                settings,
                &state_message(state, UiText::SelectionIssue, &[("message", &message)]),
            );
        }
        InputHostEvent::InsertionDispatched { session_id } => {
            // The paste command reached the target. Waiting for the clipboard
            // transaction to settle (up to 2.5 s) kept the visualizer on the
            // screen after the text had already appeared.
            let owns_indicator = {
                let mut state = state.borrow_mut();
                if state.indicator_session == Some(session_id) {
                    state.indicator_session = None;
                    state.active_visualizer_style = None;
                    true
                } else {
                    false
                }
            };
            if owns_indicator {
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("insertion_dispatched");
            }
        }
        InputHostEvent::InsertionCompleted {
            session_id,
            method,
            delivery,
            dispatch_duration_ms,
        } => {
            runtime.insertion_notice.resolve(session_id);
            let (owns_indicator, total_after_release_ms, transcript_latency) = {
                let mut state = state.borrow_mut();
                state.targets.remove(&session_id);
                let total = state
                    .release_started
                    .remove(&session_id)
                    .map(|started| started.elapsed().as_millis() as u64);
                let transcript_latency = state
                    .transcripts
                    .get(&session_id)
                    .map(|transcript| transcript.latency);
                let owns_indicator = if state.indicator_session == Some(session_id) {
                    state.indicator_session = None;
                    state.active_visualizer_style = None;
                    true
                } else {
                    false
                };
                (owns_indicator, total, transcript_latency)
            };
            if owns_indicator {
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("insertion_completed");
            }
            let method_name = match method {
                rust_protocol::InsertionMethod::NativeEditMessage => "native_edit_message",
                rust_protocol::InsertionMethod::ClipboardTransaction => "clipboard_transaction",
                rust_protocol::InsertionMethod::WezTermCli => "wezterm_cli",
                rust_protocol::InsertionMethod::UnicodeInput => "unicode_input",
            };
            let delivery_name = match delivery {
                rust_protocol::InsertionDelivery::Committed => "committed",
                rust_protocol::InsertionDelivery::PayloadRead => "payload_read",
                rust_protocol::InsertionDelivery::Verified => "verified",
                rust_protocol::InsertionDelivery::Dispatched => "dispatched",
            };
            if let Some(total) = total_after_release_ms {
                let latency = transcript_latency.unwrap_or_default();
                rust_app_core::diagnostics::log_line(&format!(
                    "latency session={} release_to_insert_ms={} insertion_dispatch_ms={} \
audio_finalize_ms={} preprocessing_ms={} final_queue_wait_ms={} inference_ms={} \
speculative_reuse={} method={} delivery={}",
                    session_id,
                    total,
                    dispatch_duration_ms,
                    latency.audio_finalize_ms,
                    latency.preprocessing_ms,
                    latency.final_queue_wait_ms,
                    latency.inference_ms,
                    latency.speculative_reuse,
                    method_name,
                    delivery_name,
                ));
            }
            let elapsed = total_after_release_ms.unwrap_or(dispatch_duration_ms);
            // User-facing status carries no internal session id.
            let elapsed = elapsed.to_string();
            let status = match delivery {
                rust_protocol::InsertionDelivery::Committed => state_message(
                    state,
                    UiText::InsertedTargetAdapter,
                    &[("milliseconds", &elapsed)],
                ),
                rust_protocol::InsertionDelivery::PayloadRead => state_message(
                    state,
                    UiText::InsertedPrivateTransaction,
                    &[("milliseconds", &elapsed)],
                ),
                rust_protocol::InsertionDelivery::Verified => state_message(
                    state,
                    UiText::VerifiedInsertion,
                    &[("milliseconds", &elapsed)],
                ),
                rust_protocol::InsertionDelivery::Dispatched => state_message(
                    state,
                    UiText::PasteDispatched,
                    &[("milliseconds", &elapsed)],
                ),
            };
            set_indicator_status(settings, &status);
        }
        InputHostEvent::Recovered { session_id: _ } => {
            set_indicator_status(settings, state_text(state, UiText::LatestTranscriptCopied));
        }
        InputHostEvent::SelectionReplaced { request_id } => {
            let _ = request_id;
            set_indicator_status(settings, state_text(state, UiText::ReplacementSent));
        }
        InputHostEvent::TextCopied { request_id } => {
            set_indicator_status(settings, state_text(state, UiText::TranslationCopied));
            #[cfg(feature = "screen-translation")]
            {
                let window_handle = state.borrow_mut().screen_copy_requests.remove(&request_id);
                let slot = window_handle.and_then(|window_handle| {
                    runtime
                        .screen_result_windows
                        .borrow()
                        .slot_by_handle(window_handle)
                });
                if let Some(slot) = slot {
                    slot.window.set_screen_copied(true);
                }
            }
            #[cfg(not(feature = "screen-translation"))]
            let _ = request_id;
        }
        InputHostEvent::Failed {
            session_id,
            code,
            message,
        } => {
            if state
                .borrow()
                .active_visualizer_style
                .is_some_and(VisualizerStyle::uses_external_renderer)
            {
                let _ = workers.send_companion(&CompanionRendererCommand::Dismiss { session_id });
            }
            rust_app_core::diagnostics::log_line(&format!(
                "input_host_action result=failed session={session_id:?} code={code} message={message}"
            ));
            let owns_indicator = {
                let mut state = state.borrow_mut();
                if session_id.is_some() && state.indicator_session == session_id {
                    state.indicator_session = None;
                    state.active_visualizer_style = None;
                    true
                } else {
                    false
                }
            };
            if owns_indicator {
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("insertion_failed");
            }
            // A dictation that could not be inserted is shown to the user with
            // a way to insert or copy it. The status line alone was invisible.
            let notice = insertion_notice::failure_reason(&code).and_then(|reason| {
                let state = state.borrow();
                let transcript = session_id.and_then(|id| state.transcripts.get(&id).cloned())?;
                Some((transcript, reason, active_locale(state.ui_locale)))
            });
            if let Some((transcript, reason, locale)) = notice {
                set_indicator_status(settings, text(locale, reason));
                runtime.insertion_notice.show(transcript, reason, locale);
                return;
            }
            set_indicator_status(
                settings,
                &state_message(
                    state,
                    UiText::TechnicalFailure,
                    &[("code", &code), ("message", &message)],
                ),
            );
        }
    }
}

/// Accepts the input host's answer to the pending shortcut request.
///
/// `registered` is what Windows accepted; `failures` are the actions it
/// refused. The user's choice (`state.hotkeys`) takes the requested set,
/// except where the request *changed* an action and Windows refused the new
/// chord: that edit did not happen, so the previous choice stays. A refused
/// chord the user did not change — another application holding it right now —
/// stays chosen and is reported, so it is retried on the next start instead of
/// being saved as switched off.
fn commit_hotkeys_if_pending(
    state: &mut AppState,
    operation_id: RequestId,
    registered: HotkeyBindings,
    failures: &[HotkeyFailure],
) -> bool {
    let Some((pending, requested)) = state.pending_hotkeys.take() else {
        return false;
    };
    if pending != operation_id {
        state.pending_hotkeys = Some((pending, requested));
        return false;
    }
    let mut chosen = requested.clone();
    for failure in failures {
        let action = failure.action;
        if requested.binding(action) != state.hotkeys.binding(action) {
            *chosen.binding_mut(action) = *state.hotkeys.binding(action);
        }
    }
    state.hotkey_errors = failures
        .iter()
        .map(|failure| (failure.action, failure.reason))
        .collect();
    state.hotkeys = chosen;
    state.registered_hotkeys = registered;
    state.layout_enabled = state.hotkeys.binding(HotkeyAction::CorrectLayout).enabled;
    state.translation_enabled = state.hotkeys.binding(HotkeyAction::Translate).enabled;
    true
}

fn reject_hotkeys_if_pending(
    state: &mut AppState,
    operation_id: RequestId,
    action: Option<HotkeyAction>,
    reason: HotkeyRejection,
) -> bool {
    let matches = state
        .pending_hotkeys
        .as_ref()
        .is_some_and(|(pending, _)| *pending == operation_id);
    if matches {
        state.pending_hotkeys = None;
        state.hotkey_errors.clear();
        if let Some(action) = action {
            state.hotkey_errors.insert(action, reason);
        }
    }
    matches
}

#[allow(clippy::too_many_arguments)]
fn handle_text_action(
    request_id: RequestId,
    intent: TextActionIntent,
    target: TargetSnapshot,
    text: String,
    request_started_unix_ms: u64,
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    chooser_presenter: &OverlayPresenter,
    action_palette: &slint::Weak<ActionPalette>,
    action_palette_presenter: &OverlayPresenter,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    match intent {
        TextActionIntent::CorrectLayout => {
            if state.borrow().layout_enabled {
                show_layout_chooser(
                    request_id,
                    target,
                    text,
                    settings,
                    chooser,
                    chooser_presenter,
                    state,
                );
            } else {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&request_id);
                set_indicator_status(
                    settings,
                    state_text(state, UiText::LayoutCorrectionDisabled),
                );
            }
        }
        TextActionIntent::Translate => {
            begin_selection_translation(
                SelectionTranslationRequest {
                    request_id,
                    target,
                    text,
                    delivery: TranslationDelivery::Preview,
                    request_started_unix_ms,
                    source_language: None,
                    target_language: None,
                },
                settings,
                translation_previews,
                state,
                workers,
            );
        }
        TextActionIntent::Adaptive => {
            let (layout_enabled, translation_enabled, mode, delivery) = {
                let state = state.borrow();
                (
                    state.layout_enabled,
                    state.translation_enabled,
                    state.text_action_mode,
                    state.translation_delivery,
                )
            };
            let enabled_count = usize::from(layout_enabled) + usize::from(translation_enabled);
            if enabled_count == 0 {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&request_id);
                set_indicator_status(settings, state_text(state, UiText::AllTextActionsDisabled));
                return;
            }
            if enabled_count == 1 && mode == TextActionMode::Adaptive {
                if translation_enabled {
                    begin_selection_translation(
                        SelectionTranslationRequest {
                            request_id,
                            target,
                            text,
                            delivery,
                            request_started_unix_ms,
                            source_language: None,
                            target_language: None,
                        },
                        settings,
                        translation_previews,
                        state,
                        workers,
                    );
                } else {
                    show_layout_chooser(
                        request_id,
                        target,
                        text,
                        settings,
                        chooser,
                        chooser_presenter,
                        state,
                    );
                }
                return;
            }

            state.borrow_mut().pending_text_action = Some(PendingTextAction {
                request_id,
                target,
                text: text.clone(),
                request_started_unix_ms,
            });
            if let Some(palette) = action_palette.upgrade() {
                palette.set_prepared(false);
                let ticket = match action_palette_presenter.begin_prepare() {
                    Ok(ticket) => ticket,
                    Err(error) => {
                        rust_app_core::diagnostics::log_line(&format!(
                            "action_palette_overlay transition=failed stage=begin_prepare \
error={error}"
                        ));
                        return;
                    }
                };
                palette.set_source_panel_height(adaptive_text_panel_height(
                    &text, 70, 2, 4, 15.0, 16.0,
                ));
                palette.set_source_text(text.into());
                palette.set_translation_enabled(translation_enabled);
                palette.set_layout_enabled(layout_enabled);
                let configured = slint_window_handle(palette.window())
                    .ok_or_else(|| "native action-palette HWND is unavailable".to_owned())
                    .and_then(|handle| {
                        rust_platform_windows::configure_action_window_for_target(
                            handle,
                            target.window_handle,
                        )
                    });
                if let Err(error) = configured {
                    action_palette_presenter.abort_prepare(
                        palette.window(),
                        ticket,
                        "configure",
                        &error,
                    );
                    return;
                }
                palette.set_prepared(true);
                if let Err(error) = action_palette_presenter.await_frame(palette.window(), ticket) {
                    palette.set_prepared(false);
                    action_palette_presenter.abort_prepare(
                        palette.window(),
                        ticket,
                        "await_frame",
                        &error,
                    );
                }
            }
        }
    }
}

fn begin_selection_translation(
    request: SelectionTranslationRequest,
    settings: &slint::Weak<SettingsWindow>,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    let SelectionTranslationRequest {
        request_id,
        target,
        text,
        delivery,
        request_started_unix_ms,
        source_language,
        target_language,
    } = request;
    let (
        enabled,
        selected_tier,
        selected_catalog_id,
        path_ready,
        waits_for_local,
        reload_on_next_use,
        keep_warm,
    ) = {
        let state = state.borrow();
        (
            state.translation_enabled,
            state.selected_translation_tier,
            state.selected_translation_model_id.clone(),
            state.translation_path_can_start(),
            state.translation_waits_for_local(),
            state.translation_reload_on_next_use,
            state.translation_keep_warm,
        )
    };
    if !enabled {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(settings, state_text(state, UiText::TranslationDisabled));
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=rejected \
reason=translation_disabled"
        ));
        return;
    }
    if !(path_ready || selected_tier.is_some() && waits_for_local) {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(settings, state_text(state, UiText::ChooseTranslationRoute));
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=rejected reason=no_selected_model"
        ));
        return;
    }
    if !path_ready {
        state.borrow_mut().queued_selection_translation = Some(SelectionTranslationRequest {
            request_id,
            target,
            text,
            delivery,
            request_started_unix_ms,
            source_language,
            target_language,
        });
        if reload_on_next_use
            && let Some(catalog_id) = selected_catalog_id
            && let Err(error) = workers
                .translation
                .send(&TranslationWorkerCommand::SelectModel {
                    catalog_id,
                    keep_warm,
                })
        {
            state.borrow_mut().queued_selection_translation = None;
            state
                .borrow_mut()
                .selection_release_gates
                .remove(&request_id);
            set_status_problem(
                settings,
                &state_message(
                    state,
                    UiText::CouldNotStartTranslation,
                    &[("error", &error.to_string())],
                ),
            );
            return;
        }
        set_indicator_status(settings, state_text(state, UiText::LocalTranslationLoading));
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=queued reason=model_loading"
        ));
        return;
    }

    if delivery == TranslationDelivery::Preview {
        translation_previews
            .borrow_mut()
            .reserve_current(request_id);
    }

    let language_policy = TranslationLanguagePolicy {
        primary: state.borrow().translation_primary_language,
        secondary: state.borrow().translation_secondary_language,
    };
    state.borrow_mut().pending_translations.insert(
        request_id,
        PendingTranslation {
            origin: TranslationOrigin::Selection { target, delivery },
            source_text: text.clone(),
            source_language,
            target_language,
            language_policy,
            started: Instant::now(),
            request_started_unix_ms,
            first_partial_shown: false,
            stable_paragraphs: None,
        },
    );
    if let Err(error) = workers
        .translation
        .send(&TranslationWorkerCommand::Translate {
            request_id,
            text,
            source_language,
            target_language,
            language_policy,
        })
    {
        let mut app = state.borrow_mut();
        app.pending_translations.remove(&request_id);
        app.selection_release_gates.remove(&request_id);
        translation_previews
            .borrow_mut()
            .clear_current_request(request_id);
        set_status_problem(
            settings,
            &state_message(
                state,
                UiText::CouldNotStartTranslation,
                &[("error", &error.to_string())],
            ),
        );
    } else {
        set_indicator_status(settings, state_text(state, UiText::TranslatingNow));
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=dispatched"
        ));
    }
}

fn show_selection_translation_preview(
    request_id: RequestId,
    target: TargetSnapshot,
    source_text: &str,
    translated_text: &str,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    final_update: bool,
) {
    show_translation_preview_at(
        request_id,
        Some(TranslationPreviewAnchor::Target(target)),
        source_text,
        translated_text,
        translation_previews,
        final_update,
    );
}

fn show_screen_translation_preview(
    request_id: RequestId,
    anchor: PhysicalRect,
    source_text: &str,
    translated_text: &str,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    final_update: bool,
) {
    show_translation_preview_at(
        request_id,
        Some(TranslationPreviewAnchor::ScreenRegion(anchor)),
        source_text,
        translated_text,
        translation_previews,
        final_update,
    );
}

#[cfg(feature = "screen-translation")]
fn retry_screen_translation(window_handle: u64, runtime: &RuntimeUi) {
    let slot = runtime
        .screen_result_windows
        .borrow()
        .slot_by_handle(window_handle);
    let Some(slot) = slot else {
        return;
    };
    if slot.window.get_screen_translating() || slot.window.get_screen_editor_visible() {
        return;
    }
    let old_id = slot.request_id();
    let result = runtime.screen_translation.borrow().result_snapshot(old_id);
    let Ok((snapshot, source_language, target_language)) = result else {
        return;
    };
    let retry = runtime
        .screen_translation
        .borrow_mut()
        .begin_retry(old_id, snapshot.revision);
    let (request_id, anchor) = match retry {
        Ok(retry) => retry,
        Err(error) => {
            let message = if error == DocumentError::TranslationInProgress {
                state_text(&runtime.state, UiText::ScreenTranslationBusy)
            } else {
                state_text(&runtime.state, UiText::TranslationFailedTryAgain)
            };
            slot.window.set_screen_translation_notice(message.into());
            let _ = present_screen_result(&slot, snapshot.crop_rect);
            return;
        }
    };
    let was_pinned = slot.is_pinned();
    let next = runtime
        .screen_translation
        .borrow()
        .result_snapshot(request_id);
    runtime
        .screen_result_windows
        .borrow_mut()
        .retire(old_id, "translation_retry");
    if let Ok((snapshot, _, _)) = next {
        show_screen_image_result(request_id, anchor, &snapshot, true, runtime);
        let next_slot = runtime
            .screen_result_windows
            .borrow()
            .slot_for_request(request_id);
        if let Some(next_slot) = next_slot {
            let _ = pin_screen_result(&next_slot, was_pinned);
            let _ = present_screen_result(&next_slot, anchor);
        }
    }
    let dispatched = begin_screen_translation(
        ScreenTranslationRequest {
            request_id,
            anchor,
            request_started_unix_ms: unix_ms(),
            source_language: Some(source_language),
            target_language: Some(target_language),
        },
        &runtime.settings,
        &runtime.translation_previews,
        &runtime.state,
        &runtime.workers,
        &runtime.screen_translation,
    );
    if !dispatched
        && !runtime
            .state
            .borrow()
            .queued_screen_translation
            .as_ref()
            .is_some_and(|queued| queued.request_id == request_id)
    {
        show_screen_translation_error(
            request_id,
            anchor,
            state_text(&runtime.state, UiText::TranslationFailedTryAgain),
            runtime,
        );
    }
    rust_app_core::diagnostics::log_line(&format!(
        "screen_translation request={request_id} stage=retry previous_request={old_id} dispatched={dispatched}"
    ));
}

#[cfg(feature = "screen-translation")]
fn show_screen_translation_error(
    request_id: RequestId,
    anchor: PhysicalRect,
    message: &str,
    runtime: &RuntimeUi,
) {
    let slot = runtime
        .screen_result_windows
        .borrow()
        .slot_for_request(request_id);
    if let Some(slot) = slot {
        slot.window.set_screen_translating(false);
        let coordinator = runtime.screen_translation.borrow();
        let terminal_document = coordinator.result_snapshot(request_id).is_ok();
        slot.window.set_pin_enabled(terminal_document);
        slot.window.set_screen_translation_retry_enabled(
            terminal_document && coordinator.can_retry(request_id),
        );
        drop(coordinator);
        slot.window.set_screen_translation_notice(message.into());
        if let Err(error) = present_screen_result(&slot, anchor) {
            fail_screen_result_presentation(request_id, anchor, &error, runtime);
            return;
        }
    } else {
        if runtime
            .translation_previews
            .borrow()
            .slot_for_request(request_id)
            .is_none()
        {
            runtime
                .translation_previews
                .borrow_mut()
                .reserve_current(request_id);
        }
        let source = runtime
            .screen_translation
            .borrow()
            .source_text(request_id)
            .unwrap_or_default();
        show_screen_translation_preview(
            request_id,
            anchor,
            &source,
            message,
            &runtime.translation_previews,
            true,
        );
    }
    set_status_problem(&runtime.settings, message);
}

#[cfg(feature = "screen-translation")]
fn fail_screen_result_presentation(
    request_id: RequestId,
    anchor: PhysicalRect,
    error: &str,
    runtime: &RuntimeUi,
) {
    runtime
        .screen_result_windows
        .borrow_mut()
        .retire(request_id, "presentation_failed");
    rust_app_core::diagnostics::log_line(&format!(
        "screen_translation request={request_id} stage=result_present result=failed error={error}"
    ));
    show_screen_translation_error(
        request_id,
        anchor,
        state_text(&runtime.state, UiText::TranslationFailedTryAgain),
        runtime,
    );
}

#[cfg(feature = "screen-translation")]
fn show_screen_image_result(
    request_id: RequestId,
    anchor: PhysicalRect,
    snapshot: &screen_result::ScreenResultSnapshot,
    translating: bool,
    runtime: &RuntimeUi,
) {
    let slot =
        match get_or_create_screen_result_slot(&runtime.screen_result_windows, request_id, || {
            create_screen_result_window(request_id)
        }) {
            Ok(slot) => slot,
            Err(error) => {
                fail_screen_result_presentation(request_id, anchor, &error, runtime);
                return;
            }
        };
    if let Err(error) = apply_screen_image_snapshot(
        &slot.window,
        snapshot,
        translating,
        runtime.screen_translation.borrow().can_retry(request_id),
    ) {
        fail_screen_result_presentation(request_id, anchor, &error, runtime);
        return;
    }
    if let Err(error) = present_screen_result(&slot, anchor) {
        fail_screen_result_presentation(request_id, anchor, &error, runtime);
        return;
    }
    // Keep the progress card until the image presentation has been queued.
    retire_terminal_compact_before_screen_result(request_id, runtime);
}

#[cfg(feature = "screen-translation")]
fn apply_screen_image_snapshot(
    window: &ScreenTranslationWindow,
    snapshot: &screen_result::ScreenResultSnapshot,
    translating: bool,
    can_retry: bool,
) -> Result<(), String> {
    window.set_pin_enabled(!translating);
    window.set_screen_saved(false);
    window.set_screen_save_error(false);
    // Set this before layout: the translating notice consumes viewport height.
    window.set_screen_translating(translating);
    window.set_screen_translation_notice("".into());
    window.set_screen_translation_retry_enabled(can_retry && !translating);
    screen_result_view::apply_snapshot(window, snapshot).map(|_| ())
}

#[cfg(feature = "screen-translation")]
fn completed_screen_result_snapshot(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    request_id: RequestId,
) -> Result<
    (
        screen_result::ScreenResultSnapshot,
        TranslationLanguage,
        TranslationLanguage,
    ),
    DocumentError,
> {
    let mut coordinator = coordinator.borrow_mut();
    coordinator.complete(request_id);
    coordinator.result_snapshot(request_id)
}

#[cfg(feature = "screen-translation")]
fn persist_completed_screen_snapshot(
    snapshot: &screen_result::ScreenResultSnapshot,
    source_language: TranslationLanguage,
    target_language: TranslationLanguage,
    store: Option<&Arc<ScreenshotStore>>,
) {
    let Some(store) = store.map(Arc::clone) else {
        return;
    };
    let snapshot = snapshot.clone();
    let request_id = snapshot.request_id;
    if std::thread::Builder::new()
        .name("screen-snapshot-finalize".to_owned())
        .spawn(move || match store.persist_snapshot(
            &snapshot,
            Some((source_language, target_language)),
        ) {
            Ok(_) => rust_app_core::diagnostics::log_line(&format!(
                "screen_snapshot request={request_id} result=translations_updated"
            )),
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "screen_snapshot request={request_id} result=translation_update_failed error={error}"
            )),
        })
        .is_err()
    {
        rust_app_core::diagnostics::log_line(&format!(
            "screen_snapshot request={request_id} result=translation_update_thread_failed"
        ));
    }
}

#[derive(Clone, Copy)]
enum TranslationPreviewAnchor {
    Target(TargetSnapshot),
    ScreenRegion(PhysicalRect),
}

fn show_translation_preview_at(
    request_id: RequestId,
    anchor: Option<TranslationPreviewAnchor>,
    source_text: &str,
    translated_text: &str,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    final_update: bool,
) {
    let Some(slot) = translation_previews.borrow().slot_for_request(request_id) else {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview request={request_id} update=ignored reason=superseded"
        ));
        return;
    };
    let preview = &slot.preview;
    let translation_preview_presenter = &slot.presenter;
    let request_id_text = request_id.to_string();
    let first_show = preview.get_request_id().as_str() != request_id_text;
    if first_show
        && (!preview.get_request_id().is_empty()
            || translation_preview_presenter.has_active_presentation())
    {
        // A new request can reuse the current slot after a terminal capture
        // failure. The old preview is still Presented even if its screen
        // coordinator no longer has an anchor, so park it before preparing
        // the new content.
        retire_translation_preview(&slot, "superseded_request");
    }
    preview.set_model_offer_visible(false);
    preview.set_model_offer_action("".into());
    preview.set_notice("".into());
    preview.set_pin_enabled(final_update);
    preview.set_processing(!final_update);
    if first_show {
        preview.set_request_id(request_id_text.into());
        match anchor {
            Some(TranslationPreviewAnchor::ScreenRegion(rect)) => {
                preview.set_screen_anchor_active(true);
                preview.set_screen_anchor_x(rect.x);
                preview.set_screen_anchor_y(rect.y);
                preview.set_screen_anchor_width(i32::try_from(rect.width).unwrap_or(i32::MAX));
                preview.set_screen_anchor_height(i32::try_from(rect.height).unwrap_or(i32::MAX));
            }
            Some(TranslationPreviewAnchor::Target(_)) | None => {
                preview.set_screen_anchor_active(false)
            }
        }
        preview.set_original_card_height(adaptive_text_panel_height(
            source_text,
            48,
            2,
            7,
            17.0,
            28.0,
        ));
        preview.set_pinned(false);
        if let Err(error) = configure_slint_translation_resize(preview, false) {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview resize_state=failed pinned=false reason=first_show error={error}"
            ));
        }
        preview.set_live_glass(false);
        if let Some(handle) = slint_window_handle(preview.window()) {
            let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
        }
    }
    update_translation_preview_content(
        preview,
        translation_preview_presenter,
        source_text,
        translated_text,
    );
    if first_show {
        preview.set_prepared(false);
        preview.invoke_reset_scroll();
        if slot.attached.get()
            && let Err(error) =
                present_translation_preview(preview, translation_preview_presenter, anchor)
        {
            preview.set_request_id("".into());
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay transition=failed stage=present error={error}"
            ));
        }
    } else if !translation_preview_presenter.has_active_presentation() && slot.attached.get() {
        // A frame timeout parks the current request. Its final text or error
        // must start a new presentation instead of refreshing a hidden card.
        preview.set_prepared(false);
        if let Err(error) =
            present_translation_preview(preview, translation_preview_presenter, anchor)
        {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay transition=failed stage=recover request={request_id} error={error}"
            ));
        }
    } else if final_update && let Some(handle) = slint_window_handle(preview.window()) {
        match translation_preview_presenter.begin_refresh() {
            Ok(ticket) => {
                refresh_cloaked_translation_backdrop(preview, handle);
                if let Err(error) =
                    translation_preview_presenter.await_frame(preview.window(), ticket)
                {
                    preview.set_prepared(false);
                    translation_preview_presenter.abort_prepare(
                        preview.window(),
                        ticket,
                        "refresh_frame",
                        &error,
                    );
                }
            }
            // Still on its way to the screen. `content_changed` above has
            // already re-armed the frame the show is waiting for — taking back
            // a queued reveal if one had been scheduled for the partial — so the
            // presentation in flight will land on the final text. This used to
            // be `if let Ok`, which threw the error away and left the window to
            // be revealed with whatever it had.
            Err(reason) => rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay update=folded_into_pending request={request_id} \
                 final_len={} reason={reason}",
                translated_text.chars().count()
            )),
        }
    }
}

fn present_translation_preview(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    anchor: Option<TranslationPreviewAnchor>,
) -> Result<(), String> {
    let ticket = presenter.begin_prepare()?;
    let handle = slint_window_handle(preview.window())
        .ok_or_else(|| "native translation-preview HWND is unavailable".to_owned())?;
    // Placed by the text it is translating, not by the pointer. The snapshot
    // was already here, used only to hand focus back; it also knows which
    // screen the user is looking at, which is the thing the placement was
    // guessing at and getting wrong across monitors.
    let placed = match anchor {
        Some(TranslationPreviewAnchor::Target(target)) => {
            rust_platform_windows::configure_translation_window_for_target(
                handle,
                target.window_handle,
                target.thread_id,
                target
                    .selection_rect
                    .map(|rect| (rect.left, rect.top, rect.right, rect.bottom)),
                target.pointer.map(|point| (point.x, point.y)),
            )
        }
        Some(TranslationPreviewAnchor::ScreenRegion(rect)) => {
            let right = i64::from(rect.x)
                .saturating_add(i64::from(rect.width))
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            let bottom = i64::from(rect.y)
                .saturating_add(i64::from(rect.height))
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            rust_platform_windows::position_screen_result_window_for_region(
                handle,
                (rect.x, rect.y, right, bottom),
            )
        }
        None => rust_platform_windows::configure_translation_window(handle),
    };
    if let Err(error) = placed {
        presenter.abort_prepare(preview.window(), ticket, "configure", &error);
        return Err(error);
    }
    refresh_cloaked_translation_backdrop(preview, handle);
    preview.set_prepared(true);
    if let Err(error) = presenter.await_frame(preview.window(), ticket) {
        preview.set_prepared(false);
        presenter.abort_prepare(preview.window(), ticket, "await_frame", &error);
        return Err(error);
    }
    if let Some(TranslationPreviewAnchor::Target(target)) = anchor
        && let Err(error) = rust_platform_windows::restore_target_if_our_action_window_activated(
            target.window_handle,
        )
    {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview target_restore=failed error={error}"
        ));
    }
    Ok(())
}

fn fallback_and_initialize_speech(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    reason: &str,
) -> Result<SpeechBackend, String> {
    let backend = workers.fallback_speech_backend(reason)?;
    let initialization = {
        let mut state = state.borrow_mut();
        state.speech_backend_active = Some(backend);
        state.speech_worker_ready = false;
        state.speech_ready = false;
        workers.speech.send(&SpeechWorkerCommand::Initialize {
            selected_model_id: state.selected_model_id.clone(),
            output_mode: state.output_mode,
            dictation_language: state.dictation_language,
            history_enabled: state.history_enabled,
            fast_release_previews: state.fast_release_previews,
            prepare_local_model: state.local_speech_enabled(),
        })
    };
    sync_action_settings(settings, state);
    initialization.map_err(|error| error.to_string())?;
    Ok(backend)
}

/// Draws one microphone frame, either in the in-process indicator or in the
/// companion renderer. Frames for anything but the active session are dropped:
/// coalescing can hand over a frame recorded just before a session ended.
fn apply_audio_level(
    session_id: SessionId,
    peak: f32,
    spectrum: Vec<f32>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    if state.borrow().registry.active_session() != Some(session_id) {
        return;
    }
    if state
        .borrow()
        .active_visualizer_style
        .is_some_and(VisualizerStyle::uses_external_renderer)
    {
        let _ = workers.send_companion(&CompanionRendererCommand::AudioLevel {
            session_id,
            peak,
            spectrum,
        });
    } else {
        indicator_animation
            .borrow_mut()
            .set_microphone_signal(peak, &spectrum);
    }
}

/// The speech installer is serial and emits its terminal event before the next
/// queued model can emit progress. Its legacy Failed event has no catalog ID:
/// clear only known speech progress, never a guessed selection or text download.
fn handle_speech_install_terminal_event(
    event: &SpeechWorkerEvent,
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) -> bool {
    let SpeechWorkerEvent::Failed {
        session_id: None,
        code,
        message,
        ..
    } = event
    else {
        return false;
    };
    if !matches!(code.as_str(), "model_install" | "model_install_cancelled") {
        return false;
    }
    let selected_model_missing = {
        let mut app = state.borrow_mut();
        let speech_ids = app
            .models
            .iter()
            .map(|model| model.catalog_id.clone())
            .collect::<HashSet<_>>();
        app.downloads
            .retain(|catalog_id, _| !speech_ids.contains(catalog_id));
        app.speech_waits_for_local()
            && app
                .models
                .iter()
                .any(|model| model.catalog_id == app.selected_model_id && !model.installed)
    };
    sync_models(settings, state);
    if selected_model_missing {
        set_runtime_status(settings, state_text(state, UiText::NotReady));
    }
    if code == "model_install_cancelled" {
        set_indicator_status(settings, state_text(state, UiText::InstallCancelled));
    } else {
        set_status_problem(
            settings,
            &state_message(
                state,
                UiText::TechnicalFailure,
                &[("code", code), ("message", message)],
            ),
        );
    }
    true
}

fn handle_speech_event(
    event: SpeechWorkerEvent,
    settings: &slint::Weak<SettingsWindow>,
    recording_overlays: &RecordingOverlays,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    // Keep normal Failed diagnostics, but do not overwrite the install helper's
    // terminal footer with the generic progress-severity message below.
    let install_terminal_handled = handle_speech_install_terminal_event(&event, settings, state);
    match event {
        SpeechWorkerEvent::Ready {
            protocol_version,
            selected_model_id,
            models,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                let mut state = state.borrow_mut();
                let locale = active_locale(state.ui_locale);
                state.speech_worker_ready = false;
                state.speech_ready = false;
                drop(state);
                set_runtime_status(settings, text(locale, UiText::WorkerMismatch));
                set_indicator_status(settings, text(locale, UiText::SpeechProtocolMismatch));
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.speech_worker_ready = true;
                state.selected_model_id = selected_model_id;
                state.models = models;
            }
            sync_models(settings, state);
            if state.borrow().local_speech_enabled() {
                set_runtime_status(settings, state_text(state, UiText::PreparingModel));
                set_indicator_status(settings, state_text(state, UiText::LoadingSpeechEngine));
            } else {
                set_runtime_status(settings, state_text(state, UiText::CloudSpeechReady));
                set_indicator_status(
                    settings,
                    state_text(state, UiText::CloudSpeechLocalDisabled),
                );
            }
            sync_action_settings(settings, state);
            let _ = workers.speech.send(&SpeechWorkerCommand::GetHistory {
                limit: HISTORY_LIMIT,
            });
        }
        SpeechWorkerEvent::ModelDownloadProgress {
            catalog_id,
            downloaded_bytes,
            total_bytes,
        } => {
            // The row this belongs to can show it, which is where a progress
            // figure is actually useful. Reporting it only as a status line
            // left the list saying "Not installed" about a file that was
            // halfway down the wire.
            state.borrow_mut().downloads.insert(
                catalog_id.clone(),
                settings_projection::Download {
                    downloaded_bytes,
                    total_bytes,
                },
            );
            sync_models(settings, state);
            let selected_local_download = {
                let app = state.borrow();
                catalog_id == app.selected_model_id && app.speech_waits_for_local()
            };
            if selected_local_download {
                set_runtime_status(settings, state_text(state, UiText::PreparingModel));
            }
            set_indicator_status(
                settings,
                &state_message(state, UiText::InstallingModel, &[("model", &catalog_id)]),
            );
        }
        SpeechWorkerEvent::ModelInstalled { catalog_id, .. } => {
            finish_download(state, &catalog_id);
            {
                let mut state = state.borrow_mut();
                if let Some(model) = state
                    .models
                    .iter_mut()
                    .find(|model| model.catalog_id == catalog_id)
                {
                    model.installed = true;
                }
            }
            sync_models(settings, state);
        }
        SpeechWorkerEvent::LocalModelState { catalog_id, loaded } => {
            {
                let mut app = state.borrow_mut();
                app.speech_loaded_model_id = loaded.then_some(catalog_id).flatten();
                app.speech_ready = loaded && app.local_speech_enabled();
                app.speech_unloading = false;
                if loaded {
                    app.speech_reload_on_next_use = false;
                }
            }
            sync_models(settings, state);
            sync_action_settings(settings, state);
        }
        SpeechWorkerEvent::ModelRemoved {
            catalog_id,
            removed_bytes,
            models,
        } => {
            {
                let mut app = state.borrow_mut();
                app.removing_models.remove(&catalog_id);
                app.models = models;
                if app.speech_loaded_model_id.as_deref() == Some(catalog_id.as_str()) {
                    app.speech_loaded_model_id = None;
                    app.speech_ready = false;
                    app.speech_reload_on_next_use = false;
                    app.speech_unloading = false;
                }
            }
            sync_models(settings, state);
            sync_action_settings(settings, state);
            let size = settings_projection::human_size(removed_bytes);
            set_indicator_status(
                settings,
                &state_message(state, UiText::ModelFilesRemoved, &[("size", &size)]),
            );
        }
        SpeechWorkerEvent::ModelReady {
            catalog_id,
            backend,
            load_duration_ms,
            warmup_duration_ms,
            ..
        } => {
            finish_download(state, &catalog_id);
            if !state.borrow().local_speech_enabled() {
                {
                    let mut state = state.borrow_mut();
                    if let Some(model) = state
                        .models
                        .iter_mut()
                        .find(|model| model.catalog_id == catalog_id)
                    {
                        model.installed = true;
                    }
                    state.speech_ready = false;
                }
                sync_models(settings, state);
                sync_action_settings(settings, state);
                set_runtime_status(settings, state_text(state, UiText::CloudSpeechReady));
                set_indicator_status(
                    settings,
                    state_text(state, UiText::CloudSpeechLocalDisabled),
                );
                rust_app_core::diagnostics::log_line(&format!(
                    "speech_model state=stale_ready_ignored model={catalog_id}"
                ));
                return;
            }
            let active = match workers.mark_speech_backend_ready(&backend) {
                Ok(active) => active,
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "speech_backend verification=failed error={error}"
                    ));
                    match fallback_and_initialize_speech(
                        settings,
                        state,
                        workers,
                        "reported_backend_mismatch",
                    ) {
                        Ok(fallback) => {
                            set_runtime_status(
                                settings,
                                state_text(state, UiText::CheckingFallback),
                            );
                            set_indicator_status(
                                settings,
                                &state_message(
                                    state,
                                    UiText::BackendFallback,
                                    &[("backend", fallback.display_name())],
                                ),
                            );
                        }
                        Err(fallback_error) => {
                            set_runtime_status(settings, state_text(state, UiText::WorkerError));
                            set_indicator_status(
                                settings,
                                &state_message(
                                    state,
                                    UiText::BackendVerificationFailed,
                                    &[("error", &format!("{error}; {fallback_error}"))],
                                ),
                            );
                        }
                    }
                    return;
                }
            };
            {
                let mut state = state.borrow_mut();
                state.speech_backend_active = Some(active);
                if let Some(model) = state
                    .models
                    .iter_mut()
                    .find(|model| model.catalog_id == catalog_id)
                {
                    model.installed = true;
                }
                if catalog_id == state.selected_model_id {
                    state.speech_ready = true;
                    state.speech_reload_on_next_use = false;
                    state.speech_unloading = false;
                }
            }
            sync_models(settings, state);
            sync_action_settings(settings, state);
            set_runtime_status(settings, state_text(state, UiText::ReadyTitle));
            let shortcut = format_hotkey(state.borrow().hotkeys.dictate.spec);
            set_indicator_status(
                settings,
                &state_message(state, UiText::ReadyHoldShortcut, &[("shortcut", &shortcut)]),
            );
            rust_app_core::diagnostics::log_line(&format!(
                "speech_backend preference={} selected={} verification=passed model={} load_ms={} warmup_ms={}",
                workers.speech_backend_preference().label(),
                active.name(),
                catalog_id,
                load_duration_ms,
                warmup_duration_ms
            ));
        }
        SpeechWorkerEvent::ModelSelected { catalog_id } => {
            finish_download(state, &catalog_id);
            {
                let mut state = state.borrow_mut();
                state.selected_model_id.clone_from(&catalog_id);
                for model in &mut state.models {
                    model.selected = model.catalog_id == catalog_id;
                }
                state.speech_ready = state.local_speech_enabled();
            }
            let _ = save_settings(&state.borrow());
            sync_models(settings, state);
            if state.borrow().local_speech_enabled() {
                set_indicator_status(settings, state_text(state, UiText::SpeechModelSelected));
            }
        }
        SpeechWorkerEvent::Listening { session_id: _ } => {}
        SpeechWorkerEvent::AudioLevel {
            session_id,
            peak,
            spectrum,
        } => {
            // Normally coalesced before dispatch; handled here as well so the
            // event stays meaningful if it ever arrives directly.
            apply_audio_level(
                session_id,
                peak,
                spectrum,
                state,
                workers,
                indicator_animation,
            );
        }
        SpeechWorkerEvent::Processing { session_id } => {
            if state.borrow().active_visualizer_style != Some(VisualizerStyle::Off) {
                indicator_animation.borrow_mut().begin_processing();
            }
            if state
                .borrow()
                .active_visualizer_style
                .is_some_and(VisualizerStyle::uses_external_renderer)
            {
                let _ =
                    workers.send_companion(&CompanionRendererCommand::Processing { session_id });
            }
            if let Some(active_style) = state.borrow().active_visualizer_style
                && !active_style.uses_external_renderer()
                && let Some(overlay) = recording_overlays.for_style(active_style)
            {
                overlay.indicator.set_processing(true);
            }
        }
        SpeechWorkerEvent::Completed {
            transcript,
            created_unix_ms,
        } => {
            let session_id = transcript.session_id;
            let stored = matches!(
                state
                    .borrow_mut()
                    .registry
                    .complete(session_id, transcript.clone()),
                Ok(rust_app_core::CompletionDisposition::Stored)
            );
            let plan = {
                let mut state = state.borrow_mut();
                let (replaced_existing, awaiting_target, captured_target) = if stored {
                    state.latest_history_session = Some(session_id);
                    state.transcripts.insert(session_id, transcript.clone());
                    let before = state.history.len();
                    state
                        .history
                        .retain(|entry| entry.transcript.session_id != session_id);
                    let replaced_existing = state.history.len() != before;
                    state.history.insert(
                        0,
                        HistoryEntry {
                            transcript: transcript.clone(),
                            created_unix_ms,
                        },
                    );
                    state.history.truncate(HISTORY_RETAINED_ROWS);
                    let awaiting_target = state
                        .pending_insert_last
                        .take_if(|pending| pending.session_id == Some(session_id))
                        .map(|pending| pending.target);
                    let captured_target = state.targets.get(&session_id).copied();
                    (replaced_existing, awaiting_target, captured_target)
                } else {
                    (false, None, None)
                };
                plan_completion(stored, replaced_existing, awaiting_target, captured_target)
            };
            // Recognition is finished here. Clipboard delivery can still wait
            // for up to 2.5 seconds for a read acknowledgement; that wait is
            // not recording or speech processing and must not keep the visual
            // on screen. Keep indicator_session until insertion settles so a
            // second dictation cannot overtake the pending delivery.
            let owns_visual = stored && state.borrow().indicator_session == Some(session_id);
            if owns_visual {
                if state
                    .borrow()
                    .active_visualizer_style
                    .is_some_and(VisualizerStyle::uses_external_renderer)
                {
                    let _ =
                        workers.send_companion(&CompanionRendererCommand::Complete { session_id });
                }
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("transcription_completed");
            }
            // The insertion command leaves before any list model work.
            // docs/LATENCY_ARCHITECTURE.md keeps history persistence and
            // presentation off the release-to-insertion path.
            if let CompletionDelivery::Insert {
                target,
                add_trailing_space,
            } = plan.delivery
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "transcription result=completed session={session_id} chars={} \
delivery={} target_pid={}",
                    transcript.text.chars().count(),
                    if add_trailing_space {
                        "original_target"
                    } else {
                        "insert_last_waiter"
                    },
                    target.process_id,
                ));
                let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                    transcript,
                    target,
                    add_trailing_space,
                });
            } else {
                rust_app_core::diagnostics::log_line(&format!(
                    "transcription result={} session={session_id} chars={} delivery=history_only",
                    if stored {
                        "completed"
                    } else {
                        "duplicate_ignored"
                    },
                    transcript.text.chars().count(),
                ));
                state.borrow_mut().indicator_session = None;
                state.borrow_mut().active_visualizer_style = None;
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("target_lost");
                set_indicator_status(
                    settings,
                    state_text(state, UiText::TranscriptSavedTargetLost),
                );
            }
            match plan.history {
                HistoryUpdate::Rebuild => sync_history(settings, state),
                HistoryUpdate::PrependLatest => {
                    let state = state.borrow();
                    if let Some(entry) = state.history.first() {
                        prepend_history_row(
                            settings,
                            entry,
                            HISTORY_RETAINED_ROWS,
                            state.ui_locale,
                        );
                    }
                }
                HistoryUpdate::Unchanged => {}
            }
        }
        SpeechWorkerEvent::CredentialSaved {
            operation_id,
            provider_id,
            error,
        } => {
            providers_view::settle_credential(
                settings,
                state,
                workers,
                operation_id,
                &provider_id,
                &error,
            );
        }
        SpeechWorkerEvent::TranscriptionProviders { statuses } => {
            rust_app_core::diagnostics::log_line(&format!(
                "transcription_providers count={} available={}",
                statuses.len(),
                statuses.iter().filter(|status| status.available).count()
            ));
            {
                let mut state = state.borrow_mut();
                if state.transcription_chain.is_empty() {
                    // Never arranged: adopt the worker's own order so the list
                    // has something to show and the first change persists.
                    state.transcription_chain = statuses
                        .iter()
                        .map(|status| TranscriptionProviderEntry {
                            id: status.id.clone(),
                            enabled: status.enabled,
                            endpoint: None,
                            model: None,
                            account: None,
                        })
                        .collect();
                }
                state.transcription_providers = statuses;
            }
            sync_providers(settings, state);
            sync_action_settings(settings, state);
        }
        SpeechWorkerEvent::TranscribedRemotely {
            session_id,
            provider_id,
        } => {
            // Where the words came from is worth stating plainly: the audio
            // left the machine for this one.
            rust_app_core::diagnostics::log_line(&format!(
                "transcription session={session_id} provider={provider_id}"
            ));
            set_indicator_status(
                settings,
                &state_message(state, UiText::TranscribedBy, &[("provider", &provider_id)]),
            );
        }
        SpeechWorkerEvent::Cancelled { session_id } => {
            rust_app_core::diagnostics::log_line(&format!(
                "transcription result=cancelled session={session_id}"
            ));
            let (owns_indicator, insert_was_waiting) = {
                let mut state = state.borrow_mut();
                let _ = state.registry.cancel(session_id);
                let insert_was_waiting = state
                    .pending_insert_last
                    .take_if(|pending| pending.session_id == Some(session_id))
                    .is_some();
                if state.indicator_session == Some(session_id) {
                    state.indicator_session = None;
                    (true, insert_was_waiting)
                } else {
                    (false, insert_was_waiting)
                }
            };
            if owns_indicator {
                if state
                    .borrow()
                    .active_visualizer_style
                    .is_some_and(VisualizerStyle::uses_external_renderer)
                {
                    let _ = workers.send_companion(&CompanionRendererCommand::Dismiss {
                        session_id: Some(session_id),
                    });
                }
                state.borrow_mut().active_visualizer_style = None;
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("transcription_cancelled");
            }
            if insert_was_waiting {
                set_indicator_status(
                    settings,
                    state_text(state, UiText::LatestDictationCancelled),
                );
            }
        }
        SpeechWorkerEvent::HistoryCleared { error } => {
            record_history_clear_answer(settings, state, HistoryOwner::Speech, error);
        }
        SpeechWorkerEvent::History { entries } => {
            {
                let mut state = state.borrow_mut();
                state.history = entries;
                state.history_loaded = true;
                let historical = state.history.clone();
                for entry in historical {
                    state
                        .transcripts
                        .insert(entry.transcript.session_id, entry.transcript.clone());
                }
                state.latest_history_session = state
                    .history
                    .first()
                    .map(|entry| entry.transcript.session_id);
            }
            sync_history(settings, state);
            let pending_insert = {
                let mut state = state.borrow_mut();
                let pending = state
                    .pending_insert_last
                    .take_if(|pending| pending.session_id.is_none());
                pending.map(|pending| (state.latest_transcript().cloned(), pending.target))
            };
            match pending_insert {
                Some((Some(transcript), target)) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=history_resolved session={} target_pid={}",
                        transcript.session_id, target.process_id
                    ));
                    let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                        transcript,
                        target,
                        add_trailing_space: false,
                    });
                }
                Some((None, _)) => {
                    rust_app_core::diagnostics::log_line("insert_last decision=history_empty");
                    set_indicator_status(settings, state_text(state, UiText::NoTranscriptToInsert));
                }
                None => {}
            }
        }
        SpeechWorkerEvent::Failed {
            session_id,
            code,
            message,
            retryable,
        } => {
            if session_id.is_none() && code == "model_busy" && state.borrow().speech_unloading {
                let mut state = state.borrow_mut();
                state.speech_unloading = false;
                state.speech_reload_on_next_use = false;
                state.speech_ready = state.speech_loaded_model_id.is_some();
            }
            if matches!(
                code.as_str(),
                "model_remove" | "model_busy" | "model_in_use"
            ) {
                state.borrow_mut().removing_models.clear();
                sync_models(settings, state);
            }
            if session_id.is_none() && code == "model_prepare" {
                rust_app_core::diagnostics::log_line(&format!(
                    "speech_backend selected={} verification=failed code={code} message={message}",
                    workers.active_speech_backend().name()
                ));
                match fallback_and_initialize_speech(
                    settings,
                    state,
                    workers,
                    "model_prepare_failed",
                ) {
                    Ok(fallback) => {
                        set_runtime_status(settings, state_text(state, UiText::CheckingFallback));
                        set_indicator_status(
                            settings,
                            &state_message(
                                state,
                                UiText::BackendFailureFallback,
                                &[("code", &code), ("backend", fallback.display_name())],
                            ),
                        );
                    }
                    Err(error) => {
                        state.borrow_mut().speech_backend_active = None;
                        sync_action_settings(settings, state);
                        set_runtime_status(settings, state_text(state, UiText::WorkerError));
                        set_indicator_status(
                            settings,
                            &state_message(
                                state,
                                UiText::NoCompatibleBackend,
                                &[("error", &error.to_string())],
                            ),
                        );
                    }
                }
                return;
            }
            rust_app_core::diagnostics::log_line(&format!(
                "transcription result=failed session={session_id:?} code={code} \
retryable={retryable} message={message}"
            ));
            let (owns_indicator, insert_was_waiting) =
                session_id.map_or((false, false), |session_id| {
                    let mut state = state.borrow_mut();
                    let _ = state.registry.fail(session_id);
                    let insert_was_waiting = state
                        .pending_insert_last
                        .take_if(|pending| pending.session_id == Some(session_id))
                        .is_some();
                    let owns_indicator = if state.indicator_session == Some(session_id) {
                        state.indicator_session = None;
                        true
                    } else {
                        false
                    };
                    (owns_indicator, insert_was_waiting)
                });
            if let Some(session_id) = session_id {
                let mut state = state.borrow_mut();
                state.targets.remove(&session_id);
                state.release_started.remove(&session_id);
            }
            if owns_indicator {
                if state
                    .borrow()
                    .active_visualizer_style
                    .is_some_and(VisualizerStyle::uses_external_renderer)
                {
                    let _ =
                        workers.send_companion(&CompanionRendererCommand::Dismiss { session_id });
                }
                state.borrow_mut().active_visualizer_style = None;
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("transcription_failed");
            }
            if insert_was_waiting {
                set_indicator_status(
                    settings,
                    state_text(state, UiText::DictationInsertionFailed),
                );
            } else if !install_terminal_handled {
                set_indicator_status(
                    settings,
                    &state_message(
                        state,
                        UiText::TechnicalFailure,
                        &[("code", &code), ("message", &message)],
                    ),
                );
            }
        }
    }
}

#[cfg(feature = "screen-translation")]
fn finish_screen_translation_failure(
    coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
    request_id: RequestId,
) -> Option<(PhysicalRect, String)> {
    // Complete the request under one borrow, returning owned presentation data
    // so no coordinator guard survives into the UI callbacks.
    let mut coordinator = coordinator.borrow_mut();
    let anchor = coordinator.anchor(request_id)?;
    let source_text = coordinator.source_text(request_id)?;
    coordinator.complete(request_id);
    Some((anchor, source_text))
}

fn handle_translation_event(event: TranslationWorkerEvent, runtime: &RuntimeUi) {
    let settings = &runtime.settings;
    let translation_previews = &runtime.translation_previews;
    let translator = &runtime.translator;
    let state = &runtime.state;
    let workers = &runtime.workers;
    let screen_translation = &runtime.screen_translation;
    match event {
        TranslationWorkerEvent::Ready {
            protocol_version,
            selected_catalog_id,
            selected_tier,
            models,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                state.borrow_mut().translation_worker_ready = false;
                set_indicator_status(
                    settings,
                    state_text(state, UiText::TranslationProtocolMismatch),
                );
                return;
            }
            let installed_models = models.iter().filter(|model| model.installed).count();
            rust_app_core::diagnostics::log_line(&format!(
                "translation_worker state=ready selected_catalog_id={selected_catalog_id:?} selected_tier={selected_tier:?} installed_models={installed_models}"
            ));
            let recovered_selection = {
                let mut state = state.borrow_mut();
                let previous = (
                    state.selected_translation_model_id.clone(),
                    state.selected_translation_tier,
                );
                state.translation_worker_ready = true;
                state.translation_models = models;
                state.selected_translation_model_id = selected_catalog_id.clone();
                state.selected_translation_tier = selected_tier;
                state.translation_ready = false;
                previous != (selected_catalog_id.clone(), selected_tier)
                    && selected_catalog_id.is_some()
            };
            if recovered_selection {
                let _ = save_settings(&state.borrow());
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_model selection=recovered catalog_id={selected_catalog_id:?} tier={selected_tier:?}"
                ));
            }
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            if selected_tier.is_none() {
                if state.borrow().local_translation_enabled() {
                    set_translation_status(
                        translator,
                        state_text(state, UiText::NoTranslationModelInstalled),
                    );
                } else {
                    set_translation_status(
                        translator,
                        state_text(state, UiText::CloudTranslationStarting),
                    );
                }
            }
            let _ = workers
                .translation
                .send(&TranslationWorkerCommand::GetHistory { limit: 20 });
        }
        TranslationWorkerEvent::ModelDownloadProgress {
            catalog_id,
            downloaded_bytes,
            total_bytes,
        } => {
            let percent = downloaded_bytes
                .saturating_mul(100)
                .checked_div(total_bytes)
                .unwrap_or(0);
            state.borrow_mut().downloads.insert(
                catalog_id.clone(),
                settings_projection::Download {
                    downloaded_bytes,
                    total_bytes,
                },
            );
            sync_translation_models(settings, state);
            let percent_text = percent.to_string();
            set_indicator_status(
                settings,
                &state_message(
                    state,
                    UiText::DownloadingNamedModel,
                    &[("model", &catalog_id), ("percent", &percent_text)],
                ),
            );
            set_translation_status(
                translator,
                &state_message(
                    state,
                    UiText::DownloadingModel,
                    &[("percent", &percent_text)],
                ),
            );
        }
        TranslationWorkerEvent::ModelInstalled {
            catalog_id, tier, ..
        } => {
            finish_download(state, &catalog_id);
            {
                let mut state = state.borrow_mut();
                state.selected_translation_model_id = Some(catalog_id.clone());
                state.selected_translation_tier = Some(tier);
                for model in &mut state.translation_models {
                    if model.catalog_id == catalog_id {
                        model.installed = true;
                    }
                    model.selected = model.catalog_id == catalog_id;
                }
            }
            let _ = save_settings(&state.borrow());
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
        }
        TranslationWorkerEvent::LocalModelState { catalog_id, loaded } => {
            apply_local_translation_model_state(&mut state.borrow_mut(), catalog_id, loaded);
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
        }
        TranslationWorkerEvent::ModelRemoved {
            catalog_id,
            removed_bytes,
            models,
            ..
        } => {
            {
                let mut app = state.borrow_mut();
                app.removing_models.remove(&catalog_id);
                app.translation_models = models;
                if app.translation_loaded_model_id.as_deref() == Some(catalog_id.as_str()) {
                    app.translation_loaded_model_id = None;
                    app.translation_ready = false;
                    app.translation_reload_on_next_use = false;
                    app.translation_unloading = false;
                }
            }
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            let size = settings_projection::human_size(removed_bytes);
            set_indicator_status(
                settings,
                &state_message(state, UiText::ModelFilesRemoved, &[("size", &size)]),
            );
        }
        TranslationWorkerEvent::ModelReady {
            catalog_id,
            tier,
            load_duration_ms,
        } => {
            // This is the one that was missing. A model already on disk is
            // reported to 100% and then handed over as ready, never installed,
            // so its row stayed at "100 %" with a Cancel button for the rest of
            // the session and Overview announced a download that was over.
            finish_download(state, &catalog_id);
            if !state.borrow().local_translation_enabled() {
                {
                    let mut state = state.borrow_mut();
                    state.translation_ready = false;
                    for model in &mut state.translation_models {
                        model.selected = model.catalog_id == catalog_id;
                        if model.catalog_id == catalog_id {
                            model.installed = true;
                        }
                    }
                }
                sync_translation_models(settings, state);
                sync_action_settings(settings, state);
                set_translation_status(
                    translator,
                    state_text(state, UiText::CloudTranslationReady),
                );
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_model state=stale_ready_ignored catalog_id={catalog_id}"
                ));
                return;
            }
            rust_app_core::diagnostics::log_line(&format!(
                "translation_model state=ready catalog_id={catalog_id} tier={tier:?} load_ms={load_duration_ms}"
            ));
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some(probe) = runtime.screen_replay.as_ref() {
                probe.borrow_mut().loaded_model_id = Some(catalog_id.clone());
            }
            let (queued_selection_translation, queued_model_translation) = {
                let mut state = state.borrow_mut();
                state.selected_translation_model_id = Some(catalog_id.clone());
                state.selected_translation_tier = Some(tier);
                state.translation_ready = true;
                state.translation_reload_on_next_use = false;
                state.translation_unloading = false;
                for model in &mut state.translation_models {
                    model.selected = model.catalog_id == catalog_id;
                    if model.catalog_id == catalog_id {
                        model.installed = true;
                    }
                }
                let queued_selection = state.queued_selection_translation.take();
                let queued_model = if state
                    .queued_model_translation
                    .as_ref()
                    .is_some_and(|queued| queued.catalog_id == catalog_id)
                {
                    state.queued_model_translation.take()
                } else {
                    None
                };
                (queued_selection, queued_model)
            };
            #[cfg(feature = "screen-translation")]
            let queued_screen_translation = state.borrow_mut().queued_screen_translation.take();
            let _ = save_settings(&state.borrow());
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            set_indicator_status(
                settings,
                &state_message(
                    state,
                    UiText::TranslationModelReady,
                    &[("milliseconds", &load_duration_ms.to_string())],
                ),
            );
            set_translation_status(
                translator,
                state_text(state, UiText::ReadyDirectionDetected),
            );
            if let Some(request) = queued_selection_translation {
                begin_selection_translation(
                    request,
                    settings,
                    translation_previews,
                    state,
                    workers,
                );
            }
            if let Some(queued) = queued_model_translation {
                let command = pending_translation_command(queued.request_id, &queued.pending);
                let origin = queued.pending.origin;
                state
                    .borrow_mut()
                    .pending_translations
                    .insert(queued.request_id, queued.pending);
                if let Err(error) = workers.translation.send(&command) {
                    let failed = state
                        .borrow_mut()
                        .pending_translations
                        .remove(&queued.request_id);
                    state
                        .borrow_mut()
                        .selection_release_gates
                        .remove(&queued.request_id);
                    let message = state_message(
                        state,
                        UiText::CouldNotStartTranslation,
                        &[("error", &error.to_string())],
                    );
                    match failed
                        .as_ref()
                        .map(|pending| pending.origin)
                        .unwrap_or(origin)
                    {
                        TranslationOrigin::Translator => {
                            if let Some(translator) = translator.upgrade() {
                                translator.set_processing(false);
                                translator.set_presentation_state(4);
                                translator.set_notice_headline(message.clone().into());
                                translator.set_notice_detail("".into());
                                translator.set_status_text(message.clone().into());
                            }
                        }
                        TranslationOrigin::Selection { target, .. } => {
                            show_selection_translation_preview(
                                queued.request_id,
                                target,
                                failed
                                    .as_ref()
                                    .map_or("", |pending| pending.source_text.as_str()),
                                &message,
                                translation_previews,
                                true,
                            );
                        }
                        TranslationOrigin::ScreenRegion { anchor } => {
                            screen_translation.borrow_mut().complete(queued.request_id);
                            show_screen_translation_preview(
                                queued.request_id,
                                anchor,
                                failed
                                    .as_ref()
                                    .map_or("", |pending| pending.source_text.as_str()),
                                &message,
                                translation_previews,
                                true,
                            );
                        }
                    }
                    set_status_problem(settings, &message);
                } else {
                    set_indicator_status(settings, state_text(state, UiText::TranslatingNow));
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_pipeline request={} stage=redispatched_after_pack_install catalog_id={catalog_id}",
                        queued.request_id
                    ));
                }
            }
            #[cfg(feature = "screen-translation")]
            if let Some(request) = queued_screen_translation {
                let request_id = request.request_id;
                let anchor = request.anchor;
                let dispatched = begin_screen_translation(
                    request,
                    settings,
                    translation_previews,
                    state,
                    workers,
                    screen_translation,
                );
                if !dispatched
                    && !state
                        .borrow()
                        .queued_screen_translation
                        .as_ref()
                        .is_some_and(|queued| queued.request_id == request_id)
                {
                    show_screen_translation_error(
                        request_id,
                        anchor,
                        state_text(state, UiText::TranslationFailedTryAgain),
                        runtime,
                    );
                }
            }
        }
        TranslationWorkerEvent::ModelSelected { catalog_id, tier } => {
            {
                let mut state = state.borrow_mut();
                state.selected_translation_model_id = Some(catalog_id.clone());
                state.selected_translation_tier = Some(tier);
                for model in &mut state.translation_models {
                    model.selected = model.catalog_id == catalog_id;
                }
            }
            let _ = save_settings(&state.borrow());
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
        }
        TranslationWorkerEvent::Partial {
            request_id,
            translated_text,
            first_token_duration_ms,
            tokenize_duration_ms,
            chunk_index,
            chunk_count,
        } => {
            let pending = {
                let mut state = state.borrow_mut();
                state
                    .pending_translations
                    .get_mut(&request_id)
                    .map(|pending| {
                        let first_partial = !pending.first_partial_shown;
                        pending.first_partial_shown = true;
                        (
                            pending.origin,
                            pending.started.elapsed().as_millis() as u64,
                            pending.request_started_unix_ms,
                            first_partial,
                            pending.source_text.clone(),
                            pending
                                .stable_paragraphs
                                .as_mut()
                                .and_then(|stream| stream.observe(&translated_text, chunk_index)),
                        )
                    })
            };
            let Some((
                origin,
                translation_elapsed_ms,
                request_started_unix_ms,
                first_partial,
                source_text,
                stable_screen_text,
            )) = pending
            else {
                return;
            };

            match origin {
                TranslationOrigin::Translator => {
                    if let Some(translator) = translator.upgrade() {
                        translator.set_result_text(translated_text.into());
                        let status = if chunk_count > 1 {
                            state_message(
                                state,
                                UiText::TranslatingLocallyProgress,
                                &[
                                    ("chunk", &chunk_index.to_string()),
                                    ("total", &chunk_count.to_string()),
                                ],
                            )
                        } else {
                            state_text(state, UiText::TranslatingLocally).to_owned()
                        };
                        translator.set_status_text(status.into());
                        translator.set_processing(true);
                        translator.set_presentation_state(1);
                    }
                }
                TranslationOrigin::Selection {
                    target,
                    delivery: TranslationDelivery::Preview,
                } => {
                    show_selection_translation_preview(
                        request_id,
                        target,
                        &source_text,
                        &translated_text,
                        translation_previews,
                        false,
                    );
                }
                TranslationOrigin::Selection { .. } => {}
                TranslationOrigin::ScreenRegion { anchor } => {
                    if let Some(stable_text) = stable_screen_text {
                        show_screen_translation_preview(
                            request_id,
                            anchor,
                            &source_text,
                            &stable_text,
                            translation_previews,
                            false,
                        );
                    }
                }
            }

            if first_partial {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_pipeline request={} stage=first_partial \
                     model_first_token_ms={} tokenize_ms={} chunk={}/{} \
                     translation_elapsed_ms={} end_to_end_ms={}",
                    request_id,
                    first_token_duration_ms,
                    tokenize_duration_ms,
                    chunk_index,
                    chunk_count,
                    translation_elapsed_ms,
                    unix_ms().saturating_sub(request_started_unix_ms),
                ));
            }
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::ScreenBlockPartial {
            request_id,
            base_revision,
            block_id,
            translated_text,
        } => {
            let (anchor, snapshot) = {
                let mut coordinator = screen_translation.borrow_mut();
                if !coordinator.is_active_work_request(request_id) {
                    return;
                }
                let anchor = coordinator.anchor(request_id);
                let snapshot = coordinator
                    .apply_partial(request_id, base_revision, block_id, translated_text)
                    .and_then(|_| coordinator.partial_result_snapshot(request_id));
                (anchor, snapshot)
            };
            match (anchor, snapshot) {
                (Some(anchor), Ok(snapshot)) => {
                    show_screen_image_result(request_id, anchor, &snapshot, true, runtime)
                }
                (_, Err(error)) => rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation request={request_id} stage=block_partial result=stale_or_invalid error={error:?}"
                )),
                (None, Ok(_)) => {}
            }
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::ScreenBlocksCompleted {
            request_id,
            base_revision,
            blocks,
            source_language,
            target_language,
            capabilities,
            processing_duration_ms,
        } => {
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some(probe) = runtime.screen_replay.as_ref() {
                probe.borrow_mut().observe_raw_screen_completion(request_id);
            }
            let (anchor, projection) = {
                let mut coordinator = screen_translation.borrow_mut();
                let anchor = coordinator.anchor(request_id);
                let projection = coordinator.apply_completed(
                    request_id,
                    base_revision,
                    blocks,
                    source_language,
                    target_language,
                    capabilities,
                );
                (anchor, projection)
            };
            match (anchor, projection) {
                (Some(anchor), Ok(projection)) => {
                    let snapshot_result =
                        completed_screen_result_snapshot(screen_translation, request_id);
                    match snapshot_result {
                        Ok((snapshot, _, _)) => {
                            show_screen_image_result(request_id, anchor, &snapshot, false, runtime);
                            persist_completed_screen_snapshot(
                                &snapshot,
                                source_language,
                                target_language,
                                runtime.screenshot_store.as_ref(),
                            );
                            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
                            if let Some(probe) = runtime.screen_replay.as_ref() {
                                let mut probe = probe.borrow_mut();
                                if probe.request_id == Some(request_id) {
                                    probe.completed = true;
                                    probe.record(format!(
                                        "request={request_id} real_translation_blocks={} completed_blocks={} failed_blocks={}",
                                        snapshot.blocks.len(),
                                        snapshot.blocks.iter().filter(|block| block.completion == rust_protocol::BlockCompletion::Completed).count(),
                                        snapshot.blocks.iter().filter(|block| block.completion != rust_protocol::BlockCompletion::Completed).count()
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            show_screen_translation_preview(
                                request_id,
                                anchor,
                                &projection.source_text,
                                &projection.translated_text,
                                translation_previews,
                                true,
                            );
                            rust_app_core::diagnostics::log_line(&format!(
                                "screen_translation request={request_id} stage=result_snapshot result=failed error={error:?}"
                            ));
                        }
                    }
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=translated revision={} structured={} context={} glossary={} duration_ms={processing_duration_ms}",
                        projection.revision,
                        projection.capabilities.structured_blocks,
                        projection.capabilities.context_instructions,
                        projection.capabilities.glossary_guidance,
                    ));
                }
                (anchor, Err(error)) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "screen_translation request={request_id} stage=translated result=stale_or_invalid error={error:?}"
                    ));
                    if let Some(anchor) = anchor
                        && !matches!(
                            error,
                            DocumentError::RequestMismatch
                                | DocumentError::RevisionMismatch
                                | DocumentError::MachineResultAlreadyApplied
                        )
                    {
                        screen_translation.borrow_mut().complete(request_id);
                        show_screen_translation_error(
                            request_id,
                            anchor,
                            state_text(state, UiText::TranslationFailedTryAgain),
                            runtime,
                        );
                    }
                }
                (None, Ok(_)) => {}
            }
        }
        #[cfg(not(feature = "screen-translation"))]
        event @ (TranslationWorkerEvent::ScreenBlockPartial { .. }
        | TranslationWorkerEvent::ScreenBlocksCompleted { .. }) => {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_worker event={} stage=screen_feature_disabled",
                rust_protocol::translation_event_name(&event)
            ));
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::GlossaryList {
            operation_id,
            entries,
        } => {
            if runtime
                .glossary_view
                .borrow_mut()
                .apply_list(operation_id, entries)
            {
                sync_glossary_settings(runtime);
            }
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::GlossarySaved {
            operation_id,
            entry,
        } => {
            let editor_saved = runtime
                .active_screen_editor
                .borrow_mut()
                .as_mut()
                .is_some_and(|active| active.state.succeed(operation_id));
            if editor_saved {
                runtime.glossary_view.borrow_mut().merge_entry(entry);
            } else if !runtime
                .glossary_view
                .borrow_mut()
                .apply_saved(operation_id, entry)
            {
                return;
            }
            sync_glossary_settings(runtime);
            if editor_saved {
                if let Some(active) = runtime.active_screen_editor.borrow().as_ref() {
                    refresh_screen_result_preview(
                        active.state.key().request_id,
                        active.block_index,
                        runtime,
                    );
                }
                close_screen_translation_editor(runtime, true);
            }
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::GlossaryDeleted {
            operation_id,
            entry_id,
        } => {
            if runtime
                .glossary_view
                .borrow_mut()
                .apply_deleted(operation_id, &entry_id)
            {
                sync_glossary_settings(runtime);
            }
        }
        #[cfg(feature = "screen-translation")]
        TranslationWorkerEvent::GlossaryFailed {
            operation_id,
            code,
            message,
            retryable,
        } => {
            let editor_failed = runtime
                .active_screen_editor
                .borrow_mut()
                .as_mut()
                .is_some_and(|active| {
                    active
                        .state
                        .fail(operation_id, format!("{code}: {message}"), retryable)
                });
            if editor_failed {
                if let Some(active) = runtime.active_screen_editor.borrow().as_ref()
                    && let Some(slot) = runtime
                        .screen_result_windows
                        .borrow()
                        .slot_by_handle(active.host_window_handle)
                {
                    slot.window.set_screen_editor_saving(false);
                    slot.window.set_screen_save_error(true);
                }
            } else if runtime.glossary_view.borrow_mut().apply_failure(
                operation_id,
                format!("{code}: {message}"),
                retryable,
            ) {
                sync_glossary_settings(runtime);
            }
        }
        #[cfg(not(feature = "screen-translation"))]
        event @ (TranslationWorkerEvent::GlossaryList { .. }
        | TranslationWorkerEvent::GlossarySaved { .. }
        | TranslationWorkerEvent::GlossaryDeleted { .. }
        | TranslationWorkerEvent::GlossaryFailed { .. }) => {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_worker event={} stage=screen_feature_disabled",
                rust_protocol::translation_event_name(&event)
            ));
        }
        TranslationWorkerEvent::Completed {
            result,
            created_unix_ms,
        } => {
            let pending = {
                let mut state = state.borrow_mut();
                let history_enabled = state.history_enabled;
                record_translation_history(
                    &mut state.translation_history,
                    history_enabled,
                    &result,
                    created_unix_ms,
                );
                state.pending_translations.remove(&result.request_id)
            };
            route_translation_result(
                result,
                pending,
                settings,
                translation_previews,
                translator,
                state,
                workers,
                screen_translation,
            );
        }
        TranslationWorkerEvent::RequestFailed {
            request_id,
            failure,
        } => {
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some(probe) = runtime.screen_replay.as_ref() {
                let mut probe = probe.borrow_mut();
                if probe.cancelled_request_ids.contains(&request_id) {
                    probe.finish(Err(format!(
                        "cancelled request={request_id} received late RequestFailed"
                    )));
                }
                if probe.request_id == Some(request_id) {
                    probe.failure_kind = Some(failure.kind.clone());
                    let detail: String = failure.detail.chars().take(512).collect();
                    probe.failure = Some(format!(
                        "kind={:?} provider={:?} detail={detail:?}",
                        failure.kind, failure.provider_id
                    ));
                    let recorded_failure = probe.failure.clone();
                    probe.record(format!(
                        "request={request_id} real_worker_failure={recorded_failure:?}"
                    ));
                    if probe.step() == Some(ReplayStep::WorkerFailure)
                        && (failure.kind != TranslationFailureKind::UnsupportedRequest
                            || !failure
                                .detail
                                .contains("source and target languages are the same"))
                    {
                        probe.finish(Err(format!("worker-failure request={request_id} did not return expected UnsupportedRequest")));
                    }
                }
            }
            // Keep the worker's original failure even if subsequent cleanup or
            // presentation fails; otherwise a UI panic hides its trigger.
            let diagnostic_detail: String = failure.detail.chars().take(512).collect();
            rust_app_core::diagnostics::log_line(&format!(
                "translation_pipeline request={request_id} stage=failed kind={:?} provider={} detail={diagnostic_detail:?}",
                failure.kind,
                failure.provider_id.as_deref().unwrap_or("none"),
            ));
            let (mut pending, install_offer) = {
                let mut app = state.borrow_mut();
                let pending = app.pending_translations.remove(&request_id);
                let install_offer = pending.as_ref().and_then(|_| {
                    required_compact_install(
                        &failure,
                        &app.translation_models,
                        app.queued_model_translation.is_some(),
                    )
                });
                (pending, install_offer)
            };
            if let Some((offer, pending_translation)) =
                take_pending_translation_install_offer(install_offer, &mut pending)
            {
                let origin = pending_translation.origin;
                let source_text = pending_translation.source_text.clone();
                let size = settings_projection::human_size(offer.size_bytes);
                let offer_message = state_message(
                    state,
                    UiText::CompactModelDownloadOffer,
                    &[("model", &offer.display_name), ("size", &size)],
                );
                let offer_action = state_text(state, UiText::DownloadModel);
                state.borrow_mut().queued_model_translation = Some(QueuedModelTranslation {
                    catalog_id: offer.catalog_id.clone(),
                    request_id,
                    pending: pending_translation,
                });
                match origin {
                    TranslationOrigin::Translator => {
                        if let Some(translator) = translator.upgrade() {
                            translator.set_processing(false);
                            translator.set_presentation_state(3);
                            translator.set_notice_headline(offer_message.clone().into());
                            translator.set_notice_detail("".into());
                            translator.set_status_text(offer_message.clone().into());
                            translator.set_model_offer_action(offer_action.into());
                            translator.set_model_offer_visible(true);
                        }
                    }
                    TranslationOrigin::Selection {
                        target,
                        delivery: TranslationDelivery::Preview,
                    } => {
                        show_selection_translation_preview(
                            request_id,
                            target,
                            &source_text,
                            &offer_message,
                            translation_previews,
                            true,
                        );
                        if let Some(slot) =
                            translation_previews.borrow().slot_for_request(request_id)
                        {
                            slot.preview.set_pin_enabled(false);
                            slot.preview.set_model_offer_action(offer_action.into());
                            slot.preview.set_model_offer_visible(true);
                        }
                    }
                    TranslationOrigin::Selection { .. } => {}
                    TranslationOrigin::ScreenRegion { anchor } => {
                        show_screen_translation_preview(
                            request_id,
                            anchor,
                            &source_text,
                            &offer_message,
                            translation_previews,
                            true,
                        );
                        if let Some(slot) =
                            translation_previews.borrow().slot_for_request(request_id)
                        {
                            slot.preview.set_pin_enabled(false);
                            slot.preview.set_model_offer_action(offer_action.into());
                            slot.preview.set_model_offer_visible(true);
                        }
                    }
                }
                set_indicator_status(settings, &offer_message);
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_pipeline request={request_id} stage=required_pack_consent catalog_id={} bytes={}",
                    offer.catalog_id, offer.size_bytes
                ));
                return;
            }
            state
                .borrow_mut()
                .selection_release_gates
                .remove(&request_id);
            let locale = active_locale(state.borrow().ui_locale);
            let notice = translator_status::failure_notice(
                locale,
                &failure,
                &state.borrow().translation_models,
            );
            let message = notice.message();

            match pending.as_ref().map(|pending| pending.origin) {
                Some(TranslationOrigin::Translator) => {
                    if let Some(translator) = translator.upgrade() {
                        translator.set_processing(false);
                        translator.set_presentation_state(4);
                        translator.set_notice_headline(notice.headline.clone().into());
                        translator.set_notice_detail(notice.detail.clone().into());
                        translator.set_status_text(message.clone().into());
                    }
                }
                Some(TranslationOrigin::Selection { target, .. }) => {
                    if translation_previews
                        .borrow()
                        .slot_for_request(request_id)
                        .is_none()
                    {
                        translation_previews
                            .borrow_mut()
                            .reserve_current(request_id);
                    }
                    show_selection_translation_preview(
                        request_id,
                        target,
                        &pending.as_ref().expect("pending translation").source_text,
                        &message,
                        translation_previews,
                        true,
                    );
                }
                Some(TranslationOrigin::ScreenRegion { anchor }) => {
                    screen_translation.borrow_mut().complete(request_id);
                    show_screen_translation_preview(
                        request_id,
                        anchor,
                        &pending.as_ref().expect("pending translation").source_text,
                        &message,
                        translation_previews,
                        true,
                    );
                }
                None => {
                    #[cfg(feature = "screen-translation")]
                    if let Some((anchor, _source_text)) =
                        finish_screen_translation_failure(screen_translation, request_id)
                    {
                        show_screen_translation_error(request_id, anchor, &message, runtime);
                    }
                }
            }

            match notice.severity {
                translator_status::TranslatorSeverity::Warning => {
                    set_indicator_status(settings, &message);
                }
                translator_status::TranslatorSeverity::Error => {
                    set_status_problem(settings, &message);
                }
            }
        }
        TranslationWorkerEvent::CredentialSaved {
            operation_id,
            provider_id,
            error,
        } => {
            providers_view::settle_credential(
                settings,
                state,
                workers,
                operation_id,
                &provider_id,
                &error,
            );
        }
        TranslationWorkerEvent::TranslationProviders { statuses } => {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_providers count={} available={}",
                statuses.len(),
                statuses.iter().filter(|status| status.available).count()
            ));
            {
                let mut state = state.borrow_mut();
                if state.translation_chain.is_empty() {
                    // Never arranged: adopt the worker's own order so the list
                    // has something to show and the first change persists.
                    state.translation_chain = statuses
                        .iter()
                        .map(|status| TranslationProviderEntry {
                            id: status.id.clone(),
                            enabled: status.enabled,
                            region: None,
                        })
                        .collect();
                }
                state.translation_providers = statuses;
            }
            sync_providers(settings, state);
        }
        TranslationWorkerEvent::Cancelled { request_id } => {
            #[cfg(all(feature = "inspect", feature = "screen-translation"))]
            if let Some(probe) = runtime.screen_replay.as_ref() {
                let mut probe = probe.borrow_mut();
                if probe.request_id == Some(request_id) {
                    probe.cancel_ack = true;
                    probe.record(format!(
                        "request={request_id} matching_worker_cancelled=true"
                    ));
                }
            }
            let pending = {
                let mut app = state.borrow_mut();
                app.selection_release_gates.remove(&request_id);
                app.pending_translations.remove(&request_id)
            };
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::ScreenRegion { .. })
            ) {
                screen_translation.borrow_mut().close(request_id);
            }
            #[cfg(feature = "screen-translation")]
            if pending.is_none()
                && screen_translation
                    .borrow()
                    .is_active_work_request(request_id)
            {
                screen_translation.borrow_mut().close(request_id);
            }
            #[cfg(feature = "screen-translation")]
            {
                let cancelled_retry = runtime
                    .screen_result_windows
                    .borrow()
                    .slot_for_request(request_id)
                    .is_some_and(|slot| slot.window.get_screen_translating());
                if cancelled_retry {
                    runtime
                        .screen_result_windows
                        .borrow_mut()
                        .retire(request_id, "translation_cancelled");
                }
            }
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::Translator)
            ) && let Some(translator) = translator.upgrade()
            {
                translator.set_processing(false);
                translator.set_presentation_state(0);
                translator.set_notice_headline("".into());
                translator.set_notice_detail("".into());
                translator.set_status_text(state_text(state, UiText::TranslationCancelled).into());
            }
            let request_slot = { translation_previews.borrow().slot_for_request(request_id) };
            if let Some(slot) = request_slot
                && !slot.preview.get_pinned()
            {
                let is_current = {
                    let previews = translation_previews.borrow();
                    slint_window_handle(previews.current.preview.window())
                        == slint_window_handle(slot.preview.window())
                };
                retire_runtime_translation_preview(&slot, "translation_cancelled", runtime);
                if is_current {
                    translation_previews.borrow_mut().current_request = None;
                }
            }
            rust_app_core::diagnostics::log_line(&format!(
                "translation_pipeline request={} stage=cancelled",
                request_id
            ));
        }
        TranslationWorkerEvent::History { entries } => {
            let mut state = state.borrow_mut();
            state.translation_history = if state.history_enabled {
                entries
            } else {
                Vec::new()
            };
        }
        TranslationWorkerEvent::HistoryCleared { error } => {
            record_history_clear_answer(settings, state, HistoryOwner::Translation, error);
        }
        TranslationWorkerEvent::Failed {
            request_id,
            catalog_id,
            code,
            message,
            ..
        } => {
            if matches!(
                code.as_str(),
                "model_remove" | "model_busy" | "model_in_use"
            ) {
                if let Some(catalog_id) = catalog_id.as_deref() {
                    state.borrow_mut().removing_models.remove(catalog_id);
                } else {
                    state.borrow_mut().removing_models.clear();
                }
                sync_translation_models(settings, state);
            }
            // A download that has ended stops offering a Cancel. The worker
            // names the row it belongs to, so exactly that row ends — this used
            // to guess from whatever happened to be downloading, and ended all
            // of them.
            if let Some(catalog_id) = catalog_id.as_deref() {
                finish_download(state, catalog_id);
                sync_translation_models(settings, state);
            }
            let pending = request_id.and_then(|id| {
                let mut app = state.borrow_mut();
                app.selection_release_gates.remove(&id);
                app.pending_translations.remove(&id)
            });
            let queued_model = if request_id.is_none() {
                let mut app = state.borrow_mut();
                let is_same_install = app.queued_model_translation.as_ref().is_some_and(|queued| {
                    catalog_id
                        .as_deref()
                        .is_none_or(|failed_id| failed_id == queued.catalog_id)
                });
                if is_same_install {
                    app.queued_model_translation.take()
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(queued) = queued_model.as_ref() {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&queued.request_id);
            }
            let queued_request_id = if request_id.is_none() {
                state
                    .borrow_mut()
                    .queued_selection_translation
                    .take()
                    .map(|request| request.request_id)
            } else {
                None
            };
            if let Some(queued_request_id) = queued_request_id {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&queued_request_id);
            }
            #[cfg(feature = "screen-translation")]
            let queued_screen = if request_id.is_none() {
                state.borrow_mut().queued_screen_translation.take()
            } else {
                None
            };
            let technical_message = state_message(
                state,
                UiText::TechnicalFailure,
                &[("code", &code), ("message", &message)],
            );
            match pending.as_ref().map(|pending| pending.origin) {
                Some(TranslationOrigin::ScreenRegion { anchor }) => {
                    let request_id = request_id.expect("pending request has an id");
                    screen_translation.borrow_mut().complete(request_id);
                    #[cfg(feature = "screen-translation")]
                    show_screen_translation_error(request_id, anchor, &technical_message, runtime);
                    #[cfg(not(feature = "screen-translation"))]
                    show_screen_translation_preview(
                        request_id,
                        anchor,
                        &pending.as_ref().expect("pending translation").source_text,
                        &technical_message,
                        translation_previews,
                        true,
                    );
                }
                _ => {
                    if let Some(request_id) = request_id {
                        #[cfg(feature = "screen-translation")]
                        if let Some((anchor, _source_text)) =
                            finish_screen_translation_failure(screen_translation, request_id)
                        {
                            show_screen_translation_error(
                                request_id,
                                anchor,
                                &technical_message,
                                runtime,
                            );
                        } else {
                            translation_previews
                                .borrow_mut()
                                .clear_current_request(request_id);
                        }
                        #[cfg(not(feature = "screen-translation"))]
                        translation_previews
                            .borrow_mut()
                            .clear_current_request(request_id);
                    }
                }
            }
            #[cfg(feature = "screen-translation")]
            if let Some(queued) = queued_screen {
                screen_translation.borrow_mut().complete(queued.request_id);
                show_screen_translation_error(
                    queued.request_id,
                    queued.anchor,
                    &technical_message,
                    runtime,
                );
            }
            if let Some(queued) = queued_model.as_ref() {
                match queued.pending.origin {
                    TranslationOrigin::Translator => {
                        if let Some(translator) = translator.upgrade() {
                            translator.set_processing(false);
                            translator.set_presentation_state(4);
                            translator.set_notice_headline(technical_message.clone().into());
                            translator.set_notice_detail("".into());
                            translator.set_status_text(technical_message.clone().into());
                        }
                    }
                    TranslationOrigin::Selection { target, .. } => {
                        show_selection_translation_preview(
                            queued.request_id,
                            target,
                            &queued.pending.source_text,
                            &technical_message,
                            translation_previews,
                            true,
                        );
                    }
                    TranslationOrigin::ScreenRegion { anchor } => {
                        screen_translation.borrow_mut().complete(queued.request_id);
                        #[cfg(feature = "screen-translation")]
                        show_screen_translation_error(
                            queued.request_id,
                            anchor,
                            &technical_message,
                            runtime,
                        );
                        #[cfg(not(feature = "screen-translation"))]
                        show_screen_translation_preview(
                            queued.request_id,
                            anchor,
                            &queued.pending.source_text,
                            &technical_message,
                            translation_previews,
                            true,
                        );
                    }
                }
            }
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::Translator)
            ) {
                set_translation_status(
                    translator,
                    &state_message(
                        state,
                        UiText::TechnicalFailure,
                        &[("code", &code), ("message", &message)],
                    ),
                );
            }
            set_indicator_status(settings, &technical_message);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn route_translation_result(
    result: TranslationResult,
    pending: Option<PendingTranslation>,
    settings: &slint::Weak<SettingsWindow>,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    translator: &slint::Weak<TranslatorWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    screen_translation: &Rc<RefCell<ScreenTranslationCoordinator>>,
) {
    let Some(pending) = pending else {
        return;
    };
    rust_app_core::diagnostics::log_line(&format!(
        "translation_pipeline request={} stage=completed model_ms={} \
         model_first_token_ms={} tokenize_ms={} chunks={} backend_requests={} \
         translation_elapsed_ms={} end_to_end_ms={}",
        result.request_id,
        result.processing_duration_ms,
        result.first_token_duration_ms,
        result.tokenize_duration_ms,
        result.chunk_count,
        result.backend_request_count,
        pending.started.elapsed().as_millis(),
        unix_ms().saturating_sub(pending.request_started_unix_ms),
    ));
    // Where the words came from, from the result itself. It used to be
    // announced in its own event just before this one, and then overwritten
    // here by a sentence that said "locally" whatever had happened.
    let locale = active_locale(state.borrow().ui_locale);
    let fallback_notice = translator_status::completion_notice(locale, &result);
    let values_notice = translator_status::protected_values_notice(locale, &result);
    if result.protected_values_changed {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={} stage=completed protected_values_changed=true",
            result.request_id
        ));
    }
    let show_values_notice = |request_id: RequestId| {
        if values_notice.is_empty() {
            return;
        }
        if let Some(slot) = translation_previews.borrow().slot_for_request(request_id) {
            slot.preview.set_notice(values_notice.as_str().into());
        }
    };
    let origin =
        settings_projection::translation_origin_localized(result.provider.as_deref(), locale);
    let completed = settings_projection::translation_completed_sentence_localized(
        result.provider.as_deref(),
        locale,
    );
    let completion_status = match pending.origin {
        TranslationOrigin::Translator => {
            if let Some(translator) = translator.upgrade() {
                translator.set_result_text(result.translated_text.into());
                let status = fallback_notice.as_ref().map_or_else(
                    || {
                        format!(
                            "{} → {} · {} ms · {origin}",
                            result.source_language.code().to_uppercase(),
                            result.target_language.code().to_uppercase(),
                            result.processing_duration_ms
                        )
                    },
                    translator_status::TranslatorNotice::message,
                );
                translator.set_presentation_state(if fallback_notice.is_some() { 3 } else { 2 });
                translator.set_notice_headline(
                    fallback_notice
                        .as_ref()
                        .map(|notice| notice.headline.as_str())
                        .unwrap_or("")
                        .into(),
                );
                translator.set_notice_detail(
                    fallback_notice
                        .as_ref()
                        .map(|notice| notice.detail.as_str())
                        .unwrap_or("")
                        .into(),
                );
                translator.set_status_text(status.into());
                translator.set_processing(false);
            }
            fallback_notice
                .as_ref()
                .map_or(completed, translator_status::TranslatorNotice::message)
        }
        TranslationOrigin::Selection { target, delivery } => match delivery {
            TranslationDelivery::Preview => {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&result.request_id);
                show_selection_translation_preview(
                    result.request_id,
                    target,
                    &result.source_text,
                    &result.translated_text,
                    translation_previews,
                    true,
                );
                show_values_notice(result.request_id);
                completed
            }
            TranslationDelivery::Replace => {
                match dispatch_or_defer_selection_replacement(
                    state,
                    workers,
                    result.request_id,
                    target,
                    result.translated_text,
                ) {
                    ReplacementDispatch::Dispatched => completed,
                    ReplacementDispatch::Deferred => {
                        state_text(state, UiText::TranslationWaitingRelease).to_owned()
                    }
                    ReplacementDispatch::Blocked(message) => {
                        state_message(state, UiText::TranslationReadyBut, &[("message", &message)])
                    }
                }
            }
            TranslationDelivery::Copy => {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&result.request_id);
                let _ = workers.input.send(&InputHostCommand::CopyText {
                    request_id: result.request_id,
                    text: result.translated_text,
                });
                completed
            }
        },
        TranslationOrigin::ScreenRegion { anchor } => {
            screen_translation.borrow_mut().complete(result.request_id);
            show_screen_translation_preview(
                result.request_id,
                anchor,
                &result.source_text,
                &result.translated_text,
                translation_previews,
                true,
            );
            show_values_notice(result.request_id);
            completed
        }
    };
    // Replace and Copy deliver the text without a card; the status line is
    // the only place left to warn.
    let completion_status =
        if values_notice.is_empty() || completion_status.contains(&values_notice) {
            completion_status
        } else {
            format!("{completion_status} {values_notice}")
        };
    set_indicator_status(settings, &completion_status);
}

#[derive(Debug, PartialEq, Eq)]
enum ReplacementDispatch {
    Dispatched,
    Deferred,
    Blocked(String),
}

fn release_gate_after_input_release(
    gate: Option<SelectionReleaseGate>,
) -> (
    Option<SelectionReleaseGate>,
    Option<(TargetSnapshot, String)>,
) {
    match gate {
        Some(SelectionReleaseGate::AwaitingReplacement { target, text }) => {
            (None, Some((target, text)))
        }
        Some(SelectionReleaseGate::Pending | SelectionReleaseGate::Released) => {
            (Some(SelectionReleaseGate::Released), None)
        }
        Some(SelectionReleaseGate::Failed(message)) => {
            (Some(SelectionReleaseGate::Failed(message)), None)
        }
        None => (None, None),
    }
}

fn release_gate_after_input_release_failure(
    gate: Option<SelectionReleaseGate>,
    message: String,
) -> (Option<SelectionReleaseGate>, bool) {
    match gate {
        Some(SelectionReleaseGate::AwaitingReplacement { .. }) => {
            (Some(SelectionReleaseGate::Failed(message)), true)
        }
        Some(
            SelectionReleaseGate::Pending
            | SelectionReleaseGate::Released
            | SelectionReleaseGate::Failed(_),
        ) => (Some(SelectionReleaseGate::Failed(message)), false),
        None => (None, false),
    }
}

fn release_gate_for_replacement(
    gate: Option<SelectionReleaseGate>,
    target: TargetSnapshot,
    text: &str,
) -> (Option<SelectionReleaseGate>, ReplacementDispatch) {
    match gate {
        Some(SelectionReleaseGate::Pending | SelectionReleaseGate::AwaitingReplacement { .. }) => (
            Some(SelectionReleaseGate::AwaitingReplacement {
                target,
                text: text.to_owned(),
            }),
            ReplacementDispatch::Deferred,
        ),
        Some(SelectionReleaseGate::Failed(message)) => {
            (None, ReplacementDispatch::Blocked(message))
        }
        Some(SelectionReleaseGate::Released) | None => (None, ReplacementDispatch::Dispatched),
    }
}

fn dispatch_or_defer_selection_replacement(
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
) -> ReplacementDispatch {
    let mut app = state.borrow_mut();
    let (next, dispatch) = release_gate_for_replacement(
        app.selection_release_gates.remove(&request_id),
        target,
        &text,
    );
    if let Some(next) = next {
        app.selection_release_gates.insert(request_id, next);
    }
    match dispatch {
        ReplacementDispatch::Dispatched => {
            drop(app);
            let _ = workers.input.send(&InputHostCommand::ReplaceSelection {
                request_id,
                target,
                text,
            });
            ReplacementDispatch::Dispatched
        }
        other => other,
    }
}

/// Makes a window a normal application window, once it has one.
///
/// Slint creates the native window on the event loop, not inside `show()`, so
/// asking for the handle on the next line always found nothing — and this
/// function quietly did nothing at all, for every window it was called on. The
/// taskbar entry, the z-order and the caption were left to whatever winit
/// happened to set.
///
/// It waits for the window instead. `spawn_local` needs the event loop, which
/// is where every caller already is.
fn configure_standard_app_window<T>(component: &T, name: &'static str)
where
    T: ComponentHandle + 'static,
{
    let weak = component.as_weak();
    if slint::spawn_local(async move {
        let Some(component) = weak.upgrade() else {
            return;
        };
        let handle = component
            .window()
            .winit_window()
            .await
            .ok()
            .as_deref()
            .and_then(native_window_handle);
        match handle {
            Some(handle) => {
                if let Err(error) = rust_platform_windows::configure_standard_window(handle) {
                    eprintln!(
                        "could not configure `{name}` as a standard application window: {error}"
                    );
                }
            }
            None => eprintln!("`{name}` never received a native window handle"),
        }
    })
    .is_err()
    {
        eprintln!("could not schedule the native configuration of `{name}`");
    }
}

fn precreate_indicator(
    indicator: &RecordingIndicator,
    indicator_presenter: &OverlayPresenter,
    transparent: bool,
) {
    indicator.set_prepared(false);
    // Slint owns creation of the native HWND, but the presenter exclusively
    // owns its native visibility after this one lifetime-establishing show.
    // The window remains logically visible to Slint and parked outside the
    // virtual desktop; no production path calls Slint show/hide again.
    indicator
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if indicator.show().is_ok() {
        let indicator = indicator.as_weak();
        let indicator_presenter = indicator_presenter.clone();
        let schedule_result = slint::spawn_local(async move {
            if let Some(indicator) = indicator.upgrade() {
                let preparation = match indicator.window().winit_window().await {
                    Ok(window) => native_window_handle(&window)
                        .ok_or_else(|| "native HWND is unavailable".to_owned())
                        .and_then(|handle| {
                            prepare_indicator_native_window(handle, transparent)?;
                            indicator_presenter.attach(handle)
                        }),
                    Err(error) => Err(format!("native window creation failed: {error}")),
                };
                indicator.set_prepared(false);
                indicator.window().request_redraw();
                if let Err(error) = preparation {
                    rust_app_core::diagnostics::log_line(&format!(
                        "recording_overlay transition=failed stage=attach error={error}"
                    ));
                }
            }
        });
        if let Err(error) = schedule_result {
            rust_app_core::diagnostics::log_line(&format!(
                "recording_overlay transition=failed stage=schedule_attach error={error}"
            ));
        }
    } else {
        rust_app_core::diagnostics::log_line(
            "recording_overlay transition=failed stage=create_native_window",
        );
    }
}

fn prepare_indicator_native_window(window_handle: u64, transparent: bool) -> Result<(), String> {
    if transparent {
        rust_platform_windows::prepare_transparent_overlay_window(window_handle)
    } else {
        rust_platform_windows::prepare_overlay_window(window_handle)
    }
}

fn show_indicator(
    indicator: &RecordingIndicator,
    indicator_presenter: &OverlayPresenter,
    transparent: bool,
    target_window_handle: Option<u64>,
) {
    indicator.set_prepared(false);
    let ticket = match indicator_presenter.begin_prepare() {
        Ok(ticket) => ticket,
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "recording_overlay transition=failed stage=begin_prepare error={error}"
            ));
            return;
        }
    };
    if let Err(error) = configure_indicator_window(indicator, transparent, target_window_handle) {
        indicator.set_prepared(false);
        indicator_presenter.abort_prepare(indicator.window(), ticket, "configure", &error);
        return;
    }
    if let Err(error) = refresh_indicator_backdrop(indicator) {
        rust_app_core::diagnostics::log_line(&format!(
            "recording_overlay backdrop=fallback generation={} error={error}",
            ticket.generation()
        ));
    }
    indicator.set_prepared(true);
    if let Err(error) = indicator_presenter.await_frame(indicator.window(), ticket) {
        indicator.set_prepared(false);
        indicator_presenter.abort_prepare(indicator.window(), ticket, "await_frame", &error);
    }
}

fn configure_indicator_window(
    indicator: &RecordingIndicator,
    transparent: bool,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    let handle = slint_window_handle(indicator.window())
        .ok_or_else(|| "native HWND is unavailable".to_owned())?;
    let style = VisualizerStyle::from_index(indicator.get_visualizer_style())
        .ok_or_else(|| "recording indicator has an unknown visualizer style".to_owned())?;
    if style.uses_transparent_window() != transparent {
        return Err(format!(
            "recording surface mismatch style={style:?} expected_transparent={} actual_transparent={transparent}",
            style.uses_transparent_window(),
        ));
    }
    let blue_glass = style == VisualizerStyle::WritingSlate
        && WritingSlateSurface::from_index(indicator.get_writing_slate_surface())
            == Some(WritingSlateSurface::BlueGlass);
    if !transparent {
        return rust_platform_windows::configure_overlay_window(handle, target_window_handle);
    }
    if blue_glass {
        rust_platform_windows::configure_blue_glass_overlay_window(handle, target_window_handle)
    } else {
        rust_platform_windows::configure_transparent_overlay_window(handle, target_window_handle)
    }
}

fn refresh_indicator_backdrop(indicator: &RecordingIndicator) -> Result<(), String> {
    if VisualizerStyle::from_index(indicator.get_visualizer_style())
        != Some(VisualizerStyle::WritingSlate)
        || WritingSlateSurface::from_index(indicator.get_writing_slate_surface())
            != Some(WritingSlateSurface::BlueGlass)
    {
        return Ok(());
    }

    indicator.set_writing_slate_glass_backdrop(Image::default());
    let Some(handle) = slint_window_handle(indicator.window()) else {
        return Err("native HWND is unavailable for backdrop capture".to_owned());
    };
    match rust_platform_windows::capture_blurred_cloaked_overlay_backdrop(handle, 1) {
        Ok(backdrop)
            if backdrop.rgba.len() == backdrop.width as usize * backdrop.height as usize * 4 =>
        {
            let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                &backdrop.rgba,
                backdrop.width,
                backdrop.height,
            );
            indicator.set_writing_slate_glass_backdrop(Image::from_rgba8(pixels));
            Ok(())
        }
        Ok(_) => Err("captured recording-indicator backdrop has invalid length".to_owned()),
        Err(error) => Err(format!(
            "could not capture recording-indicator backdrop: {error}"
        )),
    }
}

fn configure_indicator_native_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
    transparent: bool,
) -> bool {
    if transparent {
        rust_platform_windows::configure_transparent_overlay_window(
            window_handle,
            target_window_handle,
        )
    } else {
        rust_platform_windows::configure_overlay_window(window_handle, target_window_handle)
    }
    .is_ok()
}

fn slint_window_handle(window: &slint::Window) -> Option<u64> {
    window.with_winit_window(native_window_handle).flatten()
}

fn native_window_handle(window: &slint::winit_030::winit::window::Window) -> Option<u64> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as u64),
        _ => None,
    }
}

/// Ends a download, whatever ended it.
///
/// The worker has several ways of saying "this model is no longer coming down
/// the wire": installed, ready, selected, or failed. Clearing on only one of
/// them leaves the row claiming to be busy forever.
fn finish_download(state: &Rc<RefCell<AppState>>, catalog_id: &str) -> bool {
    state.borrow_mut().downloads.remove(catalog_id).is_some()
}

/// How many dictations the History page asks for.
///
/// It asked for five, and then the page said "5 kept" as though that were the
/// whole of it. The list scrolls, so the only thing more rows cost is the size
/// of one message — and this is every entry the worker keeps
/// (`MAX_HISTORY_ENTRIES` in the speech worker), so the page is now showing all
/// of them rather than a window onto them.
pub(crate) const HISTORY_LIMIT: usize = 250;

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(all(test, feature = "screen-translation"))]
mod screen_translation_edit_borrow_tests {
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::{PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext};

    use super::{
        ScreenTranslationCoordinator, ScreenTranslationDocument, expire_screen_translation_pixels,
        save_screen_translation_edit,
    };

    #[test]
    fn saving_an_edit_releases_the_mutable_coordinator_borrow_before_refresh_reads() {
        let request_id = RequestId::new();
        let block_id = ScreenBlockId::from("block-1");
        let crop = Arc::new(
            RgbaFrame::try_new(
                PhysicalRect {
                    x: 3_002,
                    y: 313,
                    width: 1,
                    height: 1,
                },
                4,
                vec![0, 0, 0, 255],
            )
            .unwrap(),
        );
        let document = ScreenTranslationDocument::new(
            request_id,
            crop,
            vec![ScreenTextBlock {
                quad: None,
                fragment_quads: Vec::new(),
                block_id: block_id.clone(),
                source_text: "Cooldown".to_owned(),
                bounds: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                fragments: Vec::new(),
                confidence_milli: 900,
            }],
            SourceContext::default(),
            1,
            2,
        )
        .unwrap();
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        coordinator.borrow_mut().insert_document(document);

        let revision = save_screen_translation_edit(
            &coordinator,
            request_id,
            &block_id,
            0,
            "Перезарядка".to_owned(),
        )
        .unwrap();

        assert_eq!(revision, 1);
        assert_eq!(
            coordinator.borrow().copy_text(request_id).unwrap(),
            "Перезарядка"
        );
    }

    #[test]
    fn expiry_dispatches_each_released_request_to_the_view_layer() {
        let request_id = RequestId::new();
        let crop = Arc::new(
            RgbaFrame::try_new(
                PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                4,
                vec![0, 0, 0, 255],
            )
            .unwrap(),
        );
        let document = ScreenTranslationDocument::new(
            request_id,
            crop,
            vec![ScreenTextBlock {
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("block-1"),
                source_text: "Source".to_owned(),
                bounds: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                fragments: Vec::new(),
                confidence_milli: 900,
            }],
            SourceContext::default(),
            1,
            2,
        )
        .unwrap();
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        coordinator.borrow_mut().insert_document(document);
        let mut notified = Vec::new();

        let released = expire_screen_translation_pixels(&coordinator, 2, |expired| {
            notified.push(expired);
        });

        assert_eq!(released, 1);
        assert_eq!(notified, vec![request_id]);
    }
}

#[cfg(test)]
mod selection_release_gate_tests {
    use rust_protocol::TargetSnapshot;

    use super::{
        ReplacementDispatch, SelectionReleaseGate, release_gate_after_input_release,
        release_gate_after_input_release_failure, release_gate_for_replacement,
    };

    #[test]
    fn replacement_is_deferred_until_the_physical_chord_is_released() {
        let target = TargetSnapshot {
            window_handle: 42,
            process_id: 7,
            thread_id: 8,
            keyboard_layout: 9,
            ..TargetSnapshot::default()
        };
        let (gate, dispatch) =
            release_gate_for_replacement(Some(SelectionReleaseGate::Pending), target, "translated");
        assert_eq!(dispatch, ReplacementDispatch::Deferred);

        let (next, deferred) = release_gate_after_input_release(gate);
        assert!(next.is_none());
        assert_eq!(deferred, Some((target, "translated".to_owned())));
    }

    #[test]
    fn released_chord_dispatches_without_an_extra_wait() {
        let (gate, dispatch) = release_gate_for_replacement(
            Some(SelectionReleaseGate::Released),
            TargetSnapshot::default(),
            "translated",
        );
        assert!(gate.is_none());
        assert_eq!(dispatch, ReplacementDispatch::Dispatched);
    }

    #[test]
    fn failed_release_never_dispatches_delayed_input() {
        let (gate, dispatch) = release_gate_for_replacement(
            Some(SelectionReleaseGate::Failed("still held".to_owned())),
            TargetSnapshot::default(),
            "translated",
        );
        assert!(gate.is_none());
        assert_eq!(
            dispatch,
            ReplacementDispatch::Blocked("still held".to_owned())
        );
    }

    #[test]
    fn late_release_failure_does_not_resurrect_a_completed_preview_gate() {
        let (gate, had_deferred_replacement) =
            release_gate_after_input_release_failure(None, "still held".to_owned());
        assert!(gate.is_none());
        assert!(!had_deferred_replacement);
    }
}

#[cfg(test)]
mod provider_path_readiness_tests {
    use rust_protocol::{
        ModelSummary, SpeechOutputMode, TranscriptionProviderEntry, TranscriptionProviderStatus,
        TranslationProviderStatus,
    };

    use super::{AppState, SavedSettings, SpeechBackend};

    fn state() -> AppState {
        AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        )
    }

    fn speech_status(id: &str, position: usize, available: bool) -> TranscriptionProviderStatus {
        TranscriptionProviderStatus {
            id: id.to_owned(),
            position,
            enabled: true,
            available,
            detail: String::new(),
            has_credential: id != "local",
            credential_hint: None,
        }
    }

    fn translation_status(id: &str, position: usize, available: bool) -> TranslationProviderStatus {
        TranslationProviderStatus {
            id: id.to_owned(),
            position,
            enabled: true,
            available,
            detail: String::new(),
            has_credential: id != "local",
            credential_hint: None,
        }
    }

    fn local_model(supports_speech_translation: bool) -> ModelSummary {
        ModelSummary {
            catalog_id: "whisper-large-v3-turbo".to_owned(),
            display_name: "Whisper".to_owned(),
            description: String::new(),
            size_bytes: 1,
            installed: true,
            selected: true,
            recommended: true,
            supports_speech_translation,
        }
    }

    fn configured(id: &str) -> TranscriptionProviderEntry {
        TranscriptionProviderEntry {
            id: id.to_owned(),
            enabled: true,
            endpoint: None,
            model: None,
            account: None,
        }
    }

    #[test]
    fn cloud_only_speech_needs_the_worker_but_not_the_local_model() {
        let mut state = state();
        state.transcription_providers = vec![speech_status("groq", 0, true)];
        assert!(!state.speech_path_ready());

        state.speech_worker_ready = true;
        assert!(state.speech_path_ready());
        assert!(!state.speech_waits_for_local());
    }

    #[test]
    fn local_first_speech_waits_for_the_local_model() {
        let mut state = state();
        state.speech_worker_ready = true;
        state.transcription_providers = vec![
            speech_status("local", 0, true),
            speech_status("groq", 1, true),
        ];
        assert!(!state.speech_path_ready());
        assert!(state.speech_waits_for_local());

        state.speech_ready = true;
        assert!(state.speech_path_ready());
    }

    #[test]
    fn manually_unloaded_speech_can_start_and_reload_without_being_reported_ready() {
        let mut state = state();
        state.speech_worker_ready = true;
        state.speech_reload_on_next_use = true;

        assert!(!state.speech_path_ready());
        assert!(state.speech_path_can_start());
    }

    #[test]
    fn an_unavailable_cloud_only_route_does_not_fall_through_to_disabled_local() {
        let mut state = state();
        state.speech_worker_ready = true;
        state.speech_ready = true;
        state.transcription_providers = vec![speech_status("groq", 0, false)];
        assert!(!state.speech_path_ready());
        assert!(!state.speech_waits_for_local());
    }

    #[test]
    fn groq_can_translate_speech_even_when_the_local_model_cannot() {
        let mut state = state();
        state.models = vec![local_model(false)];
        state.transcription_chain = vec![configured("groq")];

        assert!(state.speech_translation_supported());
    }

    #[test]
    fn transcription_only_services_do_not_unlock_speech_translation() {
        let mut state = state();
        state.models = vec![local_model(false)];
        state.transcription_chain = vec![configured("deepgram"), configured("azure")];

        assert!(!state.speech_translation_supported());
    }

    #[test]
    fn cloud_only_translation_needs_the_worker_but_not_the_local_model() {
        let mut state = state();
        state.translation_providers = vec![translation_status("deepl", 0, true)];
        assert!(!state.translation_path_ready());

        state.translation_worker_ready = true;
        assert!(state.translation_path_ready());
        assert!(!state.translation_waits_for_local());
    }

    #[test]
    fn manually_unloaded_translation_can_start_and_reload_without_being_reported_ready() {
        let mut state = state();
        state.translation_worker_ready = true;
        state.translation_reload_on_next_use = true;

        assert!(!state.translation_path_ready());
        assert!(state.translation_path_can_start());
    }

    #[test]
    fn an_unavailable_text_service_does_not_use_a_disabled_local_model() {
        let mut state = state();
        state.translation_worker_ready = true;
        state.translation_ready = true;
        state.translation_providers = vec![translation_status("deepl", 0, false)];
        assert!(!state.translation_path_ready());
        assert!(!state.translation_waits_for_local());
    }
}

#[cfg(test)]
mod hotkey_ack_tests {
    use rust_protocol::{
        HotkeyAction, HotkeyFailure, HotkeyRejection, HotkeySpec, RequestId, SpeechOutputMode,
    };

    use super::{
        AppState, SavedSettings, SpeechBackend, commit_hotkeys_if_pending,
        reject_hotkeys_if_pending, settings_document,
    };

    fn state() -> AppState {
        AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        )
    }

    #[test]
    fn a_rejected_binding_is_never_promoted_to_persisted_state() {
        let mut state = state();
        let committed = state.hotkeys.clone();
        let mut proposed = committed.clone();
        proposed.binding_mut(HotkeyAction::Translate).spec = HotkeySpec::control_alt(b'G' as u16);
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, proposed));

        assert!(reject_hotkeys_if_pending(
            &mut state,
            operation_id,
            Some(HotkeyAction::Translate),
            HotkeyRejection::RegistrationFailed,
        ));
        assert_eq!(state.hotkeys, committed);
        assert!(state.pending_hotkeys.is_none());
        assert_eq!(
            state.hotkey_errors.get(&HotkeyAction::Translate),
            Some(&HotkeyRejection::RegistrationFailed)
        );
    }

    fn refused(action: HotkeyAction) -> HotkeyFailure {
        HotkeyFailure {
            action,
            reason: HotkeyRejection::RegistrationFailed,
            message: "Windows refused the shortcut (error 1409).".to_owned(),
        }
    }

    #[test]
    fn only_the_matching_ack_promotes_the_requested_configuration() {
        let mut state = state();
        let mut proposed = state.hotkeys.clone();
        proposed.binding_mut(HotkeyAction::Translate).enabled = true;
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, proposed.clone()));

        assert!(!commit_hotkeys_if_pending(
            &mut state,
            RequestId::new(),
            proposed.clone(),
            &[],
        ));
        assert!(
            state.pending_hotkeys.is_some(),
            "a stale ack keeps the wait"
        );
        assert!(commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            proposed.clone(),
            &[],
        ));
        assert_eq!(state.hotkeys, proposed);
        assert_eq!(state.registered_hotkeys, proposed);
        assert!(state.translation_enabled);
        assert!(state.hotkey_errors.is_empty());
    }

    /// The machine this was found on: another application holds Ctrl+Alt+P.
    #[test]
    fn a_chord_windows_refuses_at_start_up_stays_chosen_and_is_saved_enabled() {
        let mut state = state();
        let chosen = state.hotkeys.clone();
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, chosen.clone()));
        let mut registered = chosen.clone();
        registered.pause_resume.enabled = false;

        assert!(commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            registered,
            &[refused(HotkeyAction::PauseResume)],
        ));

        assert_eq!(state.hotkeys, chosen, "the user's choice is not rewritten");
        assert!(!state.registered_hotkeys.pause_resume.enabled);
        assert_eq!(
            state.hotkey_errors.get(&HotkeyAction::PauseResume),
            Some(&HotkeyRejection::RegistrationFailed)
        );
        let saved = settings_document::SavedSettings::from_state(&state);
        assert!(saved.hotkeys.pause_resume.enabled);
        assert_eq!(rust_app_core::hotkeys::validate(&saved.hotkeys), Ok(()));
    }

    #[test]
    fn after_a_refused_start_up_chord_pause_can_be_moved_to_a_free_one() {
        let mut state = state();
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, state.hotkeys.clone()));
        let mut registered = state.hotkeys.clone();
        registered.pause_resume.enabled = false;
        commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            registered,
            &[refused(HotkeyAction::PauseResume)],
        );

        // What the shortcut editor builds when the user records Ctrl+Alt+O.
        let mut edited = state.hotkeys.clone();
        edited.pause_resume.spec = HotkeySpec::control_alt(u16::from(b'O'));
        assert_eq!(rust_app_core::hotkeys::validate(&edited), Ok(()));

        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, edited.clone()));
        assert!(commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            edited.clone(),
            &[],
        ));
        assert_eq!(state.hotkeys, edited);
        assert!(state.registered_hotkeys.pause_resume.enabled);
        assert!(state.hotkey_errors.is_empty());
    }

    #[test]
    fn a_refused_edit_keeps_the_previous_choice_and_other_edits_still_land() {
        let mut state = state();
        let before = state.hotkeys.clone();
        let mut requested = before.clone();
        requested.correct_layout.spec = HotkeySpec::control_alt(u16::from(b'L'));
        requested.insert_last.spec = HotkeySpec::control_alt(u16::from(b'B'));
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, requested.clone()));
        // The input host kept the old layout chord live and moved insert-last.
        let mut registered = requested.clone();
        registered.correct_layout = before.correct_layout;

        assert!(commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            registered,
            &[refused(HotkeyAction::CorrectLayout)],
        ));

        assert_eq!(state.hotkeys.correct_layout, before.correct_layout);
        assert_eq!(state.hotkeys.insert_last, requested.insert_last);
        assert!(state.layout_enabled);
    }

    #[test]
    fn refusing_the_translate_chord_does_not_switch_the_feature_off() {
        let mut state = state();
        state.hotkeys.translate.enabled = true;
        state.translation_enabled = true;
        let chosen = state.hotkeys.clone();
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, chosen.clone()));
        let mut registered = chosen;
        registered.translate.enabled = false;

        commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            registered,
            &[refused(HotkeyAction::Translate)],
        );

        assert!(state.translation_enabled);
        assert!(state.hotkeys.translate.enabled);
    }

    #[test]
    fn chosen_chords_survive_a_restart_after_a_start_up_refusal() {
        let mut state = state();
        state.hotkeys.dictate.spec = HotkeySpec::control_alt(u16::from(b'D'));
        state.hotkeys.insert_last.spec = HotkeySpec::control_alt(u16::from(b'I'));
        let chosen = state.hotkeys.clone();
        let operation_id = RequestId::new();
        state.pending_hotkeys = Some((operation_id, chosen.clone()));
        let mut registered = chosen.clone();
        registered.pause_resume.enabled = false;
        commit_hotkeys_if_pending(
            &mut state,
            operation_id,
            registered,
            &[refused(HotkeyAction::PauseResume)],
        );

        let document =
            serde_json::to_vec_pretty(&settings_document::SavedSettings::from_state(&state))
                .unwrap();
        let reloaded = settings_document::parse_settings(&document);

        assert_eq!(reloaded.hotkeys, chosen);
    }
}

#[cfg(test)]
mod insert_last_tests {
    use rust_protocol::{SessionId, SpeechOutputMode, TargetSnapshot, Transcript};

    use super::{AppState, InsertLastResolution};

    fn transcript(session_id: SessionId, text: &str) -> Transcript {
        Transcript {
            session_id,
            text: text.to_owned(),
            language: "ru".to_owned(),
            audio_duration_ms: 500,
            processing_duration_ms: 100,
            provider: None,
            latency: Default::default(),
        }
    }

    fn target(window_handle: u64) -> TargetSnapshot {
        TargetSnapshot {
            window_handle,
            process_id: 7,
            thread_id: 8,
            keyboard_layout: 9,
            ..TargetSnapshot::default()
        }
    }

    #[test]
    fn speech_clear_is_not_undone_by_translation_failure() {
        let mut app = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        let id = SessionId::new();
        app.registry.begin(id, 1).unwrap();
        app.registry.end(id).unwrap();
        app.registry
            .complete(id, transcript(id, "deleted speech"))
            .unwrap();
        app.transcripts.insert(id, transcript(id, "deleted speech"));
        app.latest_history_session = Some(id);
        app.history_clear = Some(super::HistoryClear::default());
        app.pending_insert_last = Some(super::PendingInsertLast {
            session_id: Some(id),
            target: target(42),
        });
        let state = std::rc::Rc::new(std::cell::RefCell::new(app));
        let settings = slint::Weak::<super::SettingsWindow>::default();
        super::record_history_clear_answer(&settings, &state, super::HistoryOwner::Speech, None);
        super::record_history_clear_answer(
            &settings,
            &state,
            super::HistoryOwner::Translation,
            Some("translation file busy".into()),
        );
        let mut app = state.borrow_mut();
        assert!(app.latest_transcript().is_none());
        assert!(app.pending_insert_last.is_none());
        assert!(matches!(
            app.request_insert_last(target(42)),
            InsertLastResolution::Unavailable(_)
        ));
    }

    #[test]
    fn new_completion_after_speech_clear_survives_later_translation_ack() {
        let mut app = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        app.history_clear = Some(super::HistoryClear::default());
        let state = std::rc::Rc::new(std::cell::RefCell::new(app));
        let settings = slint::Weak::<super::SettingsWindow>::default();
        super::record_history_clear_answer(&settings, &state, super::HistoryOwner::Speech, None);
        let id = SessionId::new();
        {
            let mut app = state.borrow_mut();
            app.registry.begin(id, 2).unwrap();
            app.registry.end(id).unwrap();
            app.registry
                .complete(id, transcript(id, "new speech"))
                .unwrap();
        }
        super::record_history_clear_answer(
            &settings,
            &state,
            super::HistoryOwner::Translation,
            None,
        );
        assert_eq!(
            state.borrow().latest_transcript().map(|t| t.text.as_str()),
            Some("new speech")
        );
    }

    #[test]
    fn successful_history_clear_removes_insert_last_and_pending_insertion() {
        let state = std::rc::Rc::new(std::cell::RefCell::new(AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        )));
        let completed = SessionId::new();
        {
            let mut app = state.borrow_mut();
            app.registry.begin(completed, 1).unwrap();
            app.registry.end(completed).unwrap();
            let old = transcript(completed, "removed text");
            app.registry.complete(completed, old.clone()).unwrap();
            app.transcripts.insert(completed, old);
            app.latest_history_session = Some(completed);
            app.pending_insert_last = Some(super::PendingInsertLast {
                session_id: Some(completed),
                target: target(42),
            });
            app.history_clear = Some(super::HistoryClear::default());
        }
        let settings = slint::Weak::<super::SettingsWindow>::default();
        super::record_history_clear_answer(
            &settings,
            &state,
            super::HistoryOwner::Translation,
            None,
        );
        assert!(
            state.borrow().latest_transcript().is_some(),
            "wait for speech acknowledgement"
        );
        super::record_history_clear_answer(&settings, &state, super::HistoryOwner::Speech, None);
        let mut app = state.borrow_mut();
        assert!(
            app.latest_transcript().is_none(),
            "clear must include the session registry"
        );
        assert!(matches!(
            app.request_insert_last(target(42)),
            InsertLastResolution::Unavailable(_)
        ));
        assert!(app.pending_insert_last.is_none());
    }

    #[test]
    fn processing_latest_session_never_falls_back_to_previous_text() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        let completed = SessionId::new();
        state.registry.begin(completed, 1).unwrap();
        state.registry.end(completed).unwrap();
        let old = transcript(completed, "предыдущий текст");
        state.registry.complete(completed, old.clone()).unwrap();
        state.transcripts.insert(completed, old);
        state.latest_history_session = Some(completed);

        let processing = SessionId::new();
        state.registry.begin(processing, 2).unwrap();
        state.registry.end(processing).unwrap();

        let requested_target = target(42);
        let resolution = state.request_insert_last(requested_target);
        assert!(matches!(
            resolution,
            InsertLastResolution::Await(session_id) if session_id == processing
        ));
        let pending = state.pending_insert_last.as_ref().unwrap();
        assert_eq!(pending.session_id, Some(processing));
        assert_eq!(pending.target, requested_target);
    }

    #[test]
    fn completed_latest_session_is_inserted_directly() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        let session_id = SessionId::new();
        state.registry.begin(session_id, 1).unwrap();
        state.registry.end(session_id).unwrap();
        let latest = transcript(session_id, "последний текст");
        state.registry.complete(session_id, latest.clone()).unwrap();
        state.transcripts.insert(session_id, latest);

        let resolution = state.request_insert_last(target(42));
        assert!(matches!(
            resolution,
            InsertLastResolution::Insert(transcript)
                if transcript.session_id == session_id && transcript.text == "последний текст"
        ));
    }

    #[test]
    fn failed_latest_session_falls_back_to_last_completed_transcript() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        let completed = SessionId::new();
        state.registry.begin(completed, 1).unwrap();
        state.registry.end(completed).unwrap();
        state
            .registry
            .complete(completed, transcript(completed, "последний успешный"))
            .unwrap();

        let failed = SessionId::new();
        state.registry.begin(failed, 2).unwrap();
        state.registry.end(failed).unwrap();
        state.registry.fail(failed).unwrap();

        let resolution = state.request_insert_last(target(42));
        assert!(matches!(
            resolution,
            InsertLastResolution::Insert(transcript)
                if transcript.session_id == completed
                    && transcript.text == "последний успешный"
        ));
    }

    #[test]
    fn cancelled_latest_session_falls_back_to_last_completed_transcript() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            super::SpeechBackend::Cpu,
            &super::SavedSettings::default(),
        );
        let completed = SessionId::new();
        state.registry.begin(completed, 1).unwrap();
        state.registry.end(completed).unwrap();
        state
            .registry
            .complete(completed, transcript(completed, "последний успешный"))
            .unwrap();

        let cancelled = SessionId::new();
        state.registry.begin(cancelled, 2).unwrap();
        state.registry.cancel(cancelled).unwrap();

        let resolution = state.request_insert_last(target(42));
        assert!(matches!(
            resolution,
            InsertLastResolution::Insert(transcript)
                if transcript.session_id == completed
                    && transcript.text == "последний успешный"
        ));
    }
}

#[cfg(test)]
mod audio_level_coalescing_tests {
    use rust_protocol::SessionId;

    use super::{AudioLevelFrame, RuntimeEvent, record_audio_level, take_audio_level};

    #[test]
    fn only_the_newest_frame_survives_and_one_wake_up_covers_a_burst() {
        let session_id = SessionId::new();
        let frame = |peak: f32| AudioLevelFrame {
            instance_id: 7,
            session_id,
            peak,
            spectrum: vec![peak; 32],
        };

        assert!(matches!(
            record_audio_level(frame(0.1)),
            Some(RuntimeEvent::SpeechAudioLevel { instance_id: 7 })
        ));
        // The UI thread has not run yet, so these must not queue more work.
        assert!(record_audio_level(frame(0.2)).is_none());
        assert!(record_audio_level(frame(0.3)).is_none());

        let taken = take_audio_level().expect("a frame is waiting");
        assert!((taken.peak - 0.3).abs() < f32::EPSILON);
        assert!(take_audio_level().is_none());

        // Once consumed, the next frame queues its own wake-up again.
        assert!(record_audio_level(frame(0.4)).is_some());
        assert!(take_audio_level().is_some_and(|frame| (frame.peak - 0.4).abs() < f32::EPSILON));
    }
}

#[cfg(test)]
mod compact_model_install_tests {
    use std::time::Instant;

    use rust_protocol::{
        RequestId, TranslationFailureKind, TranslationFailureReport, TranslationLanguage,
        TranslationLanguagePolicy, TranslationModelSummary, TranslationTier,
    };

    use super::{
        CompactInstallOffer, PendingTranslation, QueuedModelMatch, QueuedModelTranslation,
        TranslationOrigin, required_compact_install, take_matching_queued_model_translation,
    };

    fn model(installed: bool) -> TranslationModelSummary {
        TranslationModelSummary {
            catalog_id: "firefox-de-en-v3".to_owned(),
            display_name: "German ↔ English".to_owned(),
            description: "Compact".to_owned(),
            tier: TranslationTier::Compact,
            languages: vec![TranslationLanguage::English, TranslationLanguage::German],
            size_bytes: 1,
            installed,
            selected: false,
            recommended: false,
            license: "MPL-2.0".to_owned(),
        }
    }

    fn missing_report() -> TranslationFailureReport {
        TranslationFailureReport {
            kind: TranslationFailureKind::LocalModelMissing,
            provider_id: None,
            retry_after_ms: None,
            required_catalog_id: Some("firefox-de-en-v3".to_owned()),
            detail: String::new(),
            attempts: Vec::new(),
        }
    }

    #[test]
    fn an_explicit_missing_compact_pack_can_be_installed_and_retried_once() {
        let models = vec![model(false)];
        assert_eq!(
            required_compact_install(&missing_report(), &models, false),
            Some(CompactInstallOffer {
                catalog_id: "firefox-de-en-v3".to_owned(),
                display_name: "German ↔ English".to_owned(),
                size_bytes: 1,
            })
        );
        assert_eq!(
            required_compact_install(&missing_report(), &models, true),
            None
        );
    }

    #[test]
    fn installed_or_untyped_failures_never_start_an_install() {
        let installed = vec![model(true)];
        assert_eq!(
            required_compact_install(&missing_report(), &installed, false),
            None
        );

        let mut failure = missing_report();
        failure.kind = TranslationFailureKind::Internal;
        assert_eq!(
            required_compact_install(&failure, &[model(false)], false),
            None
        );
    }

    #[test]
    fn dismissing_an_offer_allows_the_next_missing_pack_request_to_offer_again() {
        let first_request = RequestId::new();
        let mut queued = Some(QueuedModelTranslation {
            catalog_id: "firefox-de-en-v3".to_owned(),
            request_id: first_request,
            pending: PendingTranslation {
                origin: TranslationOrigin::Translator,
                source_text: "Hallo".to_owned(),
                source_language: None,
                target_language: None,
                language_policy: TranslationLanguagePolicy {
                    primary: TranslationLanguage::English,
                    secondary: TranslationLanguage::German,
                },
                started: Instant::now(),
                request_started_unix_ms: 0,
                first_partial_shown: false,
                stable_paragraphs: None,
            },
        });

        let dismissed = take_matching_queued_model_translation(
            &mut queued,
            QueuedModelMatch::Request(first_request),
        );

        assert!(dismissed.is_some());
        assert!(queued.is_none());
        assert!(
            required_compact_install(&missing_report(), &[model(false)], queued.is_some())
                .is_some()
        );
    }
}

#[cfg(all(test, feature = "screen-translation"))]
mod translation_failure_transition_tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::Arc,
        time::Instant,
    };

    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::{
        PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext,
        TranslationLanguagePolicy,
    };

    use super::{
        CompactInstallOffer, PendingTranslation, ScreenTranslationCancellation,
        ScreenTranslationCoordinator, ScreenTranslationDocument, TranslationOrigin,
        TranslationPreview, TranslationPreviewRegistry, TranslationPreviewSlot,
        apply_screen_image_snapshot, apply_screen_translation_edit_for_scope,
        cloak_compact_previews_owned, close_screen_translation_request,
        completed_screen_result_snapshot, finish_screen_translation_failure,
        screen_translation::ScreenTranslationBegin, show_translation_preview_at,
        take_pending_translation_install_offer, take_superseded_screen_compact,
        take_terminal_compact_before_screen_result,
    };
    use crate::{
        overlay_presenter::test_utils::{
            attached_presenter, prepare_without_native, presented_presenter,
        },
        translation_editor::EditorCommitScope,
        translation_view::retire_translation_preview,
    };

    fn recognized_request(
        coordinator: &Rc<RefCell<ScreenTranslationCoordinator>>,
        request_id: RequestId,
    ) -> PhysicalRect {
        let anchor = PhysicalRect {
            x: 1909,
            y: 611,
            width: 1,
            height: 1,
        };
        let crop = Arc::new(RgbaFrame::try_new(anchor, 4, vec![0, 0, 0, 255]).unwrap());
        let document = ScreenTranslationDocument::new(
            request_id,
            crop,
            vec![ScreenTextBlock {
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("block-1"),
                source_text: "Recognized screen text".to_owned(),
                bounds: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                fragments: Vec::new(),
                confidence_milli: 937,
            }],
            SourceContext::default(),
            1,
            2,
        )
        .unwrap();
        let mut coordinator = coordinator.borrow_mut();
        assert!(matches!(
            coordinator.begin(request_id),
            ScreenTranslationBegin::Started(_)
        ));
        assert!(coordinator.selected(request_id, anchor));
        assert_eq!(coordinator.recognized(request_id), Some(anchor));
        coordinator.insert_document(document);
        anchor
    }

    #[test]
    fn screen_failure_returns_source_and_releases_borrows_before_the_next_request() {
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let failed_request = RequestId::new();
        let anchor = recognized_request(&coordinator, failed_request);
        assert!(coordinator.borrow().has_active_work());

        assert_eq!(
            finish_screen_translation_failure(&coordinator, failed_request),
            Some((anchor, "Recognized screen text".to_owned())),
        );
        assert!(!coordinator.borrow().has_active_work());

        let next_request = RequestId::new();
        recognized_request(&coordinator, next_request);
        assert!(coordinator.borrow().has_active_work());
        assert_eq!(coordinator.borrow().anchor(next_request), Some(anchor));
    }

    #[test]
    fn partial_image_is_busy_without_failure_markers_until_terminal_completion() {
        use rust_protocol::{
            BlockCompletion, TranslatedScreenBlock, TranslationBackendCapabilities,
            TranslationLanguage,
        };
        use slint::Model;

        i_slint_backend_testing::init_no_event_loop();
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let request_id = RequestId::new();
        let anchor = recognized_request(&coordinator, request_id);
        let mut blocks = coordinator
            .borrow()
            .translation_input(request_id)
            .unwrap()
            .1;
        let mut second = blocks[0].clone();
        second.block_id = ScreenBlockId::from("block-2");
        second.source_text = "Still translating".into();
        blocks.push(second);
        coordinator.borrow_mut().insert_document(
            ScreenTranslationDocument::new(
                request_id,
                Arc::new(RgbaFrame::try_new(anchor, 4, vec![0, 0, 0, 255]).unwrap()),
                blocks,
                SourceContext::default(),
                1,
                2,
            )
            .unwrap(),
        );
        coordinator
            .borrow_mut()
            .apply_partial(
                request_id,
                0,
                ScreenBlockId::from("block-1"),
                "Перевод".into(),
            )
            .unwrap();
        let snapshot = coordinator
            .borrow()
            .partial_result_snapshot(request_id)
            .unwrap();
        let window = super::ScreenTranslationWindow::new().unwrap();

        apply_screen_image_snapshot(&window, &snapshot, true, true).unwrap();
        assert!(window.get_screen_translating());
        assert!(!window.get_screen_translation_retry_enabled());
        assert_eq!(window.get_screen_notice_height(), 52.0);
        assert_eq!(window.get_screen_failed_blocks(), 0);
        assert_eq!(window.get_screen_crop_image().size().width, 1);
        assert_eq!(
            window.get_screen_blocks().row_data(0).unwrap().translation,
            "Перевод"
        );
        assert!(!window.get_screen_blocks().row_data(1).unwrap().failed);
        assert!(
            !window
                .get_screen_blocks()
                .row_data(1)
                .unwrap()
                .not_placeable
        );
        assert!(coordinator.borrow().is_active_work_request(request_id));

        coordinator
            .borrow_mut()
            .apply_completed(
                request_id,
                0,
                vec![
                    TranslatedScreenBlock {
                        block_id: ScreenBlockId::from("block-1"),
                        translated_text: "Перевод".into(),
                        completion: BlockCompletion::Completed,
                        failure_message: None,
                    },
                    TranslatedScreenBlock {
                        block_id: ScreenBlockId::from("block-2"),
                        translated_text: "".into(),
                        completion: BlockCompletion::Failed,
                        failure_message: Some("timeout".into()),
                    },
                ],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        coordinator.borrow_mut().complete(request_id);
        let (snapshot, source, target) = coordinator.borrow().result_snapshot(request_id).unwrap();
        assert_eq!(
            (source, target),
            (TranslationLanguage::English, TranslationLanguage::Russian)
        );
        let can_retry = coordinator.borrow().can_retry(request_id);
        assert!(can_retry, "partial terminal failure retains retry metadata");
        apply_screen_image_snapshot(&window, &snapshot, false, can_retry).unwrap();
        assert!(!window.get_screen_translating());
        assert!(window.get_screen_translation_retry_enabled());
        assert_eq!(window.get_screen_failed_blocks(), 1);
        assert!(window.get_screen_blocks().row_data(1).unwrap().failed);
    }

    #[test]
    fn completed_screen_result_can_retire_another_terminal_compact_without_borrow_panic() {
        use rust_protocol::{
            BlockCompletion, TranslatedScreenBlock, TranslationBackendCapabilities,
            TranslationLanguage,
        };

        i_slint_backend_testing::init_no_event_loop();
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let screen_b = RequestId::new();
        recognized_request(&coordinator, screen_b);
        coordinator
            .borrow_mut()
            .apply_completed(
                screen_b,
                0,
                vec![TranslatedScreenBlock {
                    block_id: ScreenBlockId::from("block-1"),
                    translated_text: "Переведённый текст".into(),
                    completion: BlockCompletion::Completed,
                    failure_message: None,
                }],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        let ordinary_a = RequestId::new();
        let current = TranslationPreviewSlot {
            preview: Rc::new(TranslationPreview::new().unwrap()),
            presenter: attached_presenter(51),
            attached: Rc::new(Cell::new(false)),
        };
        let previews = Rc::new(RefCell::new(TranslationPreviewRegistry {
            current: current.clone(),
            current_request: Some(ordinary_a),
            pinned: Vec::new(),
            blur_radius: 0,
        }));

        let (snapshot, _, _) = completed_screen_result_snapshot(&coordinator, screen_b).unwrap();
        let retired = take_terminal_compact_before_screen_result(&previews, screen_b, |request| {
            coordinator.borrow().is_active_work_request(request)
        })
        .unwrap();
        retire_translation_preview(&retired, "screen_result_ready");
        close_screen_translation_request(&coordinator, ordinary_a, |_| {});

        assert_eq!(snapshot.request_id, screen_b);
        assert_eq!(snapshot.blocks[0].translation, "Переведённый текст");
        assert_eq!(previews.borrow().current_request, None);
        assert!(!current.presenter.has_active_presentation());
        assert!(coordinator.borrow().result_snapshot(screen_b).is_ok());
    }

    #[test]
    fn stale_screen_failure_does_not_complete_a_newer_request() {
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let stale_request = RequestId::new();
        recognized_request(&coordinator, stale_request);
        let current_request = RequestId::new();
        let anchor = recognized_request(&coordinator, current_request);

        assert_eq!(
            finish_screen_translation_failure(&coordinator, stale_request),
            None
        );
        assert!(coordinator.borrow().has_active_work());
        assert_eq!(coordinator.borrow().anchor(current_request), Some(anchor));
    }

    #[test]
    fn dismissing_ocr_progress_releases_the_borrow_before_cancellation_cleanup() {
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let request_id = RequestId::new();
        let anchor = PhysicalRect {
            x: 1909,
            y: 611,
            width: 1,
            height: 1,
        };
        coordinator.borrow_mut().begin(request_id);
        assert!(coordinator.borrow_mut().selected(request_id, anchor));
        let mut cancelled = false;

        close_screen_translation_request(&coordinator, request_id, |cancellation| {
            assert_eq!(cancellation, ScreenTranslationCancellation::Ocr(request_id));
            coordinator.borrow_mut().discard_pending(request_id);
            cancelled = true;
        });

        assert!(cancelled);
        assert!(!coordinator.borrow().has_active_work());
        let next_request = RequestId::new();
        recognized_request(&coordinator, next_request);
        assert_eq!(coordinator.borrow().anchor(next_request), Some(anchor));
    }

    fn pending_translation() -> PendingTranslation {
        PendingTranslation {
            origin: TranslationOrigin::Translator,
            source_text: "Retain the failed request".to_owned(),
            source_language: None,
            target_language: None,
            language_policy: TranslationLanguagePolicy::default(),
            started: Instant::now(),
            request_started_unix_ms: 1,
            first_partial_shown: false,
            stable_paragraphs: None,
        }
    }

    #[test]
    fn failed_cloak_releases_registry_and_empty_id_presenter_is_retired_before_reuse() {
        i_slint_backend_testing::init_no_event_loop();
        let stale = RequestId::new();
        let next = RequestId::new();
        let current = TranslationPreviewSlot {
            preview: Rc::new(TranslationPreview::new().unwrap()),
            presenter: presented_presenter(42),
            attached: Rc::new(Cell::new(false)),
        };
        let pinned = TranslationPreviewSlot {
            preview: Rc::new(TranslationPreview::new().unwrap()),
            presenter: attached_presenter(43),
            attached: Rc::new(Cell::new(false)),
        };
        pinned.preview.set_pinned(true);
        let registry = Rc::new(RefCell::new(TranslationPreviewRegistry {
            current: current.clone(),
            current_request: Some(stale),
            pinned: vec![pinned.clone()],
            blur_radius: 0,
        }));

        assert!(cloak_compact_previews_owned(&registry).is_err());
        assert!(registry.try_borrow_mut().is_ok());
        current.preview.set_request_id("".into());
        let retired = take_terminal_compact_before_screen_result(&registry, next, |_| false)
            .expect("terminal unpinned preview must be retired even with empty request id");
        retire_translation_preview(&retired, "test_result_ready");
        assert!(!current.presenter.has_active_presentation());
        assert_eq!(registry.borrow().pinned.len(), 1);
        assert!(pinned.preview.get_pinned());

        registry.borrow_mut().reserve_current(next);
        prepare_without_native(&current.presenter);
        show_translation_preview_at(next, None, "", "retry result", &registry, true);
        assert_eq!(current.preview.get_request_id().as_str(), next.to_string());
        assert!(!current.presenter.has_active_presentation());
        prepare_without_native(&current.presenter);
    }

    #[test]
    fn failed_screen_document_is_released_when_its_unpinned_compact_is_superseded() {
        i_slint_backend_testing::init_no_event_loop();
        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let failed = RequestId::new();
        recognized_request(&coordinator, failed);
        assert!(finish_screen_translation_failure(&coordinator, failed).is_some());
        let current = TranslationPreviewSlot {
            preview: Rc::new(TranslationPreview::new().unwrap()),
            presenter: attached_presenter(45),
            attached: Rc::new(Cell::new(false)),
        };
        let previews = Rc::new(RefCell::new(TranslationPreviewRegistry {
            current: current.clone(),
            current_request: Some(failed),
            pinned: Vec::new(),
            blur_radius: 0,
        }));
        let next = RequestId::new();
        assert!(matches!(
            coordinator.borrow_mut().begin(next),
            ScreenTranslationBegin::Started(_)
        ));
        assert!(coordinator.borrow().source_text(failed).is_some());
        let retired = take_superseded_screen_compact(&previews, &coordinator, Some(failed), next);
        assert!(retired.is_some());
        assert!(coordinator.borrow().source_text(failed).is_none());
        assert_eq!(previews.borrow().current_request, None);
        assert!(coordinator.borrow().is_selecting(next));

        coordinator.borrow_mut().close(next);
        let pinned = RequestId::new();
        recognized_request(&coordinator, pinned);
        assert!(finish_screen_translation_failure(&coordinator, pinned).is_some());
        previews.borrow_mut().current_request = Some(pinned);
        current.preview.set_pinned(true);
        let later = RequestId::new();
        assert!(matches!(
            coordinator.borrow_mut().begin(later),
            ScreenTranslationBegin::Started(_)
        ));
        assert!(
            take_superseded_screen_compact(&previews, &coordinator, Some(pinned), later).is_none()
        );
        assert!(coordinator.borrow().source_text(pinned).is_some());
    }

    #[test]
    fn glossary_term_save_never_replaces_the_whole_screen_block() {
        use rust_protocol::{
            BlockCompletion, TranslatedScreenBlock, TranslationBackendCapabilities,
            TranslationLanguage,
        };

        let coordinator = Rc::new(RefCell::new(ScreenTranslationCoordinator::default()));
        let request_id = RequestId::new();
        recognized_request(&coordinator, request_id);
        coordinator
            .borrow_mut()
            .apply_completed(
                request_id,
                0,
                vec![TranslatedScreenBlock {
                    block_id: ScreenBlockId::from("block-1"),
                    translated_text: "Бросает молнию и наносит урон".into(),
                    completion: BlockCompletion::Completed,
                    failure_message: None,
                }],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        let before = coordinator.borrow().result_snapshot(request_id).unwrap().0;
        let id = ScreenBlockId::from("block-1");
        assert_eq!(
            apply_screen_translation_edit_for_scope(
                &coordinator,
                request_id,
                &id,
                before.revision,
                EditorCommitScope::for_source("Recognized screen text", "lightning"),
                "молния".into(),
            )
            .unwrap(),
            None,
        );
        let after_term = coordinator.borrow().result_snapshot(request_id).unwrap().0;
        assert_eq!(after_term.revision, before.revision);
        assert_eq!(
            after_term.blocks[0].translation,
            before.blocks[0].translation
        );
        assert_eq!(
            EditorCommitScope::for_source("Recognized screen text", " Recognized screen text "),
            EditorCommitScope::GlossaryTerm,
        );
        assert_eq!(
            apply_screen_translation_edit_for_scope(
                &coordinator,
                request_id,
                &id,
                before.revision,
                EditorCommitScope::for_source("Recognized screen text", " Recognized screen text "),
                "один термин".into(),
            )
            .unwrap(),
            None,
        );
        assert_eq!(
            coordinator
                .borrow()
                .result_snapshot(request_id)
                .unwrap()
                .0
                .blocks[0]
                .translation,
            before.blocks[0].translation,
        );

        assert!(
            apply_screen_translation_edit_for_scope(
                &coordinator,
                request_id,
                &id,
                before.revision,
                EditorCommitScope::for_source("Recognized screen text", "Recognized screen text"),
                "Исправленная строка".into(),
            )
            .unwrap()
            .is_some()
        );
        assert_eq!(
            coordinator
                .borrow()
                .result_snapshot(request_id)
                .unwrap()
                .0
                .blocks[0]
                .translation,
            "Исправленная строка"
        );
    }

    #[test]
    fn ordinary_failure_keeps_pending_origin_when_no_install_is_offered() {
        let mut pending = Some(pending_translation());
        assert!(take_pending_translation_install_offer(None, &mut pending).is_none());
        let retained = pending.expect("ordinary failure must retain its delivery origin");
        assert!(matches!(retained.origin, TranslationOrigin::Translator));
        assert_eq!(retained.source_text, "Retain the failed request");
    }

    #[test]
    fn offered_install_takes_the_pending_request_once() {
        let mut pending = Some(pending_translation());
        let offer = CompactInstallOffer {
            catalog_id: "compact-test".to_owned(),
            display_name: "Compact test".to_owned(),
            size_bytes: 7,
        };
        let (actual, transferred) =
            take_pending_translation_install_offer(Some(offer.clone()), &mut pending).unwrap();
        assert_eq!(actual, offer);
        assert!(pending.is_none());
        assert_eq!(transferred.source_text, "Retain the failed request");
        assert!(take_pending_translation_install_offer(Some(offer), &mut pending).is_none());
    }
}

#[cfg(test)]
mod product_icon_tests {
    #[test]
    fn tray_waveform_is_the_accent_without_a_graphite_stripe() {
        let source = include_str!("../assets/tray-icon.svg");
        assert_eq!(source.matches("fill=\"#ef5048\"").count(), 5);
        assert!(
            !source.contains("stroke=\"#111317\"") && !source.contains("fill=\"#111317\""),
            "the tiny tray mark must not paint graphite over the red waveform",
        );
    }
}

#[cfg(test)]
mod screen_translation_privacy_tests {
    use rust_protocol::{RequestId, TranslationLanguage, TranslationResult};

    use super::record_translation_history;

    fn result(source: &str) -> TranslationResult {
        TranslationResult {
            request_id: RequestId::new(),
            source_text: source.to_owned(),
            translated_text: "translated".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            tier: None,
            provider: None,
            attempts: Vec::new(),
            processing_duration_ms: 1,
            first_token_duration_ms: 1,
            backend_request_count: 1,
            tokenize_duration_ms: 0,
            chunk_count: 1,
            protected_values_changed: false,
        }
    }

    #[test]
    fn recognized_screen_text_never_enters_history_when_history_is_disabled() {
        let canary = "SECRET_SCREEN_OCR_CANARY";
        let mut history = Vec::new();
        record_translation_history(&mut history, false, &result(canary), 7);
        assert!(history.is_empty());

        record_translation_history(&mut history, true, &result(canary), 8);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].result.source_text, canary);
    }

    #[test]
    fn screen_translation_route_has_no_implicit_clipboard_or_temp_file_sink() {
        let source = include_str!("main.rs");
        let dispatch = source
            .split_once("fn dispatch_screen_capture_to_ocr(")
            .expect("screen capture dispatcher exists")
            .1
            .split_once("fn handle_ocr_event(")
            .expect("OCR event handler follows dispatcher")
            .0;
        let translation = source
            .split_once("fn begin_screen_translation(")
            .expect("screen translation dispatcher exists")
            .1
            .split_once("fn handle_input_event(")
            .expect("input handler follows screen translation dispatcher")
            .0;

        assert!(dispatch.contains("encode(&frame.pixels)"));
        assert_eq!(dispatch.matches("send_ocr(&prepared.command)").count(), 1);
        assert_eq!(translation.matches("translation.send(&command)").count(), 1);
        for forbidden in ["CopyText", "std::fs", "tempfile", "File::create"] {
            assert!(
                !dispatch.contains(forbidden),
                "unexpected sink: {forbidden}"
            );
            assert!(
                !translation.contains(forbidden),
                "unexpected sink: {forbidden}"
            );
        }
    }
}

#[cfg(all(test, feature = "screen-translation"))]
mod screen_translation_outcome_tests {
    use rust_platform_windows::screen_capture::CaptureError;

    use super::{ScreenTranslationOutcome, screen_outcome_for_capture_error};

    #[test]
    fn protected_and_empty_capture_failures_keep_their_user_facing_meaning() {
        assert_eq!(
            screen_outcome_for_capture_error(&CaptureError::ProtectedContent),
            ScreenTranslationOutcome::ProtectedContent
        );
        assert_eq!(
            screen_outcome_for_capture_error(&CaptureError::SecureDesktop),
            ScreenTranslationOutcome::ProtectedContent
        );
        assert_eq!(
            screen_outcome_for_capture_error(&CaptureError::EmptyRegion),
            ScreenTranslationOutcome::EmptySelection
        );
        assert_eq!(
            screen_outcome_for_capture_error(&CaptureError::NoOutputs),
            ScreenTranslationOutcome::CaptureUnavailable
        );
    }
}

#[cfg(test)]
mod worker_exit_isolation_tests {
    use std::time::Instant;

    use rust_protocol::{
        PhysicalRect, RequestId, SpeechOutputMode, TranslationLanguage, TranslationLanguagePolicy,
    };

    use super::{
        AppState, PendingTranslation, SavedSettings, SpeechBackend, TranslationOrigin, WorkerKind,
        supersede_translator_requests, worker_exit_affects_dictation,
    };

    fn pending(origin: TranslationOrigin) -> PendingTranslation {
        PendingTranslation {
            origin,
            source_text: "Hallo".to_owned(),
            source_language: None,
            target_language: None,
            language_policy: TranslationLanguagePolicy {
                primary: TranslationLanguage::English,
                secondary: TranslationLanguage::Russian,
            },
            started: Instant::now(),
            request_started_unix_ms: 0,
            first_partial_shown: false,
            stable_paragraphs: None,
        }
    }

    #[test]
    fn only_the_input_host_and_the_speech_worker_end_a_dictation() {
        assert!(worker_exit_affects_dictation(WorkerKind::Input));
        assert!(worker_exit_affects_dictation(WorkerKind::Speech));
        assert!(!worker_exit_affects_dictation(WorkerKind::Translation));
        assert!(!worker_exit_affects_dictation(WorkerKind::Companion));
        #[cfg(feature = "screen-translation")]
        assert!(!worker_exit_affects_dictation(WorkerKind::Ocr));
    }

    #[test]
    fn a_new_translator_request_supersedes_only_older_translator_requests() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        let older = RequestId::new();
        let screen = RequestId::new();
        state
            .pending_translations
            .insert(older, pending(TranslationOrigin::Translator));
        state.pending_translations.insert(
            screen,
            pending(TranslationOrigin::ScreenRegion {
                anchor: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
            }),
        );

        let superseded = supersede_translator_requests(&mut state);

        assert_eq!(superseded, vec![older]);
        // The replaced request's late cancel finds nothing to reset.
        assert!(!state.pending_translations.contains_key(&older));
        assert!(state.pending_translations.contains_key(&screen));
    }
}

#[cfg(test)]
mod history_clear_tests {
    use super::{HistoryClear, HistoryOwner};

    #[test]
    fn cleared_is_reported_only_after_both_workers_answer() {
        let mut progress = HistoryClear::default();
        assert_eq!(progress.outcome(), None);

        progress.answer(HistoryOwner::Speech, None);
        assert_eq!(progress.outcome(), None, "translation has not answered");

        progress.answer(HistoryOwner::Translation, None);
        assert_eq!(progress.outcome(), Some(Ok(())));
    }

    #[test]
    fn one_failed_worker_makes_the_whole_clear_a_failure() {
        let mut progress = HistoryClear::default();
        progress.answer(HistoryOwner::Translation, None);
        progress.answer(
            HistoryOwner::Speech,
            Some("history.json is in use by another process".to_owned()),
        );

        assert_eq!(
            progress.outcome(),
            Some(Err("history.json is in use by another process".to_owned()))
        );
    }

    #[test]
    fn a_late_second_answer_does_not_overwrite_the_first() {
        let mut progress = HistoryClear::default();
        progress.answer(HistoryOwner::Speech, Some("in use".to_owned()));
        progress.answer(HistoryOwner::Speech, None);
        assert!(!progress.is_waiting_for(HistoryOwner::Speech));
        assert!(progress.is_waiting_for(HistoryOwner::Translation));
        progress.answer(HistoryOwner::Translation, None);

        assert_eq!(progress.outcome(), Some(Err("in use".to_owned())));
    }
}

#[cfg(test)]
mod local_translation_model_state_tests {
    use rust_protocol::SpeechOutputMode;

    use super::{AppState, SavedSettings, SpeechBackend, apply_local_translation_model_state};

    fn state(keep_warm: bool) -> AppState {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        state.translation_keep_warm = keep_warm;
        state.translation_worker_ready = true;
        state
    }

    #[test]
    fn a_per_request_release_shows_the_model_as_unloaded_but_ready() {
        let mut app = state(false);
        apply_local_translation_model_state(&mut app, Some("qwen35-9b-q4".to_owned()), true);
        assert_eq!(
            app.translation_loaded_model_id.as_deref(),
            Some("qwen35-9b-q4")
        );

        apply_local_translation_model_state(&mut app, Some("qwen35-9b-q4".to_owned()), false);

        assert_eq!(
            app.translation_loaded_model_id, None,
            "Resources must not say loaded"
        );
        assert_eq!(app.translation_ready, app.local_translation_enabled());
    }

    #[test]
    fn with_keep_warm_on_an_unloaded_model_is_not_ready() {
        let mut app = state(true);
        apply_local_translation_model_state(&mut app, Some("qwen35-9b-q4".to_owned()), false);

        assert_eq!(app.translation_loaded_model_id, None);
        assert!(!app.translation_ready);
    }
}
