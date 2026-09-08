#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    error::Error,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use preview_modes::{
    indicator_presentation_probe_cycles, indicator_preview_slate_surface, indicator_preview_style,
    run_action_palette_preview, run_indicator_presentation_probe, run_indicator_preview,
    run_layout_preview, run_translation_presentation_probe, run_translation_preview,
    translation_presentation_probe_cycles, translation_preview_mode,
};
use providers_view::sync_providers;
use rust_app_core::{
    BeginDisposition, EndDisposition, LayoutCandidate, SessionPhase, SessionRegistry,
    dictation::{CompletionDelivery, HistoryUpdate, plan_completion},
};
use rust_protocol::{
    CompanionRendererCommand, CompanionRendererEvent, HistoryEntry, InputHostCommand,
    InputHostEvent, ModelSummary, PROTOCOL_VERSION, RequestId, SessionId, SpeechOutputMode,
    SpeechWorkerCommand, SpeechWorkerEvent, TargetSnapshot, TextActionIntent, Transcript,
    TranscriptionProviderEntry, TranscriptionProviderStatus, TranslationHistoryEntry,
    TranslationModelSummary, TranslationProviderEntry, TranslationProviderStatus,
    TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
use serde::{Deserialize, Serialize};
use settings_document::{SavedSettings, data_root, load_settings, save_settings};
use settings_view::{
    precreate_action_windows, prepend_history_row, set_indicator_status, set_runtime_status,
    set_status_problem, show_layout_chooser, sync_action_settings, sync_history, sync_models,
    sync_output_mode, sync_translation_models, wire_action_palette, wire_chooser, wire_settings,
    wire_translator, wire_tray,
};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, Timer,
    TimerMode, VecModel,
};
use translation_view::{
    TranslationPreviewRegistry, TranslationPreviewSlot, adaptive_text_panel_height,
    apply_translation_preview_layout, apply_translation_preview_pin_state,
    configure_slint_translation_resize, create_translation_preview_slot,
    precreate_translation_preview, refresh_cloaked_translation_backdrop,
    retire_translation_preview, set_translation_status, translation_pairs,
    wire_translation_preview,
};
use visualizer::{
    CircularSpectrumPalette, INDICATOR_FRAME_INTERVAL, IndicatorAnimation, RecordingSurface,
    VisualizerStyle, WritingSlateSurface, new_recording_indicators,
};
use worker_supervisor::{
    InitialWorkers, SessionEventRouter, StartupSessionGate, WorkerLifecycle, WorkerRecovery,
    Workers, spawn_initial_workers,
};

mod flow_sphere;
mod overlay_presenter;
mod preview_modes;
mod providers_view;
mod settings_document;
mod settings_preview;
mod settings_projection;
mod settings_view;
mod speech_backend;
mod translation_view;
mod visualizer;
mod worker_supervisor;

use overlay_presenter::{OverlayContract, OverlayPresenter};
use speech_backend::{SpeechBackend, SpeechBackendPreference};

slint::include_modules!();

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const DEFAULT_TRANSLATION_BLUR_RADIUS: u32 = 3;
const MAX_TRANSLATION_BLUR_RADIUS: u32 = 8;
/// Visible and in-memory history depth. The speech worker keeps the same
/// number of entries on disk.
const HISTORY_RETAINED_ROWS: usize = 250;

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
    Input(InputHostEvent),
    Speech {
        instance_id: u64,
        event: SpeechWorkerEvent,
    },
    /// A coalesced microphone frame is waiting in `LATEST_AUDIO_LEVEL`.
    SpeechAudioLevel {
        instance_id: u64,
    },
    Translation(TranslationWorkerEvent),
    Companion(CompanionRendererEvent),
    DismissTranslationPreview(u64),
    TranslationPreviewPinChanged {
        window_handle: u64,
        pinned: bool,
    },
    ScrollTranslationPreview {
        window_handle: u64,
        delta: i32,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WorkerKind {
    Input,
    Speech,
    Translation,
    Companion,
}

type UiEventHandler = Box<dyn FnMut(RuntimeEvent)>;

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
    translator: slint::Weak<TranslatorWindow>,
    state: Rc<RefCell<AppState>>,
    workers: Rc<Workers>,
    indicator_animation: Rc<RefCell<IndicatorAnimation>>,
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
    fn for_style(&self, style: VisualizerStyle) -> &RecordingOverlay {
        match style.recording_surface() {
            RecordingSurface::Panel => &self.panel,
            RecordingSurface::Transparent => &self.transparent,
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
            RecordingSurface::Panel => &self.transparent,
            RecordingSurface::Transparent => &self.panel,
        };
        inactive.indicator.set_prepared(false);
        inactive
            .presenter
            .retire(inactive.indicator.window(), "visualizer_surface_switch");
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
enum TranslationOrigin {
    Selection {
        target: TargetSnapshot,
        delivery: TranslationDelivery,
    },
    Translator,
}

struct PendingTranslation {
    origin: TranslationOrigin,
    source_text: String,
    started: Instant,
    request_started_unix_ms: u64,
    first_partial_shown: bool,
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
    speech_backend_preference: SpeechBackendPreference,
    speech_backend_active: Option<SpeechBackend>,
    output_mode: SpeechOutputMode,
    fast_release_previews: bool,
    visualizer_style: VisualizerStyle,
    circular_spectrum_palette: CircularSpectrumPalette,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    writing_slate_surface: WritingSlateSurface,
    companion_home_position: Option<(i32, i32)>,
    input_ready: bool,
    speech_ready: bool,
    indicator_session: Option<SessionId>,
    active_visualizer_style: Option<VisualizerStyle>,
    release_started: HashMap<SessionId, Instant>,
    pending_insert_last: Option<PendingInsertLast>,
    pending_selection: Option<PendingSelection>,
    translation_models: Vec<TranslationModelSummary>,
    selected_translation_tier: Option<TranslationTier>,
    translation_ready: bool,
    translation_enabled: bool,
    layout_enabled: bool,
    translation_keep_warm: bool,
    translation_blur_radius: u32,
    text_action_mode: TextActionMode,
    translation_delivery: TranslationDelivery,
    history_enabled: bool,
    pending_text_action: Option<PendingTextAction>,
    pending_selection_captures: HashMap<RequestId, u64>,
    selection_release_gates: HashMap<RequestId, SelectionReleaseGate>,
    pending_translations: HashMap<RequestId, PendingTranslation>,
    queued_selection_translation: Option<SelectionTranslationRequest>,
    translation_history: Vec<TranslationHistoryEntry>,
    /// Installs in flight, by catalog id. The workers report progress for
    /// exactly as long as a download runs, so an entry here is the same fact
    /// as "this row is downloading" and the two cannot disagree.
    downloads: HashMap<String, settings_projection::Download>,
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
            speech_backend_preference: saved_settings.speech_backend_preference,
            speech_backend_active: Some(speech_backend_active),
            output_mode,
            fast_release_previews: saved_settings.fast_release_previews,
            visualizer_style: saved_settings.visualizer_style,
            circular_spectrum_palette: saved_settings.circular_spectrum_palette,
            flow_sphere_palette: saved_settings.flow_sphere_palette,
            writing_slate_surface: saved_settings.writing_slate_surface,
            companion_home_position: saved_settings.companion_home_position,
            input_ready: false,
            speech_ready: false,
            indicator_session: None,
            active_visualizer_style: None,
            release_started: HashMap::new(),
            pending_insert_last: None,
            pending_selection: None,
            translation_models: Vec::new(),
            selected_translation_tier: saved_settings.selected_translation_tier,
            translation_ready: false,
            translation_enabled: saved_settings.translation_enabled,
            layout_enabled: saved_settings.layout_enabled,
            translation_keep_warm: saved_settings.translation_keep_warm,
            translation_blur_radius: saved_settings.translation_blur_radius,
            text_action_mode: saved_settings.text_action_mode,
            translation_delivery: saved_settings.translation_delivery,
            pending_text_action: None,
            pending_selection_captures: HashMap::new(),
            selection_release_gates: HashMap::new(),
            pending_translations: HashMap::new(),
            queued_selection_translation: None,
            translation_history: Vec::new(),
            downloads: HashMap::new(),
            transcription_providers: Vec::new(),
            transcription_chain: saved_settings.transcription_providers.clone(),
            translation_providers: Vec::new(),
            translation_chain: saved_settings.translation_providers.clone(),
            history_enabled: saved_settings.history_enabled,
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
    rust_platform_windows::configure_process_error_mode();
    rust_platform_windows::configure_safe_dll_search()?;
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
    if env::args().any(|argument| argument.starts_with("--settings-preview")) {
        return settings_preview::run_settings_preview();
    }

    if let Err(error) = rust_app_core::product::migrate_legacy_data_root() {
        eprintln!("SayFlex data migration is deferred: {error}");
    }
    let _ = rust_app_core::diagnostics::initialize(&data_root().join("logs"), "desktop");
    rust_app_core::lifecycle::install_panic_hook();
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.Desktop")?;
    let mut run_session =
        rust_app_core::lifecycle::RunSession::begin(&data_root(), env!("CARGO_PKG_VERSION"))?;
    let settings = SettingsWindow::new()?;
    settings.set_app_version(env!("CARGO_PKG_VERSION").into());
    settings.set_today(rust_platform_windows::local_date_line().into());
    let saved_settings = load_settings();
    let (panel_indicator, transparent_indicator, indicator_animation) = new_recording_indicators()?;
    let panel_indicator_presenter = OverlayPresenter::install(
        panel_indicator.window(),
        "recording_panel_overlay",
        OverlayContract::ClickThrough,
        None,
    )
    .map_err(|error| format!("recording overlay initialization failed: {error}"))?;
    let transparent_indicator_presenter = OverlayPresenter::install(
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
    recording_overlays.set_style(saved_settings.visualizer_style);
    recording_overlays.set_circular_palette(saved_settings.circular_spectrum_palette);
    recording_overlays.set_writing_slate_surface(saved_settings.writing_slate_surface);
    let chooser = SelectionChooser::new()?;
    let action_palette = ActionPalette::new()?;
    let translation_preview = Rc::new(TranslationPreview::new()?);
    translation_preview.set_blur_radius(saved_settings.translation_blur_radius as i32);
    let chooser_presenter = OverlayPresenter::install(
        chooser.window(),
        "selection_chooser_overlay",
        OverlayContract::Interactive,
        None,
    )
    .map_err(|error| format!("selection chooser initialization failed: {error}"))?;
    let action_palette_presenter = OverlayPresenter::install(
        action_palette.window(),
        "action_palette_overlay",
        OverlayContract::Interactive,
        None,
    )
    .map_err(|error| format!("action palette initialization failed: {error}"))?;
    let translation_preview_presenter = OverlayPresenter::install(
        translation_preview.window(),
        "translation_preview_overlay",
        OverlayContract::Interactive,
        Some(Arc::new(rust_platform_windows::activate_popup_dismiss)),
    )
    .map_err(|error| format!("translation preview initialization failed: {error}"))?;
    let translation_preview_slot = TranslationPreviewSlot {
        preview: Rc::clone(&translation_preview),
        presenter: translation_preview_presenter.clone(),
        attached: Rc::new(Cell::new(false)),
    };
    let translation_previews = Rc::new(RefCell::new(TranslationPreviewRegistry {
        current: translation_preview_slot.clone(),
        current_request: None,
        pinned: Vec::new(),
        blur_radius: saved_settings.translation_blur_radius,
    }));
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
        input_child: RefCell::new(input_child),
        speech_child: RefCell::new(speech_child),
        speech_backends: RefCell::new(speech_backends),
        translation_child: RefCell::new(translation_child),
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
    let output_mode = if selected_model_id == DEFAULT_MODEL
        && saved_settings.output_mode == SpeechOutputMode::TranslateToEnglish
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
        history_enabled: saved_settings.history_enabled,
        fast_release_previews: saved_settings.fast_release_previews,
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
            selected_tier: saved_settings.selected_translation_tier,
            keep_warm: saved_settings.translation_keep_warm,
            history_enabled: saved_settings.history_enabled,
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

    wire_settings(
        &settings,
        &translator,
        &translation_previews,
        &state,
        &workers,
        &recording_overlays,
        &indicator_animation,
    );
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
    wire_translation_preview(&translation_preview, &translation_preview_presenter);
    wire_translator(&translator, &state, &workers);
    install_runtime_handler(RuntimeUi {
        settings: settings.as_weak(),
        recording_overlays: recording_overlays.clone(),
        chooser: chooser.as_weak(),
        chooser_presenter: chooser_presenter.clone(),
        action_palette: action_palette.as_weak(),
        action_palette_presenter: action_palette_presenter.clone(),
        translation_previews: Rc::clone(&translation_previews),
        translator: translator.as_weak(),
        state: Rc::clone(&state),
        workers: Rc::clone(&workers),
        indicator_animation: Rc::clone(&indicator_animation),
    });
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
    let animation_timer = start_indicator_animation(
        vec![
            recording_overlays.panel.indicator.as_weak(),
            recording_overlays.transparent.indicator.as_weak(),
        ],
        &indicator_animation,
    );

    if !env::args().any(|argument| argument == "--background") {
        settings.show()?;
        configure_standard_app_window(&settings, "Settings");
    }
    tray.show()?;
    slint::run_event_loop()?;
    animation_timer.stop();
    UI_EVENT_HANDLER.with(|handler| {
        handler.borrow_mut().take();
    });
    workers.shutdown();
    run_session.finish("event_loop_returned");
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

fn start_indicator_animation(
    indicators: Vec<slint::Weak<RecordingIndicator>>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) -> Timer {
    let timer = Timer::default();
    let indicator_animation = Rc::clone(indicator_animation);
    timer.start(TimerMode::Repeated, INDICATOR_FRAME_INTERVAL, move || {
        let live_indicators = indicators
            .iter()
            .filter_map(slint::Weak::upgrade)
            .collect::<Vec<_>>();
        let references = live_indicators.iter().collect::<Vec<_>>();
        indicator_animation.borrow_mut().tick(&references);
    });
    timer
}

fn handle_runtime_event(event: RuntimeEvent, runtime: &RuntimeUi) {
    match event {
        RuntimeEvent::Input(event) => {
            handle_input_event(event, runtime);
        }
        RuntimeEvent::Speech { instance_id, event } => {
            if !runtime
                .workers
                .is_current_instance(WorkerKind::Speech, instance_id)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "worker kind=Speech instance={instance_id} event=stale_ignored"
                ));
                return;
            }
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
        RuntimeEvent::Translation(event) => {
            handle_translation_event(
                event,
                &runtime.settings,
                &runtime.translation_previews,
                &runtime.translator,
                &runtime.state,
                &runtime.workers,
            );
        }
        RuntimeEvent::Companion(event) => match event {
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
                        "Companion Orb protocol mismatch. Reinstall the complete application.",
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
                set_indicator_status(&runtime.settings, &format!("Companion Orb: {message}"));
            }
        },
        RuntimeEvent::DismissTranslationPreview(window_handle) => {
            let slot = runtime
                .translation_previews
                .borrow()
                .slot_by_handle(window_handle);
            if let Some(slot) = slot {
                retire_translation_preview(&slot, "click_away");
                let is_current = {
                    let previews = runtime.translation_previews.borrow();
                    slint_window_handle(previews.current.preview.window()) == Some(window_handle)
                };
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
            rust_platform_windows::set_popup_pinned(window_handle, pinned);
            let slot = runtime
                .translation_previews
                .borrow()
                .slot_by_handle(window_handle);
            if let Some(slot) = slot {
                let is_current = {
                    let previews = runtime.translation_previews.borrow();
                    slint_window_handle(previews.current.preview.window()) == Some(window_handle)
                };
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
            runtime.indicator_animation.borrow_mut().stop();
            runtime.recording_overlays.retire_all("worker_error");
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Worker error".into());
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
    let message = format!(
        "{kind:?} worker exited with {exit_code} after {} ms",
        exit.uptime.as_millis()
    );
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
                    "Companion Orb restarted. Dictation continued normally.",
                );
            }
            Ok(WorkerRecovery::Deferred | WorkerRecovery::Stopping | WorkerRecovery::Stale) => {}
            Err(error) => set_status_problem(
                &runtime.settings,
                &format!("Companion Orb is unavailable: {error}"),
            ),
        }
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

    let active_session = {
        let mut state = runtime.state.borrow_mut();
        match kind {
            WorkerKind::Input => state.input_ready = false,
            WorkerKind::Speech => state.speech_ready = false,
            WorkerKind::Translation => {
                state.translation_ready = false;
                state.pending_translations.clear();
            }
            WorkerKind::Companion => {}
        }
        let active = state.indicator_session.take();
        state.active_visualizer_style = None;
        if let Some(session_id) = active {
            let _ = state.registry.fail(session_id);
            state.targets.remove(&session_id);
            state.release_started.remove(&session_id);
        }
        active
    };
    if kind != WorkerKind::Speech
        && let Some(session_id) = active_session
    {
        let _ = runtime
            .workers
            .speech
            .send(&SpeechWorkerCommand::Cancel { session_id });
    }
    runtime.indicator_animation.borrow_mut().stop();
    runtime.recording_overlays.retire_all("worker_exit");

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
                            history_enabled: state.history_enabled,
                            fast_release_previews: state.fast_release_previews,
                        })
                }
                WorkerKind::Translation => {
                    let state = runtime.state.borrow();
                    runtime
                        .workers
                        .translation
                        .send(&TranslationWorkerCommand::Initialize {
                            selected_tier: state.selected_translation_tier,
                            keep_warm: state.translation_keep_warm,
                            history_enabled: state.history_enabled,
                        })
                }
                WorkerKind::Companion => Ok(()),
            };
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Recovering worker".into());
            }
            // A restart that worked is progress; one that did not is trouble.
            // Reading that out of the sentence is what the old classifier got
            // wrong in both directions.
            match initialization {
                Ok(()) => set_indicator_status(
                    &runtime.settings,
                    &format!("{message}; worker restarted automatically"),
                ),
                Err(error) => set_status_problem(
                    &runtime.settings,
                    &format!("{message}; restart initialization failed: {error}"),
                ),
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
            if kind == WorkerKind::Speech {
                runtime.state.borrow_mut().speech_backend_active = None;
                sync_action_settings(&runtime.settings, &runtime.state);
            }
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Worker unavailable".into());
            }
            set_status_problem(&runtime.settings, &format!("{message}; {error}"));
        }
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
                set_runtime_status(settings, "Input ready");
            } else {
                state.borrow_mut().input_ready = false;
                set_runtime_status(settings, "Worker mismatch");
                set_indicator_status(
                    settings,
                    "Input worker protocol mismatch. Reinstall the complete application.",
                );
            }
        }
        InputHostEvent::DictationBegin {
            session_id,
            started_unix_ms,
            target,
        } => {
            if !state.borrow().input_ready {
                set_status_problem(settings, "Input worker is not ready.");
                return;
            }
            if !state.borrow().speech_ready {
                set_indicator_status(settings, "Speech model is still being prepared.");
                return;
            }
            if state.borrow().indicator_session.is_some() {
                set_indicator_status(settings, "Finishing the previous dictation…");
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
                let _ = state.registry.fail(session_id);
                state.targets.remove(&session_id);
                state.indicator_session = None;
                state.active_visualizer_style = None;
                set_status_problem(settings, &format!("Could not start recording: {error}"));
                return;
            }
            let active_style = state.borrow().active_visualizer_style.unwrap_or_default();
            indicator_animation.borrow_mut().set_style(active_style);
            indicator_animation.borrow_mut().begin_listening();
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
                        &format!("Listening; Companion Orb unavailable: {error}"),
                    );
                }
            } else {
                recording_overlays.retire_inactive_surface(active_style);
                let overlay = recording_overlays.for_style(active_style);
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
            set_indicator_status(settings, "Listening…");
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
                    set_status_problem(settings, &format!("Could not finish recording: {error}"));
                    return;
                }
                indicator_animation.borrow_mut().begin_processing();
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
                {
                    let indicator = &recording_overlays.for_style(active_style).indicator;
                    indicator.set_processing(true);
                    indicator.set_signal_opacity(0.72);
                    indicator.set_disc_opacity(0.82);
                }
                set_indicator_status(settings, "Processing locally…");
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
                        &format!(
                            "Dictation {} is still processing; it will be inserted here when ready.",
                            short_id(session_id)
                        ),
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
                    set_indicator_status(settings, "Loading the latest transcript…");
                }
                InsertLastResolution::Unavailable(message) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "insert_last decision=unavailable target_pid={}",
                        target.process_id
                    ));
                    set_indicator_status(settings, message);
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
                    retire_translation_preview(&slot, "new_translation");
                    translation_previews.borrow_mut().current_request = None;
                }
            }
            set_indicator_status(settings, "Reading selected text…");
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
                set_indicator_status(settings, &format!("Translation ready, but {message}"));
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
            set_indicator_status(settings, &format!("Selection: {message}"));
        }
        InputHostEvent::InsertionCompleted {
            session_id,
            method,
            delivery,
            dispatch_duration_ms,
        } => {
            if state
                .borrow()
                .active_visualizer_style
                .is_some_and(VisualizerStyle::uses_external_renderer)
            {
                let _ = workers.send_companion(&CompanionRendererCommand::Complete { session_id });
            }
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
            let status = match delivery {
                rust_protocol::InsertionDelivery::Committed => {
                    format!(
                        "Inserted {} through the target adapter in {} ms",
                        short_id(session_id),
                        elapsed
                    )
                }
                rust_protocol::InsertionDelivery::PayloadRead => {
                    format!(
                        "Inserted {} after the target read the private transaction in {} ms",
                        short_id(session_id),
                        elapsed
                    )
                }
                rust_protocol::InsertionDelivery::Verified => {
                    format!(
                        "Verified insertion {} in {} ms",
                        short_id(session_id),
                        elapsed
                    )
                }
                rust_protocol::InsertionDelivery::Dispatched => {
                    format!(
                        "Paste dispatched {} without target verification in {} ms",
                        short_id(session_id),
                        elapsed
                    )
                }
            };
            set_indicator_status(settings, &status);
        }
        InputHostEvent::Recovered { session_id } => {
            set_indicator_status(
                settings,
                &format!(
                    "Latest transcript {} copied. Shift+Insert is ready.",
                    short_id(session_id)
                ),
            );
        }
        InputHostEvent::SelectionReplaced { request_id } => {
            let _ = request_id;
            set_indicator_status(settings, "Replacement was sent to the target.");
        }
        InputHostEvent::TextCopied { .. } => {
            set_indicator_status(settings, "Translation copied to the clipboard.");
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
            set_indicator_status(settings, &format!("{code}: {message}"));
        }
    }
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
                set_indicator_status(settings, "Keyboard-layout correction is disabled.");
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
                set_indicator_status(settings, "All selected-text actions are disabled.");
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
    } = request;
    let (enabled, selected_tier, ready) = {
        let state = state.borrow();
        (
            state.translation_enabled,
            state.selected_translation_tier,
            state.translation_ready,
        )
    };
    if !enabled {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(settings, "Translation is disabled in Actions.");
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=rejected \
reason=translation_disabled"
        ));
        return;
    }
    if selected_tier.is_none() {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(
            settings,
            "Choose and download a translation model in Settings first.",
        );
        rust_app_core::diagnostics::log_line(&format!(
            "translation_pipeline request={request_id} stage=rejected reason=no_selected_model"
        ));
        return;
    }
    if !ready {
        state.borrow_mut().queued_selection_translation = Some(SelectionTranslationRequest {
            request_id,
            target,
            text,
            delivery,
            request_started_unix_ms,
        });
        set_indicator_status(settings, "The translation model is still loading.");
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

    state.borrow_mut().pending_translations.insert(
        request_id,
        PendingTranslation {
            origin: TranslationOrigin::Selection { target, delivery },
            source_text: text.clone(),
            started: Instant::now(),
            request_started_unix_ms,
            first_partial_shown: false,
        },
    );
    if let Err(error) = workers
        .translation
        .send(&TranslationWorkerCommand::Translate {
            request_id,
            text,
            source_language: None,
            target_language: None,
        })
    {
        let mut app = state.borrow_mut();
        app.pending_translations.remove(&request_id);
        app.selection_release_gates.remove(&request_id);
        translation_previews
            .borrow_mut()
            .clear_current_request(request_id);
        set_status_problem(settings, &format!("Could not start translation: {error}"));
    } else {
        set_indicator_status(settings, "Translating locally…");
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
    if first_show {
        preview.set_request_id(request_id_text.into());
        preview.set_source_text(source_text.into());
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
    apply_translation_preview_layout(preview, translated_text);
    preview.set_translated_text(translated_text.into());
    preview.set_dual_pairs(ModelRc::new(VecModel::from(translation_pairs(
        source_text,
        translated_text,
    ))));
    if first_show {
        preview.set_prepared(false);
        preview.invoke_reset_scroll();
        if slot.attached.get()
            && let Err(error) =
                present_translation_preview(preview, translation_preview_presenter, Some(target))
        {
            preview.set_request_id("".into());
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay transition=failed stage=present error={error}"
            ));
        }
    } else if final_update
        && let Some(handle) = slint_window_handle(preview.window())
        && let Ok(ticket) = translation_preview_presenter.begin_refresh()
    {
        refresh_cloaked_translation_backdrop(preview, handle);
        if let Err(error) = translation_preview_presenter.await_frame(preview.window(), ticket) {
            preview.set_prepared(false);
            translation_preview_presenter.abort_prepare(
                preview.window(),
                ticket,
                "refresh_frame",
                &error,
            );
        }
    }
}

fn present_translation_preview(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    target: Option<TargetSnapshot>,
) -> Result<(), String> {
    let ticket = presenter.begin_prepare()?;
    let handle = slint_window_handle(preview.window())
        .ok_or_else(|| "native translation-preview HWND is unavailable".to_owned())?;
    // Placed by the text it is translating, not by the pointer. The snapshot
    // was already here, used only to hand focus back; it also knows which
    // screen the user is looking at, which is the thing the placement was
    // guessing at and getting wrong across monitors.
    let placed = match &target {
        Some(target) => rust_platform_windows::configure_translation_window_for_target(
            handle,
            target.window_handle,
            target.thread_id,
        ),
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
    if let Some(target_window_handle) = target.map(|target| target.window_handle)
        && let Err(error) = rust_platform_windows::restore_target_if_our_action_window_activated(
            target_window_handle,
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
        state.speech_ready = false;
        workers.speech.send(&SpeechWorkerCommand::Initialize {
            selected_model_id: state.selected_model_id.clone(),
            output_mode: state.output_mode,
            history_enabled: state.history_enabled,
            fast_release_previews: state.fast_release_previews,
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

fn handle_speech_event(
    event: SpeechWorkerEvent,
    settings: &slint::Weak<SettingsWindow>,
    recording_overlays: &RecordingOverlays,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    match event {
        SpeechWorkerEvent::Ready {
            protocol_version,
            selected_model_id,
            models,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                state.borrow_mut().speech_ready = false;
                set_runtime_status(settings, "Worker mismatch");
                set_indicator_status(
                    settings,
                    "Speech worker protocol mismatch. Reinstall the complete application.",
                );
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.selected_model_id = selected_model_id;
                state.models = models;
            }
            sync_models(settings, state);
            set_runtime_status(settings, "Preparing model");
            set_indicator_status(settings, "Loading and warming the local speech engine…");
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
            set_indicator_status(settings, &format!("Installing {catalog_id}…"));
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
        SpeechWorkerEvent::ModelReady {
            catalog_id,
            backend,
            load_duration_ms,
            warmup_duration_ms,
            ..
        } => {
            finish_download(state, &catalog_id);
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
                            set_runtime_status(settings, "Checking fallback");
                            set_indicator_status(
                                settings,
                                &format!(
                                    "The speech backend was inconsistent; checking {} instead.",
                                    fallback.display_name()
                                ),
                            );
                        }
                        Err(fallback_error) => {
                            set_runtime_status(settings, "Worker error");
                            set_indicator_status(
                                settings,
                                &format!(
                                    "Speech backend verification failed: {error}; {fallback_error}"
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
                }
            }
            sync_models(settings, state);
            sync_action_settings(settings, state);
            set_runtime_status(settings, "Ready");
            set_indicator_status(settings, "Ready · hold Ctrl + Alt + Space to speak.");
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
                state.speech_ready = true;
            }
            let _ = save_settings(&state.borrow());
            sync_models(settings, state);
            set_indicator_status(settings, "Speech model selected.");
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
            indicator_animation.borrow_mut().begin_processing();
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
            {
                recording_overlays
                    .for_style(active_style)
                    .indicator
                    .set_processing(true);
            }
        }
        SpeechWorkerEvent::Completed {
            transcript,
            created_unix_ms,
        } => {
            let session_id = transcript.session_id;
            let stored = state
                .borrow_mut()
                .registry
                .complete(session_id, transcript.clone())
                .is_ok();
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
                if state
                    .borrow()
                    .active_visualizer_style
                    .is_some_and(VisualizerStyle::uses_external_renderer)
                {
                    let _ =
                        workers.send_companion(&CompanionRendererCommand::Complete { session_id });
                }
                state.borrow_mut().active_visualizer_style = None;
                indicator_animation.borrow_mut().stop();
                recording_overlays.retire_all("target_lost");
                set_indicator_status(settings, "Transcript saved to history; target was lost.");
            }
            match plan.history {
                HistoryUpdate::Rebuild => sync_history(settings, state),
                HistoryUpdate::PrependLatest => {
                    let state = state.borrow();
                    if let Some(entry) = state.history.first() {
                        prepend_history_row(settings, entry, HISTORY_RETAINED_ROWS);
                    }
                }
                HistoryUpdate::Unchanged => {}
            }
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
            set_indicator_status(settings, &format!("Transcribed by {provider_id}."));
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
                    "The latest dictation was cancelled and cannot be inserted.",
                );
            }
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
                    set_indicator_status(settings, "There is no transcript to insert yet.");
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
                        set_runtime_status(settings, "Checking fallback");
                        set_indicator_status(
                            settings,
                            &format!(
                                "{code} on the selected backend; checking {} automatically.",
                                fallback.display_name()
                            ),
                        );
                    }
                    Err(error) => {
                        state.borrow_mut().speech_backend_active = None;
                        sync_action_settings(settings, state);
                        set_runtime_status(settings, "Worker error");
                        set_indicator_status(
                            settings,
                            &format!("No compatible speech backend is available: {error}"),
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
                    &format!("{code}: the latest dictation could not be inserted: {message}"),
                );
            } else {
                set_indicator_status(settings, &format!("{code}: {message}"));
            }
        }
    }
}

fn handle_translation_event(
    event: TranslationWorkerEvent,
    settings: &slint::Weak<SettingsWindow>,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    translator: &slint::Weak<TranslatorWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    match event {
        TranslationWorkerEvent::Ready {
            protocol_version,
            selected_tier,
            models,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                set_indicator_status(
                    settings,
                    "Translation worker protocol mismatch. Reinstall the complete application.",
                );
                return;
            }
            let installed_models = models.iter().filter(|model| model.installed).count();
            rust_app_core::diagnostics::log_line(&format!(
                "translation_worker state=ready selected_tier={selected_tier:?} installed_models={installed_models}"
            ));
            let recovered_selection = {
                let mut state = state.borrow_mut();
                let previous = state.selected_translation_tier;
                state.translation_models = models;
                state.selected_translation_tier = selected_tier;
                state.translation_ready = false;
                previous != selected_tier && selected_tier.is_some()
            };
            if recovered_selection {
                let _ = save_settings(&state.borrow());
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_model selection=recovered tier={selected_tier:?}"
                ));
            }
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            if selected_tier.is_none() {
                set_translation_status(
                    translator,
                    "No model installed. Choose Fast or Quality in Settings.",
                );
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
            set_indicator_status(settings, &format!("Downloading {catalog_id}: {percent}%"));
            set_translation_status(translator, &format!("Downloading model… {percent}%"));
        }
        TranslationWorkerEvent::ModelInstalled {
            catalog_id, tier, ..
        } => {
            finish_download(state, &catalog_id);
            {
                let mut state = state.borrow_mut();
                state.selected_translation_tier = Some(tier);
                for model in &mut state.translation_models {
                    if model.catalog_id == catalog_id {
                        model.installed = true;
                    }
                    model.selected = model.tier == tier;
                }
            }
            let _ = save_settings(&state.borrow());
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
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
            rust_app_core::diagnostics::log_line(&format!(
                "translation_model state=ready catalog_id={catalog_id} tier={tier:?} load_ms={load_duration_ms}"
            ));
            let queued_selection_translation = {
                let mut state = state.borrow_mut();
                state.selected_translation_tier = Some(tier);
                state.translation_ready = true;
                for model in &mut state.translation_models {
                    model.selected = model.tier == tier;
                    if model.catalog_id == catalog_id {
                        model.installed = true;
                    }
                }
                state.queued_selection_translation.take()
            };
            let _ = save_settings(&state.borrow());
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            set_indicator_status(
                settings,
                &format!("Translation model ready in {load_duration_ms} ms."),
            );
            set_translation_status(
                translator,
                "Ready · language direction is detected locally.",
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
        }
        TranslationWorkerEvent::ModelSelected { tier } => {
            {
                let mut state = state.borrow_mut();
                state.selected_translation_tier = Some(tier);
                for model in &mut state.translation_models {
                    model.selected = model.tier == tier;
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
                        )
                    })
            };
            let Some((
                origin,
                translation_elapsed_ms,
                request_started_unix_ms,
                first_partial,
                source_text,
            )) = pending
            else {
                return;
            };

            match origin {
                TranslationOrigin::Translator => {
                    if let Some(translator) = translator.upgrade() {
                        translator.set_result_text(translated_text.into());
                        let status = if chunk_count > 1 {
                            format!("Translating locally… {chunk_index}/{chunk_count}")
                        } else {
                            "Translating locally…".to_owned()
                        };
                        translator.set_status_text(status.into());
                        translator.set_processing(true);
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
        TranslationWorkerEvent::Completed {
            result,
            created_unix_ms,
        } => {
            let pending = {
                let mut state = state.borrow_mut();
                state.translation_history.insert(
                    0,
                    TranslationHistoryEntry {
                        result: result.clone(),
                        created_unix_ms,
                    },
                );
                state.translation_history.truncate(250);
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
        TranslationWorkerEvent::TranslatedRemotely {
            request_id,
            provider_id,
        } => {
            // Where the words came from is worth stating plainly: the text left
            // the machine for this one.
            rust_app_core::diagnostics::log_line(&format!(
                "translation request={request_id} provider={provider_id}"
            ));
            set_indicator_status(settings, &format!("Translated by {provider_id}."));
        }
        TranslationWorkerEvent::Cancelled { request_id } => {
            let pending = {
                let mut app = state.borrow_mut();
                app.selection_release_gates.remove(&request_id);
                app.pending_translations.remove(&request_id)
            };
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::Translator)
            ) && let Some(translator) = translator.upgrade()
            {
                translator.set_processing(false);
                translator.set_status_text("Translation cancelled.".into());
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
                retire_translation_preview(&slot, "translation_cancelled");
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
            state.borrow_mut().translation_history = entries;
        }
        TranslationWorkerEvent::Failed {
            request_id,
            code,
            message,
            ..
        } => {
            // A failure with no request behind it did not come from a
            // translation — every one of those carries its id — so it came from
            // the install that was running. Ending it here stops the row
            // offering a Cancel for a download that is already over.
            //
            // This is a guess the protocol should not make necessary:
            // `Failed` needs the catalog id that `ModelDownloadProgress`
            // already carries.
            if request_id.is_none() {
                let installing: Vec<String> = state.borrow().downloads.keys().cloned().collect();
                if !installing.is_empty() {
                    for catalog_id in installing {
                        finish_download(state, &catalog_id);
                    }
                    sync_translation_models(settings, state);
                }
            }
            if let Some(request_id) = request_id {
                translation_previews
                    .borrow_mut()
                    .clear_current_request(request_id);
            }
            let pending = request_id.and_then(|id| {
                let mut app = state.borrow_mut();
                app.selection_release_gates.remove(&id);
                app.pending_translations.remove(&id)
            });
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
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::Translator)
            ) {
                set_translation_status(translator, &format!("{code}: {message}"));
            }
            set_indicator_status(settings, &format!("{code}: {message}"));
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
    let completion_status = match pending.origin {
        TranslationOrigin::Translator => {
            if let Some(translator) = translator.upgrade() {
                translator.set_result_text(result.translated_text.into());
                translator.set_status_text(
                    format!(
                        "{} → {} · {} ms · local",
                        result.source_language.code().to_uppercase(),
                        result.target_language.code().to_uppercase(),
                        result.processing_duration_ms
                    )
                    .into(),
                );
                translator.set_processing(false);
            }
            "Translation completed locally.".to_owned()
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
                "Translation completed locally.".to_owned()
            }
            TranslationDelivery::Replace => {
                match dispatch_or_defer_selection_replacement(
                    state,
                    workers,
                    result.request_id,
                    target,
                    result.translated_text,
                ) {
                    ReplacementDispatch::Dispatched => "Translation completed locally.".to_owned(),
                    ReplacementDispatch::Deferred => {
                        "Translation ready; waiting for the shortcut to be released.".to_owned()
                    }
                    ReplacementDispatch::Blocked(message) => {
                        format!("Translation ready, but {message}")
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
                "Translation completed locally.".to_owned()
            }
        },
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

fn short_id(session_id: SessionId) -> String {
    session_id.to_string().chars().take(8).collect()
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
        }
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
