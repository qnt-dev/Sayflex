#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    error::Error,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rust_app_core::{
    BeginDisposition, EndDisposition, LayoutCandidate, LayoutDirection, SessionPhase,
    SessionRegistry, build_layout_candidates,
};
use rust_protocol::{
    AUDIO_SPECTRUM_BANDS, HistoryEntry, InputHostCommand, InputHostEvent, ModelSummary,
    PROTOCOL_VERSION, RequestId, SessionId, SpeechOutputMode, SpeechWorkerCommand,
    SpeechWorkerEvent, TargetSnapshot, TextActionIntent, Transcript, TranslationHistoryEntry,
    TranslationModelSummary, TranslationResult, TranslationTier, TranslationWorkerCommand,
    TranslationWorkerEvent,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

mod flow_sphere;

slint::include_modules!();

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const INPUT_WORKER_EXE: &str = "StyleFlex.InputHost.exe";
const SPEECH_WORKER_EXE: &str = "StyleFlex.Speech.exe";
const TRANSLATION_WORKER_EXE: &str = "StyleFlex.Translation.exe";
const SPEECH_CPU_EXE: &str = "StyleFlex.Speech.Cpu.exe";
const SPEECH_CUDA_EXE: &str = "StyleFlex.Speech.Cuda.exe";
const DEV_INPUT_WORKER_EXE: &str = "styleflex-input-host.exe";
const DEV_SPEECH_WORKER_EXE: &str = "styleflex-speech-worker.exe";
const DEV_TRANSLATION_WORKER_EXE: &str = "styleflex-translation-worker.exe";
const SELECTION_WINDOW_TITLE: &str = "Correct keyboard layout";
const ACTION_PALETTE_WINDOW_TITLE: &str = "Text actions";
const TRANSLATION_PREVIEW_WINDOW_TITLE: &str = "Translation preview";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INDICATOR_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const INDICATOR_BAR_PROFILE: [f32; 10] = [0.25, 0.4, 0.62, 0.82, 1.0, 1.0, 0.82, 0.62, 0.4, 0.25];
const WORKER_RESTART_WINDOW: Duration = Duration::from_secs(60);
const WORKER_STARTUP_STABILITY: Duration = Duration::from_secs(2);
static NEXT_WORKER_INSTANCE: AtomicU64 = AtomicU64::new(1);

enum RuntimeEvent {
    Input(InputHostEvent),
    Speech(SpeechWorkerEvent),
    Translation(TranslationWorkerEvent),
    DismissTranslationPreview,
    TranslationPreviewPinChanged(bool),
    ScrollTranslationPreview(i32),
    SessionEnd(rust_platform_windows::SessionEndEvent),
    WorkerError(String),
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

struct WorkerProcess {
    child: Child,
    instance_id: u64,
}

struct InitialWorkers {
    job: rust_platform_windows::WorkerJob,
    input_child: WorkerProcess,
    input: JsonWriter,
    speech_child: WorkerProcess,
    speech: JsonWriter,
    translation_child: WorkerProcess,
    translation: JsonWriter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerLifecycle {
    Running,
    SessionEnding,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupSessionPhase {
    Running,
    Query,
    Confirmed,
}

struct StartupSessionGate {
    phase: Mutex<StartupSessionPhase>,
    changed: Condvar,
}

impl StartupSessionGate {
    fn new() -> Self {
        Self {
            phase: Mutex::new(StartupSessionPhase::Running),
            changed: Condvar::new(),
        }
    }

    fn observe(&self, event: rust_platform_windows::SessionEndEvent) {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *phase = match event {
            rust_platform_windows::SessionEndEvent::Query => StartupSessionPhase::Query,
            rust_platform_windows::SessionEndEvent::Confirmed => StartupSessionPhase::Confirmed,
            rust_platform_windows::SessionEndEvent::Cancelled => StartupSessionPhase::Running,
        };
        self.changed.notify_all();
    }

    fn wait_until_worker_start_is_allowed(&self) -> bool {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            match *phase {
                StartupSessionPhase::Confirmed => return false,
                StartupSessionPhase::Running if !rust_platform_windows::session_is_ending() => {
                    return true;
                }
                StartupSessionPhase::Running | StartupSessionPhase::Query => {
                    phase = self
                        .changed
                        .wait(phase)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }
    }
}

#[derive(Default)]
struct SessionEventRouteState {
    runtime_ready: bool,
    pending: Vec<rust_platform_windows::SessionEndEvent>,
}

struct SessionEventRouter {
    startup_gate: Arc<StartupSessionGate>,
    state: Mutex<SessionEventRouteState>,
}

impl SessionEventRouter {
    fn new(startup_gate: Arc<StartupSessionGate>) -> Self {
        Self {
            startup_gate,
            state: Mutex::new(SessionEventRouteState::default()),
        }
    }

    fn receive(&self, event: rust_platform_windows::SessionEndEvent) {
        self.startup_gate.observe(event);
        let should_dispatch = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.runtime_ready {
                true
            } else {
                state.pending.push(event);
                false
            }
        };
        if should_dispatch {
            dispatch_runtime_event(RuntimeEvent::SessionEnd(event));
        }
    }

    fn attach_runtime(&self) {
        let pending = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.runtime_ready = true;
            std::mem::take(&mut state.pending)
        };
        for event in pending {
            dispatch_runtime_event(RuntimeEvent::SessionEnd(event));
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RestartAttempt {
    at: Instant,
    startup_failure: bool,
}

#[derive(Clone, Copy, Debug)]
struct DeferredWorkerExit {
    kind: WorkerKind,
    instance_id: u64,
    exit: rust_platform_windows::ProcessExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerRecovery {
    Restarted,
    Deferred,
    Stopping,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestartPolicyDecision {
    Restart,
    Defer,
    Stop,
    Stale,
    RejectRepeatedStartupFailure,
    RejectRateLimit,
}

struct Workers {
    _job: rust_platform_windows::WorkerJob,
    input: JsonWriter,
    speech: JsonWriter,
    translation: JsonWriter,
    input_child: RefCell<WorkerProcess>,
    speech_child: RefCell<WorkerProcess>,
    translation_child: RefCell<WorkerProcess>,
    lifecycle: Cell<WorkerLifecycle>,
    restart_attempts: RefCell<HashMap<WorkerKind, Vec<RestartAttempt>>>,
    deferred_exits: RefCell<HashMap<WorkerKind, DeferredWorkerExit>>,
}

impl Workers {
    fn shutdown(&self) {
        if self.lifecycle.replace(WorkerLifecycle::Stopping) == WorkerLifecycle::Stopping {
            return;
        }
        rust_app_core::diagnostics::log_line("desktop lifecycle=stopping workers");
        let _ = self.input.send(&InputHostCommand::Shutdown);
        let _ = self.speech.send(&SpeechWorkerCommand::Shutdown);
        let _ = self.translation.send(&TranslationWorkerCommand::Shutdown);
    }

    fn begin_session_end(&self) {
        if self.lifecycle.get() == WorkerLifecycle::Running {
            self.lifecycle.set(WorkerLifecycle::SessionEnding);
            rust_app_core::diagnostics::log_line("desktop lifecycle=session_end_query");
        }
    }

    fn confirm_session_end(&self) {
        rust_app_core::diagnostics::log_line("desktop lifecycle=session_end_confirmed");
        self.shutdown();
    }

    fn cancel_session_end(&self) -> Vec<DeferredWorkerExit> {
        if self.lifecycle.get() != WorkerLifecycle::SessionEnding {
            return Vec::new();
        }
        self.lifecycle.set(WorkerLifecycle::Running);
        rust_app_core::diagnostics::log_line("desktop lifecycle=session_end_cancelled");
        self.deferred_exits
            .borrow_mut()
            .drain()
            .map(|(_, exit)| exit)
            .collect()
    }

    fn is_current_instance(&self, kind: WorkerKind, instance_id: u64) -> bool {
        self.process(kind).borrow().instance_id == instance_id
    }

    fn recover(
        &self,
        kind: WorkerKind,
        instance_id: u64,
        exit: rust_platform_windows::ProcessExit,
    ) -> Result<WorkerRecovery, String> {
        let lifecycle = if rust_platform_windows::session_is_ending()
            && self.lifecycle.get() == WorkerLifecycle::Running
        {
            self.lifecycle.set(WorkerLifecycle::SessionEnding);
            WorkerLifecycle::SessionEnding
        } else {
            self.lifecycle.get()
        };
        let now = Instant::now();
        let decision = {
            let mut attempts = self.restart_attempts.borrow_mut();
            let attempts = attempts.entry(kind).or_default();
            decide_worker_restart(
                lifecycle,
                self.is_current_instance(kind, instance_id),
                exit,
                attempts,
                now,
            )
        };
        match decision {
            RestartPolicyDecision::Stale => return Ok(WorkerRecovery::Stale),
            RestartPolicyDecision::Stop => return Ok(WorkerRecovery::Stopping),
            RestartPolicyDecision::Defer => {
                self.deferred_exits.borrow_mut().insert(
                    kind,
                    DeferredWorkerExit {
                        kind,
                        instance_id,
                        exit,
                    },
                );
                return Ok(WorkerRecovery::Deferred);
            }
            RestartPolicyDecision::RejectRepeatedStartupFailure => {
                return Err(format!(
                    "{kind:?} worker failed during startup twice within one minute"
                ));
            }
            RestartPolicyDecision::RejectRateLimit => {
                return Err(format!(
                    "{kind:?} worker stopped at least three times in one minute"
                ));
            }
            RestartPolicyDecision::Restart => {}
        }

        match kind {
            WorkerKind::Input => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Input,
                    INPUT_WORKER_EXE,
                    RuntimeEvent::Input,
                )
                .map_err(|error| error.to_string())?;
                self.input.replace_with(writer);
                replace_process(&self.input_child, child);
            }
            WorkerKind::Speech => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Speech,
                    SPEECH_WORKER_EXE,
                    RuntimeEvent::Speech,
                )
                .map_err(|error| error.to_string())?;
                self.speech.replace_with(writer);
                replace_process(&self.speech_child, child);
            }
            WorkerKind::Translation => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Translation,
                    TRANSLATION_WORKER_EXE,
                    RuntimeEvent::Translation,
                )
                .map_err(|error| error.to_string())?;
                self.translation.replace_with(writer);
                replace_process(&self.translation_child, child);
            }
        }
        Ok(WorkerRecovery::Restarted)
    }

    fn process(&self, kind: WorkerKind) -> &RefCell<WorkerProcess> {
        match kind {
            WorkerKind::Input => &self.input_child,
            WorkerKind::Speech => &self.speech_child,
            WorkerKind::Translation => &self.translation_child,
        }
    }
}

fn decide_worker_restart(
    lifecycle: WorkerLifecycle,
    current_instance: bool,
    exit: rust_platform_windows::ProcessExit,
    attempts: &mut Vec<RestartAttempt>,
    now: Instant,
) -> RestartPolicyDecision {
    if !current_instance {
        return RestartPolicyDecision::Stale;
    }
    match lifecycle {
        WorkerLifecycle::SessionEnding => return RestartPolicyDecision::Defer,
        WorkerLifecycle::Stopping => return RestartPolicyDecision::Stop,
        WorkerLifecycle::Running => {}
    }

    attempts.retain(|attempt| now.duration_since(attempt.at) < WORKER_RESTART_WINDOW);
    let startup_failure = exit.uptime < WORKER_STARTUP_STABILITY;
    if startup_failure && attempts.iter().any(|attempt| attempt.startup_failure) {
        return RestartPolicyDecision::RejectRepeatedStartupFailure;
    }
    if attempts.len() >= 3 {
        return RestartPolicyDecision::RejectRateLimit;
    }
    attempts.push(RestartAttempt {
        at: now,
        startup_failure,
    });
    RestartPolicyDecision::Restart
}

fn replace_process(slot: &RefCell<WorkerProcess>, replacement: WorkerProcess) {
    let mut previous = slot.replace(replacement);
    if previous.child.try_wait().ok().flatten().is_none() {
        let _ = previous.child.kill();
    }
    let _ = previous.child.wait();
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
            let mut process = child.borrow_mut();
            if process.child.try_wait().ok().flatten().is_none() {
                let _ = process.child.kill();
            }
        }
    }
}

fn spawn_initial_workers(
    startup_gate: &StartupSessionGate,
) -> Result<Option<InitialWorkers>, Box<dyn Error>> {
    if !startup_gate.wait_until_worker_start_is_allowed() {
        return Ok(None);
    }
    let job = rust_platform_windows::WorkerJob::new()?;

    let Some((input_child, input)) = spawn_initial_worker(
        &job,
        startup_gate,
        WorkerKind::Input,
        INPUT_WORKER_EXE,
        RuntimeEvent::Input,
    )?
    else {
        return Ok(None);
    };
    let Some((speech_child, speech)) = spawn_initial_worker(
        &job,
        startup_gate,
        WorkerKind::Speech,
        SPEECH_WORKER_EXE,
        RuntimeEvent::Speech,
    )?
    else {
        return Ok(None);
    };
    let Some((translation_child, translation)) = spawn_initial_worker(
        &job,
        startup_gate,
        WorkerKind::Translation,
        TRANSLATION_WORKER_EXE,
        RuntimeEvent::Translation,
    )?
    else {
        return Ok(None);
    };
    if !startup_gate.wait_until_worker_start_is_allowed() {
        return Ok(None);
    }

    Ok(Some(InitialWorkers {
        job,
        input_child,
        input,
        speech_child,
        speech,
        translation_child,
        translation,
    }))
}

fn spawn_initial_worker<T>(
    worker_job: &rust_platform_windows::WorkerJob,
    startup_gate: &StartupSessionGate,
    kind: WorkerKind,
    executable: &str,
    wrap: fn(T) -> RuntimeEvent,
) -> Result<Option<(WorkerProcess, JsonWriter)>, Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
{
    guarded_initial_spawn(
        startup_gate,
        || spawn_worker(worker_job, kind, executable, wrap),
        rust_platform_windows::session_is_ending,
        || {
            rust_app_core::diagnostics::log_line(&format!(
                "worker={executable} initial_spawn=retry_after_session_end_cancelled"
            ));
        },
    )
}

fn guarded_initial_spawn<T, E>(
    startup_gate: &StartupSessionGate,
    mut spawn: impl FnMut() -> Result<T, E>,
    session_is_ending: impl Fn() -> bool,
    mut on_cancelled_retry: impl FnMut(),
) -> Result<Option<T>, E> {
    loop {
        if !startup_gate.wait_until_worker_start_is_allowed() {
            return Ok(None);
        }
        match spawn() {
            Ok(worker) => {
                if startup_gate.wait_until_worker_start_is_allowed() {
                    return Ok(Some(worker));
                }
                return Ok(None);
            }
            Err(_error) if session_is_ending() => {
                if !startup_gate.wait_until_worker_start_is_allowed() {
                    return Ok(None);
                }
                on_cancelled_retry();
            }
            Err(error) => return Err(error),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VisualizerStyle {
    #[default]
    VoiceOrb,
    SymmetricSpectrum,
    FluidRibbon,
    BroadcastMic,
    CircularSpectrum,
    #[serde(alias = "flow_sphere_preview")]
    FlowSphere,
    #[serde(alias = "writing_slate_preview")]
    WritingSlate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CircularSpectrumPalette {
    #[default]
    Sunset,
    Arctic,
    Monochrome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WritingSlateSurface {
    #[default]
    DarkPanel,
    MatteGlass,
}

impl CircularSpectrumPalette {
    const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Sunset),
            1 => Some(Self::Arctic),
            2 => Some(Self::Monochrome),
            _ => None,
        }
    }

    const fn index(self) -> i32 {
        match self {
            Self::Sunset => 0,
            Self::Arctic => 1,
            Self::Monochrome => 2,
        }
    }
}

impl WritingSlateSurface {
    const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::DarkPanel),
            1 => Some(Self::MatteGlass),
            _ => None,
        }
    }

    const fn index(self) -> i32 {
        match self {
            Self::DarkPanel => 0,
            Self::MatteGlass => 1,
        }
    }
}

impl VisualizerStyle {
    const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::VoiceOrb),
            1 => Some(Self::SymmetricSpectrum),
            2 => Some(Self::FluidRibbon),
            3 => Some(Self::BroadcastMic),
            4 => Some(Self::CircularSpectrum),
            5 => Some(Self::FlowSphere),
            6 => Some(Self::WritingSlate),
            _ => None,
        }
    }

    const fn index(self) -> i32 {
        match self {
            Self::VoiceOrb => 0,
            Self::SymmetricSpectrum => 1,
            Self::FluidRibbon => 2,
            Self::BroadcastMic => 3,
            Self::CircularSpectrum => 4,
            Self::FlowSphere => 5,
            Self::WritingSlate => 6,
        }
    }

    const fn uses_transparent_window(self) -> bool {
        matches!(
            self,
            Self::CircularSpectrum | Self::FlowSphere | Self::WritingSlate
        )
    }
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum IndicatorMode {
    #[default]
    Hidden,
    Listening,
    Processing,
    Success,
}

struct IndicatorAnimation {
    bars: Rc<VecModel<f32>>,
    spectrum: Rc<VecModel<f32>>,
    slate_lines: Rc<VecModel<SlateLine>>,
    style: VisualizerStyle,
    mode: IndicatorMode,
    target_level: f32,
    display_level: f32,
    target_spectrum: [f32; AUDIO_SPECTRUM_BANDS],
    display_spectrum: [f32; AUDIO_SPECTRUM_BANDS],
    phase: f32,
    mode_progress: f32,
    flow_voice_impulse: f32,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    last_frame: Instant,
    flow_sphere_renderer: Option<flow_sphere::FlowSphereRenderer>,
    writing_slate: WritingSlateAnimation,
}

#[derive(Clone)]
struct WritingSlateLine {
    x: f32,
    width: f32,
    target_width: f32,
    y: f32,
    target_y: f32,
    opacity: f32,
    glow: f32,
    color_index: i32,
    active: bool,
}

#[derive(Default)]
struct WritingSlateAnimation {
    lines: Vec<WritingSlateLine>,
    voice_active: bool,
    silence_seconds: f32,
    next_line_delay: f32,
    sequence_index: usize,
    cursor_x: f32,
    cursor_y: f32,
}

impl WritingSlateAnimation {
    const CONTENT_WIDTH: f32 = 98.0;
    const SCROLL_Y: f32 = 88.0;
    const ROW_STEP: f32 = 8.5;
    const SEGMENT_GAP: f32 = 4.0;
    const COLOR_SEQUENCE: [i32; 12] = [0, 1, 2, 1, 0, 2, 0, 2, 1, 2, 0, 1];
    const WIDTH_SEQUENCE: [f32; 12] = [
        18.0, 27.0, 12.0, 22.0, 15.0, 30.0, 20.0, 10.0, 25.0, 16.0, 29.0, 13.0,
    ];
    const CADENCE_SEQUENCE: [f32; 12] = [
        0.072, 0.060, 0.086, 0.104, 0.066, 0.080, 0.058, 0.096, 0.070, 0.088, 0.062, 0.100,
    ];

    fn reset(&mut self) {
        self.lines.clear();
        self.voice_active = false;
        self.silence_seconds = 0.0;
        self.next_line_delay = 0.0;
        self.sequence_index = 0;
        self.cursor_x = 0.0;
        self.cursor_y = 0.0;
    }

    fn stop_input(&mut self) {
        self.voice_active = false;
        self.silence_seconds = 0.0;
        self.commit_active();
    }

    fn advance(&mut self, frame_scale: f32, mode: IndicatorMode, energy: f32) -> Vec<SlateLine> {
        let elapsed_seconds = 0.016 * frame_scale;
        self.next_line_delay = (self.next_line_delay - elapsed_seconds).max(0.0);

        for line in &mut self.lines {
            let y_response = 1.0 - 0.72_f32.powf(frame_scale);
            line.y += (line.target_y - line.y) * y_response;
            line.glow *= 0.90_f32.powf(frame_scale);
            let target_opacity = if line.target_y < 0.0 {
                0.0
            } else if line.active {
                1.0
            } else {
                0.78
            };
            let opacity_response = 1.0 - 0.68_f32.powf(frame_scale);
            line.opacity += (target_opacity - line.opacity) * opacity_response;
        }

        if mode == IndicatorMode::Listening {
            let threshold = if self.voice_active { 0.22 } else { 0.34 };
            let speaking = energy >= threshold;
            if speaking {
                self.voice_active = true;
                self.silence_seconds = 0.0;
                if !self.lines.last().is_some_and(|line| line.active) && self.next_line_delay <= 0.0
                {
                    self.start_line();
                }
                let reached_target =
                    if let Some(line) = self.lines.last_mut().filter(|line| line.active) {
                        let fill_response = 1.0 - (1.0 - (0.22 + energy * 0.10)).powf(frame_scale);
                        line.width += (line.target_width - line.width) * fill_response;
                        line.glow = line.glow.max((0.34 + energy * 0.66).clamp(0.0, 1.0));
                        line.opacity = (line.opacity + 0.20 * frame_scale).min(1.0);
                        line.width >= line.target_width - 0.45
                    } else {
                        false
                    };
                if reached_target {
                    self.commit_active();
                    let cadence =
                        self.sequence_index.saturating_sub(1) % Self::CADENCE_SEQUENCE.len();
                    self.next_line_delay = Self::CADENCE_SEQUENCE[cadence];
                }
            } else {
                self.voice_active = false;
                self.silence_seconds += elapsed_seconds;
                if self.silence_seconds >= 0.16 {
                    self.commit_active();
                }
            }
        } else {
            self.stop_input();
        }

        self.lines
            .retain(|line| line.target_y > -Self::ROW_STEP && line.opacity > 0.02);
        self.lines
            .iter()
            .map(|line| SlateLine {
                x: line.x,
                width: line.width,
                y: line.y,
                opacity: line.opacity,
                glow: line.glow,
                color_index: line.color_index,
            })
            .collect()
    }

    fn start_line(&mut self) {
        let sequence = self.sequence_index % Self::COLOR_SEQUENCE.len();
        let target_width = Self::WIDTH_SEQUENCE[sequence];
        if self.cursor_x > 0.0 && self.cursor_x + target_width > Self::CONTENT_WIDTH {
            self.cursor_x = 0.0;
            self.cursor_y += Self::ROW_STEP;
        }
        if self.cursor_y >= Self::SCROLL_Y {
            for line in &mut self.lines {
                line.target_y -= Self::ROW_STEP;
            }
            self.cursor_y -= Self::ROW_STEP;
        }
        self.lines.retain(|line| line.target_y > -Self::ROW_STEP);

        let x = self.cursor_x;
        let target_y = self.cursor_y;
        self.sequence_index = self.sequence_index.wrapping_add(1);
        self.lines.push(WritingSlateLine {
            x,
            width: 3.0,
            target_width,
            y: target_y + 2.5,
            target_y,
            opacity: 0.34,
            glow: 1.0,
            color_index: Self::COLOR_SEQUENCE[sequence],
            active: true,
        });
        self.cursor_x += target_width + Self::SEGMENT_GAP;
    }

    fn commit_active(&mut self) {
        if let Some(line) = self.lines.last_mut().filter(|line| line.active) {
            line.active = false;
            line.width = line.width.max(6.0).min(line.target_width);
            line.opacity = line.opacity.max(0.78);
            line.glow = line.glow.max(0.18);
        }
    }
}

struct IndicatorFrame {
    heights: Vec<f32>,
    spectrum_heights: Vec<f32>,
    signal_opacity: f32,
    disc_opacity: f32,
    orb_size: f32,
    orb_glow: f32,
    ribbon_y1: f32,
    ribbon_y2: f32,
    ribbon_y3: f32,
    ribbon_y4: f32,
    mic_edge_height: f32,
    mic_outer_height: f32,
    mic_middle_height: f32,
    mic_inner_height: f32,
    mic_near_height: f32,
    visual_scale: f32,
    visual_opacity: f32,
}

impl IndicatorAnimation {
    fn new(
        bars: Rc<VecModel<f32>>,
        spectrum: Rc<VecModel<f32>>,
        slate_lines: Rc<VecModel<SlateLine>>,
    ) -> Self {
        Self {
            bars,
            spectrum,
            slate_lines,
            style: VisualizerStyle::default(),
            mode: IndicatorMode::Hidden,
            target_level: 0.08,
            display_level: 0.08,
            target_spectrum: [0.0; AUDIO_SPECTRUM_BANDS],
            display_spectrum: [0.0; AUDIO_SPECTRUM_BANDS],
            phase: 0.0,
            mode_progress: 0.0,
            flow_voice_impulse: 0.0,
            flow_sphere_palette: flow_sphere::FlowSpherePalette::default(),
            last_frame: Instant::now(),
            flow_sphere_renderer: None,
            writing_slate: WritingSlateAnimation::default(),
        }
    }

    fn set_style(&mut self, style: VisualizerStyle) {
        self.style = style;
        if style == VisualizerStyle::FlowSphere && self.flow_sphere_renderer.is_none() {
            self.flow_sphere_renderer = Some(flow_sphere::FlowSphereRenderer::start());
        } else if style != VisualizerStyle::FlowSphere {
            self.flow_sphere_renderer = None;
        }
    }

    fn set_flow_sphere_palette(&mut self, palette: flow_sphere::FlowSpherePalette) {
        self.flow_sphere_palette = palette;
    }

    fn begin_listening(&mut self) {
        self.mode = IndicatorMode::Listening;
        self.target_level = 0.08;
        self.display_level = self.display_level.max(0.08);
        self.target_spectrum.fill(0.0);
        self.display_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        if self.style == VisualizerStyle::WritingSlate {
            self.writing_slate.reset();
            self.slate_lines.set_vec(Vec::new());
        }
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    fn set_microphone_signal(&mut self, microphone_level: f32, spectrum: &[f32]) {
        if self.mode == IndicatorMode::Listening {
            let next_level = (microphone_level.clamp(0.0, 1.0).sqrt() * 2.35).clamp(0.0, 1.0);
            if self.style == VisualizerStyle::FlowSphere {
                let onset = (next_level - self.target_level).max(0.0);
                self.flow_voice_impulse =
                    self.flow_voice_impulse.max((onset * 1.65).clamp(0.0, 1.0));
            }
            self.target_level = next_level;
            for (index, target) in self.target_spectrum.iter_mut().enumerate() {
                *target = spectrum.get(index).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }
    }

    fn begin_processing(&mut self) {
        self.mode = IndicatorMode::Processing;
        self.target_level = 0.18;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        if self.style == VisualizerStyle::WritingSlate {
            self.writing_slate.stop_input();
        }
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    fn begin_success(&mut self) {
        self.mode = IndicatorMode::Success;
        self.target_level = 0.0;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    fn stop(&mut self) {
        self.mode = IndicatorMode::Hidden;
        self.target_level = 0.08;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
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
        self.spectrum.set_vec(frame.spectrum_heights);
        if self.style == VisualizerStyle::WritingSlate {
            let slate_energy = self.target_level.max(self.display_level);
            self.slate_lines.set_vec(self.writing_slate.advance(
                frame_scale,
                self.mode,
                slate_energy,
            ));
        }

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
            indicator.set_mic_edge_height(frame.mic_edge_height);
            indicator.set_mic_outer_height(frame.mic_outer_height);
            indicator.set_mic_middle_height(frame.mic_middle_height);
            indicator.set_mic_inner_height(frame.mic_inner_height);
            indicator.set_mic_near_height(frame.mic_near_height);
            indicator.set_visual_scale(frame.visual_scale);
            indicator.set_visual_opacity(frame.visual_opacity);
            if self.style == VisualizerStyle::FlowSphere
                && let Some(renderer) = &self.flow_sphere_renderer
                && let Some(frame) = renderer.render_latest(
                    self.phase,
                    self.display_level,
                    self.flow_voice_impulse,
                    self.flow_sphere_palette,
                    &self.display_spectrum,
                    frame.visual_opacity,
                )
            {
                indicator.set_flow_sphere_frame(frame);
            }
        }
    }

    fn advance_frame(&mut self, frame_scale: f32) -> IndicatorFrame {
        if self.mode == IndicatorMode::Success {
            self.mode_progress = (self.mode_progress + 0.045 * frame_scale).min(1.0);
        }
        let level_response: f32 = if self.style == VisualizerStyle::FlowSphere {
            if self.target_level > self.display_level {
                0.34
            } else {
                0.085
            }
        } else {
            0.18
        };
        let smoothing = 1.0 - (1.0 - level_response).powf(frame_scale);
        self.display_level += (self.target_level - self.display_level) * smoothing;
        for (display, target) in self.display_spectrum.iter_mut().zip(self.target_spectrum) {
            let response = if target > *display {
                if self.style == VisualizerStyle::FlowSphere {
                    0.52_f32
                } else {
                    0.46_f32
                }
            } else if self.style == VisualizerStyle::FlowSphere {
                0.095_f32
            } else {
                0.16_f32
            };
            let spectrum_smoothing = 1.0 - (1.0 - response).powf(frame_scale);
            *display += (target - *display) * spectrum_smoothing;
        }
        let flow_activity = ((self.display_level - 0.14) / 0.48).clamp(0.0, 1.0);
        let phase_step = match self.mode {
            IndicatorMode::Listening if self.style == VisualizerStyle::FlowSphere => {
                0.045 + flow_activity * 0.245 + self.flow_voice_impulse * 0.075
            }
            IndicatorMode::Listening => 0.29,
            IndicatorMode::Processing => 0.18,
            IndicatorMode::Success => 0.24,
            IndicatorMode::Hidden => 0.0,
        };
        self.phase += phase_step * frame_scale;
        self.flow_voice_impulse *= 0.78_f32.powf(frame_scale);

        let processing_pulse = ((self.phase * 0.72).sin() + 1.0) / 2.0;
        let energy = match self.mode {
            IndicatorMode::Listening => self.display_level.max(0.07),
            IndicatorMode::Processing => self.target_level + (processing_pulse * 0.08),
            IndicatorMode::Success => 0.0,
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
        let spectrum_heights = self
            .display_spectrum
            .iter()
            .enumerate()
            .map(|(index, band)| match self.mode {
                IndicatorMode::Listening => {
                    let voice_activation = ((self.display_level - 0.24) / 0.76).clamp(0.0, 1.0);
                    (2.8 + band * 16.2 + band.sqrt() * voice_activation * 2.5).clamp(2.8, 21.5)
                }
                IndicatorMode::Processing => {
                    let angular_position =
                        index as f32 / AUDIO_SPECTRUM_BANDS as f32 * std::f32::consts::TAU;
                    let travelling = ((angular_position - self.phase * 0.72).cos() + 1.0) * 0.5;
                    2.8 + travelling.powi(5) * 9.4
                }
                IndicatorMode::Success => 2.8,
                IndicatorMode::Hidden => 2.8,
            })
            .collect();
        let mic_edge_height = (3.0
            + energy * (4.0 + (((self.phase * 0.67 + 0.45).sin() + 1.0) / 2.0) * 5.0))
            .clamp(3.0, 12.0);
        let mic_outer_height = (4.0
            + energy * (5.4 + (((self.phase * 0.78).sin() + 1.0) / 2.0) * 7.6))
            .clamp(4.0, 17.0);
        let mic_middle_height = (5.0
            + energy * (6.2 + (((self.phase * 1.06 + 1.4).sin() + 1.0) / 2.0) * 6.8))
            .clamp(5.0, 18.0);
        let mic_inner_height = (3.0
            + energy * (4.8 + (((self.phase * 1.34 + 2.6).sin() + 1.0) / 2.0) * 5.6))
            .clamp(3.0, 14.0);
        let mic_near_height = (4.0
            + energy * (5.0 + (((self.phase * 1.52 + 3.35).sin() + 1.0) / 2.0) * 6.0))
            .clamp(4.0, 15.0);
        IndicatorFrame {
            heights,
            spectrum_heights,
            signal_opacity: (0.5 + energy.min(1.0) * 0.5).clamp(0.5, 1.0),
            disc_opacity: match self.mode {
                IndicatorMode::Processing => 0.72 + processing_pulse * 0.28,
                IndicatorMode::Listening => 1.0,
                IndicatorMode::Success => 1.0 - self.mode_progress,
                IndicatorMode::Hidden => 0.0,
            },
            orb_size: (9.0 + energy * 10.0 + processing_pulse * 1.5).clamp(8.0, 20.0),
            orb_glow: (0.12 + energy * 0.38 + processing_pulse * 0.12).clamp(0.12, 0.62),
            ribbon_y1: 9.5 + (self.phase * 0.93).sin() * energy * 6.5,
            ribbon_y2: 9.5 + (self.phase * 0.93 + 1.35).sin() * energy * 7.4,
            ribbon_y3: 9.5 + (self.phase * 0.93 + 2.7).sin() * energy * 6.8,
            ribbon_y4: 9.5 + (self.phase * 0.93 + 4.05).sin() * energy * 5.8,
            mic_edge_height,
            mic_outer_height,
            mic_middle_height,
            mic_inner_height,
            mic_near_height,
            visual_scale: match self.mode {
                IndicatorMode::Success => {
                    let eased =
                        self.mode_progress * self.mode_progress * (3.0 - 2.0 * self.mode_progress);
                    1.0 - eased * 0.55
                }
                IndicatorMode::Processing => 1.0 + processing_pulse * 0.025,
                IndicatorMode::Listening | IndicatorMode::Hidden => 1.0,
            },
            visual_opacity: match self.mode {
                IndicatorMode::Success => 1.0 - self.mode_progress,
                IndicatorMode::Listening | IndicatorMode::Processing => 1.0,
                IndicatorMode::Hidden => 0.0,
            },
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
    circular_spectrum_palette: CircularSpectrumPalette,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    writing_slate_surface: WritingSlateSurface,
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
    pending_selection_captures: HashMap<RequestId, u64>,
    selection_release_gates: HashMap<RequestId, SelectionReleaseGate>,
    pending_translations: HashMap<RequestId, PendingTranslation>,
    queued_selection_translation: Option<SelectionTranslationRequest>,
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
            circular_spectrum_palette: saved_settings.circular_spectrum_palette,
            flow_sphere_palette: saved_settings.flow_sphere_palette,
            writing_slate_surface: saved_settings.writing_slate_surface,
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
            pending_selection_captures: HashMap::new(),
            selection_release_gates: HashMap::new(),
            pending_translations: HashMap::new(),
            queued_selection_translation: None,
            translation_history: Vec::new(),
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

    if let Err(error) = rust_app_core::product::migrate_legacy_data_root() {
        eprintln!("StyleFlex data migration is deferred: {error}");
    }
    let _ = rust_app_core::diagnostics::initialize(&data_root().join("logs"), "desktop");
    rust_app_core::diagnostics::log_line("desktop lifecycle=starting");

    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\StyleFlex.Desktop")?;
    let settings = SettingsWindow::new()?;
    let saved_settings = load_settings();
    let (indicator, indicator_animation) = new_indicator()?;
    indicator_animation
        .borrow_mut()
        .set_style(saved_settings.visualizer_style);
    indicator_animation
        .borrow_mut()
        .set_flow_sphere_palette(saved_settings.flow_sphere_palette);
    indicator.set_visualizer_style(saved_settings.visualizer_style.index());
    indicator.set_circular_palette(saved_settings.circular_spectrum_palette.index());
    indicator.set_writing_slate_surface(saved_settings.writing_slate_surface.index());
    let chooser = SelectionChooser::new()?;
    let action_palette = ActionPalette::new()?;
    let translation_preview = TranslationPreview::new()?;
    let translator = TranslatorWindow::new()?;
    let tray = AppTray::new()?;
    precreate_indicator(&indicator);
    precreate_action_windows(&chooser, &action_palette, &translation_preview);

    let Some(initial_workers) = spawn_initial_workers(&startup_gate)? else {
        rust_app_core::diagnostics::log_line(
            "desktop lifecycle=startup_aborted_session_end_confirmed",
        );
        return Ok(());
    };
    let InitialWorkers {
        job: worker_job,
        input_child,
        input,
        speech_child,
        speech,
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
        translation_child: RefCell::new(translation_child),
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
        &indicator,
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
    wire_translation_preview(&translation_preview);
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
    session_event_router.attach_runtime();
    let _popup_dismiss_monitor = match rust_platform_windows::PopupDismissMonitor::start(
        || {
            dispatch_runtime_event(RuntimeEvent::DismissTranslationPreview);
        },
        |pinned| {
            dispatch_runtime_event(RuntimeEvent::TranslationPreviewPinChanged(pinned));
        },
        |delta| {
            dispatch_runtime_event(RuntimeEvent::ScrollTranslationPreview(delta));
        },
    ) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            eprintln!("translation popup click-away is unavailable: {error}");
            None
        }
    };
    let animation_timer = start_indicator_animation(&indicator, &indicator_animation);

    if !env::args().any(|argument| argument == "--background") {
        settings.show()?;
        configure_standard_app_window(settings.window(), "Settings");
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
    let spectrum_bands = Rc::new(VecModel::from(vec![2.8; AUDIO_SPECTRUM_BANDS]));
    let slate_lines = Rc::new(VecModel::from(Vec::<SlateLine>::new()));
    indicator.set_bar_heights(ModelRc::from(Rc::clone(&indicator_bars)));
    indicator.set_spectrum_heights(ModelRc::from(Rc::clone(&spectrum_bands)));
    indicator.set_slate_lines(ModelRc::from(Rc::clone(&slate_lines)));
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(
        indicator_bars,
        spectrum_bands,
        slate_lines,
    )));
    Ok((indicator, animation))
}

fn indicator_preview_style() -> Option<VisualizerStyle> {
    env::args().find_map(|argument| indicator_preview_style_for_argument(&argument))
}

fn indicator_preview_style_for_argument(argument: &str) -> Option<VisualizerStyle> {
    match argument {
        "--indicator-preview" | "--indicator-preview=orb" => Some(VisualizerStyle::VoiceOrb),
        "--indicator-preview=spectrum" => Some(VisualizerStyle::SymmetricSpectrum),
        "--indicator-preview=ribbon" => Some(VisualizerStyle::FluidRibbon),
        "--indicator-preview=mic" | "--indicator-preview=microphone" => {
            Some(VisualizerStyle::BroadcastMic)
        }
        "--indicator-preview=circle" | "--indicator-preview=circular-spectrum" => {
            Some(VisualizerStyle::CircularSpectrum)
        }
        "--indicator-preview=flow-sphere" | "--indicator-preview=sphere" => {
            Some(VisualizerStyle::FlowSphere)
        }
        "--indicator-preview=slate" | "--indicator-preview=writing-slate" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview=slate-transparent"
        | "--indicator-preview=writing-slate-transparent" => Some(VisualizerStyle::WritingSlate),
        "--indicator-preview=slate-frosted" | "--indicator-preview=writing-slate-frosted" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=circle" | "--indicator-preview-live=circular-spectrum" => {
            Some(VisualizerStyle::CircularSpectrum)
        }
        "--indicator-preview-live=flow-sphere" | "--indicator-preview-live=sphere" => {
            Some(VisualizerStyle::FlowSphere)
        }
        "--indicator-preview-live=slate" | "--indicator-preview-live=writing-slate" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=slate-transparent"
        | "--indicator-preview-live=writing-slate-transparent" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=slate-frosted"
        | "--indicator-preview-live=writing-slate-frosted" => Some(VisualizerStyle::WritingSlate),
        _ => None,
    }
}

fn indicator_preview_slate_surface() -> WritingSlateSurface {
    env::args()
        .find_map(|argument| indicator_preview_slate_surface_for_argument(&argument))
        .unwrap_or_default()
}

fn indicator_preview_slate_surface_for_argument(argument: &str) -> Option<WritingSlateSurface> {
    match argument {
        "--indicator-preview=slate-transparent"
        | "--indicator-preview=writing-slate-transparent"
        | "--indicator-preview-live=slate-transparent"
        | "--indicator-preview-live=writing-slate-transparent"
        | "--indicator-preview=slate-frosted"
        | "--indicator-preview=writing-slate-frosted"
        | "--indicator-preview-live=slate-frosted"
        | "--indicator-preview-live=writing-slate-frosted" => Some(WritingSlateSurface::MatteGlass),
        _ => None,
    }
}

fn indicator_preview_palette() -> CircularSpectrumPalette {
    env::args()
        .find_map(|argument| match argument.as_str() {
            "--indicator-palette=sunset" => Some(CircularSpectrumPalette::Sunset),
            "--indicator-palette=arctic" | "--indicator-palette=blue" => {
                Some(CircularSpectrumPalette::Arctic)
            }
            "--indicator-palette=monochrome" | "--indicator-palette=white" => {
                Some(CircularSpectrumPalette::Monochrome)
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn indicator_preview_flow_sphere_palette() -> flow_sphere::FlowSpherePalette {
    env::args()
        .find_map(|argument| match argument.as_str() {
            "--flow-sphere-palette=reference" | "--flow-sphere-palette=original" => {
                Some(flow_sphere::FlowSpherePalette::Reference)
            }
            "--flow-sphere-palette=black-yellow" | "--flow-sphere-palette=yellow" => {
                Some(flow_sphere::FlowSpherePalette::BlackYellow)
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn translation_preview_mode() -> Option<bool> {
    env::args().find_map(|argument| match argument.as_str() {
        "--translation-preview" => Some(false),
        "--translation-preview=long" => Some(true),
        _ => None,
    })
}

fn run_indicator_preview(
    style: VisualizerStyle,
    live_microphone: bool,
    writing_slate_surface: WritingSlateSurface,
) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\StyleFlex.IndicatorPreview")?;
    let (indicator, animation) = new_indicator()?;
    {
        let mut animation = animation.borrow_mut();
        animation.set_style(style);
        animation.set_flow_sphere_palette(indicator_preview_flow_sphere_palette());
        animation.begin_listening();
    }
    indicator.set_visualizer_style(style.index());
    indicator.set_indicator_title("SayFlex Recording Indicator".into());
    indicator.set_writing_slate_surface(writing_slate_surface.index());
    indicator.set_circular_palette(indicator_preview_palette().index());
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
                .is_some_and(|handle| {
                    configure_indicator_native_window(handle, None, style.uses_transparent_window())
                });
            if !configured {
                eprintln!("could not configure recording indicator preview");
            }
        }
    })?;

    let live_signal = Arc::new(Mutex::new(None::<(f32, Vec<f32>)>));
    let live_audio_worker = if live_microphone {
        let worker_job = rust_platform_windows::WorkerJob::new()
            .map_err(|error| format!("could not create preview worker job: {error}"))?;
        let path = worker_path(SPEECH_WORKER_EXE)?;
        let mut command = Command::new(&path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        worker_job
            .assign(&child)
            .map_err(|error| format!("could not contain preview audio worker: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("preview audio worker stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("preview audio worker stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("preview audio worker stderr is unavailable")?;
        let writer = JsonWriter {
            stdin: RefCell::new(BufWriter::new(stdin)),
        };
        let session_id = SessionId::new();
        writer.send(&SpeechWorkerCommand::SetFastReleasePreviews { enabled: false })?;
        writer.send(&SpeechWorkerCommand::Begin {
            session_id,
            started_unix_ms: unix_ms(),
        })?;

        let signal_for_reader = Arc::clone(&live_signal);
        std::thread::Builder::new()
            .name("indicator-preview-audio".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Ok(event) = serde_json::from_str::<SpeechWorkerEvent>(&line) else {
                        continue;
                    };
                    if let SpeechWorkerEvent::AudioLevel {
                        session_id: event_session,
                        peak,
                        spectrum,
                    } = event
                        && event_session == session_id
                        && let Ok(mut latest) = signal_for_reader.lock()
                    {
                        *latest = Some((peak, spectrum));
                    }
                }
            })?;
        std::thread::Builder::new()
            .name("indicator-preview-audio-diagnostics".to_owned())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("preview audio worker: {line}");
                }
            })?;
        Some((worker_job, child, writer, session_id))
    } else {
        None
    };

    let timer = Timer::default();
    let indicator = indicator.as_weak();
    let animation_for_timer = Rc::clone(&animation);
    let started = Instant::now();
    let segment = Rc::new(Cell::new(IndicatorMode::Listening));
    let segment_for_timer = Rc::clone(&segment);
    let live_signal_for_timer = Arc::clone(&live_signal);
    timer.start(TimerMode::Repeated, Duration::from_millis(12), move || {
        let elapsed = started.elapsed().as_secs_f32();
        let (cycle_length, listening_until, processing_until) =
            if style == VisualizerStyle::WritingSlate {
                (17.0, 14.0, 16.0)
            } else {
                (10.0, 7.0, 9.0)
            };
        let cycle_position = elapsed % cycle_length;
        let next_segment = if cycle_position < listening_until {
            IndicatorMode::Listening
        } else if cycle_position < processing_until {
            IndicatorMode::Processing
        } else {
            IndicatorMode::Success
        };
        if segment_for_timer.get() != next_segment {
            segment_for_timer.set(next_segment);
            match next_segment {
                IndicatorMode::Listening => animation_for_timer.borrow_mut().begin_listening(),
                IndicatorMode::Processing => animation_for_timer.borrow_mut().begin_processing(),
                IndicatorMode::Success => animation_for_timer.borrow_mut().begin_success(),
                IndicatorMode::Hidden => {}
            }
        }
        if next_segment == IndicatorMode::Listening {
            if live_microphone {
                if let Ok(mut latest) = live_signal_for_timer.try_lock()
                    && let Some((peak, spectrum)) = latest.take()
                {
                    animation_for_timer
                        .borrow_mut()
                        .set_microphone_signal(peak, &spectrum);
                }
            } else {
                let simulated_level = (0.08 + ((elapsed * 2.7).sin() + 1.0) * 0.18).clamp(0.0, 1.0);
                let simulated_spectrum = (0..AUDIO_SPECTRUM_BANDS)
                    .map(|index| {
                        let position = index as f32 / AUDIO_SPECTRUM_BANDS as f32;
                        let low_voice = (1.0 - (position - 0.24).abs() * 3.1).clamp(0.0, 1.0);
                        let formant = (1.0 - (position - 0.52).abs() * 5.4).clamp(0.0, 1.0);
                        let motion = ((elapsed * 3.2 + index as f32 * 0.47).sin() + 1.0) * 0.5;
                        ((low_voice * 0.72 + formant * 0.38)
                            * (0.52 + motion * 0.48)
                            * simulated_level
                            * 2.9)
                            .clamp(0.0, 1.0)
                    })
                    .collect::<Vec<_>>();
                animation_for_timer
                    .borrow_mut()
                    .set_microphone_signal(simulated_level, &simulated_spectrum);
            }
        }
        animation_for_timer.borrow_mut().tick(&indicator);
    });
    slint::run_event_loop()?;
    timer.stop();
    if let Some((_worker_job, mut child, writer, session_id)) = live_audio_worker {
        let _ = writer.send(&SpeechWorkerCommand::Cancel { session_id });
        let _ = writer.send(&SpeechWorkerCommand::Shutdown);
        drop(writer);
        let _ = child.wait();
    }
    Ok(())
}

fn run_action_palette_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\StyleFlex.ActionPreview")?;
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
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\StyleFlex.TranslationPreview")?;
    let preview = TranslationPreview::new()?;
    let translated = if long_text {
        "Длинный перевод должен оставаться удобным для чтения. Карточка сохраняет компактную форму, а результат прокручивается внутри неё. Это позволяет быстро проверить смысл, не терять выбранное место в исходном приложении и не открывать отдельное большое рабочее окно. Прокрутка появляется только тогда, когда текст действительно не помещается. ".repeat(12)
    } else {
        "Предпросмотр должен увеличиваться вместе с содержимым, а не оставлять пустой холст."
            .to_owned()
    };
    apply_translation_preview_layout(&preview, &translated);
    preview.set_translated_text(translated.clone().into());
    preview.set_prepared(true);
    wire_translation_preview(&preview);
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
                .and_then(rust_platform_windows::configure_translation_window)
            {
                eprintln!("could not configure translation preview: {error}");
            } else if let Some(handle) = handle {
                rust_platform_windows::activate_popup_dismiss(handle);
            }
        }
    })?;
    let preview_for_pin = preview.as_weak();
    let _popup_dismiss_monitor = rust_platform_windows::PopupDismissMonitor::start(
        || {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        },
        move |pinned| {
            let preview = preview_for_pin.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(preview) = preview.upgrade() {
                    preview.set_pinned(pinned);
                }
            });
        },
        {
            let preview = preview.as_weak();
            move |delta| {
                let preview = preview.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(preview) = preview.upgrade() {
                        preview.invoke_scroll_by(delta as f32);
                    }
                });
            }
        },
    )?;
    slint::run_event_loop()?;
    Ok(())
}

fn run_layout_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\StyleFlex.LayoutPreview")?;
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
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
{
    let path = worker_path(executable)?;
    let started_at = Instant::now();
    let instance_id = NEXT_WORKER_INSTANCE.fetch_add(1, Ordering::Relaxed);
    let mut command = Command::new(&path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(SPEECH_CUDA_EXE))
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
    if let Err(error) = rust_platform_windows::watch_process_exit(&child, started_at, move |exit| {
        dispatch_runtime_event(RuntimeEvent::WorkerExited {
            kind,
            instance_id,
            exit,
        });
    }) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("could not supervise {executable}: {error}").into());
    }
    let stdin = child.stdin.take().ok_or("worker stdin is unavailable")?;
    let stdout = child.stdout.take().ok_or("worker stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("worker stderr is unavailable")?;

    let output_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(executable)
        .to_owned();
    std::thread::Builder::new()
        .name(format!("{executable}-events"))
        .spawn(move || {
            let mut stream_result = "closed".to_owned();
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        stream_result = format!("read_error:{error}");
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
            rust_app_core::diagnostics::log_line(&format!(
                "worker={output_name} instance={instance_id} event_stream={stream_result}"
            ));
        })?;

    let diagnostics_name = executable.to_owned();
    std::thread::Builder::new()
        .name(format!("{executable}-diagnostics"))
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if routine_native_worker_log(&diagnostics_name, &line) {
                    continue;
                }
                rust_app_core::diagnostics::log_line(&format!("worker={diagnostics_name} {line}"));
                eprintln!("{diagnostics_name}: {line}");
            }
        })?;

    Ok((
        WorkerProcess { child, instance_id },
        JsonWriter {
            stdin: RefCell::new(BufWriter::new(stdin)),
        },
    ))
}

fn routine_native_worker_log(executable: &str, line: &str) -> bool {
    executable == SPEECH_WORKER_EXE
        && (line.starts_with("whisper_init_state:")
            || line.starts_with("whisper_backend_init_gpu:")
            || line.starts_with("whisper_full_with_state: auto-detected language:"))
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
    if executable == SPEECH_WORKER_EXE {
        let cuda = parent.join(SPEECH_CUDA_EXE);
        if cuda.exists() && trusted_cuda_runtime_directory().is_some() {
            return Ok(cuda);
        }
        let cpu = parent.join(SPEECH_CPU_EXE);
        if cpu.exists() {
            return Ok(cpu);
        }
        let development = parent.join(DEV_SPEECH_WORKER_EXE);
        if development.exists() {
            return Ok(development);
        }
    }
    let sibling = parent.join(executable);
    if sibling.exists() {
        return Ok(sibling);
    }
    let development = match executable {
        INPUT_WORKER_EXE => Some(DEV_INPUT_WORKER_EXE),
        TRANSLATION_WORKER_EXE => Some(DEV_TRANSLATION_WORKER_EXE),
        _ => None,
    }
    .map(|name| parent.join(name));
    if let Some(development) = development.filter(|path| path.exists()) {
        return Ok(development);
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
        RuntimeEvent::DismissTranslationPreview => {
            if let Some(preview) = runtime.translation_preview.upgrade() {
                preview.set_pinned(false);
                let _ = preview.hide();
            }
            rust_platform_windows::deactivate_popup_dismiss();
        }
        RuntimeEvent::TranslationPreviewPinChanged(pinned) => {
            if let Some(preview) = runtime.translation_preview.upgrade() {
                preview.set_pinned(pinned);
            }
        }
        RuntimeEvent::ScrollTranslationPreview(delta) => {
            if let Some(preview) = runtime.translation_preview.upgrade() {
                preview.invoke_scroll_by(delta as f32);
            }
        }
        RuntimeEvent::SessionEnd(event) => {
            handle_session_end(event, runtime);
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

    match runtime.workers.recover(kind, instance_id, exit) {
        Ok(WorkerRecovery::Restarted) => {
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
            if intent == TextActionIntent::Translate
                && let Some(preview) = translation_preview.upgrade()
            {
                preview.set_pinned(false);
                let _ = preview.hide();
                rust_platform_windows::deactivate_popup_dismiss();
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
                action_palette,
                translation_preview,
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
                let latency = transcript_latency.unwrap_or_default();
                rust_app_core::diagnostics::log_line(&format!(
                    "latency session={} release_to_insert_ms={} insertion_dispatch_ms={} \
audio_finalize_ms={} preprocessing_ms={} final_queue_wait_ms={} inference_ms={} \
speculative_reuse={} method={}",
                    session_id,
                    total,
                    dispatch_duration_ms,
                    latency.audio_finalize_ms,
                    latency.preprocessing_ms,
                    latency.final_queue_wait_ms,
                    latency.inference_ms,
                    latency.speculative_reuse,
                    method_name,
                ));
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
            let _ = request_id;
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
    request_started_unix_ms: u64,
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
                request_started_unix_ms,
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
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
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
    _translation_preview: &slint::Weak<TranslationPreview>,
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
        return;
    }

    state.borrow_mut().pending_translations.insert(
        request_id,
        PendingTranslation {
            origin: TranslationOrigin::Selection { target, delivery },
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
        set_indicator_status(settings, &format!("Could not start translation: {error}"));
    } else {
        set_indicator_status(settings, "Translating locally…");
    }
}

fn show_selection_translation_preview(
    request_id: RequestId,
    target: TargetSnapshot,
    translated_text: &str,
    translation_preview: &slint::Weak<TranslationPreview>,
) {
    let Some(preview) = translation_preview.upgrade() else {
        return;
    };
    let request_id_text = request_id.to_string();
    let first_show = preview.get_request_id().as_str() != request_id_text;
    if first_show {
        preview.set_request_id(request_id_text.into());
        preview.set_pinned(false);
    }
    apply_translation_preview_layout(&preview, translated_text);
    preview.set_translated_text(translated_text.into());
    if first_show {
        preview.invoke_reset_scroll();
        let _ = preview.show();
        prepare_shown_translation_window(
            slint_window_handle(preview.window()),
            TRANSLATION_PREVIEW_WINDOW_TITLE,
            target.window_handle,
        );
    }
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
        SpeechWorkerEvent::AudioLevel {
            session_id,
            peak,
            spectrum,
        } => {
            if state.borrow().registry.active_session() == Some(session_id) {
                indicator_animation
                    .borrow_mut()
                    .set_microphone_signal(peak, &spectrum);
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
                        )
                    })
            };
            let Some((origin, translation_elapsed_ms, request_started_unix_ms, first_partial)) =
                pending
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
                        &translated_text,
                        translation_preview,
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
                translation_preview,
                translator,
                state,
                workers,
            );
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
            if let Some(preview) = translation_preview.upgrade()
                && preview.get_request_id().as_str() == request_id.to_string()
                && !preview.get_pinned()
            {
                let _ = preview.hide();
                rust_platform_windows::deactivate_popup_dismiss();
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
            let _ = translation_preview;
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
                    &result.translated_text,
                    translation_preview,
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

fn wire_settings(
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    indicator: &RecordingIndicator,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    {
        let translator = translator.as_weak();
        settings.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                configure_standard_app_window(translator.window(), "Translator");
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
        let indicator = indicator.as_weak();
        let indicator_animation = Rc::clone(indicator_animation);
        settings.on_select_visualizer(move |index| {
            let Some(style) = VisualizerStyle::from_index(index) else {
                return;
            };
            state.borrow_mut().visualizer_style = style;
            indicator_animation.borrow_mut().set_style(style);
            if let Some(indicator) = indicator.upgrade() {
                indicator.set_visualizer_style(style.index());
            }
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
        let indicator = indicator.as_weak();
        let state = Rc::clone(state);
        settings.on_select_circular_palette(move |index| {
            let Some(palette) = CircularSpectrumPalette::from_index(index) else {
                return;
            };
            state.borrow_mut().circular_spectrum_palette = palette;
            if let Some(indicator) = indicator.upgrade() {
                indicator.set_circular_palette(palette.index());
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Circular spectrum palette updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let indicator_animation = Rc::clone(indicator_animation);
        settings.on_select_flow_sphere_palette(move |index| {
            let Some(palette) = flow_sphere::FlowSpherePalette::from_index(index) else {
                return;
            };
            state.borrow_mut().flow_sphere_palette = palette;
            indicator_animation
                .borrow_mut()
                .set_flow_sphere_palette(palette);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Flow Sphere palette updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let indicator = indicator.as_weak();
        let state = Rc::clone(state);
        settings.on_select_writing_slate_surface(move |index| {
            let Some(surface) = WritingSlateSurface::from_index(index) else {
                return;
            };
            state.borrow_mut().writing_slate_surface = surface;
            if let Some(indicator) = indicator.upgrade() {
                indicator.set_writing_slate_surface(surface.index());
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Writing Slate surface updated for the next dictation.",
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
                configure_standard_app_window(settings.window(), "Settings");
                settings.window().request_redraw();
            }
        });
    }
    {
        let translator = translator.as_weak();
        tray.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                configure_standard_app_window(translator.window(), "Translator");
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
            let _ = dispatch_or_defer_selection_replacement(
                &state,
                &workers,
                pending.request_id,
                pending.target,
                candidate.text.clone(),
            );
        });
    }
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        chooser.on_cancel(move || {
            let pending = state.borrow_mut().pending_selection.take();
            if let Some(pending) = pending {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&pending.request_id);
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
                    request_started_unix_ms: pending.request_started_unix_ms,
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
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(pending) = pending {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&pending.request_id);
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

fn wire_translation_preview(preview: &TranslationPreview) {
    {
        let preview_weak = preview.as_weak();
        preview.on_begin_drag(move || {
            if let Some(preview) = preview_weak.upgrade()
                && let Some(handle) = slint_window_handle(preview.window())
                && let Err(error) = rust_platform_windows::begin_window_drag(handle)
            {
                eprintln!("could not begin translation popup drag: {error}");
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
                    "No model installed. Choose Fast or Quality in Settings.".into(),
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
                    started: Instant::now(),
                    request_started_unix_ms: unix_ms(),
                    first_partial_shown: false,
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
    settings.set_selected_visualizer(state.visualizer_style.index());
    settings.set_selected_circular_palette(state.circular_spectrum_palette.index());
    settings.set_selected_flow_sphere_palette(state.flow_sphere_palette.index());
    settings.set_selected_writing_slate_surface(state.writing_slate_surface.index());
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
}

fn translation_preview_layout(result_text: &str) -> TranslationPreviewLayout {
    let content_characters = result_text.chars().count();
    let card_width = match content_characters {
        0..=28 => 280.0,
        29..=140 => 400.0,
        _ => 560.0,
    };

    TranslationPreviewLayout { card_width }
}

fn apply_translation_preview_layout(preview: &TranslationPreview, result_text: &str) {
    let layout = translation_preview_layout(result_text);
    preview.set_card_width(layout.card_width);
}

fn prepare_shown_translation_window(
    window_handle: Option<u64>,
    window_title: &'static str,
    target_window_handle: u64,
) {
    if let Err(error) = slint::invoke_from_event_loop(move || {
        let configured = window_handle
            .ok_or_else(|| "native window handle is unavailable".to_owned())
            .and_then(rust_platform_windows::configure_translation_window);
        match configured {
            Ok(()) => {
                if let Some(window_handle) = window_handle {
                    rust_platform_windows::activate_popup_dismiss(window_handle);
                }
            }
            Err(error) => {
                eprintln!("could not configure `{window_title}` as a translation popup: {error}");
            }
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

fn configure_standard_app_window(window: &slint::Window, name: &str) {
    if let Err(error) = slint_window_handle(window)
        .ok_or_else(|| "native window handle is unavailable".to_owned())
        .and_then(rust_platform_windows::configure_standard_window)
    {
        eprintln!("could not configure `{name}` as a standard application window: {error}");
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
                        rust_platform_windows::configure_translation_window(handle).is_ok()
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
                    Ok(window) => native_window_handle(&window).is_some_and(|handle| {
                        configure_indicator_native_window(
                            handle,
                            None,
                            VisualizerStyle::from_index(indicator.get_visualizer_style())
                                .is_some_and(VisualizerStyle::uses_transparent_window),
                        )
                    }),
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
    slint_window_handle(indicator.window()).is_some_and(|handle| {
        configure_indicator_native_window(
            handle,
            target_window_handle,
            VisualizerStyle::from_index(indicator.get_visualizer_style())
                .is_some_and(VisualizerStyle::uses_transparent_window),
        )
    })
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

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn data_root() -> PathBuf {
    rust_app_core::product::data_root()
}

fn settings_path() -> PathBuf {
    data_root().join("settings.json")
}

struct SavedSettings {
    selected_model_id: Option<String>,
    output_mode: SpeechOutputMode,
    fast_release_previews: bool,
    visualizer_style: VisualizerStyle,
    circular_spectrum_palette: CircularSpectrumPalette,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    writing_slate_surface: WritingSlateSurface,
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
            circular_spectrum_palette: CircularSpectrumPalette::default(),
            flow_sphere_palette: flow_sphere::FlowSpherePalette::default(),
            writing_slate_surface: WritingSlateSurface::default(),
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
    let circular_spectrum_palette = value
        .as_ref()
        .and_then(|value| value.get("circular_spectrum_palette"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let flow_sphere_palette = value
        .as_ref()
        .and_then(|value| value.get("flow_sphere_palette"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let writing_slate_surface = value
        .as_ref()
        .and_then(|value| value.get("writing_slate_surface"))
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
        circular_spectrum_palette,
        flow_sphere_palette,
        writing_slate_surface,
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
        "circular_spectrum_palette": state.circular_spectrum_palette,
        "flow_sphere_palette": state.flow_sphere_palette,
        "writing_slate_surface": state.writing_slate_surface,
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

    use rust_protocol::AUDIO_SPECTRUM_BANDS;
    use slint::VecModel;

    use super::{
        CircularSpectrumPalette, IndicatorAnimation, IndicatorMode, SlateLine, VisualizerStyle,
        WritingSlateAnimation, WritingSlateSurface, flow_sphere,
        indicator_preview_slate_surface_for_argument, indicator_preview_style_for_argument,
    };

    fn animation() -> IndicatorAnimation {
        IndicatorAnimation::new(
            Rc::new(VecModel::from(vec![0.0; 10])),
            Rc::new(VecModel::from(vec![0.0; AUDIO_SPECTRUM_BANDS])),
            Rc::new(VecModel::from(Vec::<SlateLine>::new())),
        )
    }

    #[test]
    fn listening_frame_has_independent_bar_motion() {
        let mut animation = animation();
        animation.begin_listening();
        animation.set_microphone_signal(0.55, &[]);
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

    #[test]
    fn success_frames_collapse_and_fade_without_blocking() {
        let mut animation = animation();
        animation.begin_success();
        let first = animation.advance_frame(1.0);
        for _ in 0..12 {
            let _ = animation.advance_frame(1.0);
        }
        let later = animation.advance_frame(1.0);

        assert!(later.visual_scale < first.visual_scale);
        assert!(later.visual_opacity < first.visual_opacity);
        assert!(later.visual_scale >= 0.45);
        assert!(later.visual_opacity >= 0.0);
    }

    #[test]
    fn every_visible_visualizer_choice_maps_to_a_style() {
        assert_eq!(
            VisualizerStyle::from_index(0),
            Some(VisualizerStyle::VoiceOrb)
        );
        assert_eq!(
            VisualizerStyle::from_index(1),
            Some(VisualizerStyle::SymmetricSpectrum)
        );
        assert_eq!(
            VisualizerStyle::from_index(2),
            Some(VisualizerStyle::FluidRibbon)
        );
        assert_eq!(
            VisualizerStyle::from_index(3),
            Some(VisualizerStyle::BroadcastMic)
        );
        assert_eq!(
            VisualizerStyle::from_index(4),
            Some(VisualizerStyle::CircularSpectrum)
        );
        assert_eq!(
            VisualizerStyle::from_index(5),
            Some(VisualizerStyle::FlowSphere)
        );
        assert_eq!(
            VisualizerStyle::from_index(6),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            serde_json::to_string(&VisualizerStyle::FlowSphere).unwrap(),
            "\"flow_sphere\""
        );
        assert_eq!(
            serde_json::from_str::<VisualizerStyle>("\"flow_sphere_preview\"").unwrap(),
            VisualizerStyle::FlowSphere
        );
        assert_eq!(
            serde_json::to_string(&VisualizerStyle::WritingSlate).unwrap(),
            "\"writing_slate\""
        );
        assert_eq!(
            serde_json::from_str::<VisualizerStyle>("\"writing_slate_preview\"").unwrap(),
            VisualizerStyle::WritingSlate
        );
    }

    #[test]
    fn writing_slate_builds_short_segments_and_scrolls_after_two_thirds() {
        let mut slate = WritingSlateAnimation::default();
        for _ in 0..80 {
            slate.start_line();
            slate.commit_active();
        }
        let mut lines = Vec::new();
        for _ in 0..12 {
            lines = slate.advance(1.0, IndicatorMode::Listening, 0.0);
        }
        let colors = lines
            .iter()
            .map(|line| line.color_index)
            .collect::<std::collections::BTreeSet<_>>();

        assert!(colors.len() >= 3);
        assert!(lines.len() < 80);
        assert!(lines.iter().all(|line| line.width <= 30.0));
        assert!(
            lines
                .windows(2)
                .any(|pair| (pair[0].y - pair[1].y).abs() < 0.2)
        );
        assert!(lines.iter().all(|line| line.y <= 91.0));
    }

    #[test]
    fn writing_slate_is_available_in_settings_and_preview_modes() {
        assert_eq!(
            VisualizerStyle::from_index(6),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview=writing-slate"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate-transparent"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument(
                "--indicator-preview-live=slate-transparent"
            ),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview-live=slate"),
            None
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate-frosted"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview-live=slate-frosted"),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            WritingSlateSurface::from_index(0),
            Some(WritingSlateSurface::DarkPanel)
        );
        assert_eq!(
            WritingSlateSurface::from_index(1),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(WritingSlateSurface::from_index(2), None);
        assert_eq!(
            serde_json::to_string(&WritingSlateSurface::MatteGlass).unwrap(),
            "\"matte_glass\""
        );
    }

    #[test]
    fn flow_sphere_live_preview_is_explicitly_addressable() {
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=flow-sphere"),
            Some(VisualizerStyle::FlowSphere)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=sphere"),
            Some(VisualizerStyle::FlowSphere)
        );
    }

    #[test]
    fn flow_sphere_is_calm_in_silence_and_accelerates_on_voice_onset() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::FlowSphere);
        animation.begin_listening();

        let quiet_start = animation.phase;
        let _ = animation.advance_frame(1.0);
        let quiet_step = animation.phase - quiet_start;

        animation.set_microphone_signal(0.62, &[0.55; AUDIO_SPECTRUM_BANDS]);
        let active_start = animation.phase;
        let _ = animation.advance_frame(1.0);
        let active_step = animation.phase - active_start;

        assert!(quiet_step < 0.06);
        assert!(active_step > quiet_step * 3.0);
        assert!(animation.flow_voice_impulse > 0.2);
    }

    #[test]
    fn every_circular_palette_choice_maps_to_a_persisted_value() {
        assert_eq!(
            CircularSpectrumPalette::from_index(0),
            Some(CircularSpectrumPalette::Sunset)
        );
        assert_eq!(
            CircularSpectrumPalette::from_index(1),
            Some(CircularSpectrumPalette::Arctic)
        );
        assert_eq!(
            CircularSpectrumPalette::from_index(2),
            Some(CircularSpectrumPalette::Monochrome)
        );
        assert_eq!(CircularSpectrumPalette::from_index(3), None);
        assert_eq!(CircularSpectrumPalette::Sunset.index(), 0);
        assert_eq!(CircularSpectrumPalette::Arctic.index(), 1);
        assert_eq!(CircularSpectrumPalette::Monochrome.index(), 2);
        assert_eq!(
            serde_json::to_string(&CircularSpectrumPalette::Arctic).unwrap(),
            "\"arctic\""
        );
        assert_eq!(
            serde_json::from_str::<CircularSpectrumPalette>("\"monochrome\"").unwrap(),
            CircularSpectrumPalette::Monochrome
        );
    }

    #[test]
    fn every_flow_sphere_palette_maps_to_a_persisted_value() {
        assert_eq!(
            flow_sphere::FlowSpherePalette::from_index(0),
            Some(flow_sphere::FlowSpherePalette::Reference)
        );
        assert_eq!(
            flow_sphere::FlowSpherePalette::from_index(1),
            Some(flow_sphere::FlowSpherePalette::BlackYellow)
        );
        assert_eq!(flow_sphere::FlowSpherePalette::from_index(2), None);
        assert_eq!(flow_sphere::FlowSpherePalette::Reference.index(), 0);
        assert_eq!(flow_sphere::FlowSpherePalette::BlackYellow.index(), 1);
        assert_eq!(
            serde_json::to_string(&flow_sphere::FlowSpherePalette::BlackYellow).unwrap(),
            "\"black_yellow\""
        );
    }

    #[test]
    fn broadcast_microphone_lines_react_independently() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::BroadcastMic);
        animation.begin_listening();
        animation.set_microphone_signal(0.64, &[]);
        let first = animation.advance_frame(1.0);
        let second = animation.advance_frame(1.0);

        assert!((3.0..=12.0).contains(&first.mic_edge_height));
        assert!((4.0..=17.0).contains(&first.mic_outer_height));
        assert!((5.0..=18.0).contains(&first.mic_middle_height));
        assert!((3.0..=14.0).contains(&first.mic_inner_height));
        assert!((4.0..=15.0).contains(&first.mic_near_height));
        assert!(
            (first.mic_edge_height - second.mic_edge_height).abs() > 0.01
                || (first.mic_outer_height - second.mic_outer_height).abs() > 0.01
                || (first.mic_middle_height - second.mic_middle_height).abs() > 0.01
                || (first.mic_inner_height - second.mic_inner_height).abs() > 0.01
                || (first.mic_near_height - second.mic_near_height).abs() > 0.01
        );
    }

    #[test]
    fn circular_spectrum_preserves_real_band_differences() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::CircularSpectrum);
        animation.begin_listening();
        let mut bands = vec![0.0; AUDIO_SPECTRUM_BANDS];
        bands[5] = 1.0;
        bands[14] = 0.52;
        animation.set_microphone_signal(0.72, &bands);

        let frame = animation.advance_frame(1.0);
        assert_eq!(frame.spectrum_heights.len(), AUDIO_SPECTRUM_BANDS);
        assert!(frame.spectrum_heights[5] > frame.spectrum_heights[14]);
        assert!(frame.spectrum_heights[14] > frame.spectrum_heights[25]);
        assert!(
            frame
                .spectrum_heights
                .iter()
                .all(|height| (2.8..=21.5).contains(height))
        );
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
mod startup_session_gate_tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use rust_platform_windows::SessionEndEvent;

    use super::{StartupSessionGate, guarded_initial_spawn};

    #[test]
    fn confirmed_session_end_prevents_initial_worker_creation() {
        let gate = StartupSessionGate::new();
        gate.observe(SessionEndEvent::Confirmed);
        let spawn_count = AtomicUsize::new(0);

        let result = guarded_initial_spawn(
            &gate,
            || {
                spawn_count.fetch_add(1, Ordering::Relaxed);
                Ok::<_, ()>(())
            },
            || true,
            || {},
        )
        .unwrap();

        assert!(result.is_none());
        assert_eq!(spawn_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn query_blocks_initial_worker_creation_until_shutdown_is_cancelled() {
        let gate = Arc::new(StartupSessionGate::new());
        gate.observe(SessionEndEvent::Query);
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let worker_gate = Arc::clone(&gate);
        let worker_spawn_count = Arc::clone(&spawn_count);
        let thread = thread::spawn(move || {
            let result = guarded_initial_spawn(
                &worker_gate,
                || {
                    worker_spawn_count.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, ()>(())
                },
                || true,
                || {},
            );
            let _ = result_sender.send(result);
        });

        assert!(
            result_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        assert_eq!(spawn_count.load(Ordering::Relaxed), 0);

        gate.observe(SessionEndEvent::Cancelled);
        assert_eq!(
            result_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            Some(())
        );
        assert_eq!(spawn_count.load(Ordering::Relaxed), 1);
        thread.join().unwrap();
    }

    #[test]
    fn confirmation_during_spawn_discards_the_new_worker() {
        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let gate = Arc::new(StartupSessionGate::new());
        let dropped = Arc::new(AtomicUsize::new(0));
        let (spawned_sender, spawned_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let worker_gate = Arc::clone(&gate);
        let worker_dropped = Arc::clone(&dropped);
        let thread = thread::spawn(move || {
            guarded_initial_spawn(
                &worker_gate,
                || {
                    let _ = spawned_sender.send(());
                    let _ = release_receiver.recv();
                    Ok::<_, ()>(DropProbe(Arc::clone(&worker_dropped)))
                },
                || true,
                || {},
            )
            .unwrap()
        });

        spawned_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        gate.observe(SessionEndEvent::Confirmed);
        let _ = release_sender.send(());

        assert!(thread.join().unwrap().is_none());
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn failed_spawn_during_query_retries_only_after_cancellation() {
        let gate = Arc::new(StartupSessionGate::new());
        let session_is_ending = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let retries = Arc::new(AtomicUsize::new(0));
        let (failed_sender, failed_receiver) = mpsc::sync_channel(1);
        let worker_gate = Arc::clone(&gate);
        let worker_ending = Arc::clone(&session_is_ending);
        let worker_attempts = Arc::clone(&attempts);
        let worker_retries = Arc::clone(&retries);
        let thread = thread::spawn(move || {
            guarded_initial_spawn(
                &worker_gate,
                || {
                    let attempt = worker_attempts.fetch_add(1, Ordering::Relaxed);
                    if attempt == 0 {
                        worker_ending.store(true, Ordering::Release);
                        worker_gate.observe(SessionEndEvent::Query);
                        let _ = failed_sender.send(());
                        Err("loader unavailable")
                    } else {
                        Ok("worker")
                    }
                },
                || worker_ending.load(Ordering::Acquire),
                || {
                    worker_retries.fetch_add(1, Ordering::Relaxed);
                },
            )
        });

        failed_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        gate.observe(SessionEndEvent::Cancelled);
        session_is_ending.store(false, Ordering::Release);

        assert_eq!(thread.join().unwrap().unwrap(), Some("worker"));
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        assert_eq!(retries.load(Ordering::Relaxed), 1);
    }
}

#[cfg(test)]
mod worker_lifecycle_tests {
    use std::time::{Duration, Instant};

    use rust_platform_windows::ProcessExit;

    use super::{RestartAttempt, RestartPolicyDecision, WorkerLifecycle, decide_worker_restart};

    fn worker_exit(uptime: Duration) -> ProcessExit {
        ProcessExit {
            exit_code: Some(0xC000_0142),
            uptime,
        }
    }

    #[test]
    fn session_end_never_consumes_a_restart_attempt_or_spawns() {
        let mut attempts = Vec::new();
        let decision = decide_worker_restart(
            WorkerLifecycle::SessionEnding,
            true,
            worker_exit(Duration::from_millis(20)),
            &mut attempts,
            Instant::now(),
        );

        assert_eq!(decision, RestartPolicyDecision::Defer);
        assert!(attempts.is_empty());
    }

    #[test]
    fn confirmed_shutdown_never_restarts_a_worker() {
        let mut attempts = Vec::new();
        let decision = decide_worker_restart(
            WorkerLifecycle::Stopping,
            true,
            worker_exit(Duration::from_secs(30)),
            &mut attempts,
            Instant::now(),
        );

        assert_eq!(decision, RestartPolicyDecision::Stop);
        assert!(attempts.is_empty());
    }

    #[test]
    fn repeated_loader_failure_is_not_relaunched_forever() {
        let mut attempts = Vec::new();
        let now = Instant::now();
        assert_eq!(
            decide_worker_restart(
                WorkerLifecycle::Running,
                true,
                worker_exit(Duration::from_millis(25)),
                &mut attempts,
                now,
            ),
            RestartPolicyDecision::Restart
        );
        assert_eq!(
            decide_worker_restart(
                WorkerLifecycle::Running,
                true,
                worker_exit(Duration::from_millis(30)),
                &mut attempts,
                now + Duration::from_secs(1),
            ),
            RestartPolicyDecision::RejectRepeatedStartupFailure
        );
        assert_eq!(attempts.len(), 1);
    }

    #[test]
    fn stale_exit_from_replaced_process_is_ignored() {
        let mut attempts = vec![RestartAttempt {
            at: Instant::now(),
            startup_failure: false,
        }];
        let decision = decide_worker_restart(
            WorkerLifecycle::Running,
            false,
            worker_exit(Duration::from_secs(30)),
            &mut attempts,
            Instant::now(),
        );

        assert_eq!(decision, RestartPolicyDecision::Stale);
        assert_eq!(attempts.len(), 1);
    }
}

#[cfg(test)]
mod adaptive_popup_tests {
    use super::{TranslationPreview, adaptive_text_panel_height, translation_preview_layout};

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
    fn short_translation_uses_the_compact_width() {
        let layout = translation_preview_layout("Привет");
        assert_eq!(layout.card_width, 280.0);
    }

    #[test]
    fn long_translation_uses_the_readable_width() {
        let layout = translation_preview_layout(&"translated text ".repeat(500));
        assert_eq!(layout.card_width, 560.0);
    }

    #[test]
    fn wrapped_preview_uses_intrinsic_text_height_and_exposes_scroll_range() {
        let preview = TranslationPreview::new().unwrap();
        preview.set_card_width(560.0);
        preview.set_translated_text(
            ("Длинный переведённый текст с пропорциональным шрифтом и переносами. ".repeat(120))
                .into(),
        );

        assert_eq!(preview.get_result_panel_height(), 280.0);
        assert!(
            preview.get_result_content_height() > preview.get_result_panel_height(),
            "wrapped text must remain reachable below the capped viewport"
        );
        assert!(preview.get_result_scrollable());

        preview.invoke_reset_scroll();
        assert_eq!(preview.get_result_scroll_offset(), 0.0);

        preview.invoke_scroll_by(-120.0);
        assert_eq!(preview.get_result_scroll_offset(), 48.0);

        for _ in 0..1_000 {
            preview.invoke_scroll_by(-120.0);
        }
        let maximum = preview.get_result_content_height() - preview.get_result_panel_height();
        assert!((preview.get_result_scroll_offset() - maximum).abs() < 0.01);

        for _ in 0..1_000 {
            preview.invoke_scroll_by(120.0);
        }
        assert_eq!(preview.get_result_scroll_offset(), 0.0);
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

    #[test]
    fn failed_latest_session_falls_back_to_last_completed_transcript() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
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
