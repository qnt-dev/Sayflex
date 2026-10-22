//! Starting the worker processes and keeping them alive.
//!
//! The core executables plus feature-gated workers share one job object and the
//! rules for what to do when one of them dies: whether the exit belongs to the process we are still talking to,
//! whether it happened while Windows was ending the session, and whether it has
//! already failed often enough that restarting it again would be a loop rather
//! than a recovery.
//!
//! Kept apart from the session logic because none of it is about dictation: a
//! worker is a process that must be running, whatever it is running for.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    error::Error,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rust_app_core::worker_stream::JsonWriter;
use rust_protocol::{
    CompanionRendererCommand, InputHostCommand, SpeechWorkerCommand, SpeechWorkerEvent,
    TranslationWorkerCommand,
};
#[cfg(feature = "screen-translation")]
use rust_protocol::{OcrWorkerCommand, OcrWorkerEvent};
use serde::de::DeserializeOwned;

use crate::{
    AudioLevelFrame, RuntimeEvent, WorkerKind, dispatch_runtime_event, record_audio_level,
    speech_backend::{SpeechBackend, SpeechBackendPlan, SpeechBackendPreference},
};

pub(crate) const INPUT_WORKER_EXE: &str = "SayFlex.InputHost.exe";

pub(crate) const SPEECH_WORKER_EXE: &str = "SayFlex.Speech.exe";

pub(crate) const TRANSLATION_WORKER_EXE: &str = "SayFlex.Translation.exe";

#[cfg(feature = "screen-translation")]
pub(crate) const OCR_WORKER_EXE: &str = "SayFlex.Ocr.exe";

pub(crate) const COMPANION_RENDERER_EXE: &str = "SayFlex.Companion.exe";

pub(crate) const SPEECH_CPU_EXE: &str = "SayFlex.Speech.Cpu.exe";

/// The CPU speech worker built for processors without AVX2 (SSE4.2 level).
pub(crate) const SPEECH_CPU_COMPAT_EXE: &str = "SayFlex.Speech.CpuCompat.exe";

pub(crate) const SPEECH_CUDA_EXE: &str = "SayFlex.Speech.Cuda.exe";

pub(crate) const SPEECH_VULKAN_EXE: &str = "SayFlex.Speech.Vulkan.exe";

pub(crate) const DEV_INPUT_WORKER_EXE: &str = "sayflex-input-host.exe";

pub(crate) const DEV_SPEECH_WORKER_EXE: &str = "sayflex-speech-worker.exe";

pub(crate) const DEV_TRANSLATION_WORKER_EXE: &str = "sayflex-translation-worker.exe";

#[cfg(feature = "screen-translation")]
pub(crate) const DEV_OCR_WORKER_EXE: &str = "sayflex-ocr-worker.exe";

pub(crate) const DEV_COMPANION_RENDERER_EXE: &str = "sayflex-companion-renderer.exe";

pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) const WORKER_RESTART_WINDOW: Duration = Duration::from_secs(60);

pub(crate) const WORKER_STARTUP_STABILITY: Duration = Duration::from_secs(2);

pub(crate) static NEXT_WORKER_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct WorkerProcess {
    pub(crate) child: Child,
    pub(crate) instance_id: u64,
}

pub(crate) struct InitialWorkers {
    pub(crate) job: rust_platform_windows::WorkerJob,
    pub(crate) input_child: WorkerProcess,
    pub(crate) input: JsonWriter,
    pub(crate) speech_child: WorkerProcess,
    pub(crate) speech: JsonWriter,
    pub(crate) speech_backends: SpeechBackendPlan,
    pub(crate) translation_child: WorkerProcess,
    pub(crate) translation: JsonWriter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerLifecycle {
    Running,
    SessionEnding,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupSessionPhase {
    Running,
    Query,
    Confirmed,
}

pub(crate) struct StartupSessionGate {
    pub(crate) phase: Mutex<StartupSessionPhase>,
    pub(crate) changed: Condvar,
}

impl StartupSessionGate {
    pub(crate) fn new() -> Self {
        Self {
            phase: Mutex::new(StartupSessionPhase::Running),
            changed: Condvar::new(),
        }
    }

    pub(crate) fn observe(&self, event: rust_platform_windows::SessionEndEvent) {
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

    pub(crate) fn wait_until_worker_start_is_allowed(&self) -> bool {
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
pub(crate) struct SessionEventRouteState {
    pub(crate) runtime_ready: bool,
    pub(crate) pending: Vec<rust_platform_windows::SessionEndEvent>,
}

pub(crate) struct SessionEventRouter {
    pub(crate) startup_gate: Arc<StartupSessionGate>,
    pub(crate) state: Mutex<SessionEventRouteState>,
}

impl SessionEventRouter {
    pub(crate) fn new(startup_gate: Arc<StartupSessionGate>) -> Self {
        Self {
            startup_gate,
            state: Mutex::new(SessionEventRouteState::default()),
        }
    }

    pub(crate) fn receive(&self, event: rust_platform_windows::SessionEndEvent) {
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

    pub(crate) fn attach_runtime(&self) {
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
pub(crate) struct RestartAttempt {
    pub(crate) at: Instant,
    pub(crate) startup_failure: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DeferredWorkerExit {
    pub(crate) kind: WorkerKind,
    pub(crate) instance_id: u64,
    pub(crate) exit: rust_platform_windows::ProcessExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerRecovery {
    Restarted,
    Deferred,
    Stopping,
    Stale,
}

#[cfg(feature = "screen-translation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OcrRequestState {
    Running,
    CancelRequested,
    #[default]
    Terminated,
}

#[cfg(feature = "screen-translation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveOcrRequest {
    request_id: rust_protocol::RequestId,
    instance_id: u64,
    state: OcrRequestState,
}

#[cfg(feature = "screen-translation")]
#[derive(Debug, Default)]
pub(crate) struct OcrRequestOwner {
    active: Option<ActiveOcrRequest>,
}

#[cfg(feature = "screen-translation")]
impl OcrRequestOwner {
    fn start(&mut self, request_id: rust_protocol::RequestId, instance_id: u64) {
        self.active = Some(ActiveOcrRequest {
            request_id,
            instance_id,
            state: OcrRequestState::Running,
        });
    }

    fn request_cancel(&mut self, request_id: rust_protocol::RequestId) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id)
        else {
            return false;
        };
        active.state = OcrRequestState::CancelRequested;
        true
    }

    fn terminate(&mut self, request_id: rust_protocol::RequestId, instance_id: u64) -> bool {
        if !self.watchdog_is_current(request_id, instance_id) {
            return false;
        }
        self.active = None;
        true
    }

    fn abandon_instance(&mut self, instance_id: u64) -> Option<rust_protocol::RequestId> {
        if self
            .active
            .is_none_or(|active| active.instance_id != instance_id)
        {
            return None;
        }
        self.active.take().map(|active| active.request_id)
    }

    fn active_request_id(&self) -> Option<rust_protocol::RequestId> {
        self.active.map(|active| active.request_id)
    }

    fn watchdog_is_current(&self, request_id: rust_protocol::RequestId, instance_id: u64) -> bool {
        self.active.is_some_and(|active| {
            active.request_id == request_id && active.instance_id == instance_id
        })
    }

    #[cfg(test)]
    fn state(&self) -> OcrRequestState {
        self.active
            .map_or(OcrRequestState::Terminated, |active| active.state)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestartPolicyDecision {
    Restart,
    Defer,
    Stop,
    Stale,
    RejectRepeatedStartupFailure,
    RejectRateLimit,
}

pub(crate) struct Workers {
    pub(crate) _job: rust_platform_windows::WorkerJob,
    pub(crate) input: JsonWriter,
    pub(crate) speech: JsonWriter,
    pub(crate) translation: JsonWriter,
    #[cfg(feature = "screen-translation")]
    pub(crate) ocr: RefCell<Option<JsonWriter>>,
    pub(crate) input_child: RefCell<WorkerProcess>,
    pub(crate) speech_child: RefCell<WorkerProcess>,
    pub(crate) speech_backends: RefCell<SpeechBackendPlan>,
    pub(crate) translation_child: RefCell<WorkerProcess>,
    #[cfg(feature = "screen-translation")]
    pub(crate) ocr_child: RefCell<Option<WorkerProcess>>,
    #[cfg(feature = "screen-translation")]
    pub(crate) ocr_request: RefCell<OcrRequestOwner>,
    pub(crate) companion: RefCell<Option<JsonWriter>>,
    pub(crate) companion_child: RefCell<Option<WorkerProcess>>,
    pub(crate) lifecycle: Cell<WorkerLifecycle>,
    pub(crate) restart_attempts: RefCell<HashMap<WorkerKind, Vec<RestartAttempt>>>,
    pub(crate) deferred_exits: RefCell<HashMap<WorkerKind, DeferredWorkerExit>>,
}

impl Workers {
    pub(crate) fn speech_pid(&self) -> u32 {
        self.speech_child.borrow().child.id()
    }

    pub(crate) fn translation_pid(&self) -> u32 {
        self.translation_child.borrow().child.id()
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn ocr_pid(&self) -> Option<u32> {
        self.ocr_child
            .borrow()
            .as_ref()
            .map(|process| process.child.id())
    }

    #[cfg(not(feature = "screen-translation"))]
    pub(crate) fn ocr_pid(&self) -> Option<u32> {
        None
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn ocr_busy(&self) -> bool {
        self.ocr_request.borrow().active_request_id().is_some()
    }

    #[cfg(not(feature = "screen-translation"))]
    pub(crate) fn ocr_busy(&self) -> bool {
        false
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn unload_ocr(&self, cause: &'static str) -> Result<bool, String> {
        if self.ocr_request.borrow().active.is_some() {
            return Err("OCR is processing a screen translation".to_owned());
        }
        if self.ocr.borrow().is_none() && self.ocr_child.borrow().is_none() {
            return Ok(false);
        }

        let send_error = self
            .ocr
            .borrow()
            .as_ref()
            .and_then(|writer| writer.send(&OcrWorkerCommand::Shutdown).err());
        let forced = {
            let mut process_slot = self.ocr_child.borrow_mut();
            match process_slot.as_mut() {
                Some(process) => {
                    terminate_child_bounded(&mut process.child, Duration::from_millis(750))
                        .map_err(|error| error.to_string())?
                }
                None => false,
            }
        };

        // Do not publish "unloaded" and do not permit ensure_ocr() to spawn a
        // replacement until the old process has definitely exited and has been
        // reaped. Its reader-thread exit notification becomes stale only now.
        self.ocr.borrow_mut().take();
        self.ocr_child.borrow_mut().take();
        self.ocr_request.borrow_mut().active.take();
        rust_app_core::diagnostics::log_line(&format!(
            "ocr_worker lifecycle=unloaded cause={cause} forced={forced} shutdown_send_failed={}",
            send_error.is_some()
        ));
        Ok(true)
    }

    #[cfg(not(feature = "screen-translation"))]
    pub(crate) fn unload_ocr(&self, _cause: &'static str) -> Result<bool, String> {
        Ok(false)
    }

    pub(crate) fn shutdown(&self) {
        if self.lifecycle.replace(WorkerLifecycle::Stopping) == WorkerLifecycle::Stopping {
            return;
        }
        rust_app_core::diagnostics::log_line("desktop lifecycle=stopping workers");
        // Stop any installation first: it holds its worker's installer thread,
        // and a partial download is resumed rather than lost.
        let _ = self.speech.send(&SpeechWorkerCommand::CancelModelInstall);
        let _ = self
            .translation
            .send(&TranslationWorkerCommand::CancelModelInstall);
        let _ = self.input.send(&InputHostCommand::Shutdown);
        let _ = self.speech.send(&SpeechWorkerCommand::Shutdown);
        let _ = self.translation.send(&TranslationWorkerCommand::Shutdown);
        #[cfg(feature = "screen-translation")]
        if let Some(ocr) = self.ocr.borrow().as_ref() {
            let _ = ocr.send(&OcrWorkerCommand::Shutdown);
        }
        if let Some(companion) = self.companion.borrow().as_ref() {
            let _ = companion.send(&CompanionRendererCommand::Shutdown);
        }
    }

    pub(crate) fn begin_session_end(&self) {
        if self.lifecycle.get() == WorkerLifecycle::Running {
            self.lifecycle.set(WorkerLifecycle::SessionEnding);
            rust_app_core::diagnostics::log_line("desktop lifecycle=session_end_query");
        }
    }

    pub(crate) fn confirm_session_end(&self) {
        rust_app_core::diagnostics::log_line("desktop lifecycle=session_end_confirmed");
        self.shutdown();
    }

    pub(crate) fn cancel_session_end(&self) -> Vec<DeferredWorkerExit> {
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

    pub(crate) fn is_current_instance(&self, kind: WorkerKind, instance_id: u64) -> bool {
        match kind {
            WorkerKind::Input => self.input_child.borrow().instance_id == instance_id,
            WorkerKind::Speech => self.speech_child.borrow().instance_id == instance_id,
            WorkerKind::Translation => self.translation_child.borrow().instance_id == instance_id,
            #[cfg(feature = "screen-translation")]
            WorkerKind::Ocr => self
                .ocr_child
                .borrow()
                .as_ref()
                .is_some_and(|process| process.instance_id == instance_id),
            WorkerKind::Companion => self
                .companion_child
                .borrow()
                .as_ref()
                .is_some_and(|process| process.instance_id == instance_id),
        }
    }

    pub(crate) fn active_speech_backend(&self) -> SpeechBackend {
        self.speech_backends.borrow().active()
    }

    pub(crate) fn speech_backend_preference(&self) -> SpeechBackendPreference {
        self.speech_backends.borrow().preference()
    }

    pub(crate) fn mark_speech_backend_ready(
        &self,
        reported: &str,
    ) -> Result<SpeechBackend, String> {
        self.speech_backends.borrow_mut().mark_verified(reported)
    }

    pub(crate) fn fallback_speech_backend(&self, reason: &str) -> Result<SpeechBackend, String> {
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

    pub(crate) fn switch_speech_backend(
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

    pub(crate) fn ensure_companion(&self) -> Result<(), String> {
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
            RuntimeEvent::companion,
        )
        .map_err(|error| error.to_string())?;
        *self.companion_child.borrow_mut() = Some(child);
        *self.companion.borrow_mut() = Some(writer);
        rust_app_core::diagnostics::log_line("companion_renderer lifecycle=spawned_lazily");
        Ok(())
    }

    pub(crate) fn send_companion(&self, command: &CompanionRendererCommand) -> Result<(), String> {
        self.ensure_companion()?;
        self.companion
            .borrow()
            .as_ref()
            .ok_or_else(|| "the Companion Orb renderer is unavailable".to_owned())?
            .send(command)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn send_companion_if_running(
        &self,
        command: &CompanionRendererCommand,
    ) -> Result<(), String> {
        let companion = self.companion.borrow();
        let Some(companion) = companion.as_ref() else {
            return Ok(());
        };
        companion.send(command).map_err(|error| error.to_string())
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn ensure_ocr(&self) -> Result<(), String> {
        if self.ocr.borrow().is_some() {
            return Ok(());
        }
        if self.lifecycle.get() != WorkerLifecycle::Running
            || rust_platform_windows::session_is_ending()
        {
            return Err("OCR cannot start while the session is ending".to_owned());
        }
        let (child, writer) = spawn_ocr_worker(&self._job).map_err(|error| error.to_string())?;
        *self.ocr_child.borrow_mut() = Some(child);
        *self.ocr.borrow_mut() = Some(writer);
        rust_app_core::diagnostics::log_line("ocr_worker lifecycle=spawned_lazily");
        Ok(())
    }

    #[cfg(feature = "screen-translation")]
    #[allow(dead_code)]
    pub(crate) fn send_ocr(&self, command: &OcrWorkerCommand) -> Result<(), String> {
        if ocr_command_requires_worker(command) {
            self.ensure_ocr()?;
        } else if self.ocr.borrow().is_none() {
            return Ok(());
        }
        let recognition = match command {
            OcrWorkerCommand::Recognize { request_id, .. } => {
                let instance_id = self
                    .current_ocr_instance_id()
                    .ok_or_else(|| "OCR worker process is unavailable".to_owned())?;
                self.ocr_request
                    .borrow_mut()
                    .start(*request_id, instance_id);
                Some((*request_id, instance_id))
            }
            OcrWorkerCommand::Cancel { request_id } => {
                self.ocr_request.borrow_mut().request_cancel(*request_id);
                None
            }
            OcrWorkerCommand::Shutdown => None,
        };
        let send_result = self
            .ocr
            .borrow()
            .as_ref()
            .ok_or_else(|| "OCR worker is unavailable".to_owned())?
            .send(command);
        if let Err(error) = send_result {
            if let Some((request_id, instance_id)) = recognition {
                self.complete_ocr_request(request_id, instance_id);
            }
            return Err(error.to_string());
        }
        Ok(())
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn complete_ocr_request(
        &self,
        request_id: rust_protocol::RequestId,
        instance_id: u64,
    ) -> bool {
        self.ocr_request
            .borrow_mut()
            .terminate(request_id, instance_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn abandon_active_ocr_request(
        &self,
        instance_id: u64,
    ) -> Option<rust_protocol::RequestId> {
        self.ocr_request.borrow_mut().abandon_instance(instance_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn current_ocr_instance_id(&self) -> Option<u64> {
        self.ocr_child
            .borrow()
            .as_ref()
            .map(|process| process.instance_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn terminate_stalled_ocr(
        &self,
        request_id: rust_protocol::RequestId,
        instance_id: u64,
    ) -> Result<bool, String> {
        let Some(current_instance) = self.current_ocr_instance_id() else {
            return Ok(false);
        };
        if current_instance != instance_id
            || !self
                .ocr_request
                .borrow()
                .watchdog_is_current(request_id, instance_id)
        {
            return Ok(false);
        }
        let mut process_slot = self.ocr_child.borrow_mut();
        let Some(process) = process_slot.as_mut() else {
            return Ok(false);
        };
        if process
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            process.child.kill().map_err(|error| error.to_string())?;
        }
        self.ocr_request
            .borrow_mut()
            .terminate(request_id, instance_id);
        Ok(true)
    }

    pub(crate) fn recover(
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
                    RuntimeEvent::input,
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
                    RuntimeEvent::translation,
                )
                .map_err(|error| error.to_string())?;
                self.translation.replace_with(writer);
                replace_process(&self.translation_child, child);
            }
            #[cfg(feature = "screen-translation")]
            WorkerKind::Ocr => {
                let (child, writer) =
                    spawn_ocr_worker(&self._job).map_err(|error| error.to_string())?;
                *self.ocr.borrow_mut() = Some(writer);
                replace_optional_process(&self.ocr_child, child);
            }
            WorkerKind::Companion => {
                let (child, writer) = spawn_worker(
                    &self._job,
                    WorkerKind::Companion,
                    COMPANION_RENDERER_EXE,
                    RuntimeEvent::companion,
                )
                .map_err(|error| error.to_string())?;
                *self.companion_child.borrow_mut() = Some(child);
                *self.companion.borrow_mut() = Some(writer);
            }
        }
        Ok(WorkerRecovery::Restarted)
    }
}

#[cfg(feature = "screen-translation")]
#[cfg(test)]
fn ocr_watchdog_is_current(
    active_request: Option<rust_protocol::RequestId>,
    current_instance: u64,
    watchdog_request: rust_protocol::RequestId,
    watchdog_instance: u64,
) -> bool {
    active_request == Some(watchdog_request) && current_instance == watchdog_instance
}

pub(crate) fn decide_worker_restart(
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

pub(crate) fn replace_process(slot: &RefCell<WorkerProcess>, replacement: WorkerProcess) {
    let mut previous = slot.replace(replacement);
    if previous.child.try_wait().ok().flatten().is_none() {
        let _ = previous.child.kill();
    }
    let _ = previous.child.wait();
}

#[cfg(feature = "screen-translation")]
fn terminate_child_bounded(child: &mut Child, grace_period: Duration) -> std::io::Result<bool> {
    let deadline = Instant::now() + grace_period;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    match child.kill() {
        Ok(()) => {}
        Err(_error) if child.try_wait()?.is_some() => return Ok(false),
        Err(error) => return Err(error),
    }
    let _ = child.wait()?;
    Ok(true)
}

#[cfg(feature = "screen-translation")]
fn replace_optional_process(slot: &RefCell<Option<WorkerProcess>>, replacement: WorkerProcess) {
    if let Some(mut previous) = slot.replace(Some(replacement)) {
        if previous.child.try_wait().ok().flatten().is_none() {
            let _ = previous.child.kill();
        }
        let _ = previous.child.wait();
    }
}

#[cfg(feature = "screen-translation")]
fn ocr_command_requires_worker(command: &OcrWorkerCommand) -> bool {
    matches!(command, OcrWorkerCommand::Recognize { .. })
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.shutdown();
        let mut children = vec![
            &mut self.input_child.get_mut().child,
            &mut self.speech_child.get_mut().child,
            &mut self.translation_child.get_mut().child,
        ];
        #[cfg(feature = "screen-translation")]
        if let Some(process) = self.ocr_child.get_mut().as_mut() {
            children.push(&mut process.child);
        }
        if let Some(process) = self.companion_child.get_mut().as_mut() {
            children.push(&mut process.child);
        }
        finish_worker_children(&mut children, Duration::from_secs(3));
    }
}

fn finish_worker_children(children: &mut [&mut Child], grace_period: Duration) {
    // All workers received Shutdown before this starts. One shared deadline
    // lets a history writer finish without multiplying the timeout by workers.
    let deadline = Instant::now() + grace_period;
    loop {
        let mut all_exited = true;
        for child in children.iter_mut() {
            if child.try_wait().ok().flatten().is_none() {
                all_exited = false;
            }
        }
        if all_exited {
            return;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    for child in children {
        if child.try_wait().ok().flatten().is_some() {
            continue;
        }
        match child.kill() {
            Ok(()) => {
                if let Err(error) = child.wait() {
                    rust_app_core::diagnostics::log_line(&format!(
                        "worker pid={} shutdown=reap_failed error={error}",
                        child.id()
                    ));
                }
            }
            Err(error) => {
                // A process may exit between try_wait and kill. Reap that
                // race, but never block indefinitely if termination failed.
                if child.try_wait().ok().flatten().is_none() {
                    rust_app_core::diagnostics::log_line(&format!(
                        "worker pid={} shutdown=kill_failed error={error}",
                        child.id()
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "worker_shutdown_regression_tests.rs"]
mod worker_shutdown_regression_tests;

pub(crate) fn spawn_initial_workers(
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
        RuntimeEvent::input,
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
        RuntimeEvent::translation,
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

pub(crate) fn spawn_initial_worker<T>(
    worker_job: &rust_platform_windows::WorkerJob,
    startup_gate: &StartupSessionGate,
    kind: WorkerKind,
    executable: &str,
    wrap: fn(u64, T) -> RuntimeEvent,
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

pub(crate) fn guarded_initial_spawn<T, E>(
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

pub(crate) fn spawn_worker<T>(
    worker_job: &rust_platform_windows::WorkerJob,
    kind: WorkerKind,
    executable: &str,
    wrap: fn(u64, T) -> RuntimeEvent,
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
{
    let path = worker_path(executable)?;
    spawn_worker_at(
        worker_job,
        kind,
        executable,
        path,
        None,
        move |instance_id, event| Some(wrap(instance_id, event)),
    )
}

pub(crate) fn spawn_speech_worker(
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
        |instance_id, event| match event {
            // Visualizer frames never enter the UI queue as individual
            // events; only the newest one is kept.
            SpeechWorkerEvent::AudioLevel {
                session_id,
                peak,
                spectrum,
            } => record_audio_level(AudioLevelFrame {
                instance_id,
                session_id,
                peak,
                spectrum,
            }),
            event => Some(RuntimeEvent::Speech { instance_id, event }),
        },
    )
}

#[cfg(feature = "screen-translation")]
pub(crate) fn spawn_ocr_worker(
    worker_job: &rust_platform_windows::WorkerJob,
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>> {
    let path = worker_path(OCR_WORKER_EXE)?;
    spawn_worker_at(
        worker_job,
        WorkerKind::Ocr,
        OCR_WORKER_EXE,
        path,
        None,
        |instance_id, event: OcrWorkerEvent| Some(RuntimeEvent::Ocr { instance_id, event }),
    )
}

pub(crate) fn spawn_worker_at<T, F>(
    worker_job: &rust_platform_windows::WorkerJob,
    kind: WorkerKind,
    executable: &str,
    path: PathBuf,
    runtime_directory: Option<PathBuf>,
    wrap: F,
) -> Result<(WorkerProcess, JsonWriter), Box<dyn Error>>
where
    T: DeserializeOwned + Send + 'static,
    F: Fn(u64, T) -> Option<RuntimeEvent> + Send + 'static,
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
                        if let Some(event) = wrap(instance_id, event) {
                            dispatch_runtime_event(event);
                        }
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
                mirror_worker_stderr(&mut std::io::stderr().lock(), &diagnostics_name, &line);
            }
        })?;

    Ok((
        WorkerProcess { child, instance_id },
        JsonWriter::start(stdin, executable)?,
    ))
}

fn mirror_worker_stderr(writer: &mut impl Write, diagnostics_name: &str, line: &str) {
    // A GUI launcher may close its inherited stderr while the worker is still running.
    let _ = writeln!(writer, "{diagnostics_name}: {line}");
}

#[cfg(test)]
mod worker_stderr_mirror_tests {
    use super::mirror_worker_stderr;
    use std::io::{self, Write};

    struct ClosedPipe {
        attempts: usize,
    }

    impl Write for ClosedPipe {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "closed launcher pipe",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn closed_stderr_does_not_abort_subsequent_worker_diagnostics() {
        let mut pipe = ClosedPipe { attempts: 0 };

        mirror_worker_stderr(&mut pipe, "SayFlex.Speech.exe", "loading model");
        mirror_worker_stderr(&mut pipe, "SayFlex.Speech.exe", "model ready");

        assert_eq!(pipe.attempts, 2);
    }

    #[test]
    fn available_stderr_keeps_worker_prefix_and_line_boundaries() {
        let mut output = Vec::new();

        mirror_worker_stderr(&mut output, "SayFlex.Speech.exe", "loading model");
        mirror_worker_stderr(&mut output, "SayFlex.Speech.exe", "model ready");

        assert_eq!(
            output,
            b"SayFlex.Speech.exe: loading model\nSayFlex.Speech.exe: model ready\n"
        );
    }
}

pub(crate) fn routine_native_worker_log(executable: &str, line: &str) -> bool {
    executable == SPEECH_WORKER_EXE
        && (line.starts_with("whisper_init_state:")
            || line.starts_with("whisper_backend_init_gpu:")
            || line.starts_with("whisper_full_with_state: auto-detected language:"))
}

pub(crate) fn worker_path(executable: &str) -> Result<PathBuf, Box<dyn Error>> {
    let current = env::current_exe()?;
    let parent = current
        .parent()
        .ok_or("desktop executable has no parent directory")?;
    let sibling = parent.join(executable);
    if sibling.exists() {
        return Ok(sibling);
    }
    let development = development_worker_name(executable).map(|name| parent.join(name));
    if let Some(development) = development.filter(|path| path.exists()) {
        return Ok(development);
    }

    Err(format!(
        "required worker is missing: {}. Build the whole workspace before launching the desktop.",
        sibling.display()
    )
    .into())
}

pub(crate) fn development_worker_name(executable: &str) -> Option<&'static str> {
    match executable {
        INPUT_WORKER_EXE => Some(DEV_INPUT_WORKER_EXE),
        TRANSLATION_WORKER_EXE => Some(DEV_TRANSLATION_WORKER_EXE),
        COMPANION_RENDERER_EXE => Some(DEV_COMPANION_RENDERER_EXE),
        #[cfg(feature = "screen-translation")]
        OCR_WORKER_EXE => Some(DEV_OCR_WORKER_EXE),
        _ => None,
    }
}

pub(crate) fn speech_worker_path(backend: SpeechBackend) -> Result<PathBuf, Box<dyn Error>> {
    let parent = env::current_exe()?
        .parent()
        .ok_or("desktop executable has no parent directory")?
        .to_owned();
    let installed = parent.join(match backend {
        SpeechBackend::Cuda => SPEECH_CUDA_EXE,
        SpeechBackend::Vulkan => SPEECH_VULKAN_EXE,
        SpeechBackend::Cpu if rust_platform_windows::cpu_supports_avx2_level() => SPEECH_CPU_EXE,
        SpeechBackend::Cpu => SPEECH_CPU_COMPAT_EXE,
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

pub(crate) fn available_speech_backends() -> Vec<SpeechBackend> {
    [
        SpeechBackend::Cuda,
        SpeechBackend::Vulkan,
        SpeechBackend::Cpu,
    ]
    .into_iter()
    .filter(|backend| {
        // The GPU workers also run ggml's AVX2 CPU code; an older processor
        // uses only the compatibility CPU worker.
        (*backend == SpeechBackend::Cpu || rust_platform_windows::cpu_supports_avx2_level())
            && speech_worker_path(*backend).is_ok()
            && (*backend != SpeechBackend::Cuda || trusted_cuda_runtime_directory().is_some())
    })
    .collect()
}

pub(crate) fn speech_backend_plan(
    preference: SpeechBackendPreference,
) -> Result<SpeechBackendPlan, String> {
    SpeechBackendPlan::new(preference, available_speech_backends())
}

pub(crate) fn trusted_cuda_runtime_directory() -> Option<PathBuf> {
    let managed = rust_model_manager::CudaRuntimeManager::new(
        rust_app_core::product::data_root().join("runtimes"),
    );
    if managed.is_installed() {
        return managed.runtime_path().canonicalize().ok();
    }
    // A fresh trial must exercise the opt-in download even on a developer's
    // machine with an existing Toolkit. No installer ships NVIDIA files, so
    // the only other source is a CUDA Toolkit the user installed themselves.
    if rust_app_core::product::is_test_build() {
        return None;
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

pub(crate) fn cuda_runtime_is_complete(directory: &std::path::Path) -> bool {
    ["cublas64_13.dll", "cublasLt64_13.dll", "cudart64_13.dll"]
        .iter()
        .all(|name| directory.join(name).is_file())
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
        let (ending_observed_sender, ending_observed_receiver) = mpsc::sync_channel(1);
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
                || {
                    let ending = worker_ending.load(Ordering::Acquire);
                    if ending {
                        let _ = ending_observed_sender.send(());
                    }
                    ending
                },
                || {
                    worker_retries.fetch_add(1, Ordering::Relaxed);
                },
            )
        });

        failed_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        ending_observed_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        session_is_ending.store(false, Ordering::Release);
        // WM_ENDSESSION clears the process-wide flag before it publishes the
        // Cancelled event. Mirror that production ordering so the test cannot
        // make the retry observe a synthetic half-cancelled state.
        gate.observe(SessionEndEvent::Cancelled);

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

#[cfg(all(test, feature = "screen-translation"))]
mod ocr_supervision_tests {
    use std::{process::Command, time::Duration};

    use rust_protocol::{OcrLanguagePolicy, OcrPixelFormat, OcrWorkerCommand, RequestId};

    use super::{
        DEV_OCR_WORKER_EXE, OCR_WORKER_EXE, OcrRequestOwner, OcrRequestState,
        development_worker_name, ocr_command_requires_worker, ocr_watchdog_is_current,
        terminate_child_bounded,
    };

    #[test]
    fn installed_ocr_worker_has_an_explicit_development_sibling() {
        assert_eq!(
            development_worker_name(OCR_WORKER_EXE),
            Some(DEV_OCR_WORKER_EXE)
        );
    }

    #[test]
    fn stale_ocr_watchdogs_cannot_terminate_a_new_request_or_worker() {
        let active = RequestId::new();
        let stale = RequestId::new();
        assert!(ocr_watchdog_is_current(Some(active), 7, active, 7));
        assert!(!ocr_watchdog_is_current(Some(active), 7, stale, 7));
        assert!(!ocr_watchdog_is_current(Some(active), 7, active, 6));
        assert!(!ocr_watchdog_is_current(None, 7, active, 7));
    }

    #[test]
    fn cancellation_keeps_the_ocr_watchdog_owned_until_a_terminal_event() {
        let request_id = RequestId::new();
        let stale = RequestId::new();
        let mut owner = OcrRequestOwner::default();

        owner.start(request_id, 7);
        assert_eq!(owner.state(), OcrRequestState::Running);
        assert!(owner.request_cancel(request_id));
        assert_eq!(owner.state(), OcrRequestState::CancelRequested);
        assert!(owner.watchdog_is_current(request_id, 7));
        assert!(!owner.terminate(stale, 7));
        assert!(owner.watchdog_is_current(request_id, 7));
        assert!(owner.terminate(request_id, 7));
        assert_eq!(owner.state(), OcrRequestState::Terminated);
        assert!(!owner.watchdog_is_current(request_id, 7));
    }

    #[test]
    fn only_recognition_starts_the_lazy_ocr_worker() {
        let request_id = RequestId::new();
        let recognize = OcrWorkerCommand::Recognize {
            request_id,
            width: 1,
            height: 1,
            stride: 4,
            pixel_format: OcrPixelFormat::Rgba8,
            language_policy: OcrLanguagePolicy::Auto,
            rgba_base64: "AAAAAA==".to_owned(),
        };

        assert!(ocr_command_requires_worker(&recognize));
        assert!(!ocr_command_requires_worker(&OcrWorkerCommand::Cancel {
            request_id,
        }));
        assert!(!ocr_command_requires_worker(&OcrWorkerCommand::Shutdown));
    }

    #[test]
    fn bounded_worker_stop_kills_and_reaps_a_process_that_ignores_the_grace_period() {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .creation_flags(super::CREATE_NO_WINDOW);
        let mut child = command.spawn().unwrap();

        let forced = terminate_child_bounded(&mut child, Duration::from_millis(20)).unwrap();

        assert!(forced);
        assert!(child.try_wait().unwrap().is_some());
    }
}
