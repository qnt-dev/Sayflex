use std::{
    cell::RefCell,
    collections::HashMap,
    io,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rust_protocol::{
    InputHostEvent, RequestId, SelectionCaptureTiming, SessionId, TargetSnapshot, TextActionIntent,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
            VK_CONTROL, VK_K, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL,
            VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_T, VK_V,
        },
        WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
            WM_SYSKEYUP,
        },
    },
};

use crate::{inject, selection_capture, target};

const INPUT_CONTROL: u32 = 1 << 0;
const INPUT_ALT: u32 = 1 << 1;
const INPUT_SHIFT: u32 = 1 << 2;
const INPUT_WINDOWS: u32 = 1 << 3;
const INPUT_SPACE: u32 = 1 << 4;
const INPUT_K: u32 = 1 << 5;
const INPUT_V: u32 = 1 << 6;
const INPUT_T: u32 = 1 << 7;
const INJECTION_UNSAFE: u32 = INPUT_CONTROL
    | INPUT_ALT
    | INPUT_SHIFT
    | INPUT_WINDOWS
    | INPUT_SPACE
    | INPUT_K
    | INPUT_V
    | INPUT_T;
const INPUT_RELEASE_STABILITY: Duration = Duration::from_millis(30);
const ASYNC_RECONCILIATION_DELAY: Duration = Duration::from_millis(75);
const COMMAND_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

static ACTIONS: OnceLock<Sender<HookAction>> = OnceLock::new();
static PHYSICAL_INPUT: AtomicU32 = AtomicU32::new(0);
static MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();
static LAST_PHYSICAL_CHANGE_US: AtomicU64 = AtomicU64::new(0);
static LAST_HOOK_EVENT_US: AtomicU64 = AtomicU64::new(0);
static SELECTION_BUSY: AtomicBool = AtomicBool::new(false);
static REGISTERED_COMMANDS: AtomicU32 = AtomicU32::new(0);

pub const HOTKEY_ID_CORRECT_LAYOUT: i32 = 0x5141;
pub const HOTKEY_ID_INSERT_LAST: i32 = 0x5142;
pub const HOTKEY_ID_TRANSLATE: i32 = 0x5143;

#[cfg(debug_assertions)]
static TRACE_EVENTS: OnceLock<std::sync::mpsc::SyncSender<HookTrace>> = OnceLock::new();
#[cfg(debug_assertions)]
static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static HOOK_STATE: RefCell<HotkeyState> = RefCell::new(HotkeyState::default());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventOrigin {
    Physical,
    OwnInjected,
    ForeignInjected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    TriggerReleased,
    RequiredModifierReleased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookAction {
    DictationStarted { gesture_id: u64 },
    DictationEnded { gesture_id: u64, reason: EndReason },
    InsertLastRequested,
    SelectionRequested { intent: TextActionIntent },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum GestureState {
    #[default]
    Idle,
    Active {
        gesture_id: u64,
    },
    Draining {
        gesture_id: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureStateName {
    Idle,
    Active,
    Draining,
}

#[derive(Clone)]
struct PhysicalKeys {
    down: [bool; 256],
}

impl Default for PhysicalKeys {
    fn default() -> Self {
        Self { down: [false; 256] }
    }
}

#[derive(Default)]
struct HotkeyState {
    keys: PhysicalKeys,
    gesture: GestureState,
    next_gesture_id: u64,
    selection_k_down: bool,
    insert_v_down: bool,
    adaptive_t_down: bool,
}

#[derive(Debug, Clone, Copy)]
struct HookDecision {
    suppress: bool,
    action: Option<HookAction>,
    state_before: GestureStateName,
    state_after: GestureStateName,
    repeated: bool,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy)]
struct HookTrace {
    sequence_number: u64,
    timestamp_unix_ms: u64,
    vk: u16,
    key_down: bool,
    key_up: bool,
    origin: EventOrigin,
    state_before: GestureStateName,
    state_after: GestureStateName,
    repeated: bool,
    suppressed: bool,
    gesture_id: Option<u64>,
    action: Option<HookAction>,
}

pub fn initialize(events: Sender<InputHostEvent>) -> io::Result<()> {
    let _ = MONOTONIC_ORIGIN.set(Instant::now());
    let (action_sender, action_receiver) = mpsc::channel();
    ACTIONS
        .set(action_sender)
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "hotkeys already initialized"))?;
    thread::Builder::new()
        .name("input-action-dispatcher".to_owned())
        .spawn(move || dispatch_actions(action_receiver, events))?;

    #[cfg(debug_assertions)]
    {
        let (trace_sender, trace_receiver) = mpsc::sync_channel(4096);
        let _ = TRACE_EVENTS.set(trace_sender);
        thread::Builder::new()
            .name("input-event-trace".to_owned())
            .spawn(move || {
                while let Ok(trace) = trace_receiver.recv() {
                    write_trace(trace);
                }
            })?;
    }

    Ok(())
}

pub unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) };
    }

    let input = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
    let message_kind = message as u32;
    let key_down = message_kind == WM_KEYDOWN || message_kind == WM_SYSKEYDOWN;
    let key_up = message_kind == WM_KEYUP || message_kind == WM_SYSKEYUP;
    if !key_down && !key_up {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) };
    }

    let origin = classify_event(input.flags, input.dwExtraInfo);
    if origin == EventOrigin::OwnInjected {
        trace_injected_event(input, key_down, key_up, origin);
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) };
    }

    let now = monotonic_us();
    let previous_hook_event = LAST_HOOK_EVENT_US.swap(now, Ordering::AcqRel);
    let needs_reconciliation =
        previous_hook_event == 0 || now.saturating_sub(previous_hook_event) >= 1_000_000;
    let (recovery_action, mut decision, physical_mask) = HOOK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let recovery_action = needs_reconciliation
            .then(|| PhysicalKeys::from_async_except(input.vkCode as u16, &state.keys))
            .and_then(|keys| state.reconcile(keys));
        let decision = state.process_event(
            input.vkCode as u16,
            key_down,
            key_up,
            REGISTERED_COMMANDS.load(Ordering::Acquire),
        );
        (recovery_action, decision, state.keys.input_mask())
    });
    PHYSICAL_INPUT.store(physical_mask, Ordering::Release);
    LAST_PHYSICAL_CHANGE_US.store(now, Ordering::Release);

    if let Some(action) = recovery_action {
        let _ = dispatch_hook_action(action);
    }

    if let Some(action) = decision.action
        && !dispatch_hook_action(action)
        && let HookAction::DictationStarted { gesture_id } = action
    {
        HOOK_STATE.with(|state| state.borrow_mut().rollback_start(gesture_id));
        decision.suppress = false;
        decision.state_after = GestureStateName::Idle;
    }

    trace_actionable_event(input, key_down, key_up, origin, decision);
    if decision.suppress {
        1
    } else {
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) }
    }
}

fn classify_event(flags: u32, extra_info: usize) -> EventOrigin {
    if extra_info == inject::SYNTHETIC_MARKER {
        EventOrigin::OwnInjected
    } else if flags & LLKHF_INJECTED != 0 {
        EventOrigin::ForeignInjected
    } else {
        EventOrigin::Physical
    }
}

fn dispatch_hook_action(action: HookAction) -> bool {
    if matches!(action, HookAction::SelectionRequested { .. })
        && SELECTION_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return true;
    }

    let sent = ACTIONS
        .get()
        .is_some_and(|actions| actions.send(action).is_ok());
    if !sent && matches!(action, HookAction::SelectionRequested { .. }) {
        SELECTION_BUSY.store(false, Ordering::Release);
    }
    sent
}

pub struct RegisteredHotkeys {
    registrations: Vec<(i32, u32)>,
}

impl RegisteredHotkeys {
    pub fn register_for_current_thread() -> Self {
        let mut registrations = Vec::with_capacity(3);
        for (id, trigger, mask, name) in [
            (
                HOTKEY_ID_CORRECT_LAYOUT,
                u32::from(VK_K),
                INPUT_K,
                "correct_layout",
            ),
            (
                HOTKEY_ID_INSERT_LAST,
                u32::from(VK_V),
                INPUT_V,
                "insert_last",
            ),
            (HOTKEY_ID_TRANSLATE, u32::from(VK_T), INPUT_T, "translate"),
        ] {
            let registered = unsafe {
                RegisterHotKey(
                    std::ptr::null_mut(),
                    id,
                    MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                    trigger,
                )
            } != 0;
            if registered {
                REGISTERED_COMMANDS.fetch_or(mask, Ordering::AcqRel);
                registrations.push((id, mask));
                crate::diagnostic_line(&format!("hotkey registration={name} result=success"));
            } else {
                crate::diagnostic_line(&format!(
                    "hotkey registration={name} result=fallback_hook win32={}",
                    unsafe { GetLastError() }
                ));
            }
        }
        Self { registrations }
    }
}

impl Drop for RegisteredHotkeys {
    fn drop(&mut self) {
        for (id, mask) in self.registrations.drain(..) {
            unsafe {
                let _ = UnregisterHotKey(std::ptr::null_mut(), id);
            }
            REGISTERED_COMMANDS.fetch_and(!mask, Ordering::AcqRel);
        }
    }
}

pub fn dispatch_registered_hotkey(id: i32) -> bool {
    registered_hotkey_action(id).is_some_and(dispatch_hook_action)
}

const fn registered_hotkey_action(id: i32) -> Option<HookAction> {
    match id {
        HOTKEY_ID_CORRECT_LAYOUT => Some(HookAction::SelectionRequested {
            intent: TextActionIntent::CorrectLayout,
        }),
        HOTKEY_ID_INSERT_LAST => Some(HookAction::InsertLastRequested),
        HOTKEY_ID_TRANSLATE => Some(HookAction::SelectionRequested {
            intent: TextActionIntent::Translate,
        }),
        _ => None,
    }
}

fn dispatch_actions(actions: mpsc::Receiver<HookAction>, events: Sender<InputHostEvent>) {
    let mut sessions: HashMap<u64, (SessionId, TargetSnapshot)> = HashMap::new();
    while let Ok(action) = actions.recv() {
        match action {
            HookAction::DictationStarted { gesture_id } => {
                let session_id = SessionId::new();
                let Some(target) = target::capture_text_target() else {
                    let _ = events.send(InputHostEvent::Failed {
                        session_id: None,
                        code: "protected_field".to_owned(),
                        message: "Dictation is disabled in password or protected fields."
                            .to_owned(),
                    });
                    continue;
                };
                sessions.insert(gesture_id, (session_id, target));
                if events
                    .send(InputHostEvent::DictationBegin {
                        session_id,
                        started_unix_ms: unix_ms(),
                        target,
                    })
                    .is_err()
                {
                    sessions.remove(&gesture_id);
                    break;
                }
            }
            HookAction::DictationEnded {
                gesture_id,
                reason: _,
            } => {
                if let Some((session_id, target)) = sessions.remove(&gesture_id)
                    && events
                        .send(InputHostEvent::DictationEnd { session_id, target })
                        .is_err()
                {
                    break;
                }
            }
            HookAction::InsertLastRequested => {
                let Some(target) = target::capture_text_target() else {
                    let _ = events.send(InputHostEvent::Failed {
                        session_id: None,
                        code: "protected_field".to_owned(),
                        message: "Text insertion is disabled in password or protected fields."
                            .to_owned(),
                    });
                    continue;
                };
                crate::diagnostic_line(&format!(
                    "hotkey action=insert_last target_pid={} target_hwnd={}",
                    target.process_id, target.window_handle
                ));
                if events
                    .send(InputHostEvent::InsertLastRequested { target })
                    .is_err()
                {
                    break;
                }
            }
            HookAction::SelectionRequested { intent } => {
                let Some(target) = target::capture_text_target() else {
                    SELECTION_BUSY.store(false, Ordering::Release);
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id: RequestId::new(),
                        message: "Selected-text actions are disabled in protected fields."
                            .to_owned(),
                    });
                    continue;
                };
                crate::diagnostic_line(&format!(
                    "hotkey action=selection intent={intent:?} target_pid={} target_hwnd={}",
                    target.process_id, target.window_handle
                ));
                if let Err(error) = begin_selection_capture(intent, target, events.clone()) {
                    SELECTION_BUSY.store(false, Ordering::Release);
                    crate::diagnostic_line(&format!(
                        "selection_capture result=thread_start_failed error={error}"
                    ));
                }
            }
        }
    }
}

fn begin_selection_capture(
    intent: TextActionIntent,
    target: TargetSnapshot,
    events: Sender<InputHostEvent>,
) -> io::Result<()> {
    let request_id = RequestId::new();
    let started_unix_ms = unix_ms();
    events
        .send(InputHostEvent::SelectionCaptureStarted {
            request_id,
            intent,
            target,
            started_unix_ms,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "desktop event stream closed"))?;
    let failure_events = events.clone();
    thread::Builder::new()
        .name("selection-capture".to_owned())
        .spawn(move || {
            let _busy_guard = SelectionBusyGuard;
            let capture_started = Instant::now();
            // UI Automation is read-only, so a successful result can start the
            // translation before the command chord is released. Clipboard copy
            // and every later insertion remain behind the explicit release gate.
            let pending_uia = selection_capture::begin_uia_capture(target);
            let release_started = Instant::now();
            let chord_mask = selection_chord_mask(intent);
            let uia_capture = selection_capture::await_uia_capture(target, pending_uia);

            let uia_ms = match uia_capture {
                Ok(selection_capture::UiaCapture::Captured(captured)) => {
                    dispatch_early_uia_selection(
                        &events,
                        request_id,
                        intent,
                        target,
                        captured,
                        &capture_started,
                        &release_started,
                    );
                    return;
                }
                Ok(selection_capture::UiaCapture::ClipboardFallback { uia_ms }) => uia_ms,
                Err(error) => {
                    crate::diagnostic_line(&format!(
                        "selection_capture request={} pid={} result=failed error={error} \
elapsed_ms={}",
                        request_id,
                        target.process_id,
                        capture_started.elapsed().as_millis(),
                    ));
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id,
                        message: match error {
                            selection_capture::CaptureError::Protected => {
                                "Selected text is protected.".to_owned()
                            }
                            selection_capture::CaptureError::Clipboard(error) => error.to_string(),
                        },
                    });
                    return;
                }
            };

            if !wait_for_mask_clear(
                chord_mask,
                COMMAND_RELEASE_TIMEOUT,
                INPUT_RELEASE_STABILITY,
            ) {
                crate::diagnostic_line(&format!(
                    "selection_capture request={} pid={} stage=hotkey_release result=timeout \
physical_mask={:#x}",
                    request_id,
                    target.process_id,
                    PHYSICAL_INPUT.load(Ordering::Acquire),
                ));
                let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                    request_id,
                    message: "Release the translation shortcut before trying again.".to_owned(),
                });
                return;
            }
            let release_wait_ms = release_started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            match selection_capture::capture_clipboard_after_release(target, uia_ms) {
                Ok(captured) if !captured.text.trim().is_empty() => {
                    let total_ms = capture_started
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    crate::diagnostic_line(&format!(
                        "selection_capture request={} pid={} method={:?} \
result=success chars={} release_wait_ms=Some({}) uia_ms={:?} clipboard_ms={:?} total_ms={}",
                        request_id,
                        target.process_id,
                        captured.method,
                        captured.text.chars().count(),
                        release_wait_ms,
                        captured.uia_ms,
                        captured.clipboard_ms,
                        total_ms,
                    ));
                    let _ = events.send(InputHostEvent::SelectionCaptured {
                        request_id,
                        intent,
                        target,
                        text: captured.text,
                        timing: SelectionCaptureTiming {
                            release_wait_ms: Some(release_wait_ms),
                            uia_ms: captured.uia_ms,
                            clipboard_ms: captured.clipboard_ms,
                            total_ms,
                        },
                    });
                }
                Ok(_) => {
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id,
                        message: "The selection is empty.".to_owned(),
                    });
                }
                Err(error) => {
                    crate::diagnostic_line(&format!(
                        "selection_capture request={} pid={} result=failed error={error} elapsed_ms={}",
                        request_id,
                        target.process_id,
                        capture_started.elapsed().as_millis(),
                    ));
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id,
                        message: error.to_string(),
                    });
                }
            }
        })
        .map(|_| ())
        .inspect_err(|error| {
            let _ = failure_events.send(InputHostEvent::SelectionCaptureFailed {
                request_id,
                message: format!("could not start selection capture: {error}"),
            });
        })
}

fn dispatch_early_uia_selection(
    events: &Sender<InputHostEvent>,
    request_id: RequestId,
    intent: TextActionIntent,
    target: TargetSnapshot,
    captured: selection_capture::CapturedSelection,
    capture_started: &Instant,
    release_started: &Instant,
) {
    if captured.text.trim().is_empty() {
        crate::diagnostic_line(&format!(
            "selection_capture request={} pid={} result=empty elapsed_ms={}",
            request_id,
            target.process_id,
            capture_started.elapsed().as_millis(),
        ));
        let _ = events.send(InputHostEvent::SelectionCaptureFailed {
            request_id,
            message: "The selection is empty.".to_owned(),
        });
        return;
    }

    let chord_mask = selection_chord_mask(intent);
    let released_at_capture =
        wait_for_mask_clear(chord_mask, Duration::ZERO, INPUT_RELEASE_STABILITY);
    let capture_ready_ms = capture_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let release_wait_ms = released_at_capture.then(|| {
        release_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    });
    crate::diagnostic_line(&format!(
        "selection_capture request={} pid={} method={:?} result=success chars={} \
release_wait_ms={:?} uia_ms={:?} clipboard_ms=None total_ms={}",
        request_id,
        target.process_id,
        captured.method,
        captured.text.chars().count(),
        release_wait_ms,
        captured.uia_ms,
        capture_ready_ms,
    ));
    if events
        .send(InputHostEvent::SelectionCaptured {
            request_id,
            intent,
            target,
            text: captured.text,
            timing: SelectionCaptureTiming {
                release_wait_ms,
                uia_ms: captured.uia_ms,
                clipboard_ms: None,
                total_ms: capture_ready_ms,
            },
        })
        .is_err()
    {
        return;
    }

    if released_at_capture {
        return;
    }

    if wait_for_mask_clear(chord_mask, COMMAND_RELEASE_TIMEOUT, INPUT_RELEASE_STABILITY) {
        let release_wait_ms = release_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        crate::diagnostic_line(&format!(
            "selection_capture request={} pid={} stage=input_release \
result=success release_wait_ms={}",
            request_id, target.process_id, release_wait_ms,
        ));
        let _ = events.send(InputHostEvent::SelectionInputReleased {
            request_id,
            release_wait_ms,
        });
    } else {
        let message =
            "The translation shortcut was not released before the safety timeout.".to_owned();
        crate::diagnostic_line(&format!(
            "selection_capture request={} pid={} stage=input_release \
result=timeout physical_mask={:#x}",
            request_id,
            target.process_id,
            PHYSICAL_INPUT.load(Ordering::Acquire),
        ));
        let _ = events.send(InputHostEvent::SelectionInputReleaseFailed {
            request_id,
            message,
        });
    }
}

struct SelectionBusyGuard;

impl Drop for SelectionBusyGuard {
    fn drop(&mut self) {
        SELECTION_BUSY.store(false, Ordering::Release);
    }
}

#[must_use]
pub fn injection_is_safe() -> bool {
    PHYSICAL_INPUT.load(Ordering::Acquire) & INJECTION_UNSAFE == 0
}

#[must_use]
pub fn wait_until_injection_safe(timeout: Duration) -> bool {
    wait_for_mask_clear(INJECTION_UNSAFE, timeout, INPUT_RELEASE_STABILITY)
}

fn wait_for_mask_clear(mask: u32, timeout: Duration, stable_for: Duration) -> bool {
    wait_for_mask_clear_using(
        mask,
        timeout,
        stable_for,
        || PHYSICAL_INPUT.load(Ordering::Acquire),
        || LAST_PHYSICAL_CHANGE_US.load(Ordering::Acquire),
        || PhysicalKeys::from_async_except(u16::MAX, &PhysicalKeys::default()).input_mask(),
        |sampled| reconcile_physical_mask(mask, sampled),
    )
}

fn wait_for_mask_clear_using(
    mask: u32,
    timeout: Duration,
    stable_for: Duration,
    mut load_input: impl FnMut() -> u32,
    mut load_last_change_us: impl FnMut() -> u64,
    mut sample_async_input: impl FnMut() -> u32,
    mut reconcile: impl FnMut(u32),
) -> bool {
    let started = Instant::now();
    loop {
        if load_input() & mask == 0 {
            let last_change = load_last_change_us();
            let stable_us = stable_for.as_micros().try_into().unwrap_or(u64::MAX);
            if monotonic_us().saturating_sub(last_change) >= stable_us {
                return true;
            }
        }
        // LL hooks can miss a key-up while focus crosses integrity/session
        // boundaries. A one-shot sample is insufficient: users commonly keep
        // the modifiers held beyond the initial 75 ms. Reconcile only the bits
        // this caller is waiting for, repeatedly and outside the hook callback,
        // then require the normal stability window before accepting release.
        if started.elapsed() >= ASYNC_RECONCILIATION_DELAY {
            reconcile(sample_async_input());
        }
        if started.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn reconcile_physical_mask(mask: u32, sampled: u32) {
    let previous = PHYSICAL_INPUT
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some((current & !mask) | (sampled & mask))
        })
        .unwrap_or_else(|current| current);
    let reconciled = (previous & !mask) | (sampled & mask);
    if reconciled != previous {
        LAST_PHYSICAL_CHANGE_US.store(monotonic_us(), Ordering::Release);
        LAST_HOOK_EVENT_US.store(0, Ordering::Release);
    }
}

const fn selection_chord_mask(intent: TextActionIntent) -> u32 {
    INPUT_CONTROL
        | INPUT_ALT
        | match intent {
            TextActionIntent::CorrectLayout => INPUT_K,
            TextActionIntent::Adaptive | TextActionIntent::Translate => INPUT_T,
        }
}

fn monotonic_us() -> u64 {
    MONOTONIC_ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl HotkeyState {
    fn reconcile(&mut self, keys: PhysicalKeys) -> Option<HookAction> {
        self.keys = keys;
        self.selection_k_down &= self.keys.is_down(VK_K);
        self.insert_v_down &= self.keys.is_down(VK_V);
        self.adaptive_t_down &= self.keys.is_down(VK_T);
        match self.gesture {
            GestureState::Active { gesture_id }
                if !self.keys.command_modifiers() || !self.keys.is_down(VK_SPACE) =>
            {
                let reason = if !self.keys.is_down(VK_SPACE) {
                    EndReason::TriggerReleased
                } else {
                    EndReason::RequiredModifierReleased
                };
                self.gesture = if self.keys.dictation_chord_released() {
                    GestureState::Idle
                } else {
                    GestureState::Draining { gesture_id }
                };
                Some(HookAction::DictationEnded { gesture_id, reason })
            }
            GestureState::Draining { .. } if self.keys.dictation_chord_released() => {
                self.gesture = GestureState::Idle;
                None
            }
            _ => None,
        }
    }

    fn process_event(
        &mut self,
        virtual_key: u16,
        key_down: bool,
        key_up: bool,
        registered_commands: u32,
    ) -> HookDecision {
        let state_before = self.gesture.name();
        let was_down = self.keys.is_down(virtual_key);
        self.keys.set(virtual_key, key_down, key_up);
        let repeated = key_down && was_down;
        let mut suppress = false;
        let mut action = None;

        match self.gesture {
            GestureState::Idle => {
                if virtual_key == VK_SPACE
                    && key_down
                    && !repeated
                    && self.keys.command_modifiers()
                    && !self.keys.shift()
                    && !self.keys.windows()
                {
                    let gesture_id = self.next_gesture_id.max(1);
                    self.next_gesture_id = gesture_id.saturating_add(1);
                    self.gesture = GestureState::Active { gesture_id };
                    suppress = true;
                    action = Some(HookAction::DictationStarted { gesture_id });
                }
            }
            GestureState::Active { gesture_id } => {
                if virtual_key == VK_SPACE {
                    suppress = true;
                }
                let trigger_released = virtual_key == VK_SPACE && key_up;
                let required_modifier_released = key_up
                    && (is_command_control(virtual_key) || is_command_alt(virtual_key))
                    && !self.keys.command_modifiers();
                if trigger_released || required_modifier_released {
                    let reason = if trigger_released {
                        EndReason::TriggerReleased
                    } else {
                        EndReason::RequiredModifierReleased
                    };
                    self.gesture = if self.keys.dictation_chord_released() {
                        GestureState::Idle
                    } else {
                        GestureState::Draining { gesture_id }
                    };
                    action = Some(HookAction::DictationEnded { gesture_id, reason });
                }
            }
            GestureState::Draining { gesture_id: _ } => {
                if virtual_key == VK_SPACE {
                    suppress = true;
                }
                if self.keys.dictation_chord_released() {
                    self.gesture = GestureState::Idle;
                }
            }
        }

        if !matches!(self.gesture, GestureState::Idle) || action.is_some() {
            return HookDecision {
                suppress,
                action,
                state_before,
                state_after: self.gesture.name(),
                repeated,
            };
        }

        if registered_commands & INPUT_K == 0 && virtual_key == VK_K {
            if self.selection_k_down {
                suppress = true;
                if key_up {
                    self.selection_k_down = false;
                }
            } else if key_down
                && !repeated
                && self.keys.command_modifiers()
                && !self.keys.shift()
                && !self.keys.windows()
            {
                self.selection_k_down = true;
                suppress = true;
                action = Some(HookAction::SelectionRequested {
                    intent: TextActionIntent::CorrectLayout,
                });
            }
        }
        if registered_commands & INPUT_V == 0 && virtual_key == VK_V {
            if self.insert_v_down {
                suppress = true;
                if key_up {
                    self.insert_v_down = false;
                }
            } else if key_down
                && !repeated
                && self.keys.command_modifiers()
                && !self.keys.shift()
                && !self.keys.windows()
            {
                self.insert_v_down = true;
                suppress = true;
                action = Some(HookAction::InsertLastRequested);
            }
        }
        if registered_commands & INPUT_T == 0 && virtual_key == VK_T {
            if self.adaptive_t_down {
                suppress = true;
                if key_up {
                    self.adaptive_t_down = false;
                }
            } else if key_down
                && !repeated
                && self.keys.command_modifiers()
                && !self.keys.shift()
                && !self.keys.windows()
            {
                self.adaptive_t_down = true;
                suppress = true;
                action = Some(HookAction::SelectionRequested {
                    intent: TextActionIntent::Translate,
                });
            }
        }

        HookDecision {
            suppress,
            action,
            state_before,
            state_after: self.gesture.name(),
            repeated,
        }
    }

    fn rollback_start(&mut self, gesture_id: u64) {
        if self.gesture == (GestureState::Active { gesture_id }) {
            self.gesture = GestureState::Idle;
        }
    }
}

impl GestureState {
    const fn name(self) -> GestureStateName {
        match self {
            Self::Idle => GestureStateName::Idle,
            Self::Active { .. } => GestureStateName::Active,
            Self::Draining { .. } => GestureStateName::Draining,
        }
    }
}

impl PhysicalKeys {
    fn from_async_except(excluded_key: u16, previous: &Self) -> Self {
        let mut keys = Self::default();
        for virtual_key in [
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LWIN,
            VK_RWIN,
            VK_SPACE,
            VK_K,
            VK_V,
            VK_T,
        ] {
            let down = if virtual_key == excluded_key {
                previous.is_down(virtual_key)
            } else {
                (unsafe { GetAsyncKeyState(i32::from(virtual_key)) }) < 0
            };
            keys.down[usize::from(virtual_key)] = down;
        }
        keys
    }

    fn is_down(&self, virtual_key: u16) -> bool {
        self.down
            .get(usize::from(virtual_key))
            .copied()
            .unwrap_or(false)
    }

    fn set(&mut self, virtual_key: u16, key_down: bool, key_up: bool) {
        if let Some(state) = self.down.get_mut(usize::from(virtual_key)) {
            if key_down {
                *state = true;
            } else if key_up {
                *state = false;
            }
        }
    }

    fn any(&self, keys: &[u16]) -> bool {
        keys.iter().any(|key| self.is_down(*key))
    }

    fn control(&self) -> bool {
        self.any(&[VK_CONTROL, VK_LCONTROL, VK_RCONTROL])
    }

    fn alt(&self) -> bool {
        self.any(&[VK_MENU, VK_LMENU, VK_RMENU])
    }

    fn command_control(&self) -> bool {
        self.any(&[VK_CONTROL, VK_LCONTROL]) && !self.is_down(VK_RCONTROL)
    }

    fn command_alt(&self) -> bool {
        self.any(&[VK_MENU, VK_LMENU]) && !self.is_down(VK_RMENU)
    }

    fn command_modifiers(&self) -> bool {
        self.command_control() && self.command_alt()
    }

    fn shift(&self) -> bool {
        self.any(&[VK_SHIFT, VK_LSHIFT, VK_RSHIFT])
    }

    fn windows(&self) -> bool {
        self.any(&[VK_LWIN, VK_RWIN])
    }

    fn dictation_chord_released(&self) -> bool {
        !self.command_control() && !self.command_alt() && !self.is_down(VK_SPACE)
    }

    fn input_mask(&self) -> u32 {
        let mut mask = 0;
        if self.control() {
            mask |= INPUT_CONTROL;
        }
        if self.alt() {
            mask |= INPUT_ALT;
        }
        if self.shift() {
            mask |= INPUT_SHIFT;
        }
        if self.windows() {
            mask |= INPUT_WINDOWS;
        }
        if self.is_down(VK_SPACE) {
            mask |= INPUT_SPACE;
        }
        if self.is_down(VK_K) {
            mask |= INPUT_K;
        }
        if self.is_down(VK_V) {
            mask |= INPUT_V;
        }
        if self.is_down(VK_T) {
            mask |= INPUT_T;
        }
        mask
    }
}

fn is_command_control(virtual_key: u16) -> bool {
    matches!(virtual_key, VK_CONTROL | VK_LCONTROL)
}

fn is_command_alt(virtual_key: u16) -> bool {
    matches!(virtual_key, VK_MENU | VK_LMENU)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(debug_assertions)]
fn trace_actionable_event(
    input: &KBDLLHOOKSTRUCT,
    key_down: bool,
    key_up: bool,
    origin: EventOrigin,
    decision: HookDecision,
) {
    if !should_trace(input.vkCode as u16, decision) {
        return;
    }
    emit_trace(HookTrace {
        sequence_number: TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        timestamp_unix_ms: unix_ms(),
        vk: input.vkCode as u16,
        key_down,
        key_up,
        origin,
        state_before: decision.state_before,
        state_after: decision.state_after,
        repeated: decision.repeated,
        suppressed: decision.suppress,
        gesture_id: decision.action.and_then(action_gesture_id),
        action: decision.action,
    });
}

#[cfg(not(debug_assertions))]
fn trace_actionable_event(
    _input: &KBDLLHOOKSTRUCT,
    _key_down: bool,
    _key_up: bool,
    _origin: EventOrigin,
    decision: HookDecision,
) {
    let _ = (decision.state_before, decision.repeated);
}

#[cfg(debug_assertions)]
fn trace_injected_event(
    input: &KBDLLHOOKSTRUCT,
    key_down: bool,
    key_up: bool,
    origin: EventOrigin,
) {
    if !is_hotkey_key(input.vkCode as u16) {
        return;
    }
    let state = HOOK_STATE.with(|state| state.borrow().gesture.name());
    emit_trace(HookTrace {
        sequence_number: TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        timestamp_unix_ms: unix_ms(),
        vk: input.vkCode as u16,
        key_down,
        key_up,
        origin,
        state_before: state,
        state_after: state,
        repeated: false,
        suppressed: false,
        gesture_id: None,
        action: None,
    });
}

#[cfg(not(debug_assertions))]
fn trace_injected_event(
    _input: &KBDLLHOOKSTRUCT,
    _key_down: bool,
    _key_up: bool,
    _origin: EventOrigin,
) {
}

#[cfg(debug_assertions)]
fn emit_trace(trace: HookTrace) {
    if let Some(events) = TRACE_EVENTS.get() {
        let _ = events.try_send(trace);
    }
}

#[cfg(debug_assertions)]
fn should_trace(virtual_key: u16, decision: HookDecision) -> bool {
    is_hotkey_key(virtual_key)
        || decision.suppress
        || decision.action.is_some()
        || decision.state_before != decision.state_after
}

#[cfg(debug_assertions)]
const fn is_hotkey_key(virtual_key: u16) -> bool {
    matches!(
        virtual_key,
        VK_CONTROL
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_MENU
            | VK_LMENU
            | VK_RMENU
            | VK_SHIFT
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LWIN
            | VK_RWIN
            | VK_SPACE
            | VK_K
            | VK_V
            | VK_T
    )
}

#[cfg(debug_assertions)]
fn write_trace(trace: HookTrace) {
    eprintln!(
        "input_trace sequence={} timestamp_ms={} vk={} down={} up={} \
origin={:?} state_before={:?} state_after={:?} repeated={} suppressed={} \
gesture_id={:?} action={:?}",
        trace.sequence_number,
        trace.timestamp_unix_ms,
        trace.vk,
        trace.key_down,
        trace.key_up,
        trace.origin,
        trace.state_before,
        trace.state_after,
        trace.repeated,
        trace.suppressed,
        trace.gesture_id,
        trace.action,
    );
}

#[cfg(debug_assertions)]
const fn action_gesture_id(action: HookAction) -> Option<u64> {
    match action {
        HookAction::DictationStarted { gesture_id }
        | HookAction::DictationEnded { gesture_id, .. } => Some(gesture_id),
        HookAction::InsertLastRequested | HookAction::SelectionRequested { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Duration};

    use rust_protocol::TextActionIntent;

    use super::{
        EndReason, EventOrigin, GestureState, HOTKEY_ID_CORRECT_LAYOUT, HOTKEY_ID_INSERT_LAST,
        HOTKEY_ID_TRANSLATE, HookAction, HotkeyState, INPUT_ALT, INPUT_CONTROL, INPUT_K, INPUT_T,
        INPUT_V, PhysicalKeys, classify_event, monotonic_us, registered_hotkey_action,
        selection_chord_mask, wait_for_mask_clear_using,
    };
    use crate::inject::SYNTHETIC_MARKER;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            VK_K, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_SPACE, VK_T,
            VK_V,
        },
        WindowsAndMessaging::LLKHF_INJECTED,
    };

    fn event(state: &mut HotkeyState, key: u16, down: bool) -> super::HookDecision {
        state.process_event(key, down, !down, 0)
    }

    fn registered_event(
        state: &mut HotkeyState,
        key: u16,
        down: bool,
        registered_commands: u32,
    ) -> super::HookDecision {
        state.process_event(key, down, !down, registered_commands)
    }

    fn gesture_id(action: Option<HookAction>) -> Option<u64> {
        match action {
            Some(
                HookAction::DictationStarted { gesture_id }
                | HookAction::DictationEnded { gesture_id, .. },
            ) => Some(gesture_id),
            _ => None,
        }
    }

    #[test]
    fn control_alt_space_emits_exactly_one_begin_and_end() {
        let mut state = HotkeyState::default();
        assert!(!event(&mut state, VK_LCONTROL, true).suppress);
        assert!(!event(&mut state, VK_LMENU, true).suppress);

        let begin = event(&mut state, VK_SPACE, true);
        assert!(begin.suppress);
        assert!(matches!(
            begin.action,
            Some(HookAction::DictationStarted { .. })
        ));
        let session = gesture_id(begin.action);

        for _ in 0..500 {
            let repeat = event(&mut state, VK_SPACE, true);
            assert!(repeat.suppress);
            assert!(repeat.action.is_none());
        }

        let end = event(&mut state, VK_SPACE, false);
        assert!(end.suppress);
        assert_eq!(gesture_id(end.action), session);
        assert!(matches!(
            end.action,
            Some(HookAction::DictationEnded {
                reason: EndReason::TriggerReleased,
                ..
            })
        ));
        assert!(matches!(state.gesture, GestureState::Draining { .. }));

        assert!(!event(&mut state, VK_LCONTROL, false).suppress);
        assert!(!event(&mut state, VK_LMENU, false).suppress);
        assert_eq!(state.gesture, GestureState::Idle);
    }

    #[test]
    fn releasing_modifier_first_ends_and_drains_trigger() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        let begin = event(&mut state, VK_SPACE, true);
        let session = gesture_id(begin.action);

        let end = event(&mut state, VK_LCONTROL, false);
        assert!(!end.suppress);
        assert_eq!(gesture_id(end.action), session);
        assert!(matches!(
            end.action,
            Some(HookAction::DictationEnded {
                reason: EndReason::RequiredModifierReleased,
                ..
            })
        ));

        assert!(event(&mut state, VK_SPACE, true).suppress);
        assert!(!event(&mut state, VK_LMENU, false).suppress);
        assert!(event(&mut state, VK_SPACE, false).suppress);
        assert_eq!(state.gesture, GestureState::Idle);
    }

    #[test]
    fn modifier_repress_during_draining_never_restarts() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        event(&mut state, VK_SPACE, true);
        assert!(event(&mut state, VK_LMENU, false).action.is_some());
        assert!(!event(&mut state, VK_LMENU, true).suppress);
        assert!(event(&mut state, VK_SPACE, true).action.is_none());
        assert!(event(&mut state, VK_SPACE, false).suppress);
        assert!(!event(&mut state, VK_LCONTROL, false).suppress);
        assert!(!event(&mut state, VK_LMENU, false).suppress);
        assert_eq!(state.gesture, GestureState::Idle);
    }

    #[test]
    fn quick_tap_is_one_complete_gesture() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        let begin = event(&mut state, VK_SPACE, true);
        let end = event(&mut state, VK_SPACE, false);
        assert!(begin.action.is_some());
        assert_eq!(gesture_id(begin.action), gesture_id(end.action));
    }

    #[test]
    fn right_alt_or_right_control_never_activates_the_command_chord() {
        for (control, alt) in [(VK_LCONTROL, VK_RMENU), (VK_RCONTROL, VK_LMENU)] {
            let mut state = HotkeyState::default();
            event(&mut state, control, true);
            event(&mut state, alt, true);
            let space = event(&mut state, VK_SPACE, true);
            assert!(!space.suppress);
            assert!(space.action.is_none());
        }
    }

    #[test]
    fn command_keys_emit_once_and_are_suppressed_until_release() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_K, VK_T, VK_V};

        let cases = [
            (VK_V, HookAction::InsertLastRequested),
            (
                VK_T,
                HookAction::SelectionRequested {
                    intent: TextActionIntent::Translate,
                },
            ),
            (
                VK_K,
                HookAction::SelectionRequested {
                    intent: TextActionIntent::CorrectLayout,
                },
            ),
        ];
        for (key, expected) in cases {
            let mut state = HotkeyState::default();
            event(&mut state, VK_LCONTROL, true);
            event(&mut state, VK_LMENU, true);
            let first = event(&mut state, key, true);
            assert!(first.suppress);
            assert_eq!(first.action, Some(expected));
            let repeat = event(&mut state, key, true);
            assert!(repeat.suppress);
            assert!(repeat.action.is_none());
            let release = event(&mut state, key, false);
            assert!(release.suppress);
            assert!(release.action.is_none());
        }
    }

    #[test]
    fn registered_commands_are_owned_by_wm_hotkey_instead_of_the_hook() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        for (key, mask) in [(VK_K, INPUT_K), (VK_V, INPUT_V), (VK_T, INPUT_T)] {
            let decision = registered_event(&mut state, key, true, mask);
            assert!(!decision.suppress);
            assert!(decision.action.is_none());
            let _ = registered_event(&mut state, key, false, mask);
        }
    }

    #[test]
    fn registered_hotkey_ids_map_to_the_expected_commands() {
        assert_eq!(
            registered_hotkey_action(HOTKEY_ID_CORRECT_LAYOUT),
            Some(HookAction::SelectionRequested {
                intent: TextActionIntent::CorrectLayout,
            })
        );
        assert_eq!(
            registered_hotkey_action(HOTKEY_ID_INSERT_LAST),
            Some(HookAction::InsertLastRequested)
        );
        assert_eq!(
            registered_hotkey_action(HOTKEY_ID_TRANSLATE),
            Some(HookAction::SelectionRequested {
                intent: TextActionIntent::Translate,
            })
        );
        assert_eq!(registered_hotkey_action(i32::MAX), None);
    }

    #[test]
    fn extra_shift_or_windows_prevents_activation() {
        for extra in [VK_LSHIFT, VK_LWIN] {
            let mut state = HotkeyState::default();
            event(&mut state, VK_LCONTROL, true);
            event(&mut state, VK_LMENU, true);
            event(&mut state, extra, true);
            let space = event(&mut state, VK_SPACE, true);
            assert!(!space.suppress);
            assert!(space.action.is_none());
        }
    }

    #[test]
    fn space_without_exact_chord_is_untouched() {
        let mut state = HotkeyState::default();
        let down = event(&mut state, VK_SPACE, true);
        let up = event(&mut state, VK_SPACE, false);
        assert!(!down.suppress);
        assert!(!up.suppress);
        assert!(down.action.is_none());
        assert!(up.action.is_none());
    }

    #[test]
    fn duplicate_key_up_never_emits_duplicate_end() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        event(&mut state, VK_SPACE, true);
        assert!(event(&mut state, VK_SPACE, false).action.is_some());
        assert!(event(&mut state, VK_SPACE, false).action.is_none());
    }

    #[test]
    fn reconciliation_ends_a_gesture_after_events_were_lost() {
        let mut state = HotkeyState::default();
        event(&mut state, VK_LCONTROL, true);
        event(&mut state, VK_LMENU, true);
        let begin = event(&mut state, VK_SPACE, true);
        let expected_id = gesture_id(begin.action);

        let recovered = state.reconcile(PhysicalKeys::default());
        assert_eq!(gesture_id(recovered), expected_id);
        assert!(matches!(
            recovered,
            Some(HookAction::DictationEnded {
                reason: EndReason::TriggerReleased,
                ..
            })
        ));
        assert_eq!(state.gesture, GestureState::Idle);
    }

    #[test]
    fn injected_events_are_classified_without_becoming_physical() {
        assert_eq!(
            classify_event(LLKHF_INJECTED, SYNTHETIC_MARKER),
            EventOrigin::OwnInjected
        );
        assert_eq!(
            classify_event(LLKHF_INJECTED, 0x1234),
            EventOrigin::ForeignInjected
        );
        assert_eq!(classify_event(0, 0), EventOrigin::Physical);
    }

    #[test]
    fn foreign_injection_remains_distinguishable_from_owned_injection() {
        assert_eq!(
            classify_event(LLKHF_INJECTED, 0x1234),
            EventOrigin::ForeignInjected
        );
    }

    #[test]
    fn selection_capture_waits_for_the_actual_trigger_key() {
        assert_eq!(
            selection_chord_mask(TextActionIntent::CorrectLayout),
            INPUT_CONTROL | INPUT_ALT | INPUT_K
        );
        assert_eq!(
            selection_chord_mask(TextActionIntent::Adaptive),
            INPUT_CONTROL | INPUT_ALT | INPUT_T
        );
        assert_eq!(
            selection_chord_mask(TextActionIntent::Translate),
            INPUT_CONTROL | INPUT_ALT | INPUT_T
        );
    }

    #[test]
    fn release_wait_reconciles_until_a_late_modifier_release_is_stable() {
        let chord = INPUT_CONTROL | INPUT_ALT | INPUT_T;
        let input = Cell::new(chord);
        let last_change = Cell::new(monotonic_us());
        let samples = Cell::new(0_u32);

        let released = wait_for_mask_clear_using(
            chord,
            Duration::from_millis(300),
            Duration::from_millis(10),
            || input.get(),
            || last_change.get(),
            || {
                let next = samples.get() + 1;
                samples.set(next);
                if next < 3 { chord } else { 0 }
            },
            |sampled| {
                let previous = input.get();
                let reconciled = (previous & !chord) | (sampled & chord);
                if reconciled != previous {
                    input.set(reconciled);
                    last_change.set(monotonic_us());
                }
            },
        );

        assert!(released);
        assert!(samples.get() >= 3);
        assert_eq!(input.get() & chord, 0);
    }
}
