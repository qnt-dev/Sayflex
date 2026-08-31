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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rust_app_core::{
    BeginDisposition, EndDisposition, LayoutCandidate, LayoutDirection, SessionPhase,
    SessionRegistry, build_layout_candidates,
};
use rust_protocol::{
    AUDIO_SPECTRUM_BANDS, CompanionRendererCommand, CompanionRendererEvent, HistoryEntry,
    InputHostCommand, InputHostEvent, ModelSummary, PROTOCOL_VERSION, RequestId, SessionId,
    SpeechOutputMode, SpeechWorkerCommand, SpeechWorkerEvent, TargetSnapshot, TextActionIntent,
    Transcript, TranslationHistoryEntry, TranslationModelSummary, TranslationResult,
    TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, SharedString,
    Timer, TimerMode, VecModel,
};

mod flow_sphere;
mod overlay_presenter;
mod speech_backend;

use overlay_presenter::{OverlayContract, OverlayPresenter};
use speech_backend::{SpeechBackend, SpeechBackendPlan, SpeechBackendPreference};

slint::include_modules!();

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const INPUT_WORKER_EXE: &str = "SayFlex.InputHost.exe";
const SPEECH_WORKER_EXE: &str = "SayFlex.Speech.exe";
const TRANSLATION_WORKER_EXE: &str = "SayFlex.Translation.exe";
const COMPANION_RENDERER_EXE: &str = "SayFlex.Companion.exe";
const SPEECH_CPU_EXE: &str = "SayFlex.Speech.Cpu.exe";
const SPEECH_CUDA_EXE: &str = "SayFlex.Speech.Cuda.exe";
const SPEECH_VULKAN_EXE: &str = "SayFlex.Speech.Vulkan.exe";
const DEV_INPUT_WORKER_EXE: &str = "sayflex-input-host.exe";
const DEV_SPEECH_WORKER_EXE: &str = "sayflex-speech-worker.exe";
const DEV_TRANSLATION_WORKER_EXE: &str = "sayflex-translation-worker.exe";
const DEFAULT_TRANSLATION_BLUR_RADIUS: u32 = 3;
const MAX_TRANSLATION_BLUR_RADIUS: u32 = 8;
const DEV_COMPANION_RENDERER_EXE: &str = "sayflex-companion-renderer.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INDICATOR_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const INDICATOR_BAR_PROFILE: [f32; 10] = [0.25, 0.4, 0.62, 0.82, 1.0, 1.0, 0.82, 0.62, 0.4, 0.25];
const WORKER_RESTART_WINDOW: Duration = Duration::from_secs(60);
const WORKER_STARTUP_STABILITY: Duration = Duration::from_secs(2);
static NEXT_WORKER_INSTANCE: AtomicU64 = AtomicU64::new(1);

enum RuntimeEvent {
    Input(InputHostEvent),
    Speech {
        instance_id: u64,
        event: SpeechWorkerEvent,
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

#[derive(Clone)]
struct TranslationPreviewSlot {
    preview: Rc<TranslationPreview>,
    presenter: OverlayPresenter,
    attached: Rc<Cell<bool>>,
}

struct TranslationPreviewRegistry {
    current: TranslationPreviewSlot,
    current_request: Option<RequestId>,
    pinned: Vec<TranslationPreviewSlot>,
    blur_radius: u32,
}

impl TranslationPreviewRegistry {
    fn current(&self) -> TranslationPreviewSlot {
        self.current.clone()
    }

    fn slot_by_handle(&self, window_handle: u64) -> Option<TranslationPreviewSlot> {
        std::iter::once(&self.current)
            .chain(self.pinned.iter())
            .find(|slot| slint_window_handle(slot.preview.window()) == Some(window_handle))
            .cloned()
    }

    fn slot_for_request(&self, request_id: RequestId) -> Option<TranslationPreviewSlot> {
        let request_id = request_id.to_string();
        self.pinned
            .iter()
            .find(|slot| slot.preview.get_request_id().as_str() == request_id)
            .cloned()
            .or_else(|| {
                self.current_request
                    .is_some_and(|current| current.to_string() == request_id)
                    .then(|| self.current.clone())
            })
    }

    fn reserve_current(&mut self, request_id: RequestId) {
        self.current_request = Some(request_id);
    }

    fn clear_current_request(&mut self, request_id: RequestId) {
        if self.current_request == Some(request_id) {
            self.current_request = None;
        }
    }

    fn set_blur_radius(&mut self, radius: u32) {
        self.blur_radius = radius;
        for slot in std::iter::once(&self.current).chain(self.pinned.iter()) {
            slot.preview.set_blur_radius(radius as i32);
            if slot.preview.get_prepared()
                && !slot.preview.get_live_glass()
                && let Some(handle) = slint_window_handle(slot.preview.window())
            {
                refresh_cloaked_translation_backdrop(&slot.preview, handle);
            }
        }
    }
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
    speech_backends: SpeechBackendPlan,
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
    speech_backends: RefCell<SpeechBackendPlan>,
    translation_child: RefCell<WorkerProcess>,
    companion: RefCell<Option<JsonWriter>>,
    companion_child: RefCell<Option<WorkerProcess>>,
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
        if let Some(companion) = self.companion.borrow().as_ref() {
            let _ = companion.send(&CompanionRendererCommand::Shutdown);
        }
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
        match kind {
            WorkerKind::Input => self.input_child.borrow().instance_id == instance_id,
            WorkerKind::Speech => self.speech_child.borrow().instance_id == instance_id,
            WorkerKind::Translation => self.translation_child.borrow().instance_id == instance_id,
            WorkerKind::Companion => self
                .companion_child
                .borrow()
                .as_ref()
                .is_some_and(|process| process.instance_id == instance_id),
        }
    }

    fn active_speech_backend(&self) -> SpeechBackend {
        self.speech_backends.borrow().active()
    }

    fn speech_backend_preference(&self) -> SpeechBackendPreference {
        self.speech_backends.borrow().preference()
    }

    fn mark_speech_backend_ready(&self, reported: &str) -> Result<SpeechBackend, String> {
        self.speech_backends.borrow_mut().mark_verified(reported)
    }

    fn fallback_speech_backend(&self, reason: &str) -> Result<SpeechBackend, String> {
        loop {
            let previous = self.active_speech_backend();
            let next = self
                .speech_backends
                .borrow_mut()
                .reject_active_and_advance()
                .ok_or_else(|| {
                    format!(
                        "all installed speech backends failed; last backend was {}",
                        previous.display_name()
                    )
                })?;
            rust_app_core::diagnostics::log_line(&format!(
                "speech_backend preference={} rejected={} selected={} reason={reason}",
                self.speech_backend_preference().label(),
                previous.name(),
                next.name(),
            ));
            match spawn_speech_worker(&self._job, next) {
                Ok((child, writer)) => {
                    self.speech.replace_with(writer);
                    replace_process(&self.speech_child, child);
                    self.restart_attempts
                        .borrow_mut()
                        .remove(&WorkerKind::Speech);
                    return Ok(next);
                }
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "speech_backend selected={} spawn=failed error={error}",
                        next.name()
                    ));
                }
            }
        }
    }

    fn switch_speech_backend(
        &self,
        preference: SpeechBackendPreference,
    ) -> Result<SpeechBackend, String> {
        let mut plan = speech_backend_plan(preference)?;
        loop {
            let backend = plan.active();
            match spawn_speech_worker(&self._job, backend) {
                Ok((child, writer)) => {
                    self.speech.replace_with(writer);
                    replace_process(&self.speech_child, child);
                    *self.speech_backends.borrow_mut() = plan;
                    self.restart_attempts
                        .borrow_mut()
                        .remove(&WorkerKind::Speech);
                    rust_app_core::diagnostics::log_line(&format!(
                        "speech_backend preference={} selected={} reason=user_selection",
                        preference.label(),
                        backend.name()
                    ));
                    return Ok(backend);
                }
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "speech_backend preference={} selected={} spawn=failed error={error}",
                        preference.label(),
                        backend.name()
                    ));
                    if plan.reject_active_and_advance().is_none() {
                        return Err(format!(
                            "no speech backend could start for the {} preference",
                            preference.label()
                        ));
                    }
                }
            }
        }
    }

    fn ensure_companion(&self) -> Result<(), String> {
        if self.companion.borrow().is_some() {
            return Ok(());
        }
        if self.lifecycle.get() != WorkerLifecycle::Running
            || rust_platform_windows::session_is_ending()
        {
            return Err("the Companion Orb cannot start while the session is ending".to_owned());
        }
        let (child, writer) = spawn_worker(
            &self._job,
            WorkerKind::Companion,
            COMPANION_RENDERER_EXE,
            RuntimeEvent::Companion,
        )
        .map_err(|error| error.to_string())?;
        *self.companion_child.borrow_mut() = Some(child);
        *self.companion.borrow_mut() = Some(writer);
        rust_app_core::diagnostics::log_line("companion_renderer lifecycle=spawned_lazily");
        Ok(())
    }

    fn send_companion(&self, command: &CompanionRendererCommand) -> Result<(), String> {
        self.ensure_companion()?;
        self.companion
            .borrow()
            .as_ref()
            .ok_or_else(|| "the Companion Orb renderer is unavailable".to_owned())?
            .send(command)
    }

    fn send_companion_if_running(&self, command: &CompanionRendererCommand) -> Result<(), String> {
        let companion = self.companion.borrow();
        let Some(companion) = companion.as_ref() else {
            return Ok(());
        };
        companion.send(command)
    }

    fn recover(
        &self,
        kind: WorkerKind,
        instance_id: u64,
        exit: rust_platform_windows::ProcessExit,
    ) -> Result<WorkerRecovery, String> {
        if kind == WorkerKind::Speech {
            let lifecycle = if rust_platform_windows::session_is_ending()
                && self.lifecycle.get() == WorkerLifecycle::Running
            {
                self.lifecycle.set(WorkerLifecycle::SessionEnding);
                WorkerLifecycle::SessionEnding
            } else {
                self.lifecycle.get()
            };
            if !self.is_current_instance(kind, instance_id) {
                return Ok(WorkerRecovery::Stale);
            }
            match lifecycle {
                WorkerLifecycle::SessionEnding => {
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
                WorkerLifecycle::Stopping => return Ok(WorkerRecovery::Stopping),
                WorkerLifecycle::Running => {}
            }
            self.fallback_speech_backend("worker_exit")?;
            return Ok(WorkerRecovery::Restarted);
        }
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
            WorkerKind::Speech => unreachable!("speech recovery is handled before generic policy"),
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
            WorkerKind::Companion => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Companion,
                    COMPANION_RENDERER_EXE,
                    RuntimeEvent::Companion,
                )
                .map_err(|error| error.to_string())?;
                *self.companion_child.borrow_mut() = Some(child);
                *self.companion.borrow_mut() = Some(writer);
            }
        }
        Ok(WorkerRecovery::Restarted)
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
        if let Some(process) = self.companion_child.get_mut().as_mut()
            && process.child.try_wait().ok().flatten().is_none()
        {
            let _ = process.child.kill();
        }
    }
}

fn spawn_initial_workers(
    startup_gate: &StartupSessionGate,
    speech_preference: SpeechBackendPreference,
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
    let speech_backends = speech_backend_plan(speech_preference)?;
    let Some((speech_child, speech)) = guarded_initial_spawn(
        startup_gate,
        || spawn_speech_worker(&job, speech_backends.active()),
        rust_platform_windows::session_is_ending,
        || {
            rust_app_core::diagnostics::log_line(
                "worker=SayFlex.Speech initial_spawn=retry_after_session_end_cancelled",
            );
        },
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
        speech_backends,
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
    CompanionOrb,
}

/// Native composition is an HWND-creation-time contract, not a property to
/// toggle while a recording indicator is already alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingSurface {
    Panel,
    Transparent,
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
    BlueGlass,
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
            2 => Some(Self::BlueGlass),
            _ => None,
        }
    }

    const fn index(self) -> i32 {
        match self {
            Self::DarkPanel => 0,
            Self::MatteGlass => 1,
            Self::BlueGlass => 2,
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
            7 => Some(Self::CompanionOrb),
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
            Self::CompanionOrb => 7,
        }
    }

    const fn recording_surface(self) -> RecordingSurface {
        match self {
            Self::CircularSpectrum | Self::FlowSphere | Self::WritingSlate | Self::CompanionOrb => {
                RecordingSurface::Transparent
            }
            Self::VoiceOrb | Self::SymmetricSpectrum | Self::FluidRibbon | Self::BroadcastMic => {
                RecordingSurface::Panel
            }
        }
    }

    const fn uses_transparent_window(self) -> bool {
        matches!(self.recording_surface(), RecordingSurface::Transparent)
    }

    const fn uses_external_renderer(self) -> bool {
        matches!(self, Self::CompanionOrb)
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

    fn tick(&mut self, indicators: &[&RecordingIndicator]) {
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

        for indicator in indicators {
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
        configure_standard_app_window(settings.window(), "Settings");
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

#[derive(Clone)]
struct IndicatorModels {
    bars: Rc<VecModel<f32>>,
    spectrum: Rc<VecModel<f32>>,
    slate_lines: Rc<VecModel<SlateLine>>,
}

fn new_indicator_models() -> IndicatorModels {
    IndicatorModels {
        bars: Rc::new(VecModel::from(vec![
            4.0, 6.0, 9.0, 12.0, 15.0, 15.0, 12.0, 9.0, 6.0, 4.0,
        ])),
        spectrum: Rc::new(VecModel::from(vec![2.8; AUDIO_SPECTRUM_BANDS])),
        slate_lines: Rc::new(VecModel::from(Vec::<SlateLine>::new())),
    }
}

fn build_indicator(models: &IndicatorModels) -> Result<RecordingIndicator, slint::PlatformError> {
    let indicator = RecordingIndicator::new()?;
    indicator.set_bar_heights(ModelRc::from(Rc::clone(&models.bars)));
    indicator.set_spectrum_heights(ModelRc::from(Rc::clone(&models.spectrum)));
    indicator.set_slate_lines(ModelRc::from(Rc::clone(&models.slate_lines)));
    Ok(indicator)
}

fn new_indicator()
-> Result<(RecordingIndicator, Rc<RefCell<IndicatorAnimation>>), slint::PlatformError> {
    let models = new_indicator_models();
    let indicator = build_indicator(&models)?;
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(
        models.bars,
        models.spectrum,
        models.slate_lines,
    )));
    Ok((indicator, animation))
}

fn new_recording_indicators() -> Result<
    (
        RecordingIndicator,
        RecordingIndicator,
        Rc<RefCell<IndicatorAnimation>>,
    ),
    slint::PlatformError,
> {
    let models = new_indicator_models();
    let panel = build_indicator(&models)?;
    let transparent = build_indicator(&models)?;
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(
        models.bars,
        models.spectrum,
        models.slate_lines,
    )));
    Ok((panel, transparent, animation))
}

fn indicator_preview_style() -> Option<VisualizerStyle> {
    env::args().find_map(|argument| indicator_preview_style_for_argument(&argument))
}

fn indicator_presentation_probe_cycles() -> Option<u32> {
    env::args().find_map(|argument| {
        if argument == "--indicator-presentation-probe" {
            return Some(30);
        }
        argument
            .strip_prefix("--indicator-presentation-probe=")
            .and_then(|cycles| cycles.parse::<u32>().ok())
            .map(|cycles| cycles.clamp(1, 200))
    })
}

fn translation_presentation_probe_cycles() -> Option<u32> {
    env::args().find_map(|argument| {
        if argument == "--translation-presentation-probe" {
            return Some(30);
        }
        argument
            .strip_prefix("--translation-presentation-probe=")
            .and_then(|cycles| cycles.parse::<u32>().ok())
            .map(|cycles| cycles.clamp(1, 200))
    })
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
        "--indicator-preview=slate-blue-glass" | "--indicator-preview=writing-slate-blue-glass" => {
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
        "--indicator-preview-live=slate-blue-glass"
        | "--indicator-preview-live=writing-slate-blue-glass" => {
            Some(VisualizerStyle::WritingSlate)
        }
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
        "--indicator-preview=slate-blue-glass"
        | "--indicator-preview=writing-slate-blue-glass"
        | "--indicator-preview-live=slate-blue-glass"
        | "--indicator-preview-live=writing-slate-blue-glass" => {
            Some(WritingSlateSurface::BlueGlass)
        }
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
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.IndicatorPreview")?;
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
    let blue_glass = writing_slate_surface == WritingSlateSurface::BlueGlass;
    indicator.set_circular_palette(indicator_preview_palette().index());
    indicator.set_prepared(true);
    indicator.show()?;
    let indicator_weak = indicator.as_weak();
    slint::spawn_local(async move {
        if let Some(indicator) = indicator_weak.upgrade() {
            let handle = indicator
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            let configured = handle.is_some_and(|handle| {
                if blue_glass {
                    rust_platform_windows::configure_blue_glass_overlay_window(handle, None).is_ok()
                } else {
                    configure_indicator_native_window(handle, None, style.uses_transparent_window())
                }
            });
            if !configured {
                eprintln!("could not configure recording indicator preview");
            } else if blue_glass && let Some(handle) = handle {
                match rust_platform_windows::capture_blurred_overlay_backdrop(handle, 1) {
                    Ok(backdrop)
                        if backdrop.rgba.len()
                            == backdrop.width as usize * backdrop.height as usize * 4 =>
                    {
                        let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            &backdrop.rgba,
                            backdrop.width,
                            backdrop.height,
                        );
                        indicator.set_writing_slate_glass_backdrop(Image::from_rgba8(pixels));
                    }
                    Ok(_) => eprintln!("captured recording-indicator backdrop has invalid length"),
                    Err(error) => {
                        eprintln!("could not capture recording-indicator backdrop: {error}")
                    }
                }
            }
        }
    })?;

    let live_signal = Arc::new(Mutex::new(None::<(f32, Vec<f32>)>));
    let live_audio_worker = if live_microphone {
        let worker_job = rust_platform_windows::WorkerJob::new()
            .map_err(|error| format!("could not create preview worker job: {error}"))?;
        let backend = speech_backend_plan(SpeechBackendPreference::Auto)?.active();
        let path = speech_worker_path(backend)?;
        let mut command = Command::new(&path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if backend == SpeechBackend::Cuda {
            let runtime = trusted_cuda_runtime_directory()
                .ok_or("the CUDA preview worker is missing its verified runtime")?;
            command.env("PATH", runtime);
        }
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
        if let Some(indicator) = indicator.upgrade() {
            animation_for_timer.borrow_mut().tick(&[&indicator]);
        }
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

fn run_indicator_presentation_probe(cycles: u32) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.OverlayProbe")?;
    let _ = rust_app_core::diagnostics::initialize(
        &data_root().join("logs"),
        "recording-overlay-probe",
    );
    let (indicator, animation) = new_indicator()?;
    let presenter = OverlayPresenter::install(
        indicator.window(),
        "recording_overlay_probe",
        OverlayContract::ClickThrough,
        None,
    )?;
    indicator.set_visualizer_style(VisualizerStyle::WritingSlate.index());
    indicator.set_writing_slate_surface(WritingSlateSurface::BlueGlass.index());
    {
        let mut animation = animation.borrow_mut();
        animation.set_style(VisualizerStyle::WritingSlate);
        animation.begin_listening();
    }
    precreate_indicator(&indicator, &presenter, true);

    let animation_timer = start_indicator_animation(vec![indicator.as_weak()], &animation);
    let cycle_timer = Timer::default();
    let step = Rc::new(Cell::new(0_u32));
    let step_for_timer = Rc::clone(&step);
    let indicator_weak = indicator.as_weak();
    let presenter_for_timer = presenter.clone();
    let animation_for_timer = Rc::clone(&animation);
    cycle_timer.start(TimerMode::Repeated, Duration::from_millis(90), move || {
        let current = step_for_timer.get();
        if current >= cycles.saturating_mul(2).saturating_add(4) {
            if let Some(indicator) = indicator_weak.upgrade() {
                indicator.set_prepared(false);
                presenter_for_timer.retire(indicator.window(), "probe_complete");
            }
            let _ = slint::quit_event_loop();
            return;
        }
        step_for_timer.set(current.saturating_add(1));

        // Leave four event-loop turns for native creation and presenter
        // attachment, then exercise full frame-gated present/retire cycles.
        if current < 4 {
            return;
        }
        if current.is_multiple_of(2) {
            animation_for_timer.borrow_mut().begin_listening();
            if let Some(indicator) = indicator_weak.upgrade() {
                show_indicator(&indicator, &presenter_for_timer, true, None);
            }
        } else {
            animation_for_timer.borrow_mut().stop();
            if let Some(indicator) = indicator_weak.upgrade() {
                indicator.set_prepared(false);
                presenter_for_timer.retire(indicator.window(), "probe_cycle");
            }
        }
    });

    slint::run_event_loop()?;
    cycle_timer.stop();
    animation_timer.stop();
    let _ = rust_app_core::diagnostics::flush();
    Ok(())
}

fn run_translation_presentation_probe(cycles: u32) -> Result<(), Box<dyn Error>> {
    let _single_instance = rust_platform_windows::SingleInstanceGuard::acquire(
        "Local\\SayFlex.TranslationOverlayProbe",
    )?;
    let _ = rust_app_core::diagnostics::initialize(
        &data_root().join("logs"),
        "translation-overlay-probe",
    );
    let preview = TranslationPreview::new()?;
    let presenter = OverlayPresenter::install(
        preview.window(),
        "translation_preview_probe",
        OverlayContract::Interactive,
        None,
    )?;
    preview.set_prepared(false);
    preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    preview.show()?;
    {
        let preview = preview.as_weak();
        let presenter = presenter.clone();
        slint::spawn_local(async move {
            if let Some(preview) = preview.upgrade() {
                let result = preview
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .ok_or_else(|| "translation probe HWND is unavailable".to_owned())
                    .and_then(|handle| {
                        rust_platform_windows::prepare_translation_window(handle)?;
                        presenter.attach(handle)
                    });
                if let Err(error) = result {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_probe transition=failed stage=attach error={error}"
                    ));
                }
            }
        })?;
    }

    let cycle_timer = Timer::default();
    let step = Rc::new(Cell::new(0_u32));
    let step_for_timer = Rc::clone(&step);
    let preview_weak = preview.as_weak();
    let presenter_for_timer = presenter.clone();
    cycle_timer.start(TimerMode::Repeated, Duration::from_millis(220), move || {
        let current = step_for_timer.get();
        if current >= cycles.saturating_mul(2).saturating_add(4) {
            if let Some(preview) = preview_weak.upgrade() {
                preview.set_prepared(false);
                presenter_for_timer.retire(preview.window(), "probe_complete");
            }
            let _ = slint::quit_event_loop();
            return;
        }
        step_for_timer.set(current.saturating_add(1));
        if current < 4 {
            return;
        }
        let Some(preview) = preview_weak.upgrade() else {
            return;
        };
        if current.is_multiple_of(2) {
            let text = if (current / 2).is_multiple_of(2) {
                "Frame-gated glass preview"
            } else {
                "The translation overlay is rendered completely before DWM reveals its native window."
            };
            apply_translation_preview_layout(&preview, text);
            preview.set_translated_text(text.into());
            preview.set_prepared(false);
            let ticket = match presenter_for_timer.begin_prepare() {
                Ok(ticket) => ticket,
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_probe transition=failed stage=begin_prepare \
error={error}"
                    ));
                    return;
                }
            };
            let Some(handle) = slint_window_handle(preview.window()) else {
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "configure",
                    "native HWND is unavailable",
                );
                return;
            };
            if let Err(error) = rust_platform_windows::configure_translation_window(handle) {
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "configure",
                    &error,
                );
                return;
            }
            refresh_cloaked_translation_backdrop(&preview, handle);
            preview.set_prepared(true);
            if let Err(error) = presenter_for_timer.await_frame(preview.window(), ticket) {
                preview.set_prepared(false);
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "await_frame",
                    &error,
                );
            }
        } else {
            preview.set_prepared(false);
            presenter_for_timer.retire(preview.window(), "probe_cycle");
        }
    });

    slint::run_event_loop()?;
    cycle_timer.stop();
    let _ = rust_app_core::diagnostics::flush();
    Ok(())
}

fn run_action_palette_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.ActionPreview")?;
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
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.TranslationPreview")?;
    let preview = TranslationPreview::new()?;
    let presenter = OverlayPresenter::install(
        preview.window(),
        "translation_preview_probe",
        OverlayContract::Interactive,
        Some(Arc::new(rust_platform_windows::activate_popup_dismiss)),
    )?;
    let translated = if long_text {
        "Длинный перевод должен оставаться удобным для чтения. Карточка сохраняет компактную форму, а результат прокручивается внутри неё. Это позволяет быстро проверить смысл, не терять выбранное место в исходном приложении и не открывать отдельное большое рабочее окно. Прокрутка появляется только тогда, когда текст действительно не помещается. ".repeat(12)
    } else {
        "Мужчина-пассажир звонит в колокольчик проводника из купе мистера Рэтчетта. Проводник подходит к двери. Пассажир отвечает по-французски."
            .to_owned()
    };
    let source = if long_text {
        "A long translation should remain easy to read. The card keeps its compact shape while the result scrolls inside it. This makes it possible to verify the meaning without losing the selected place in the source application."
    } else {
        "A male passenger rings the conductor's bell from Mr. Ratchett's compartment. The conductor comes to the door. The passenger answers in French."
    };
    let dual_pairs = if long_text {
        vec![TranslationPair {
            source: source.into(),
            translation: translated.clone().into(),
        }]
    } else {
        vec![
            TranslationPair {
                source:
                    "A male passenger rings the conductor's bell from Mr. Ratchett's compartment."
                        .into(),
                translation:
                    "Мужчина-пассажир звонит в колокольчик проводника из купе мистера Рэтчетта."
                        .into(),
            },
            TranslationPair {
                source: "The conductor comes to the door.".into(),
                translation: "Проводник подходит к двери.".into(),
            },
            TranslationPair {
                source: "The passenger answers in French.".into(),
                translation: "Пассажир отвечает по-французски.".into(),
            },
        ]
    };
    apply_translation_preview_layout(&preview, &translated);
    preview.set_card_width(480.0);
    preview.set_source_text(source.into());
    preview.set_original_card_height(if long_text { 124.0 } else { 96.0 });
    preview.set_dual_pairs(ModelRc::new(VecModel::from(dual_pairs)));
    preview.set_translated_text(translated.clone().into());
    preview.set_prepared(false);
    wire_translation_preview(&preview, &presenter);
    preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    preview.show()?;
    let preview_weak = preview.as_weak();
    let presenter_for_window = presenter.clone();
    slint::spawn_local(async move {
        if let Some(preview) = preview_weak.upgrade() {
            let handle = preview
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            let prepared = handle
                .ok_or_else(|| "native translation window is unavailable".to_owned())
                .and_then(rust_platform_windows::prepare_translation_window)
                .and_then(|()| presenter_for_window.attach(handle.unwrap_or_default()));
            if let Err(error) = prepared {
                eprintln!("could not configure translation preview: {error}");
            } else if let Some(handle) = handle {
                let ticket = match presenter_for_window.begin_prepare() {
                    Ok(ticket) => ticket,
                    Err(error) => {
                        eprintln!("could not prepare translation preview: {error}");
                        return;
                    }
                };
                if let Err(error) = rust_platform_windows::configure_translation_window(handle) {
                    presenter_for_window.abort_prepare(
                        preview.window(),
                        ticket,
                        "configure",
                        &error,
                    );
                    return;
                }
                refresh_cloaked_translation_backdrop(&preview, handle);
                preview.set_prepared(true);
                if let Err(error) = presenter_for_window.await_frame(preview.window(), ticket) {
                    preview.set_prepared(false);
                    presenter_for_window.abort_prepare(
                        preview.window(),
                        ticket,
                        "await_frame",
                        &error,
                    );
                }
            }
        }
    })?;
    // The design probe intentionally has no global click-away monitor. It must
    // remain visible while the user compares ORIG and DUAL, even after clicking
    // another window. Production keeps its normal popup lifecycle.
    slint::run_event_loop()?;
    Ok(())
}

fn run_layout_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.LayoutPreview")?;
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
    spawn_worker_at(worker_job, kind, executable, path, None, move |_, event| {
        wrap(event)
    })
}

fn spawn_speech_worker(
    worker_job: &rust_platform_windows::WorkerJob,
    backend: SpeechBackend,
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>> {
    let path = speech_worker_path(backend)?;
    let runtime_directory = (backend == SpeechBackend::Cuda)
        .then(trusted_cuda_runtime_directory)
        .flatten();
    if backend == SpeechBackend::Cuda && runtime_directory.is_none() {
        return Err("the CUDA worker is missing its verified runtime".into());
    }
    spawn_worker_at(
        worker_job,
        WorkerKind::Speech,
        SPEECH_WORKER_EXE,
        path,
        runtime_directory,
        |instance_id, event| RuntimeEvent::Speech { instance_id, event },
    )
}

fn spawn_worker_at<T, F>(
    worker_job: &rust_platform_windows::WorkerJob,
    kind: WorkerKind,
    executable: &str,
    path: PathBuf,
    runtime_directory: Option<PathBuf>,
    wrap: F,
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
    F: Fn(u64, T) -> RuntimeEvent + Send + 'static,
{
    let started_at = Instant::now();
    let instance_id = NEXT_WORKER_INSTANCE.fetch_add(1, Ordering::Relaxed);
    let mut command = Command::new(&path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(runtime_directory) = runtime_directory {
        // The CUDA executable imports cuBLAS before Rust main() can harden dynamic
        // DLL loading. Give that child only the validated application runtime
        // (or a trusted system toolkit fallback) instead of inheriting PATH.
        command.env("PATH", runtime_directory);
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
                        dispatch_runtime_event(wrap(instance_id, event));
                    }
                    Err(error) => {
                        dispatch_runtime_event(RuntimeEvent::WorkerError {
                            kind,
                            instance_id,
                            message: format!("{output_name} sent invalid data: {error}"),
                        });
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
    let sibling = parent.join(executable);
    if sibling.exists() {
        return Ok(sibling);
    }
    let development = match executable {
        INPUT_WORKER_EXE => Some(DEV_INPUT_WORKER_EXE),
        TRANSLATION_WORKER_EXE => Some(DEV_TRANSLATION_WORKER_EXE),
        COMPANION_RENDERER_EXE => Some(DEV_COMPANION_RENDERER_EXE),
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

fn speech_worker_path(backend: SpeechBackend) -> Result<PathBuf, Box<dyn Error>> {
    let parent = env::current_exe()?
        .parent()
        .ok_or("desktop executable has no parent directory")?
        .to_owned();
    let installed = parent.join(match backend {
        SpeechBackend::Cuda => SPEECH_CUDA_EXE,
        SpeechBackend::Vulkan => SPEECH_VULKAN_EXE,
        SpeechBackend::Cpu => SPEECH_CPU_EXE,
    });
    if installed.is_file() {
        return Ok(installed);
    }
    if backend == SpeechBackend::Cpu {
        let development = parent.join(DEV_SPEECH_WORKER_EXE);
        if development.is_file() {
            return Ok(development);
        }
    }
    Err(format!(
        "the {} speech worker is not installed at {}",
        backend.display_name(),
        installed.display()
    )
    .into())
}

fn available_speech_backends() -> Vec<SpeechBackend> {
    [
        SpeechBackend::Cuda,
        SpeechBackend::Vulkan,
        SpeechBackend::Cpu,
    ]
    .into_iter()
    .filter(|backend| {
        speech_worker_path(*backend).is_ok()
            && (*backend != SpeechBackend::Cuda || trusted_cuda_runtime_directory().is_some())
    })
    .collect()
}

fn speech_backend_plan(preference: SpeechBackendPreference) -> Result<SpeechBackendPlan, String> {
    SpeechBackendPlan::new(preference, available_speech_backends())
}

fn trusted_cuda_runtime_directory() -> Option<PathBuf> {
    let application_runtime = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .map(|path| path.join("runtimes").join("cuda"));
    if let Some(directory) = application_runtime
        .filter(|directory| cuda_runtime_is_complete(directory))
        .and_then(|directory| directory.canonicalize().ok())
    {
        return Some(directory);
    }

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
        cuda_runtime_is_complete(&directory).then_some(directory)
    })
}

fn cuda_runtime_is_complete(directory: &std::path::Path) -> bool {
    directory.join("cublas64_13.dll").is_file() && directory.join("cublasLt64_13.dll").is_file()
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
            Err(error) => set_indicator_status(
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
            if kind == WorkerKind::Speech {
                runtime.state.borrow_mut().speech_backend_active = None;
                sync_action_settings(&runtime.settings, &runtime.state);
            }
            if let Some(settings) = runtime.settings.upgrade() {
                settings.set_runtime_status("Worker unavailable".into());
                settings.set_indicator_status(format!("{message}; {error}").into());
            }
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
                set_indicator_status(settings, &format!("Could not start recording: {error}"));
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
                    set_indicator_status(settings, &format!("Could not finish recording: {error}"));
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
                    let _ = workers
                        .speech
                        .send(&SpeechWorkerCommand::GetHistory { limit: 20 });
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

fn show_layout_chooser(
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    chooser_presenter: &OverlayPresenter,
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
        chooser.set_prepared(false);
        let ticket = match chooser_presenter.begin_prepare() {
            Ok(ticket) => ticket,
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "selection_chooser_overlay transition=failed stage=begin_prepare error={error}"
                ));
                return;
            }
        };
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
        let configured = slint_window_handle(chooser.window())
            .ok_or_else(|| "native selection-chooser HWND is unavailable".to_owned())
            .and_then(|handle| {
                rust_platform_windows::configure_action_window_for_target(
                    handle,
                    target.window_handle,
                )
            });
        if let Err(error) = configured {
            chooser_presenter.abort_prepare(chooser.window(), ticket, "configure", &error);
            return;
        }
        chooser.set_prepared(true);
        if let Err(error) = chooser_presenter.await_frame(chooser.window(), ticket) {
            chooser.set_prepared(false);
            chooser_presenter.abort_prepare(chooser.window(), ticket, "await_frame", &error);
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
        set_indicator_status(settings, &format!("Could not start translation: {error}"));
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
        if let Err(error) = configure_slint_translation_resize(&preview, false) {
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview resize_state=failed pinned=false reason=first_show error={error}"
            ));
        }
        preview.set_live_glass(false);
        if let Some(handle) = slint_window_handle(preview.window()) {
            let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
        }
    }
    apply_translation_preview_layout(&preview, translated_text);
    preview.set_translated_text(translated_text.into());
    preview.set_dual_pairs(ModelRc::new(VecModel::from(translation_pairs(
        source_text,
        translated_text,
    ))));
    if first_show {
        preview.set_prepared(false);
        preview.invoke_reset_scroll();
        if slot.attached.get()
            && let Err(error) = present_translation_preview(
                &preview,
                translation_preview_presenter,
                Some(target.window_handle),
            )
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
        refresh_cloaked_translation_backdrop(&preview, handle);
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

fn translation_pairs(source: &str, translation: &str) -> Vec<TranslationPair> {
    let source_units = translation_units(source);
    let translation_units = translation_units(translation);
    if source_units.len() > 1 && source_units.len() == translation_units.len() {
        return source_units
            .into_iter()
            .zip(translation_units)
            .map(|(source, translation)| TranslationPair {
                source: source.into(),
                translation: translation.into(),
            })
            .collect();
    }

    vec![TranslationPair {
        source: source.trim().into(),
        translation: translation.trim().into(),
    }]
}

fn translation_units(text: &str) -> Vec<String> {
    let characters = text.chars().collect::<Vec<_>>();
    let mut units = Vec::new();
    let mut current = String::new();
    let mut token = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        current.push(character);
        if character.is_alphabetic() {
            token.push(character);
        }
        let punctuation = matches!(character, '.' | '!' | '?' | '。' | '！' | '？');
        if !punctuation {
            if !character.is_alphabetic() && character != '\'' && character != '’' {
                token.clear();
            }
            continue;
        }

        let next = characters.get(index + 1).copied();
        let numeric_period = character == '.'
            && index > 0
            && characters[index - 1].is_ascii_digit()
            && next.is_some_and(|next| next.is_ascii_digit());
        let abbreviation = character == '.' && is_sentence_abbreviation(&token);
        let boundary = !numeric_period
            && !abbreviation
            && next
                .is_none_or(|next| next.is_whitespace() || matches!(next, '"' | '”' | ')' | ']'));
        token.clear();
        if boundary {
            let unit = current.trim();
            if !unit.is_empty() {
                units.push(unit.to_owned());
            }
            current.clear();
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        units.push(tail.to_owned());
    }
    if units.is_empty() && !text.trim().is_empty() {
        units.push(text.trim().to_owned());
    }
    units
}

fn is_sentence_abbreviation(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "mr" | "mrs" | "ms" | "dr" | "prof" | "sr" | "jr" | "st" | "vs" | "etc" | "e" | "i"
    )
}

fn present_translation_preview(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    let ticket = presenter.begin_prepare()?;
    let handle = slint_window_handle(preview.window())
        .ok_or_else(|| "native translation-preview HWND is unavailable".to_owned())?;
    if let Err(error) = rust_platform_windows::configure_translation_window(handle) {
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
    if let Some(target_window_handle) = target_window_handle
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
    initialization?;
    Ok(backend)
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

fn wire_settings(
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    recording_overlays: &RecordingOverlays,
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
        settings.on_cycle_speech_backend(move || {
            let preference = {
                let state = state.borrow();
                SpeechBackendPreference::from_index(
                    (state.speech_backend_preference.index() + 1) % 4,
                )
                .unwrap_or_default()
            };
            match workers.switch_speech_backend(preference) {
                Ok(active) => {
                    let initialization = {
                        let mut state = state.borrow_mut();
                        state.speech_backend_preference = preference;
                        state.speech_backend_active = Some(active);
                        state.speech_ready = false;
                        workers.speech.send(&SpeechWorkerCommand::Initialize {
                            selected_model_id: state.selected_model_id.clone(),
                            output_mode: state.output_mode,
                            history_enabled: state.history_enabled,
                            fast_release_previews: state.fast_release_previews,
                        })
                    };
                    let _ = save_settings(&state.borrow());
                    sync_action_settings(&settings_weak, &state);
                    if let Err(error) = initialization {
                        set_indicator_status(
                            &settings_weak,
                            &format!("Speech backend initialization failed: {error}"),
                        );
                    } else {
                        set_runtime_status(&settings_weak, "Checking backend");
                        set_indicator_status(
                            &settings_weak,
                            &format!(
                                "Checking {} with the selected Whisper model…",
                                active.display_name()
                            ),
                        );
                    }
                }
                Err(error) => set_indicator_status(
                    &settings_weak,
                    &format!("Speech backend could not be changed: {error}"),
                ),
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
        let recording_overlays = recording_overlays.clone();
        let indicator_animation = Rc::clone(indicator_animation);
        let workers = Rc::clone(workers);
        settings.on_select_visualizer(move |index| {
            let Some(style) = VisualizerStyle::from_index(index) else {
                return;
            };
            if style.uses_external_renderer() {
                if let Err(error) = workers.ensure_companion().and_then(|()| {
                    workers.send_companion(&CompanionRendererCommand::SetEnabled { enabled: true })
                }) {
                    set_indicator_status(
                        &settings_weak,
                        &format!("Could not start Companion Orb: {error}"),
                    );
                    return;
                }
            } else {
                let _ = workers.send_companion_if_running(&CompanionRendererCommand::SetEnabled {
                    enabled: false,
                });
            }
            let session_active = state.borrow().indicator_session.is_some();
            state.borrow_mut().visualizer_style = style;
            if !session_active {
                indicator_animation.borrow_mut().set_style(style);
                recording_overlays.set_style(style);
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                if style.uses_external_renderer() {
                    "Companion Orb is active on the desktop."
                } else {
                    "Visualizer updated. The next dictation uses the selected style."
                },
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let recording_overlays = recording_overlays.clone();
        let state = Rc::clone(state);
        settings.on_select_circular_palette(move |index| {
            let Some(palette) = CircularSpectrumPalette::from_index(index) else {
                return;
            };
            state.borrow_mut().circular_spectrum_palette = palette;
            recording_overlays.set_circular_palette(palette);
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
        let recording_overlays = recording_overlays.clone();
        let state = Rc::clone(state);
        settings.on_select_writing_slate_surface(move |index| {
            let Some(surface) = WritingSlateSurface::from_index(index) else {
                return;
            };
            state.borrow_mut().writing_slate_surface = surface;
            recording_overlays.set_writing_slate_surface(surface);
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
        let settings_weak = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        settings.on_decrease_translation_blur(move || {
            let radius = {
                let mut state = state.borrow_mut();
                state.translation_blur_radius = state.translation_blur_radius.saturating_sub(1);
                state.translation_blur_radius
            };
            translation_previews.borrow_mut().set_blur_radius(radius);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        settings.on_increase_translation_blur(move || {
            let radius = {
                let mut state = state.borrow_mut();
                state.translation_blur_radius =
                    (state.translation_blur_radius + 1).min(MAX_TRANSLATION_BLUR_RADIUS);
                state.translation_blur_radius
            };
            translation_previews.borrow_mut().set_blur_radius(radius);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_quit_requested(move || {
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=exit_requested reason=settings_quit",
            );
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
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=exit_requested reason=tray_quit",
            );
            workers.shutdown();
            let _ = slint::quit_event_loop();
        });
    }
}

fn wire_chooser(
    chooser: &SelectionChooser,
    presenter: &OverlayPresenter,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let presenter = presenter.clone();
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
                chooser.set_prepared(false);
                presenter.retire(chooser.window(), "candidate_selected");
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
        let presenter = presenter.clone();
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
                chooser.set_prepared(false);
                presenter.retire(chooser.window(), "cancelled");
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn wire_action_palette(
    palette: &ActionPalette,
    palette_presenter: &OverlayPresenter,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    chooser: &SelectionChooser,
    chooser_presenter: &OverlayPresenter,
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let palette_weak = palette.as_weak();
        let settings = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let palette_presenter = palette_presenter.clone();
        palette.on_translate(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                palette.set_prepared(false);
                palette_presenter.retire(palette.window(), "translate_selected");
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
                &translation_previews,
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
        let palette_presenter = palette_presenter.clone();
        let chooser_presenter = chooser_presenter.clone();
        palette.on_correct_layout(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                palette.set_prepared(false);
                palette_presenter.retire(palette.window(), "layout_selected");
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
                &chooser_presenter,
                &state,
            );
        });
    }
    {
        let palette_weak = palette.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let presenter = palette_presenter.clone();
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
                palette.set_prepared(false);
                presenter.retire(palette.window(), "cancelled");
            }
        });
    }
}

fn wire_translation_preview(preview: &TranslationPreview, presenter: &OverlayPresenter) {
    let drag_active = Arc::new(AtomicBool::new(false));
    {
        let preview_weak = preview.as_weak();
        preview.on_pin_state_changed(move |pinned| {
            if let Some(preview) = preview_weak.upgrade() {
                if let Some(window_handle) = slint_window_handle(preview.window()) {
                    dispatch_runtime_event(RuntimeEvent::TranslationPreviewPinChanged {
                        window_handle,
                        pinned,
                    });
                }
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        preview.on_begin_resize(move |edge| {
            let Some(preview) = preview_weak.upgrade() else {
                return;
            };
            if !preview.get_pinned() {
                return;
            }
            if let Err(error) = begin_slint_window_resize(preview.window(), edge) {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview resize=failed stage=native_resize error={error}"
                ));
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        preview.on_view_mode_changed(move |_| {
            let Some(preview) = preview_weak.upgrade() else {
                return;
            };
            if preview.get_pinned()
                && let Err(error) = configure_slint_translation_resize(&preview, true)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview resize_state=failed pinned=true reason=view_mode error={error}"
                ));
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        let presenter = presenter.clone();
        let drag_active = Arc::clone(&drag_active);
        preview.on_begin_drag(move || {
            if let Some(preview) = preview_weak.upgrade()
                && preview.get_pinned()
            {
                if let Err(error) = begin_slint_window_drag(preview.window()) {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=native_drag error={error}"
                    ));
                }
                return;
            }
            if drag_active.swap(true, Ordering::AcqRel) {
                return;
            }
            if let Some(preview) = preview_weak.upgrade()
                && let Some(handle) = slint_window_handle(preview.window())
            {
                preview.set_live_glass(true);
                if let Err(error) = rust_platform_windows::enable_translation_live_backdrop(handle)
                {
                    drag_active.store(false, Ordering::Release);
                    preview.set_live_glass(false);
                    let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=enable_live_glass error={error}"
                    ));
                    return;
                }

                let preview_after_frame = preview_weak.clone();
                let presenter_after_frame = presenter.clone();
                let drag_after_frame = Arc::clone(&drag_active);
                let queue_result = presenter.after_next_frame(preview.window(), move || {
                    let Some(preview) = preview_after_frame.upgrade() else {
                        drag_after_frame.store(false, Ordering::Release);
                        return;
                    };
                    if !preview.get_prepared()
                        || !preview.get_live_glass()
                        || slint_window_handle(preview.window()) != Some(handle)
                    {
                        drag_after_frame.store(false, Ordering::Release);
                        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                        return;
                    }

                    if let Err(error) = begin_slint_window_drag(preview.window()) {
                        drag_after_frame.store(false, Ordering::Release);
                        preview.set_live_glass(false);
                        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                        rust_app_core::diagnostics::log_line(&format!(
                            "translation_preview drag=failed stage=native_drag error={error}"
                        ));
                        return;
                    }

                    start_translation_drag_watcher(
                        preview_after_frame,
                        presenter_after_frame,
                        drag_after_frame,
                        handle,
                    );
                });
                if let Err(error) = queue_result {
                    drag_active.store(false, Ordering::Release);
                    preview.set_live_glass(false);
                    let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=queue_rendered_frame error={error}"
                    ));
                }
                return;
            }
            drag_active.store(false, Ordering::Release);
        });
    }
}

fn create_translation_preview_slot(blur_radius: u32) -> Result<TranslationPreviewSlot, String> {
    let preview = Rc::new(
        TranslationPreview::new()
            .map_err(|error| format!("could not create translation window: {error}"))?,
    );
    preview.set_blur_radius(blur_radius as i32);
    let presenter = OverlayPresenter::install(
        preview.window(),
        "translation_preview_overlay",
        OverlayContract::Interactive,
        Some(Arc::new(rust_platform_windows::activate_popup_dismiss)),
    )?;
    wire_translation_preview(&preview, &presenter);
    Ok(TranslationPreviewSlot {
        preview,
        presenter,
        attached: Rc::new(Cell::new(false)),
    })
}

fn retire_translation_preview(slot: &TranslationPreviewSlot, reason: &str) {
    slot.preview.set_pinned(false);
    if let Err(error) = configure_slint_translation_resize(&slot.preview, false) {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview resize_state=failed pinned=false reason={reason} error={error}"
        ));
    }
    slot.preview.set_live_glass(false);
    if let Some(handle) = slint_window_handle(slot.preview.window()) {
        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
        rust_platform_windows::deactivate_popup_dismiss(handle);
    }
    slot.preview.set_prepared(false);
    slot.preview.set_request_id("".into());
    slot.presenter.retire(slot.preview.window(), reason);
}

fn apply_translation_preview_pin_state(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    pinned: bool,
) -> Result<(), String> {
    preview.set_pinned(pinned);
    configure_slint_translation_resize(preview, pinned)?;

    let Some(window_handle) = slint_window_handle(preview.window()) else {
        return Err("translation window handle is not available".to_owned());
    };
    if pinned {
        // A resizable surface must have one compositor owner for its entire
        // pinned lifetime. DWM continuously recomposes this material as the
        // HWND changes size; no captured bitmap participates in resize.
        // The native material is explicitly dark, so its typography must not
        // inherit a contrast decision made from the previous captured frame.
        preview.set_dark_text(false);
        preview.set_live_glass(true);
        rust_platform_windows::enable_translation_live_backdrop(window_handle)?;
        preview.window().request_redraw();
        return Ok(());
    }

    if preview.get_prepared() && preview.get_live_glass() {
        settle_translation_preview_backdrop(preview, presenter, window_handle)
    } else {
        preview.set_live_glass(false);
        rust_platform_windows::disable_translation_live_backdrop(window_handle)
    }
}

fn configure_slint_translation_resize(
    preview: &TranslationPreview,
    resizable: bool,
) -> Result<(), String> {
    use slint::winit_030::winit::dpi::LogicalSize;

    let minimum_height = if preview.get_view_mode() == 1 {
        f64::from(preview.get_original_card_height()) + 98.0
    } else {
        80.0
    };
    preview
        .window()
        .with_winit_window(|window| {
            window.set_min_inner_size(
                resizable.then_some(LogicalSize::new(252.0_f64, minimum_height)),
            );
            window.set_resizable(resizable);
        })
        .ok_or_else(|| "translation window is not backed by winit".to_owned())
}

fn begin_slint_window_resize(window: &slint::Window, edge: i32) -> Result<(), String> {
    use slint::winit_030::winit::window::ResizeDirection;

    let direction = match edge {
        0 => ResizeDirection::West,
        1 => ResizeDirection::East,
        2 => ResizeDirection::North,
        3 => ResizeDirection::South,
        4 => ResizeDirection::NorthWest,
        5 => ResizeDirection::NorthEast,
        6 => ResizeDirection::SouthWest,
        7 => ResizeDirection::SouthEast,
        _ => return Err(format!("invalid translation resize edge {edge}")),
    };
    window
        .with_winit_window(|window| {
            window
                .drag_resize_window(direction)
                .map_err(|error| error.to_string())
        })
        .ok_or_else(|| "translation window is not backed by winit".to_owned())?
}

fn start_translation_drag_watcher(
    preview_after_drag: slint::Weak<TranslationPreview>,
    presenter_after_drag: OverlayPresenter,
    drag_active: Arc<AtomicBool>,
    window_handle: u64,
) {
    let watcher_active = Arc::clone(&drag_active);
    let preview_on_spawn_failure = preview_after_drag.clone();
    let presenter_on_spawn_failure = presenter_after_drag.clone();
    let spawn_result = std::thread::Builder::new()
        .name("translation-preview-drag-watch".to_owned())
        .spawn(move || {
            wait_for_primary_pointer_release();
            watcher_active.store(false, Ordering::Release);
            let _ = preview_after_drag.upgrade_in_event_loop(move |preview| {
                if preview.get_prepared()
                    && preview.get_live_glass()
                    && slint_window_handle(preview.window()) == Some(window_handle)
                    && let Err(error) = settle_translation_preview_backdrop(
                        &preview,
                        &presenter_after_drag,
                        window_handle,
                    )
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview backdrop_refresh=failed error={error}"
                    ));
                }
            });
        });
    if let Err(error) = spawn_result {
        drag_active.store(false, Ordering::Release);
        if let Some(preview) = preview_on_spawn_failure.upgrade()
            && let Err(refresh_error) = settle_translation_preview_backdrop(
                &preview,
                &presenter_on_spawn_failure,
                window_handle,
            )
        {
            preview.set_live_glass(false);
            let _ = rust_platform_windows::disable_translation_live_backdrop(window_handle);
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview backdrop_refresh=failed error={refresh_error}"
            ));
        }
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview drag=failed stage=watcher error={error}"
        ));
    }
}

fn begin_slint_window_drag(window: &slint::Window) -> Result<(), String> {
    window
        .with_winit_window(|window| window.drag_window().map_err(|error| error.to_string()))
        .ok_or_else(|| "translation window is not backed by winit".to_owned())?
}

fn wait_for_primary_pointer_release() {
    let pressed_deadline = Instant::now() + Duration::from_millis(120);
    while !rust_platform_windows::primary_pointer_button_is_down()
        && Instant::now() < pressed_deadline
    {
        std::thread::sleep(Duration::from_millis(2));
    }
    while rust_platform_windows::primary_pointer_button_is_down() {
        std::thread::sleep(Duration::from_millis(8));
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
                    source_text: text.clone(),
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
    settings.set_speech_backend_label(
        format!("Backend: {}", state.speech_backend_preference.label()).into(),
    );
    settings.set_speech_backend_detail(
        state
            .speech_backend_active
            .map_or_else(
                || "No compatible speech backend is active".to_owned(),
                |active| {
                    format!(
                        "Preference: {} · active now: {}",
                        state.speech_backend_preference.label(),
                        active.display_name()
                    )
                },
            )
            .into(),
    );
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
            TextActionMode::Adaptive => "Ctrl+Alt+F: adaptive",
            TextActionMode::AlwaysPalette => "Ctrl+Alt+F: always show palette",
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
    settings.set_translation_blur_radius(state.translation_blur_radius as i32);
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
        0..=28 => 252.0,
        29..=140 => 352.0,
        _ => 480.0,
    };

    TranslationPreviewLayout { card_width }
}

fn apply_translation_preview_layout(preview: &TranslationPreview, result_text: &str) {
    let layout = translation_preview_layout(result_text);
    preview.set_card_width(layout.card_width);
}

fn refresh_cloaked_translation_backdrop(preview: &TranslationPreview, window_handle: u64) {
    let blur_radius = preview
        .get_blur_radius()
        .clamp(0, MAX_TRANSLATION_BLUR_RADIUS as i32) as u32;
    let result =
        rust_platform_windows::capture_blurred_cloaked_overlay_backdrop(window_handle, blur_radius);
    match result.and_then(|backdrop| apply_translation_backdrop(preview, backdrop)) {
        Ok(()) => preview.set_live_glass(false),
        Err(error) => {
            preview.set_live_glass(true);
            if let Err(live_error) =
                rust_platform_windows::enable_translation_live_backdrop(window_handle)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview backdrop=failed error={error}; live_fallback={live_error}"
                ));
            } else {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview backdrop=captured_failed fallback=live error={error}"
                ));
            }
        }
    }
}

fn settle_translation_preview_backdrop(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    window_handle: u64,
) -> Result<(), String> {
    let ticket = presenter.begin_refresh()?;
    if let Err(error) = rust_platform_windows::disable_translation_live_backdrop(window_handle) {
        presenter.abort_prepare(preview.window(), ticket, "disable_live_glass", &error);
        return Err(error);
    }
    refresh_cloaked_translation_backdrop(preview, window_handle);
    presenter
        .await_frame(preview.window(), ticket)
        .inspect_err(|error| {
            preview.set_prepared(false);
            presenter.abort_prepare(preview.window(), ticket, "drag_refresh", error);
        })
}

fn apply_translation_backdrop(
    preview: &TranslationPreview,
    backdrop: rust_platform_windows::BlurredBackdrop,
) -> Result<(), String> {
    if backdrop.rgba.len() != backdrop.width as usize * backdrop.height as usize * 4 {
        return Err("invalid pixel buffer".to_owned());
    }
    preview.set_dark_text(backdrop_prefers_dark_text(
        &backdrop.rgba,
        preview.get_dark_text(),
    ));
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &backdrop.rgba,
        backdrop.width,
        backdrop.height,
    );
    preview.set_glass_backdrop(Image::from_rgba8(pixels));
    preview.window().request_redraw();
    Ok(())
}

fn backdrop_prefers_dark_text(rgba: &[u8], currently_dark: bool) -> bool {
    const SAMPLE_BUDGET: usize = 4_096;
    const SWITCH_TO_DARK: u8 = 156;
    const SWITCH_TO_LIGHT: u8 = 116;

    let pixels = rgba.len() / 4;
    if pixels == 0 {
        return currently_dark;
    }
    let stride = pixels.div_ceil(SAMPLE_BUDGET).max(1);
    let mut luminances = rgba
        .chunks_exact(4)
        .step_by(stride)
        .map(|pixel| {
            let red = u32::from(pixel[0]);
            let green = u32::from(pixel[1]);
            let blue = u32::from(pixel[2]);
            ((54 * red + 183 * green + 19 * blue) >> 8) as u8
        })
        .collect::<Vec<_>>();
    let midpoint = luminances.len() / 2;
    let (_, median, _) = luminances.select_nth_unstable(midpoint);
    if currently_dark {
        *median >= SWITCH_TO_LIGHT
    } else {
        *median >= SWITCH_TO_DARK
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
    chooser_presenter: &OverlayPresenter,
    palette: &ActionPalette,
    palette_presenter: &OverlayPresenter,
) {
    chooser.set_prepared(false);
    chooser
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if chooser.show().is_ok() {
        let chooser = chooser.as_weak();
        let presenter = chooser_presenter.clone();
        let _ = slint::spawn_local(async move {
            if let Some(chooser) = chooser.upgrade() {
                let attached = chooser
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::prepare_action_window(handle)
                            .and_then(|()| presenter.attach(handle))
                            .is_ok()
                    });
                if !attached {
                    rust_app_core::diagnostics::log_line(
                        "selection_chooser_overlay transition=failed stage=attach",
                    );
                }
            }
        });
    }

    palette.set_prepared(false);
    palette
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if palette.show().is_ok() {
        let palette = palette.as_weak();
        let presenter = palette_presenter.clone();
        let _ = slint::spawn_local(async move {
            if let Some(palette) = palette.upgrade() {
                let attached = palette
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::prepare_action_window(handle)
                            .and_then(|()| presenter.attach(handle))
                            .is_ok()
                    });
                if !attached {
                    rust_app_core::diagnostics::log_line(
                        "action_palette_overlay transition=failed stage=attach",
                    );
                }
            }
        });
    }
}

fn precreate_translation_preview(slot: &TranslationPreviewSlot) {
    slot.preview.set_prepared(false);
    slot.preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if let Err(error) = slot.preview.show() {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview_overlay transition=failed stage=create error={error}"
        ));
        return;
    }

    let preview = slot.preview.as_weak();
    let presenter = slot.presenter.clone();
    let attached = Rc::clone(&slot.attached);
    let _ = slint::spawn_local(async move {
        let Some(preview) = preview.upgrade() else {
            return;
        };
        let result = preview
            .window()
            .winit_window()
            .await
            .ok()
            .as_deref()
            .and_then(native_window_handle)
            .ok_or_else(|| "native translation-preview HWND is unavailable".to_owned())
            .and_then(|handle| {
                rust_platform_windows::prepare_translation_window(handle)?;
                presenter.attach(handle)?;
                configure_slint_translation_resize(&preview, false)
            });
        match result {
            Ok(()) => {
                attached.set(true);
                if !preview.get_request_id().is_empty()
                    && !preview.get_prepared()
                    && let Err(error) = present_translation_preview(&preview, &presenter, None)
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_overlay transition=failed stage=deferred_present error={error}"
                    ));
                }
            }
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay transition=failed stage=attach error={error}"
            )),
        }
    });
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
    speech_backend_preference: SpeechBackendPreference,
    output_mode: SpeechOutputMode,
    fast_release_previews: bool,
    visualizer_style: VisualizerStyle,
    circular_spectrum_palette: CircularSpectrumPalette,
    flow_sphere_palette: flow_sphere::FlowSpherePalette,
    writing_slate_surface: WritingSlateSurface,
    companion_home_position: Option<(i32, i32)>,
    selected_translation_tier: Option<TranslationTier>,
    translation_enabled: bool,
    layout_enabled: bool,
    translation_keep_warm: bool,
    translation_blur_radius: u32,
    text_action_mode: TextActionMode,
    translation_delivery: TranslationDelivery,
    history_enabled: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        Self {
            selected_model_id: None,
            speech_backend_preference: SpeechBackendPreference::default(),
            output_mode: SpeechOutputMode::default(),
            fast_release_previews: true,
            visualizer_style: VisualizerStyle::default(),
            circular_spectrum_palette: CircularSpectrumPalette::default(),
            flow_sphere_palette: flow_sphere::FlowSpherePalette::default(),
            writing_slate_surface: WritingSlateSurface::default(),
            companion_home_position: None,
            selected_translation_tier: None,
            translation_enabled: false,
            layout_enabled: true,
            translation_keep_warm: true,
            translation_blur_radius: DEFAULT_TRANSLATION_BLUR_RADIUS,
            text_action_mode: TextActionMode::default(),
            translation_delivery: TranslationDelivery::default(),
            history_enabled: true,
        }
    }
}

fn normalize_translation_blur_radius(value: u64) -> u32 {
    value.min(MAX_TRANSLATION_BLUR_RADIUS as u64) as u32
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
    let speech_backend_preference = value
        .as_ref()
        .and_then(|value| value.get("speech_backend_preference"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
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
    let companion_home_position = value
        .as_ref()
        .and_then(|value| value.get("companion_home_position"))
        .and_then(|value| serde_json::from_value::<[i32; 2]>(value.clone()).ok())
        .map(|[x, y]| (x, y));
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
    let translation_blur_radius = value
        .as_ref()
        .and_then(|value| value.get("translation_blur_radius"))
        .and_then(serde_json::Value::as_u64)
        .map(normalize_translation_blur_radius)
        .unwrap_or(DEFAULT_TRANSLATION_BLUR_RADIUS);
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
        speech_backend_preference,
        output_mode,
        fast_release_previews,
        visualizer_style,
        circular_spectrum_palette,
        flow_sphere_palette,
        writing_slate_surface,
        companion_home_position,
        selected_translation_tier,
        translation_enabled,
        layout_enabled,
        translation_keep_warm,
        translation_blur_radius,
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
        "speech_backend_preference": state.speech_backend_preference,
        "output_mode": state.output_mode,
        "fast_release_previews": state.fast_release_previews,
        "visualizer_style": state.visualizer_style,
        "circular_spectrum_palette": state.circular_spectrum_palette,
        "flow_sphere_palette": state.flow_sphere_palette,
        "writing_slate_surface": state.writing_slate_surface,
        "companion_home_position": state.companion_home_position.map(|(x, y)| [x, y]),
        "selected_translation_tier": state.selected_translation_tier,
        "translation_enabled": state.translation_enabled,
        "layout_enabled": state.layout_enabled,
        "translation_keep_warm": state.translation_keep_warm,
        "translation_blur_radius": state.translation_blur_radius,
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
        CircularSpectrumPalette, IndicatorAnimation, IndicatorMode, RecordingSurface, SlateLine,
        VisualizerStyle, WritingSlateAnimation, WritingSlateSurface, flow_sphere,
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
    fn visualizer_styles_are_partitioned_by_immutable_native_surface() {
        for style in [
            VisualizerStyle::VoiceOrb,
            VisualizerStyle::SymmetricSpectrum,
            VisualizerStyle::FluidRibbon,
            VisualizerStyle::BroadcastMic,
        ] {
            assert_eq!(style.recording_surface(), RecordingSurface::Panel);
            assert!(!style.uses_transparent_window());
        }
        for style in [
            VisualizerStyle::CircularSpectrum,
            VisualizerStyle::FlowSphere,
            VisualizerStyle::WritingSlate,
            VisualizerStyle::CompanionOrb,
        ] {
            assert_eq!(style.recording_surface(), RecordingSurface::Transparent);
            assert!(style.uses_transparent_window());
        }
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
            VisualizerStyle::from_index(7),
            Some(VisualizerStyle::CompanionOrb)
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
            indicator_preview_style_for_argument("--indicator-preview=slate-blue-glass"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview=slate-blue-glass"),
            Some(WritingSlateSurface::BlueGlass)
        );
        assert_eq!(
            WritingSlateSurface::from_index(0),
            Some(WritingSlateSurface::DarkPanel)
        );
        assert_eq!(
            WritingSlateSurface::from_index(1),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            WritingSlateSurface::from_index(2),
            Some(WritingSlateSurface::BlueGlass)
        );
        assert_eq!(
            serde_json::to_string(&WritingSlateSurface::BlueGlass).unwrap(),
            "\"blue_glass\""
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
    use slint::ComponentHandle;

    use super::{
        TranslationPreview, adaptive_text_panel_height, backdrop_prefers_dark_text,
        normalize_translation_blur_radius, translation_pairs, translation_preview_layout,
        translation_units,
    };

    fn uniform_backdrop(value: u8) -> Vec<u8> {
        [value, value, value, u8::MAX].repeat(5_000)
    }

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
        assert_eq!(layout.card_width, 252.0);
    }

    #[test]
    fn long_translation_uses_the_readable_width() {
        let layout = translation_preview_layout(&"translated text ".repeat(500));
        assert_eq!(layout.card_width, 480.0);
    }

    #[test]
    fn backdrop_contrast_uses_dark_text_on_light_material() {
        assert!(backdrop_prefers_dark_text(&uniform_backdrop(230), false));
    }

    #[test]
    fn backdrop_contrast_uses_light_text_on_dark_material() {
        assert!(!backdrop_prefers_dark_text(&uniform_backdrop(40), true));
    }

    #[test]
    fn backdrop_contrast_has_hysteresis_around_midtones() {
        let backdrop = uniform_backdrop(135);
        assert!(!backdrop_prefers_dark_text(&backdrop, false));
        assert!(backdrop_prefers_dark_text(&backdrop, true));
    }

    #[test]
    fn translation_blur_setting_is_bounded_to_the_supported_range() {
        assert_eq!(normalize_translation_blur_radius(0), 0);
        assert_eq!(normalize_translation_blur_radius(3), 3);
        assert_eq!(normalize_translation_blur_radius(200), 8);
    }

    #[test]
    fn dual_mode_pairs_matching_sentences_in_order() {
        let pairs = translation_pairs(
            "First sentence. Second sentence! Third sentence?",
            "Первое предложение. Второе предложение! Третье предложение?",
        );
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1].source.as_str(), "Second sentence!");
        assert_eq!(pairs[1].translation.as_str(), "Второе предложение!");
    }

    #[test]
    fn dual_mode_keeps_an_unpunctuated_phrase_as_one_pair() {
        let pairs = translation_pairs("order number", "номер заказа");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].source.as_str(), "order number");
        assert_eq!(pairs[0].translation.as_str(), "номер заказа");
    }

    #[test]
    fn dual_mode_does_not_claim_alignment_when_sentence_counts_differ() {
        let pairs = translation_pairs(
            "One sentence. A second sentence.",
            "Один объединённый перевод без точки",
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].source.as_str(), "One sentence. A second sentence.");
    }

    #[test]
    fn sentence_splitter_preserves_common_titles_and_decimal_numbers() {
        assert_eq!(
            translation_units("Mr. Ratchett paid 3.5 dollars. Then he left."),
            vec!["Mr. Ratchett paid 3.5 dollars.", "Then he left."]
        );
    }

    #[test]
    fn wrapped_preview_uses_intrinsic_text_height_and_exposes_scroll_range() {
        let preview = TranslationPreview::new().unwrap();
        preview
            .window()
            .set_size(slint::LogicalSize::new(480.0, 302.0));
        preview.set_card_width(480.0);
        preview.set_translated_text(
            ("Длинный переведённый текст с пропорциональным шрифтом и переносами. ".repeat(120))
                .into(),
        );

        assert_eq!(preview.get_result_panel_height(), 252.0);
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
        let maximum = preview.get_result_content_height() - preview.get_result_viewport_height();
        assert!(
            (preview.get_result_scroll_offset() - maximum).abs() < 0.01,
            "offset={} maximum={} content={} viewport={}",
            preview.get_result_scroll_offset(),
            maximum,
            preview.get_result_content_height(),
            preview.get_result_viewport_height(),
        );

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
