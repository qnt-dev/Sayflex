#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    error::Error,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

use rust_app_core::{
    BeginDisposition, EndDisposition, LayoutCandidate, LayoutDirection, SessionPhase,
    SessionRegistry, build_layout_candidates,
};
use rust_protocol::{
    HistoryEntry, InputHostCommand, InputHostEvent, ModelSummary, PROTOCOL_VERSION, RequestId,
    SessionId, SpeechOutputMode, SpeechWorkerCommand, SpeechWorkerEvent, TargetSnapshot,
    TextActionIntent, Transcript, TranslationHistoryEntry, TranslationModelSummary,
    TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const SELECTION_WINDOW_TITLE: &str = "Correct keyboard layout";
const ACTION_PALETTE_WINDOW_TITLE: &str = "Text actions";
const TRANSLATION_PREVIEW_WINDOW_TITLE: &str = "Translation preview";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INDICATOR_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const INDICATOR_BAR_PROFILE: [f32; 10] = [0.25, 0.4, 0.62, 0.82, 1.0, 1.0, 0.82, 0.62, 0.4, 0.25];

enum RuntimeEvent {
    Input(InputHostEvent),
    Speech(SpeechWorkerEvent),
    Translation(TranslationWorkerEvent),
    WorkerError(String),
    WorkerExited { kind: WorkerKind, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WorkerKind {
    Input,
    Speech,
    Translation,
}

type UiEventHandler = Box<dyn FnMut(RuntimeEvent)>;

thread_local! {
    static UI_EVENT_HANDLER: RefCell<Option<UiEventHandler>> = RefCell::new(None);
}

#[derive(Clone)]
struct RuntimeUi {
    settings: slint::Weak<SettingsWindow>,
    indicator: slint::Weak<RecordingIndicator>,
    chooser: slint::Weak<SelectionChooser>,
    action_palette: slint::Weak<ActionPalette>,
    translation_preview: slint::Weak<TranslationPreview>,
    translator: slint::Weak<TranslatorWindow>,
    state: Rc<RefCell<AppState>>,
    workers: Rc<Workers>,
    indicator_animation: Rc<RefCell<IndicatorAnimation>>,
}

struct JsonWriter {
    stdin: RefCell<BufWriter<ChildStdin>>,
}

impl JsonWriter {
    fn send<T: Serialize>(&self, message: &T) -> Result<(), String> {
        let mut writer = self.stdin.borrow_mut();
        serde_json::to_writer(&mut *writer, message).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    }

    fn replace_with(&self, replacement: Self) {
        *self.stdin.borrow_mut() = replacement.stdin.into_inner();
    }
}

struct Workers {
    _job: rust_platform_windows::WorkerJob,
    input: JsonWriter,
    speech: JsonWriter,
    translation: JsonWriter,
    input_child: RefCell<Child>,
    speech_child: RefCell<Child>,
    translation_child: RefCell<Child>,
    restart_attempts: RefCell<HashMap<WorkerKind, Vec<Instant>>>,
}

impl Workers {
    fn shutdown(&self) {
        let _ = self.input.send(&InputHostCommand::Shutdown);
        let _ = self.speech.send(&SpeechWorkerCommand::Shutdown);
        let _ = self.translation.send(&TranslationWorkerCommand::Shutdown);
    }

    fn restart(&self, kind: WorkerKind) -> Result<(), String> {
        let now = Instant::now();
        {
            let mut attempts = self.restart_attempts.borrow_mut();
            let attempts = attempts.entry(kind).or_default();
            attempts.retain(|attempt| now.duration_since(*attempt) < Duration::from_secs(60));
            if attempts.len() >= 3 {
                return Err(format!(
                    "{kind:?} worker stopped at least three times in one minute"
                ));
            }
            attempts.push(now);
        }

        match kind {
            WorkerKind::Input => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Input,
                    "rust-input-host.exe",
                    RuntimeEvent::Input,
                )
                .map_err(|error| error.to_string())?;
                self.input.replace_with(writer);
                replace_child(&self.input_child, child);
            }
            WorkerKind::Speech => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Speech,
                    "rust-speech-worker.exe",
                    RuntimeEvent::Speech,
                )
                .map_err(|error| error.to_string())?;
                self.speech.replace_with(writer);
                replace_child(&self.speech_child, child);
            }
            WorkerKind::Translation => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Translation,
                    "rust-translation-worker.exe",
                    RuntimeEvent::Translation,
                )
                .map_err(|error| error.to_string())?;
                self.translation.replace_with(writer);
                replace_child(&self.translation_child, child);
            }
        }
        Ok(())
    }
}

fn replace_child(slot: &RefCell<Child>, replacement: Child) {
    let mut previous = slot.replace(replacement);
    if previous.try_wait().ok().flatten().is_none() {
        let _ = previous.kill();
    }
    let _ = previous.wait();
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.shutdown();
        std::thread::sleep(Duration::from_millis(80));
        for child in [
            &self.input_child,
            &self.speech_child,
            &self.translation_child,
        ] {
            let mut child = child.borrow_mut();
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
        }
    }
}

struct PendingSelection {
    request_id: RequestId,
    target: TargetSnapshot,
    candidates: Vec<LayoutCandidate>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TextActionMode {
    #[default]
    Adaptive,
    AlwaysPalette,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TranslationDelivery {
    #[default]
    Preview,
    Replace,
    Copy,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VisualizerStyle {
    #[default]
    VoiceOrb,
    SymmetricSpectrum,
    FluidRibbon,
}

impl VisualizerStyle {
    const fn next(self) -> Self {
        match self {
            Self::VoiceOrb => Self::SymmetricSpectrum,
            Self::SymmetricSpectrum => Self::FluidRibbon,
            Self::FluidRibbon => Self::VoiceOrb,
        }
    }

    const fn index(self) -> i32 {
        match self {
            Self::VoiceOrb => 0,
            Self::SymmetricSpectrum => 1,
            Self::FluidRibbon => 2,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::VoiceOrb => "Visualizer: Voice Orb",
            Self::SymmetricSpectrum => "Visualizer: Symmetric Spectrum",
            Self::FluidRibbon => "Visualizer: Fluid Ribbon",
        }
    }
}

#[derive(Clone)]
struct PendingTextAction {
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
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
}

struct SelectionTranslationRequest {
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    delivery: TranslationDelivery,
}

#[derive(Clone)]
struct PreviewTranslation {
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum IndicatorMode {
    #[default]
    Hidden,
    Listening,
    Processing,
}

struct IndicatorAnimation {
    bars: Rc<VecModel<f32>>,
    style: VisualizerStyle,
    mode: IndicatorMode,
    target_level: f32,
    display_level: f32,
    phase: f32,
    last_frame: Instant,
}

struct IndicatorFrame {
    heights: Vec<f32>,
    signal_opacity: f32,
    disc_opacity: f32,
    orb_size: f32,
    orb_glow: f32,
    ribbon_y1: f32,
    ribbon_y2: f32,
    ribbon_y3: f32,
    ribbon_y4: f32,
}

impl IndicatorAnimation {
    fn new(bars: Rc<VecModel<f32>>) -> Self {
        Self {
            bars,
            style: VisualizerStyle::default(),
            mode: IndicatorMode::Hidden,
            target_level: 0.08,
            display_level: 0.08,
            phase: 0.0,
            last_frame: Instant::now(),
        }
    }

    fn set_style(&mut self, style: VisualizerStyle) {
        self.style = style;
    }

    fn begin_listening(&mut self) {
        self.mode = IndicatorMode::Listening;
        self.target_level = 0.08;
        self.display_level = self.display_level.max(0.08);
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    fn set_microphone_level(&mut self, microphone_level: f32) {
        if self.mode == IndicatorMode::Listening {
            self.target_level = (microphone_level.clamp(0.0, 1.0).sqrt() * 2.35).clamp(0.0, 1.0);
        }
    }

    fn begin_processing(&mut self) {
        self.mode = IndicatorMode::Processing;
        self.target_level = 0.18;
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    fn stop(&mut self) {
        self.mode = IndicatorMode::Hidden;
        self.target_level = 0.08;
    }

    fn tick(&mut self, indicator: &slint::Weak<RecordingIndicator>) {
        if self.mode == IndicatorMode::Hidden
            || self.last_frame.elapsed() < INDICATOR_FRAME_INTERVAL
        {
            return;
        }

        let elapsed = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let frame_scale =
            (elapsed.as_secs_f32() / INDICATOR_FRAME_INTERVAL.as_secs_f32()).clamp(0.5, 2.5);
        let frame = self.advance_frame(frame_scale);
        self.bars.set_vec(frame.heights);

        if let Some(indicator) = indicator.upgrade() {
            indicator.set_visualizer_style(self.style.index());
            indicator.set_processing(self.mode == IndicatorMode::Processing);
            indicator.set_signal_opacity(frame.signal_opacity);
            indicator.set_disc_opacity(frame.disc_opacity);
            indicator.set_orb_size(frame.orb_size);
            indicator.set_orb_glow(frame.orb_glow);
            indicator.set_ribbon_y1(frame.ribbon_y1);
            indicator.set_ribbon_y2(frame.ribbon_y2);
            indicator.set_ribbon_y3(frame.ribbon_y3);
            indicator.set_ribbon_y4(frame.ribbon_y4);
        }
    }

    fn advance_frame(&mut self, frame_scale: f32) -> IndicatorFrame {
        let phase_step = match self.mode {
            IndicatorMode::Listening => 0.29,
            IndicatorMode::Processing => 0.18,
            IndicatorMode::Hidden => 0.0,
        };
        self.phase += phase_step * frame_scale;
        let smoothing = 1.0 - 0.82_f32.powf(frame_scale);
        self.display_level += (self.target_level - self.display_level) * smoothing;

        let processing_pulse = ((self.phase * 0.72).sin() + 1.0) / 2.0;
        let energy = match self.mode {
            IndicatorMode::Listening => self.display_level.max(0.07),
            IndicatorMode::Processing => self.target_level + (processing_pulse * 0.08),
            IndicatorMode::Hidden => 0.0,
        };
        let heights: Vec<f32> = INDICATOR_BAR_PROFILE
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                let travelling_wave = ((self.phase + index as f32 * 0.62).sin() + 1.0) / 2.0;
                (3.0 + (profile * 5.0) + (travelling_wave * energy * 12.0)).clamp(3.0, 19.0)
            })
            .collect();
        IndicatorFrame {
            heights,
            signal_opacity: (0.5 + energy.min(1.0) * 0.5).clamp(0.5, 1.0),
            disc_opacity: match self.mode {
                IndicatorMode::Processing => 0.72 + processing_pulse * 0.28,
                IndicatorMode::Listening => 1.0,
                IndicatorMode::Hidden => 0.0,
            },
            orb_size: (9.0 + energy * 10.0 + processing_pulse * 1.5).clamp(8.0, 20.0),
            orb_glow: (0.12 + energy * 0.38 + processing_pulse * 0.12).clamp(0.12, 0.62),
            ribbon_y1: 9.5 + (self.phase * 0.93).sin() * energy * 6.5,
            ribbon_y2: 9.5 + (self.phase * 0.93 + 1.35).sin() * energy * 7.4,
            ribbon_y3: 9.5 + (self.phase * 0.93 + 2.7).sin() * energy * 6.8,
            ribbon_y4: 9.5 + (self.phase * 0.93 + 4.05).sin() * energy * 5.8,
        }
    }
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
    output_mode: SpeechOutputMode,
    fast_release_previews: bool,
    visualizer_style: VisualizerStyle,
    input_ready: bool,
    speech_ready: bool,
    indicator_session: Option<SessionId>,
    release_started: HashMap<SessionId, Instant>,
    pending_insert_last: Option<PendingInsertLast>,
    pending_selection: Option<PendingSelection>,
    translation_models: Vec<TranslationModelSummary>,
    selected_translation_tier: Option<TranslationTier>,
    translation_ready: bool,
    translation_enabled: bool,
    layout_enabled: bool,
    translation_keep_warm: bool,
    text_action_mode: TextActionMode,
    translation_delivery: TranslationDelivery,
    history_enabled: bool,
    pending_text_action: Option<PendingTextAction>,
    pending_translations: HashMap<RequestId, PendingTranslation>,
    queued_selection_translation: Option<SelectionTranslationRequest>,
    preview_translation: Option<PreviewTranslation>,
    translation_history: Vec<TranslationHistoryEntry>,
}

impl AppState {
    fn new(
        selected_model_id: String,
        output_mode: SpeechOutputMode,
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
            output_mode,
            fast_release_previews: saved_settings.fast_release_previews,
            visualizer_style: saved_settings.visualizer_style,
            input_ready: false,
            speech_ready: false,
            indicator_session: None,
            release_started: HashMap::new(),
            pending_insert_last: None,
            pending_selection: None,
            translation_models: Vec::new(),
            selected_translation_tier: saved_settings.selected_translation_tier,
            translation_ready: false,
            translation_enabled: saved_settings.translation_enabled,
            layout_enabled: saved_settings.layout_enabled,
            translation_keep_warm: saved_settings.translation_keep_warm,
            text_action_mode: saved_settings.text_action_mode,
            translation_delivery: saved_settings.translation_delivery,
            pending_text_action: None,
            pending_translations: HashMap::new(),
            queued_selection_translation: None,
            preview_translation: None,
            translation_history: Vec::new(),
            history_enabled: saved_settings.history_enabled,
        }
    }

    fn latest_transcript(&self) -> Option<&Transcript> {
        self.registry
            .latest_session()
            .or(self.latest_history_session)
            .and_then(|session_id| self.transcripts.get(&session_id))
    }

    fn request_insert_last(&mut self, target: TargetSnapshot) -> InsertLastResolution {
        if let Some(session_id) = self.registry.latest_session() {
            return match self.registry.phase(session_id) {
                Some(SessionPhase::Completed) => self.transcripts.get(&session_id).cloned().map_or(
                    InsertLastResolution::Unavailable(
                        "The latest dictation has no completed transcript.",
                    ),
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
                    InsertLastResolution::Unavailable(
                        "The latest dictation did not produce recoverable text.",
                    )
                }
            };
        }

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
    rust_platform_windows::configure_safe_dll_search()?;
    configure_ui_backend()?;
    if let Some(style) = indicator_preview_style() {
        return run_indicator_preview(style);
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

    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\RustVoice.Desktop")?;
    let settings = SettingsWindow::new()?;
    let (indicator, indicator_animation) = new_indicator()?;
    let chooser = SelectionChooser::new()?;
    let action_palette = ActionPalette::new()?;
    let translation_preview = TranslationPreview::new()?;
    let translator = TranslatorWindow::new()?;
    let tray = AppTray::new()?;
    precreate_indicator(&indicator);
    precreate_action_windows(&chooser, &action_palette, &translation_preview);

    let worker_job = rust_platform_windows::WorkerJob::new()?;
    let (input_child, input) = spawn_worker(
        &worker_job,
        WorkerKind::Input,
        "rust-input-host.exe",
        RuntimeEvent::Input,
    )?;
    let (speech_child, speech) = spawn_worker(
        &worker_job,
        WorkerKind::Speech,
        "rust-speech-worker.exe",
        RuntimeEvent::Speech,
    )?;
    let (translation_child, translation) = spawn_worker(
        &worker_job,
        WorkerKind::Translation,
        "rust-translation-worker.exe",
        RuntimeEvent::Translation,
    )?;
    let workers = Rc::new(Workers {
        _job: worker_job,
        input,
        speech,
        translation,
        input_child: RefCell::new(input_child),
        speech_child: RefCell::new(speech_child),
        translation_child: RefCell::new(translation_child),
        restart_attempts: RefCell::new(HashMap::new()),
    });
    let saved_settings = load_settings();
    indicator_animation
        .borrow_mut()
        .set_style(saved_settings.visualizer_style);
    indicator.set_visualizer_style(saved_settings.visualizer_style.index());
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
        &saved_settings,
    )));
    sync_output_mode(&settings.as_weak(), &state);
    sync_action_settings(&settings.as_weak(), &state);

    workers.speech.send(&SpeechWorkerCommand::Initialize {
        selected_model_id,
        output_mode,
        history_enabled: saved_settings.history_enabled,
        fast_release_previews: saved_settings.fast_release_previews,
    })?;
    workers
        .translation
        .send(&TranslationWorkerCommand::Initialize {
            selected_tier: saved_settings.selected_translation_tier,
            keep_warm: saved_settings.translation_keep_warm,
            history_enabled: saved_settings.history_enabled,
        })?;

    wire_settings(
        &settings,
        &translator,
        &state,
        &workers,
        &indicator_animation,
    );
    wire_tray(&tray, &settings, &translator, &state, &workers);
    wire_chooser(&chooser, &state, &workers);
    wire_action_palette(
        &action_palette,
        &translation_preview,
        &chooser,
        &settings,
        &state,
        &workers,
    );
    wire_translation_preview(&translation_preview, &state, &workers);
    wire_translator(&translator, &state, &workers);
    install_runtime_handler(RuntimeUi {
        settings: settings.as_weak(),
        indicator: indicator.as_weak(),
        chooser: chooser.as_weak(),
        action_palette: action_palette.as_weak(),
        translation_preview: translation_preview.as_weak(),
        translator: translator.as_weak(),
        state: Rc::clone(&state),
        workers: Rc::clone(&workers),
        indicator_animation: Rc::clone(&indicator_animation),
    });
    let animation_timer = start_indicator_animation(&indicator, &indicator_animation);

    if !env::args().any(|argument| argument == "--background") {
        settings.show()?;
    }
    tray.show()?;
    slint::run_event_loop()?;
    animation_timer.stop();
    UI_EVENT_HANDLER.with(|handler| {
        handler.borrow_mut().take();
    });
    workers.shutdown();
    Ok(())
}

fn configure_ui_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()
}

fn new_indicator()
-> Result<(RecordingIndicator, Rc<RefCell<IndicatorAnimation>>), slint::PlatformError> {
    let indicator = RecordingIndicator::new()?;
    let indicator_bars = Rc::new(VecModel::from(vec![
        4.0, 6.0, 9.0, 12.0, 15.0, 15.0, 12.0, 9.0, 6.0, 4.0,
    ]));
    indicator.set_bar_heights(ModelRc::from(Rc::clone(&indicator_bars)));
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(indicator_bars)));
    Ok((indicator, animation))
}

fn indicator_preview_style() -> Option<VisualizerStyle> {
    env::args().find_map(|argument| match argument.as_str() {
        "--indicator-preview" | "--indicator-preview=orb" => Some(VisualizerStyle::VoiceOrb),
        "--indicator-preview=spectrum" => Some(VisualizerStyle::SymmetricSpectrum),
        "--indicator-preview=ribbon" => Some(VisualizerStyle::FluidRibbon),
        _ => None,
    })
}

fn translation_preview_mode() -> Option<bool> {
    env::args().find_map(|argument| match argument.as_str() {
        "--translation-preview" => Some(false),
        "--translation-preview=long" => Some(true),
        _ => None,
    })
}

fn run_indicator_preview(style: VisualizerStyle) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\RustVoice.IndicatorPreview")?;
    let (indicator, animation) = new_indicator()?;
    {
        let mut animation = animation.borrow_mut();
        animation.set_style(style);
        animation.begin_listening();
    }
    indicator.set_visualizer_style(style.index());
    indicator.set_prepared(true);
    indicator.show()?;
    let indicator_weak = indicator.as_weak();
    slint::spawn_local(async move {
        if let Some(indicator) = indicator_weak.upgrade() {
            let configured = indicator
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle)
                .is_some_and(|handle| configure_indicator_native_window(handle, None));
            if !configured {
                eprintln!("could not configure recording indicator preview");
            }
        }
    })?;

    let timer = Timer::default();
    let indicator = indicator.as_weak();
    let animation_for_timer = Rc::clone(&animation);
    let started = Instant::now();
    let segment = Rc::new(Cell::new(IndicatorMode::Listening));
    let segment_for_timer = Rc::clone(&segment);
    timer.start(TimerMode::Repeated, Duration::from_millis(12), move || {
        let elapsed = started.elapsed().as_secs_f32();
        let next_segment = if (elapsed % 10.0) < 7.0 {
            IndicatorMode::Listening
        } else {
            IndicatorMode::Processing
        };
        if segment_for_timer.get() != next_segment {
            segment_for_timer.set(next_segment);
            match next_segment {
                IndicatorMode::Listening => animation_for_timer.borrow_mut().begin_listening(),
                IndicatorMode::Processing => animation_for_timer.borrow_mut().begin_processing(),
                IndicatorMode::Hidden => {}
            }
        }
        if next_segment == IndicatorMode::Listening {
            let simulated_level = (0.08 + ((elapsed * 2.7).sin() + 1.0) * 0.18).clamp(0.0, 1.0);
            animation_for_timer
                .borrow_mut()
                .set_microphone_level(simulated_level);
        }
        animation_for_timer.borrow_mut().tick(&indicator);
    });
    slint::run_event_loop()?;
    timer.stop();
    Ok(())
}

fn run_action_palette_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\RustVoice.ActionPreview")?;
    let palette = ActionPalette::new()?;
    let source =
        "A compact command surface should fit the selected text and preserve editor focus.";
    palette.set_source_text(source.into());
    palette.set_source_panel_height(adaptive_text_panel_height(source, 70, 2, 4, 15.0, 16.0));
    palette.set_translation_enabled(true);
    palette.set_layout_enabled(true);
    palette.set_prepared(true);
    palette.on_translate(|| {
        let _ = slint::quit_event_loop();
    });
    palette.on_correct_layout(|| {
        let _ = slint::quit_event_loop();
    });
    palette.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    palette.show()?;
    let palette_weak = palette.as_weak();
    slint::spawn_local(async move {
        if let Some(palette) = palette_weak.upgrade() {
            let handle = palette
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            if let Err(error) = handle
                .ok_or_else(|| "native action window is unavailable".to_owned())
                .and_then(rust_platform_windows::configure_action_window)
            {
                eprintln!("could not configure action preview: {error}");
            }
        }
    })?;
    slint::run_event_loop()?;
    Ok(())
}

fn run_translation_preview(long_text: bool) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\RustVoice.TranslationPreview")?;
    let preview = TranslationPreview::new()?;
    let source = if long_text {
        "A long translation should stay readable without turning the compact action card into a full workspace."
    } else {
        "The preview should grow with its content, not with a fixed empty canvas."
    };
    let translated = if long_text {
        "Длинный перевод должен оставаться удобным для чтения. Карточка сохраняет компактную форму, а результат прокручивается внутри неё. Это позволяет быстро проверить смысл, не терять выбранное место в исходном приложении и не открывать отдельное большое рабочее окно. Прокрутка появляется только тогда, когда текст действительно не помещается. ".repeat(12)
    } else {
        "Предпросмотр должен увеличиваться вместе с содержимым, а не оставлять пустой холст."
            .to_owned()
    };
    apply_translation_preview_layout(&preview, source, &translated);
    preview.set_source_text(source.into());
    preview.set_translated_text(translated.clone().into());
    preview.set_direction_label("EN → RU · 94 ms".into());
    preview.set_processing(false);
    preview.set_prepared(true);
    preview.on_replace(|| {
        let _ = slint::quit_event_loop();
    });
    preview.on_copy(|| {
        let _ = slint::quit_event_loop();
    });
    preview.on_dismiss(|| {
        let _ = slint::quit_event_loop();
    });
    preview.show()?;
    let preview_weak = preview.as_weak();
    slint::spawn_local(async move {
        if let Some(preview) = preview_weak.upgrade() {
            let handle = preview
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            if let Err(error) = handle
                .ok_or_else(|| "native translation window is unavailable".to_owned())
                .and_then(rust_platform_windows::configure_action_window)
            {
                eprintln!("could not configure translation preview: {error}");
            }
        }
    })?;
    slint::run_event_loop()?;
    Ok(())
}

fn run_layout_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\RustVoice.LayoutPreview")?;
    let chooser = SelectionChooser::new()?;
    let source = "ghbdtn vbh";
    chooser.set_source_text(source.into());
    chooser.set_source_panel_height(adaptive_text_panel_height(source, 82, 2, 5, 15.0, 16.0));
    chooser.set_candidate_labels(ModelRc::new(VecModel::from(vec![SharedString::from(
        "English keys → Cyrillic layout",
    )])));
    chooser.set_candidate_texts(ModelRc::new(VecModel::from(vec![SharedString::from(
        "привет мир",
    )])));
    chooser.set_prepared(true);
    chooser.on_choose_candidate(|_| {
        let _ = slint::quit_event_loop();
    });
    chooser.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    chooser.show()?;
    let chooser_weak = chooser.as_weak();
    slint::spawn_local(async move {
        if let Some(chooser) = chooser_weak.upgrade() {
            let handle = chooser
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            if let Err(error) = handle
                .ok_or_else(|| "native selection window is unavailable".to_owned())
                .and_then(rust_platform_windows::configure_action_window)
            {
                eprintln!("could not configure layout preview: {error}");
            }
        }
    })?;
    slint::run_event_loop()?;
    Ok(())
}

fn spawn_worker<T>(
    worker_job: &rust_platform_windows::WorkerJob,
    kind: WorkerKind,
    executable: &str,
    wrap: fn(T) -> RuntimeEvent,
) -> Result<(Child, JsonWriter), Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
{
    let path = worker_path(executable)?;
    let mut command = Command::new(&path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if path
        .file_name()
        .is_some_and(|name| name == "rust-speech-worker-cuda.exe")
    {
        let cuda = trusted_cuda_runtime_directory()
            .ok_or("the CUDA worker was selected without a trusted CUDA runtime")?;
        // The CUDA executable imports cuBLAS before Rust main() can harden dynamic
        // DLL loading. Give that child only the validated toolkit directory instead
        // of inheriting arbitrary user-writable PATH entries.
        command.env("PATH", cuda);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    if let Err(error) = worker_job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("could not contain {executable} in the worker job: {error}").into());
    }
    let stdin = child.stdin.take().ok_or("worker stdin is unavailable")?;
    let stdout = child.stdout.take().ok_or("worker stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("worker stderr is unavailable")?;

    let output_name = executable.to_owned();
    std::thread::Builder::new()
        .name(format!("{executable}-events"))
        .spawn(move || {
            let mut exit_message = format!("{output_name} closed its event stream");
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        exit_message = format!("{output_name} output failed: {error}");
                        break;
                    }
                };
                match serde_json::from_str::<T>(&line) {
                    Ok(event) => {
                        dispatch_runtime_event(wrap(event));
                    }
                    Err(error) => {
                        dispatch_runtime_event(RuntimeEvent::WorkerError(format!(
                            "{output_name} sent invalid data: {error}"
                        )));
                    }
                }
            }
            dispatch_runtime_event(RuntimeEvent::WorkerExited {
                kind,
                message: exit_message,
            });
        })?;

    let diagnostics_name = executable.to_owned();
    std::thread::Builder::new()
        .name(format!("{executable}-diagnostics"))
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("{diagnostics_name}: {line}");
            }
        })?;

    Ok((
        child,
        JsonWriter {
            stdin: RefCell::new(BufWriter::new(stdin)),
        },
    ))
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

fn worker_path(executable: &str) -> Result<PathBuf, Box<dyn Error>> {
    let current = env::current_exe()?;
    let parent = current
        .parent()
        .ok_or("desktop executable has no parent directory")?;
    if executable == "rust-speech-worker.exe" {
        let cuda = parent.join("rust-speech-worker-cuda.exe");
        if cuda.exists() && trusted_cuda_runtime_directory().is_some() {
            return Ok(cuda);
        }
        let cpu = parent.join("rust-speech-worker-cpu.exe");
        if cpu.exists() {
            return Ok(cpu);
        }
    }
    let sibling = parent.join(executable);
    if sibling.exists() {
        return Ok(sibling);
    }

    Err(format!(
        "required worker is missing: {}. Build the whole workspace before launching the desktop.",
        sibling.display()
    )
    .into())
}

fn trusted_cuda_runtime_directory() -> Option<PathBuf> {
    let program_files = env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())?;
    let mut roots = Vec::new();
    if let Some(path) = env::var_os("CUDA_PATH") {
        roots.push(PathBuf::from(path));
    }
    let mut versioned = env::vars_os()
        .filter(|(name, _)| name.to_string_lossy().starts_with("CUDA_PATH_V"))
        .map(|(_, path)| PathBuf::from(path))
        .collect::<Vec<_>>();
    versioned.sort_by(|left, right| right.cmp(left));
    roots.extend(versioned);

    let toolkit_root = program_files
        .join("NVIDIA GPU Computing Toolkit")
        .join("CUDA");
    if let Ok(entries) = fs::read_dir(toolkit_root) {
        let mut installed = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        installed.sort_by(|left, right| right.cmp(left));
        roots.extend(installed);
    }

    roots.into_iter().find_map(|root| {
        let trusted_root = root.canonicalize().ok()?;
        if !trusted_root.starts_with(&program_files) {
            return None;
        }
        let directory = trusted_root.join("bin").join("x64").canonicalize().ok()?;
        (directory.join("cublas64_13.dll").is_file()
            && directory.join("cublasLt64_13.dll").is_file())
        .then_some(directory)
    })
}

fn install_runtime_handler(runtime: RuntimeUi) {
    UI_EVENT_HANDLER.with(|handler| {
        *handler.borrow_mut() = Some(Box::new(move |event| {
            handle_runtime_event(event, &runtime);
        }));
    });
}

fn start_indicator_animation(
    indicator: &RecordingIndicator,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) -> Timer {
    let timer = Timer::default();
    let indicator = indicator.as_weak();
    let indicator_animation = Rc::clone(indicator_animation);
    timer.start(TimerMode::Repeated, INDICATOR_FRAME_INTERVAL, move || {
        indicator_animation.borrow_mut().tick(&indicator);
    });
    timer
}

fn handle_runtime_event(event: RuntimeEvent, runtime: &RuntimeUi) {
    match event {
        RuntimeEvent::Input(event) => {
            handle_input_event(event, runtime);
        }
        RuntimeEvent::Speech(event) => {
            handle_speech_event(
                event,
                &runtime.settings,
                &runtime.indicator,
                &runtime.state,
                &runtime.workers,
                &runtime.indicator_animation,
            );
        }
        RuntimeEvent::Translation(event) => {
            handle_translation_event(
                event,
                &runtime.settings,
                &runtime.translation_preview,
                &runtime.translator,
                &runtime.state,
                &runtime.workers,
            );
        }
        RuntimeEvent::WorkerError(message) => {
            runtime.state.borrow_mut().indicator_session = None;
            runtime.indicator_animation.borrow_mut().stop();
            if let Some(indicator) = runtime.indicator.upgrade() {
                let _ = indicator.hide();
            }
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Worker error".into());
                settings.set_indicator_status(message.into());
            }
        }
        RuntimeEvent::WorkerExited { kind, message } => {
            handle_worker_exit(kind, &message, runtime);
        }
    }
}

fn handle_worker_exit(kind: WorkerKind, message: &str, runtime: &RuntimeUi) {
    let active_session = {
        let mut state = runtime.state.borrow_mut();
        match kind {
            WorkerKind::Input => state.input_ready = false,
            WorkerKind::Speech => state.speech_ready = false,
            WorkerKind::Translation => {
                state.translation_ready = false;
                state.pending_translations.clear();
            }
        }
        let active = state.indicator_session.take();
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
    if let Some(indicator) = runtime.indicator.upgrade() {
        let _ = indicator.hide();
    }

    match runtime.workers.restart(kind) {
        Ok(()) => {
            let initialization = match kind {
                WorkerKind::Input => Ok(()),
                WorkerKind::Speech => {
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
            };
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Recovering worker".into());
                let detail = initialization.map_or_else(
                    |error| format!("{message}; restart initialization failed: {error}"),
                    |()| format!("{message}; worker restarted automatically"),
                );
                settings.set_indicator_status(detail.into());
            }
        }
        Err(error) => {
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Worker unavailable".into());
                settings.set_indicator_status(format!("{message}; {error}").into());
            }
        }
    }
}

fn handle_input_event(event: InputHostEvent, runtime: &RuntimeUi) {
    let settings = &runtime.settings;
    let indicator = &runtime.indicator;
    let chooser = &runtime.chooser;
    let action_palette = &runtime.action_palette;
    let translation_preview = &runtime.translation_preview;
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
                set_indicator_status(settings, "Input worker is not ready.");
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
            }
            if let Err(error) = workers.speech.send(&SpeechWorkerCommand::Begin {
                session_id,
                started_unix_ms,
            }) {
                let mut state = state.borrow_mut();
                let _ = state.registry.fail(session_id);
                state.targets.remove(&session_id);
                state.indicator_session = None;
                set_indicator_status(settings, &format!("Could not start recording: {error}"));
                return;
            }
            indicator_animation.borrow_mut().begin_listening();
            if let Some(indicator) = indicator.upgrade() {
                indicator.set_processing(false);
                indicator.set_signal_opacity(0.54);
                indicator.set_disc_opacity(1.0);
                show_indicator(&indicator, Some(target.window_handle));
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
                    let _ = state.registry.fail(session_id);
                    state.targets.remove(&session_id);
                    state.indicator_session = None;
                    indicator_animation.borrow_mut().stop();
                    if let Some(indicator) = indicator.upgrade() {
                        let _ = indicator.hide();
                    }
                    set_indicator_status(settings, &format!("Could not finish recording: {error}"));
                    return;
                }
                indicator_animation.borrow_mut().begin_processing();
                if let Some(indicator) = indicator.upgrade() {
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
                    let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                        transcript,
                        target,
                        add_trailing_space: false,
                    });
                }
                InsertLastResolution::Await(session_id) => {
                    set_indicator_status(
                        settings,
                        &format!(
                            "Dictation {} is still processing; it will be inserted here when ready.",
                            short_id(session_id)
                        ),
                    );
                }
                InsertLastResolution::LoadHistory => {
                    let _ = workers
                        .speech
                        .send(&SpeechWorkerCommand::GetHistory { limit: 20 });
                    set_indicator_status(settings, "Loading the latest transcript…");
                }
                InsertLastResolution::Unavailable(message) => {
                    set_indicator_status(settings, message);
                }
            }
        }
        InputHostEvent::SelectionCaptured {
            request_id,
            intent,
            target,
            text,
        } => {
            handle_text_action(
                request_id,
                intent,
                target,
                text,
                settings,
                chooser,
                action_palette,
                translation_preview,
                state,
                workers,
            );
        }
        InputHostEvent::SelectionCaptureFailed { message, .. } => {
            set_indicator_status(settings, &format!("Selection: {message}"));
        }
        InputHostEvent::InsertionDispatched {
            session_id,
            method,
            dispatch_duration_ms,
        } => {
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
                    true
                } else {
                    false
                };
                (owns_indicator, total, transcript_latency)
            };
            if owns_indicator {
                indicator_animation.borrow_mut().stop();
                if let Some(indicator) = indicator.upgrade() {
                    let _ = indicator.hide();
                }
            }
            let method_name = match method {
                rust_protocol::InsertionMethod::ClipboardPaste => "clipboard",
                rust_protocol::InsertionMethod::UnicodeInput => "unicode",
            };
            if let Some(total) = total_after_release_ms {
                let inference = transcript_latency.map_or(0, |latency| latency.inference_ms);
                let speculative =
                    transcript_latency.is_some_and(|latency| latency.speculative_reuse);
                eprintln!(
                    "latency session={} release_to_insert_ms={} insertion_dispatch_ms={} \
inference_ms={} speculative_reuse={} method={}",
                    session_id, total, dispatch_duration_ms, inference, speculative, method_name,
                );
            }
            set_indicator_status(
                settings,
                &format!(
                    "Inserted {} in {} ms",
                    short_id(session_id),
                    total_after_release_ms.unwrap_or(dispatch_duration_ms)
                ),
            );
        }
        InputHostEvent::ClipboardRestored { session_id: _ } => {}
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
            if state
                .borrow()
                .preview_translation
                .as_ref()
                .is_some_and(|pending| pending.request_id == request_id)
            {
                state.borrow_mut().preview_translation = None;
                if let Some(preview) = translation_preview.upgrade() {
                    let _ = preview.hide();
                }
            }
            set_indicator_status(settings, "Selected text was replaced.");
        }
        InputHostEvent::TextCopied { .. } => {
            set_indicator_status(settings, "Translation copied to the clipboard.");
        }
        InputHostEvent::Failed {
            session_id,
            code,
            message,
        } => {
            if code == "replace_selection"
                && let Some(preview) = translation_preview.upgrade()
            {
                preview.set_processing(false);
                preview
                    .set_direction_label("Target focus changed · copy the result or retry".into());
            }
            let owns_indicator = {
                let mut state = state.borrow_mut();
                if session_id.is_some() && state.indicator_session == session_id {
                    state.indicator_session = None;
                    true
                } else {
                    false
                }
            };
            if owns_indicator {
                indicator_animation.borrow_mut().stop();
                if let Some(indicator) = indicator.upgrade() {
                    let _ = indicator.hide();
                }
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
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    action_palette: &slint::Weak<ActionPalette>,
    translation_preview: &slint::Weak<TranslationPreview>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    match intent {
        TextActionIntent::CorrectLayout => {
            if state.borrow().layout_enabled {
                show_layout_chooser(request_id, target, text, settings, chooser, state);
            } else {
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
                },
                settings,
                translation_preview,
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
                        },
                        settings,
                        translation_preview,
                        state,
                        workers,
                    );
                } else {
                    show_layout_chooser(request_id, target, text, settings, chooser, state);
                }
                return;
            }

            state.borrow_mut().pending_text_action = Some(PendingTextAction {
                request_id,
                target,
                text: text.clone(),
            });
            if let Some(palette) = action_palette.upgrade() {
                palette.set_source_panel_height(adaptive_text_panel_height(
                    &text, 70, 2, 4, 15.0, 16.0,
                ));
                palette.set_source_text(text.into());
                palette.set_translation_enabled(translation_enabled);
                palette.set_layout_enabled(layout_enabled);
                let _ = palette.show();
                prepare_shown_action_window(
                    slint_window_handle(palette.window()),
                    ACTION_PALETTE_WINDOW_TITLE,
                    target.window_handle,
                );
            }
        }
    }
}

fn show_layout_chooser(
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    state: &Rc<RefCell<AppState>>,
) {
    let candidates = build_layout_candidates(&text);
    if candidates.is_empty() {
        set_indicator_status(settings, "No RU/EN layout conversion is available.");
        return;
    }
    if let Some(chooser) = chooser.upgrade() {
        chooser.set_source_panel_height(adaptive_text_panel_height(&text, 82, 2, 5, 15.0, 16.0));
        chooser.set_source_text(text.into());
        chooser.set_candidate_labels(ModelRc::new(VecModel::from(
            candidates
                .iter()
                .map(|candidate| match candidate.direction {
                    LayoutDirection::ToCyrillic => {
                        SharedString::from("English keys → Cyrillic layout")
                    }
                    LayoutDirection::ToLatin => SharedString::from("Cyrillic keys → Latin layout"),
                })
                .collect::<Vec<_>>(),
        )));
        chooser.set_candidate_texts(ModelRc::new(VecModel::from(
            candidates
                .iter()
                .map(|candidate| SharedString::from(&candidate.text))
                .collect::<Vec<_>>(),
        )));
        state.borrow_mut().pending_selection = Some(PendingSelection {
            request_id,
            target,
            candidates,
        });
        let _ = chooser.show();
        prepare_shown_action_window(
            slint_window_handle(chooser.window()),
            SELECTION_WINDOW_TITLE,
            target.window_handle,
        );
    }
}

fn begin_selection_translation(
    request: SelectionTranslationRequest,
    settings: &slint::Weak<SettingsWindow>,
    translation_preview: &slint::Weak<TranslationPreview>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    let SelectionTranslationRequest {
        request_id,
        target,
        text,
        delivery,
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
        if delivery == TranslationDelivery::Preview {
            show_selection_translation_preview(
                request_id,
                target,
                &text,
                "Translation is disabled in Settings.",
                "Unavailable",
                true,
                translation_preview,
            );
        }
        set_indicator_status(settings, "Translation is disabled in Actions.");
        return;
    }
    if selected_tier.is_none() {
        if delivery == TranslationDelivery::Preview {
            show_selection_translation_preview(
                request_id,
                target,
                &text,
                "Choose and download a translation model in Settings.",
                "Model required",
                true,
                translation_preview,
            );
        }
        set_indicator_status(
            settings,
            "Choose and download a translation model in Settings first.",
        );
        return;
    }
    if !ready {
        if delivery == TranslationDelivery::Preview {
            show_selection_translation_preview(
                request_id,
                target,
                &text,
                "Loading the local translation model…",
                "Preparing locally",
                true,
                translation_preview,
            );
        }
        state.borrow_mut().queued_selection_translation = Some(SelectionTranslationRequest {
            request_id,
            target,
            text,
            delivery,
        });
        set_indicator_status(settings, "The translation model is still loading.");
        return;
    }

    state.borrow_mut().pending_translations.insert(
        request_id,
        PendingTranslation {
            origin: TranslationOrigin::Selection { target, delivery },
        },
    );
    if delivery == TranslationDelivery::Preview {
        show_selection_translation_preview(
            request_id,
            target,
            &text,
            "Translating locally…",
            "Detecting language…",
            true,
            translation_preview,
        );
    }
    if let Err(error) = workers
        .translation
        .send(&TranslationWorkerCommand::Translate {
            request_id,
            text,
            source_language: None,
            target_language: None,
        })
    {
        state.borrow_mut().pending_translations.remove(&request_id);
        set_indicator_status(settings, &format!("Could not start translation: {error}"));
    } else {
        set_indicator_status(settings, "Translating locally…");
    }
}

fn show_selection_translation_preview(
    request_id: RequestId,
    target: TargetSnapshot,
    source_text: &str,
    result_text: &str,
    direction_label: &str,
    processing: bool,
    translation_preview: &slint::Weak<TranslationPreview>,
) {
    let Some(preview) = translation_preview.upgrade() else {
        return;
    };
    preview.set_request_id(request_id.to_string().into());
    apply_translation_preview_layout(&preview, source_text, result_text);
    preview.set_source_text(source_text.into());
    preview.set_translated_text(result_text.into());
    preview.set_direction_label(direction_label.into());
    preview.set_processing(processing);
    let _ = preview.show();
    prepare_shown_action_window(
        slint_window_handle(preview.window()),
        TRANSLATION_PREVIEW_WINDOW_TITLE,
        target.window_handle,
    );
}

fn handle_speech_event(
    event: SpeechWorkerEvent,
    settings: &slint::Weak<SettingsWindow>,
    indicator: &slint::Weak<RecordingIndicator>,
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
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::GetHistory { limit: 5 });
        }
        SpeechWorkerEvent::ModelDownloadProgress {
            catalog_id,
            downloaded_bytes,
            total_bytes,
        } => {
            let percent = downloaded_bytes
                .saturating_mul(100)
                .checked_div(total_bytes)
                .unwrap_or(0);
            set_runtime_status(settings, &format!("Model {percent}%"));
            set_indicator_status(settings, &format!("Installing {catalog_id}…"));
        }
        SpeechWorkerEvent::ModelInstalled { catalog_id, .. } => {
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
            {
                let mut state = state.borrow_mut();
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
            set_runtime_status(settings, "Ready");
            set_indicator_status(settings, "Ready · hold Ctrl + Alt + Space to speak.");
            eprintln!(
                "speech engine ready model={} backend={} load_ms={} warmup_ms={}",
                catalog_id, backend, load_duration_ms, warmup_duration_ms
            );
        }
        SpeechWorkerEvent::ModelSelected { catalog_id } => {
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
        SpeechWorkerEvent::AudioLevel { session_id, peak } => {
            if state.borrow().registry.active_session() == Some(session_id) {
                indicator_animation.borrow_mut().set_microphone_level(peak);
            }
        }
        SpeechWorkerEvent::Processing { session_id: _ } => {
            indicator_animation.borrow_mut().begin_processing();
            if let Some(indicator) = indicator.upgrade() {
                indicator.set_processing(true);
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
            let delivery = if stored {
                let mut state = state.borrow_mut();
                state.latest_history_session = Some(session_id);
                state.transcripts.insert(session_id, transcript.clone());
                state
                    .history
                    .retain(|entry| entry.transcript.session_id != session_id);
                state.history.insert(
                    0,
                    HistoryEntry {
                        transcript: transcript.clone(),
                        created_unix_ms,
                    },
                );
                state.history.truncate(250);
                state
                    .pending_insert_last
                    .take_if(|pending| pending.session_id == Some(session_id))
                    .map(|pending| (pending.target, false))
                    .or_else(|| {
                        state
                            .targets
                            .get(&session_id)
                            .copied()
                            .map(|target| (target, true))
                    })
            } else {
                None
            };
            sync_history(settings, state);
            if let Some((target, add_trailing_space)) = delivery {
                let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                    transcript,
                    target,
                    add_trailing_space,
                });
            } else {
                state.borrow_mut().indicator_session = None;
                indicator_animation.borrow_mut().stop();
                if let Some(indicator) = indicator.upgrade() {
                    let _ = indicator.hide();
                }
                set_indicator_status(settings, "Transcript saved to history; target was lost.");
            }
        }
        SpeechWorkerEvent::Cancelled { session_id } => {
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
                indicator_animation.borrow_mut().stop();
                if let Some(indicator) = indicator.upgrade() {
                    let _ = indicator.hide();
                }
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
                    let _ = workers.input.send(&InputHostCommand::InsertTranscript {
                        transcript,
                        target,
                        add_trailing_space: false,
                    });
                }
                Some((None, _)) => {
                    set_indicator_status(settings, "There is no transcript to insert yet.");
                }
                None => {}
            }
        }
        SpeechWorkerEvent::Failed {
            session_id,
            code,
            message,
            ..
        } => {
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
                indicator_animation.borrow_mut().stop();
                if let Some(indicator) = indicator.upgrade() {
                    let _ = indicator.hide();
                }
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
    translation_preview: &slint::Weak<TranslationPreview>,
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
            {
                let mut state = state.borrow_mut();
                state.translation_models = models;
                state.selected_translation_tier = selected_tier;
                state.translation_ready = false;
            }
            sync_translation_models(settings, state);
            sync_action_settings(settings, state);
            if selected_tier.is_none() {
                set_translation_status(
                    translator,
                    "No model installed. Choose Tiny or Teacher in Settings.",
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
            set_indicator_status(settings, &format!("Downloading {catalog_id}: {percent}%"));
            set_translation_status(translator, &format!("Downloading model… {percent}%"));
        }
        TranslationWorkerEvent::ModelInstalled {
            catalog_id, tier, ..
        } => {
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
                begin_selection_translation(request, settings, translation_preview, state, workers);
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
                translation_preview,
                translator,
                state,
                workers,
            );
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
            let pending =
                request_id.and_then(|id| state.borrow_mut().pending_translations.remove(&id));
            let queued_request_id = if request_id.is_none() {
                state
                    .borrow_mut()
                    .queued_selection_translation
                    .take()
                    .map(|request| request.request_id)
            } else {
                None
            };
            if matches!(
                pending.as_ref().map(|pending| pending.origin),
                Some(TranslationOrigin::Translator)
            ) {
                set_translation_status(translator, &format!("{code}: {message}"));
            }
            if let Some(preview) = translation_preview.upgrade()
                && request_id
                    .or(queued_request_id)
                    .map(|id| preview.get_request_id() == id.to_string())
                    .unwrap_or(false)
            {
                preview.set_processing(true);
                preview.set_direction_label("Translation failed".into());
                let error_text = format!("{code}: {message}");
                let source_text = preview.get_source_text().to_string();
                apply_translation_preview_layout(&preview, &source_text, &error_text);
                preview.set_translated_text(error_text.into());
            }
            set_indicator_status(settings, &format!("{code}: {message}"));
        }
    }
}

fn route_translation_result(
    result: TranslationResult,
    pending: Option<PendingTranslation>,
    settings: &slint::Weak<SettingsWindow>,
    translation_preview: &slint::Weak<TranslationPreview>,
    translator: &slint::Weak<TranslatorWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    let Some(pending) = pending else {
        return;
    };
    match pending.origin {
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
        }
        TranslationOrigin::Selection { target, delivery } => match delivery {
            TranslationDelivery::Preview => {
                state.borrow_mut().preview_translation = Some(PreviewTranslation {
                    request_id: result.request_id,
                    target,
                    text: result.translated_text.clone(),
                });
                if let Some(preview) = translation_preview.upgrade() {
                    preview.set_request_id(result.request_id.to_string().into());
                    apply_translation_preview_layout(
                        &preview,
                        &result.source_text,
                        &result.translated_text,
                    );
                    preview.set_source_text(result.source_text.into());
                    preview.set_translated_text(result.translated_text.into());
                    preview.set_processing(false);
                    preview.set_direction_label(
                        format!(
                            "{} → {} · {} ms",
                            result.source_language.code().to_uppercase(),
                            result.target_language.code().to_uppercase(),
                            result.processing_duration_ms
                        )
                        .into(),
                    );
                }
            }
            TranslationDelivery::Replace => {
                let _ = workers.input.send(&InputHostCommand::ReplaceSelection {
                    request_id: result.request_id,
                    target,
                    text: result.translated_text,
                });
            }
            TranslationDelivery::Copy => {
                let _ = workers.input.send(&InputHostCommand::CopyText {
                    request_id: result.request_id,
                    text: result.translated_text,
                });
            }
        },
    }
    set_indicator_status(settings, "Translation completed locally.");
}

fn wire_settings(
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    {
        let translator = translator.as_weak();
        settings.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                translator.window().request_redraw();
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_select_model(move |index| {
            let Some(model) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().models.get(index).cloned())
            else {
                return;
            };
            let reset_output_mode = {
                let mut state = state.borrow_mut();
                state.selected_model_id.clone_from(&model.catalog_id);
                state.speech_ready = false;
                if !model.supports_speech_translation
                    && state.output_mode == SpeechOutputMode::TranslateToEnglish
                {
                    state.output_mode = SpeechOutputMode::OriginalLanguage;
                    true
                } else {
                    false
                }
            };
            if reset_output_mode {
                let _ = workers.speech.send(&SpeechWorkerCommand::SetOutputMode {
                    output_mode: SpeechOutputMode::OriginalLanguage,
                });
                sync_output_mode(&settings_weak, &state);
            }
            let _ = save_settings(&state.borrow());
            let _ = workers.speech.send(&SpeechWorkerCommand::SelectModel {
                catalog_id: model.catalog_id,
            });
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_refresh_history(move || {
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::GetHistory { limit: 5 });
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_history_enabled(move || {
            let enabled = {
                let mut state = state.borrow_mut();
                state.history_enabled = !state.history_enabled;
                state.history_enabled
            };
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetHistoryEnabled { enabled });
            let _ = workers
                .translation
                .send(&TranslationWorkerCommand::SetHistoryEnabled { enabled });
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let indicator_animation = Rc::clone(indicator_animation);
        settings.on_cycle_visualizer(move || {
            let style = {
                let mut state = state.borrow_mut();
                state.visualizer_style = state.visualizer_style.next();
                state.visualizer_style
            };
            indicator_animation.borrow_mut().set_style(style);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Visualizer updated. The next dictation uses the selected style.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_clear_history(move || {
            let _ = workers.speech.send(&SpeechWorkerCommand::ClearHistory);
            let _ = workers
                .translation
                .send(&TranslationWorkerCommand::ClearHistory);
            {
                let mut state = state.borrow_mut();
                state.history.clear();
                state.translation_history.clear();
                state.transcripts.clear();
                state.latest_history_session = None;
                state.history_loaded = true;
            }
            sync_history(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Local voice and translation history cleared.",
            );
        });
    }
    {
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_copy_history(move |index| {
            let Some(transcript) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().history.get(index).cloned())
                .map(|entry| entry.transcript)
            else {
                return;
            };
            let _ = workers
                .input
                .send(&InputHostCommand::CopyTranscript { transcript });
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_output_mode(move || {
            let supports_translation = {
                let state = state.borrow();
                state
                    .models
                    .iter()
                    .find(|model| model.catalog_id == state.selected_model_id)
                    .is_none_or(|model| model.supports_speech_translation)
            };
            if !supports_translation {
                set_indicator_status(
                    &settings_weak,
                    "Whisper Turbo is transcription-only. Translation will use the separate selection translator.",
                );
                return;
            }
            let output_mode = {
                let mut state = state.borrow_mut();
                state.output_mode = match state.output_mode {
                    SpeechOutputMode::OriginalLanguage => SpeechOutputMode::TranslateToEnglish,
                    SpeechOutputMode::TranslateToEnglish => SpeechOutputMode::OriginalLanguage,
                };
                state.output_mode
            };
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetOutputMode { output_mode });
            let _ = save_settings(&state.borrow());
            sync_output_mode(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_release_optimization(move || {
            let enabled = {
                let mut state = state.borrow_mut();
                state.fast_release_previews = !state.fast_release_previews;
                state.fast_release_previews
            };
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetFastReleasePreviews { enabled });
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_select_translation_model(move |index| {
            let Some(model) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().translation_models.get(index).cloned())
            else {
                return;
            };
            {
                let mut state = state.borrow_mut();
                state.translation_enabled = true;
                state.translation_ready = false;
            }
            let command = if model.installed {
                TranslationWorkerCommand::SelectTier {
                    tier: model.tier,
                    keep_warm: state.borrow().translation_keep_warm,
                }
            } else {
                TranslationWorkerCommand::EnsureModel { tier: model.tier }
            };
            if let Err(error) = workers.translation.send(&command) {
                set_indicator_status(
                    &settings_weak,
                    &format!("Could not configure translation model: {error}"),
                );
            } else {
                set_indicator_status(
                    &settings_weak,
                    if model.installed {
                        "Loading the selected translation model…"
                    } else {
                        "Downloading and verifying the translation model…"
                    },
                );
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_toggle_translation_enabled(move || {
            {
                let mut state = state.borrow_mut();
                state.translation_enabled = !state.translation_enabled;
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_toggle_layout_enabled(move || {
            {
                let mut state = state.borrow_mut();
                state.layout_enabled = !state.layout_enabled;
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_cycle_text_action_mode(move || {
            {
                let mut state = state.borrow_mut();
                state.text_action_mode = match state.text_action_mode {
                    TextActionMode::Adaptive => TextActionMode::AlwaysPalette,
                    TextActionMode::AlwaysPalette => TextActionMode::Adaptive,
                };
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_cycle_translation_delivery(move || {
            {
                let mut state = state.borrow_mut();
                state.translation_delivery = match state.translation_delivery {
                    TranslationDelivery::Preview => TranslationDelivery::Replace,
                    TranslationDelivery::Replace => TranslationDelivery::Copy,
                    TranslationDelivery::Copy => TranslationDelivery::Preview,
                };
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_translation_warm(move || {
            let selected = {
                let mut state = state.borrow_mut();
                state.translation_keep_warm = !state.translation_keep_warm;
                state.selected_translation_tier
            };
            if let Some(tier) = selected {
                state.borrow_mut().translation_ready = false;
                let _ = workers
                    .translation
                    .send(&TranslationWorkerCommand::SelectTier {
                        tier,
                        keep_warm: state.borrow().translation_keep_warm,
                    });
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_quit_requested(move || {
            workers.shutdown();
            let _ = slint::quit_event_loop();
        });
    }
}

fn wire_tray(
    tray: &AppTray,
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let settings = settings.as_weak();
        tray.on_open_settings(move || {
            if let Some(settings) = settings.upgrade() {
                let _ = settings.show();
                settings.window().request_redraw();
            }
        });
    }
    {
        let translator = translator.as_weak();
        tray.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                translator.window().request_redraw();
            }
        });
    }
    {
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        tray.on_copy_last(move || {
            if let Some(transcript) = state.borrow().latest_transcript().cloned() {
                let _ = workers
                    .input
                    .send(&InputHostCommand::CopyTranscript { transcript });
            }
        });
    }
    {
        let workers = Rc::clone(workers);
        tray.on_quit_requested(move || {
            workers.shutdown();
            let _ = slint::quit_event_loop();
        });
    }
}

fn wire_chooser(chooser: &SelectionChooser, state: &Rc<RefCell<AppState>>, workers: &Rc<Workers>) {
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        chooser.on_choose_candidate(move |index| {
            let pending = state.borrow_mut().pending_selection.take();
            let Some(pending) = pending else {
                return;
            };
            let Some(candidate) = usize::try_from(index)
                .ok()
                .and_then(|index| pending.candidates.get(index))
            else {
                return;
            };
            if let Some(chooser) = chooser_weak.upgrade() {
                let _ = chooser.hide();
            }
            let _ = workers.input.send(&InputHostCommand::ReplaceSelection {
                request_id: pending.request_id,
                target: pending.target,
                text: candidate.text.clone(),
            });
        });
    }
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        chooser.on_cancel(move || {
            if let Some(pending) = state.borrow_mut().pending_selection.take() {
                let _ = workers.input.send(&InputHostCommand::CancelSelection {
                    request_id: pending.request_id,
                });
            }
            if let Some(chooser) = chooser_weak.upgrade() {
                let _ = chooser.hide();
            }
        });
    }
}

fn wire_action_palette(
    palette: &ActionPalette,
    translation_preview: &TranslationPreview,
    chooser: &SelectionChooser,
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let palette_weak = palette.as_weak();
        let preview = translation_preview.as_weak();
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        palette.on_translate(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                let _ = palette.hide();
            }
            let Some(pending) = pending else {
                return;
            };
            let delivery = state.borrow().translation_delivery;
            begin_selection_translation(
                SelectionTranslationRequest {
                    request_id: pending.request_id,
                    target: pending.target,
                    text: pending.text,
                    delivery,
                },
                &settings,
                &preview,
                &state,
                &workers,
            );
        });
    }
    {
        let palette_weak = palette.as_weak();
        let chooser = chooser.as_weak();
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        palette.on_correct_layout(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                let _ = palette.hide();
            }
            let Some(pending) = pending else {
                return;
            };
            show_layout_chooser(
                pending.request_id,
                pending.target,
                pending.text,
                &settings,
                &chooser,
                &state,
            );
        });
    }
    {
        let palette_weak = palette.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        palette.on_cancel(move || {
            if let Some(pending) = state.borrow_mut().pending_text_action.take() {
                let _ = workers.input.send(&InputHostCommand::CancelSelection {
                    request_id: pending.request_id,
                });
            }
            if let Some(palette) = palette_weak.upgrade() {
                let _ = palette.hide();
            }
        });
    }
}

fn wire_translation_preview(
    preview: &TranslationPreview,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let preview_weak = preview.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        preview.on_replace(move || {
            let pending = state.borrow().preview_translation.clone();
            let Some(pending) = pending else {
                return;
            };
            if let Some(preview) = preview_weak.upgrade() {
                preview.set_processing(true);
                preview.set_direction_label("Replacing in the original selection…".into());
            }
            let _ = workers.input.send(&InputHostCommand::ReplaceSelection {
                request_id: pending.request_id,
                target: pending.target,
                text: pending.text,
            });
        });
    }
    {
        let preview_weak = preview.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        preview.on_copy(move || {
            let pending = state.borrow_mut().preview_translation.take();
            let Some(pending) = pending else {
                return;
            };
            if let Some(preview) = preview_weak.upgrade() {
                let _ = preview.hide();
            }
            let _ = workers.input.send(&InputHostCommand::CopyText {
                request_id: pending.request_id,
                text: pending.text,
            });
        });
    }
    {
        let preview_weak = preview.as_weak();
        let state = Rc::clone(state);
        preview.on_dismiss(move || {
            state.borrow_mut().preview_translation = None;
            if let Some(preview) = preview_weak.upgrade() {
                let _ = preview.hide();
            }
        });
    }
}

fn wire_translator(
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let translator_weak = translator.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        translator.on_translate(move || {
            let Some(translator) = translator_weak.upgrade() else {
                return;
            };
            let text = translator.get_source_text().trim().to_owned();
            if text.is_empty() {
                translator.set_status_text("Enter text to translate.".into());
                return;
            }
            let (selected_tier, ready) = {
                let state = state.borrow();
                (state.selected_translation_tier, state.translation_ready)
            };
            if selected_tier.is_none() {
                translator.set_status_text(
                    "No model installed. Choose Tiny or Teacher in Settings.".into(),
                );
                return;
            }
            if !ready {
                translator.set_status_text("Translation model is still loading.".into());
                return;
            }
            let request_id = RequestId::new();
            state.borrow_mut().pending_translations.insert(
                request_id,
                PendingTranslation {
                    origin: TranslationOrigin::Translator,
                },
            );
            translator.set_processing(true);
            translator.set_result_text("".into());
            translator.set_status_text("Translating locally…".into());
            if let Err(error) = workers
                .translation
                .send(&TranslationWorkerCommand::Translate {
                    request_id,
                    text,
                    source_language: None,
                    target_language: None,
                })
            {
                state.borrow_mut().pending_translations.remove(&request_id);
                translator.set_processing(false);
                translator.set_status_text(format!("Could not translate: {error}").into());
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        let workers = Rc::clone(workers);
        translator.on_copy_result(move || {
            let Some(translator) = translator_weak.upgrade() else {
                return;
            };
            let text = translator.get_result_text().to_string();
            if !text.trim().is_empty() {
                let _ = workers.input.send(&InputHostCommand::CopyText {
                    request_id: RequestId::new(),
                    text,
                });
                translator.set_status_text("Copied to clipboard.".into());
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        translator.on_swap_text(move || {
            if let Some(translator) = translator_weak.upgrade() {
                let source = translator.get_source_text();
                let result = translator.get_result_text();
                translator.set_source_text(result);
                translator.set_result_text(source);
                translator
                    .set_status_text("Direction will be detected again when you translate.".into());
            }
        });
    }
}

fn sync_models(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = state
        .borrow()
        .models
        .iter()
        .map(|model| ModelRow {
            title: model.display_name.clone().into(),
            detail: format!(
                "{} · {:.1} GB{}",
                model.description,
                model.size_bytes as f64 / 1_000_000_000.0,
                if model.recommended {
                    " · recommended"
                } else {
                    ""
                }
            )
            .into(),
            state: if model.installed {
                "Installed".into()
            } else {
                "Download".into()
            },
            selected: model.selected,
        })
        .collect::<Vec<_>>();
    settings.set_model_rows(ModelRc::new(VecModel::from(rows)));
}

fn sync_translation_models(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = state
        .borrow()
        .translation_models
        .iter()
        .map(|model| ModelRow {
            title: model.display_name.clone().into(),
            detail: format!(
                "{} · {:.0} MB · {}{}",
                model.description,
                model.size_bytes as f64 / 1_000_000.0,
                model.license,
                if model.recommended {
                    " · recommended for your system language"
                } else {
                    ""
                }
            )
            .into(),
            state: if model.installed {
                "Installed".into()
            } else {
                "Download".into()
            },
            selected: model.selected,
        })
        .collect::<Vec<_>>();
    settings.set_translation_model_rows(ModelRc::new(VecModel::from(rows)));
}

fn sync_action_settings(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    settings.set_translation_enabled_label(
        if state.translation_enabled {
            "Translation: enabled"
        } else {
            "Translation: disabled"
        }
        .into(),
    );
    settings.set_layout_enabled_label(
        if state.layout_enabled {
            "Layout correction: enabled"
        } else {
            "Layout correction: disabled"
        }
        .into(),
    );
    settings.set_text_action_mode_label(
        match state.text_action_mode {
            TextActionMode::Adaptive => "Ctrl+Alt+T: adaptive",
            TextActionMode::AlwaysPalette => "Ctrl+Alt+T: always show palette",
        }
        .into(),
    );
    settings.set_translation_delivery_label(
        match state.translation_delivery {
            TranslationDelivery::Preview => "Translation result: preview",
            TranslationDelivery::Replace => "Translation result: replace selection",
            TranslationDelivery::Copy => "Translation result: copy",
        }
        .into(),
    );
    settings.set_translation_warm_label(
        if state.translation_keep_warm {
            "Warm engine: on (fastest)"
        } else {
            "Warm engine: off (less memory)"
        }
        .into(),
    );
    settings.set_release_optimization_label(
        if state.fast_release_previews {
            "Release optimization: fastest"
        } else {
            "Release optimization: efficient"
        }
        .into(),
    );
    settings.set_visualizer_label(state.visualizer_style.label().into());
    settings.set_translation_ready(state.translation_ready);
    settings.set_history_enabled_label(
        if state.history_enabled {
            "History: enabled"
        } else {
            "History: disabled"
        }
        .into(),
    );
}

fn sync_history(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = state
        .borrow()
        .history
        .iter()
        .map(|entry| HistoryRow {
            text: entry.transcript.text.clone().into(),
            detail: if entry.transcript.latency.after_release_ms > 0 {
                format!(
                    "{} · {:.1}s audio · {}ms release{}",
                    entry.transcript.language,
                    entry.transcript.audio_duration_ms as f64 / 1000.0,
                    entry.transcript.latency.after_release_ms,
                    if entry.transcript.latency.speculative_reuse {
                        " · prepared"
                    } else {
                        ""
                    }
                )
                .into()
            } else {
                format!(
                    "{} · {:.1}s audio · {}ms processing",
                    entry.transcript.language,
                    entry.transcript.audio_duration_ms as f64 / 1000.0,
                    entry.transcript.processing_duration_ms
                )
                .into()
            },
        })
        .collect::<Vec<_>>();
    settings.set_history_rows(ModelRc::new(VecModel::from(rows)));
}

fn sync_output_mode(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    if let Some(settings) = settings.upgrade() {
        settings.set_output_mode_label(
            match state.borrow().output_mode {
                SpeechOutputMode::OriginalLanguage => "Output: original language",
                SpeechOutputMode::TranslateToEnglish => "Output: translate to English",
            }
            .into(),
        );
    }
}

fn set_runtime_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    if let Some(settings) = settings.upgrade() {
        settings.set_runtime_status(message.into());
    }
}

fn set_indicator_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    if let Some(settings) = settings.upgrade() {
        settings.set_indicator_status(message.into());
    }
}

fn adaptive_text_panel_height(
    text: &str,
    characters_per_line: usize,
    minimum_lines: usize,
    maximum_lines: usize,
    line_height: f32,
    vertical_padding: f32,
) -> f32 {
    let estimated_lines =
        estimated_wrapped_lines(text, characters_per_line).clamp(minimum_lines, maximum_lines);
    estimated_lines as f32 * line_height + vertical_padding
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TranslationPreviewLayout {
    card_width: f32,
    source_panel_height: f32,
    result_panel_height: f32,
    result_content_height: f32,
    result_scrollable: bool,
}

fn translation_preview_layout(source_text: &str, result_text: &str) -> TranslationPreviewLayout {
    let content_characters = source_text.chars().count().max(result_text.chars().count());
    let (card_width, characters_per_line) = match content_characters {
        0..=48 => (360.0, 42),
        49..=180 => (480.0, 62),
        _ => (620.0, 82),
    };

    let source_lines = estimated_wrapped_lines(source_text, characters_per_line).clamp(1, 3);
    let result_lines = estimated_wrapped_lines(result_text, characters_per_line).max(1);
    let source_panel_height = source_lines as f32 * 14.0 + 12.0;
    let result_text_height = result_lines as f32 * 18.0;
    let result_panel_height = (result_text_height + 14.0).clamp(44.0, 300.0);
    let visible_result_height = (result_panel_height - 14.0).max(18.0);
    let result_content_height = result_text_height.max(visible_result_height);

    TranslationPreviewLayout {
        card_width,
        source_panel_height,
        result_panel_height,
        result_content_height,
        result_scrollable: result_content_height > visible_result_height + 0.5,
    }
}

fn apply_translation_preview_layout(
    preview: &TranslationPreview,
    source_text: &str,
    result_text: &str,
) {
    let layout = translation_preview_layout(source_text, result_text);
    preview.set_card_width(layout.card_width);
    preview.set_source_panel_height(layout.source_panel_height);
    preview.set_result_panel_height(layout.result_panel_height);
    preview.set_result_content_height(layout.result_content_height);
    preview.set_result_scrollable(layout.result_scrollable);
}

fn estimated_wrapped_lines(text: &str, characters_per_line: usize) -> usize {
    text.split('\n')
        .map(|line| {
            let characters = line.chars().count().max(1);
            characters.div_ceil(characters_per_line)
        })
        .sum()
}

fn set_translation_status(translator: &slint::Weak<TranslatorWindow>, message: &str) {
    if let Some(translator) = translator.upgrade() {
        translator.set_status_text(message.into());
    }
}

fn prepare_shown_action_window(
    window_handle: Option<u64>,
    window_title: &'static str,
    target_window_handle: u64,
) {
    // winit can restore its default extended styles when a hidden window is
    // shown again. Queue native finalization behind the current UI operation
    // so it runs after winit has committed visibility and cannot be overwritten
    // by that same show transaction.
    if let Err(error) = slint::invoke_from_event_loop(move || {
        if let Err(error) = window_handle
            .ok_or_else(|| "native window handle is unavailable".to_owned())
            .and_then(rust_platform_windows::configure_action_window)
        {
            eprintln!("could not configure `{window_title}` as an action window: {error}");
        }
        if let Err(error) = rust_platform_windows::restore_target_if_our_action_window_activated(
            target_window_handle,
        ) {
            eprintln!("could not restore action target for `{window_title}`: {error}");
        }
    }) {
        eprintln!("could not finalize `{window_title}` after showing it: {error}");
    }
}

fn precreate_action_windows(
    chooser: &SelectionChooser,
    palette: &ActionPalette,
    preview: &TranslationPreview,
) {
    chooser.set_prepared(false);
    if chooser.show().is_ok() {
        let chooser = chooser.as_weak();
        let _ = slint::spawn_local(async move {
            if let Some(chooser) = chooser.upgrade() {
                let configured = chooser
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::configure_action_window(handle).is_ok()
                    });
                if !configured {
                    eprintln!("selection action window could not be configured as no-activate");
                }
                let _ = chooser.hide();
                chooser.set_prepared(true);
            }
        });
    }

    palette.set_prepared(false);
    if palette.show().is_ok() {
        let palette = palette.as_weak();
        let _ = slint::spawn_local(async move {
            if let Some(palette) = palette.upgrade() {
                let configured = palette
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::configure_action_window(handle).is_ok()
                    });
                if !configured {
                    eprintln!("text action palette could not be configured as no-activate");
                }
                let _ = palette.hide();
                palette.set_prepared(true);
            }
        });
    }

    preview.set_prepared(false);
    if preview.show().is_ok() {
        let preview = preview.as_weak();
        let _ = slint::spawn_local(async move {
            if let Some(preview) = preview.upgrade() {
                let configured = preview
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::configure_action_window(handle).is_ok()
                    });
                if !configured {
                    eprintln!("translation preview could not be configured as no-activate");
                }
                let _ = preview.hide();
                preview.set_prepared(true);
            }
        });
    }
}

fn precreate_indicator(indicator: &RecordingIndicator) {
    indicator.set_prepared(false);
    if indicator.show().is_ok() {
        let indicator = indicator.as_weak();
        let fallback_indicator = indicator.clone();
        let schedule_result = slint::spawn_local(async move {
            if let Some(indicator) = indicator.upgrade() {
                let prepared = match indicator.window().winit_window().await {
                    Ok(window) => native_window_handle(&window)
                        .is_some_and(|handle| configure_indicator_native_window(handle, None)),
                    Err(_) => false,
                };
                let _ = indicator.hide();
                indicator.set_prepared(prepared);
            }
        });
        if schedule_result.is_err()
            && let Some(indicator) = fallback_indicator.upgrade()
        {
            let _ = indicator.hide();
        }
    }
}

fn show_indicator(indicator: &RecordingIndicator, target_window_handle: Option<u64>) {
    let prepared = configure_indicator_window(indicator, target_window_handle);
    indicator.set_prepared(prepared);
    if prepared {
        let _ = indicator.show();
    } else {
        eprintln!("recording indicator was not shown because native setup failed");
    }
}

fn configure_indicator_window(
    indicator: &RecordingIndicator,
    target_window_handle: Option<u64>,
) -> bool {
    slint_window_handle(indicator.window())
        .is_some_and(|handle| configure_indicator_native_window(handle, target_window_handle))
}

fn configure_indicator_native_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
) -> bool {
    rust_platform_windows::configure_overlay_window(window_handle, target_window_handle).is_ok()
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

fn data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(".rust-voice-data")
        },
        |root| Path::new(&root).join("KeyboopRust"),
    )
}

fn settings_path() -> PathBuf {
    data_root().join("settings.json")
}

struct SavedSettings {
    selected_model_id: Option<String>,
    output_mode: SpeechOutputMode,
    fast_release_previews: bool,
    visualizer_style: VisualizerStyle,
    selected_translation_tier: Option<TranslationTier>,
    translation_enabled: bool,
    layout_enabled: bool,
    translation_keep_warm: bool,
    text_action_mode: TextActionMode,
    translation_delivery: TranslationDelivery,
    history_enabled: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        Self {
            selected_model_id: None,
            output_mode: SpeechOutputMode::default(),
            fast_release_previews: true,
            visualizer_style: VisualizerStyle::default(),
            selected_translation_tier: None,
            translation_enabled: false,
            layout_enabled: true,
            translation_keep_warm: true,
            text_action_mode: TextActionMode::default(),
            translation_delivery: TranslationDelivery::default(),
            history_enabled: true,
        }
    }
}

fn load_settings() -> SavedSettings {
    let value = fs::read(settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let selected_model_id = value
        .as_ref()
        .and_then(|value| value.get("selected_model_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let output_mode = value
        .as_ref()
        .and_then(|value| value.get("output_mode"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let fast_release_previews = value
        .as_ref()
        .and_then(|value| value.get("fast_release_previews"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let visualizer_style = value
        .as_ref()
        .and_then(|value| value.get("visualizer_style"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let selected_translation_tier = value
        .as_ref()
        .and_then(|value| value.get("selected_translation_tier"))
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let translation_enabled = value
        .as_ref()
        .and_then(|value| value.get("translation_enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let layout_enabled = value
        .as_ref()
        .and_then(|value| value.get("layout_enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let translation_keep_warm = value
        .as_ref()
        .and_then(|value| value.get("translation_keep_warm"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let text_action_mode = value
        .as_ref()
        .and_then(|value| value.get("text_action_mode"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let translation_delivery = value
        .as_ref()
        .and_then(|value| value.get("translation_delivery"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let history_enabled = value
        .as_ref()
        .and_then(|value| value.get("history_enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    SavedSettings {
        selected_model_id,
        output_mode,
        fast_release_previews,
        visualizer_style,
        selected_translation_tier,
        translation_enabled,
        layout_enabled,
        translation_keep_warm,
        text_action_mode,
        translation_delivery,
        history_enabled,
    }
}

fn save_settings(state: &AppState) -> Result<(), Box<dyn Error>> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "selected_model_id": &state.selected_model_id,
        "output_mode": state.output_mode,
        "fast_release_previews": state.fast_release_previews,
        "visualizer_style": state.visualizer_style,
        "selected_translation_tier": state.selected_translation_tier,
        "translation_enabled": state.translation_enabled,
        "layout_enabled": state.layout_enabled,
        "translation_keep_warm": state.translation_keep_warm,
        "text_action_mode": state.text_action_mode,
        "translation_delivery": state.translation_delivery,
        "history_enabled": state.history_enabled,
    }))?;
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("previous");
    fs::write(&temporary, bytes)?;
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    let had_previous = path.exists();
    if had_previous {
        fs::rename(&path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        if had_previous {
            let _ = fs::rename(&backup, &path);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod indicator_animation_tests {
    use std::rc::Rc;

    use slint::VecModel;

    use super::IndicatorAnimation;

    fn animation() -> IndicatorAnimation {
        IndicatorAnimation::new(Rc::new(VecModel::from(vec![0.0; 10])))
    }

    #[test]
    fn listening_frame_has_independent_bar_motion() {
        let mut animation = animation();
        animation.begin_listening();
        animation.set_microphone_level(0.55);
        let frame = animation.advance_frame(1.0);

        assert_eq!(frame.heights.len(), 10);
        assert!(
            frame
                .heights
                .windows(2)
                .any(|pair| (pair[0] - pair[1]).abs() > 0.05)
        );
        assert!(frame.signal_opacity > 0.5);
        assert_eq!(frame.disc_opacity, 1.0);
    }

    #[test]
    fn processing_frames_keep_moving_until_completion() {
        let mut animation = animation();
        animation.begin_processing();
        let first = animation.advance_frame(1.0);
        let second = animation.advance_frame(1.0);

        assert_ne!(first.heights, second.heights);
        assert_ne!(first.disc_opacity, second.disc_opacity);
    }
}

#[cfg(test)]
mod adaptive_popup_tests {
    use super::{adaptive_text_panel_height, translation_preview_layout};

    #[test]
    fn short_text_uses_the_compact_minimum() {
        assert_eq!(
            adaptive_text_panel_height("Short", 70, 2, 4, 15.0, 16.0),
            46.0
        );
    }

    #[test]
    fn wrapped_text_grows_until_the_defined_cap() {
        let medium = adaptive_text_panel_height(&"word ".repeat(50), 70, 2, 4, 15.0, 16.0);
        let very_long = adaptive_text_panel_height(&"word ".repeat(500), 70, 2, 4, 15.0, 16.0);

        assert_eq!(medium, 76.0);
        assert_eq!(very_long, 76.0);
    }

    #[test]
    fn explicit_lines_are_included_in_the_estimate() {
        assert_eq!(
            adaptive_text_panel_height("one\ntwo\nthree", 70, 1, 6, 15.0, 16.0),
            61.0
        );
    }

    #[test]
    fn short_translation_stays_compact_without_a_scrollbar() {
        let layout = translation_preview_layout("Hello", "Привет");
        assert_eq!(layout.card_width, 360.0);
        assert!(!layout.result_scrollable);
        assert_eq!(layout.result_panel_height, 44.0);
    }

    #[test]
    fn long_translation_is_capped_and_becomes_scrollable() {
        let layout = translation_preview_layout("Source", &"translated text ".repeat(500));
        assert_eq!(layout.card_width, 620.0);
        assert_eq!(layout.result_panel_height, 300.0);
        assert!(layout.result_content_height > layout.result_panel_height);
        assert!(layout.result_scrollable);
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
}
