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

use rust_protocol::{InputHostEvent, RequestId, SessionId, TargetSnapshot, TextActionIntent};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_K, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
            VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_T, VK_V,
        },
        WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
            WM_SYSKEYUP,
        },
    },
};

use crate::{clipboard, inject, target};

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

static ACTIONS: OnceLock<Sender<HookAction>> = OnceLock::new();
static PHYSICAL_INPUT: AtomicU32 = AtomicU32::new(0);
static MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();
static LAST_PHYSICAL_CHANGE_US: AtomicU64 = AtomicU64::new(0);
static LAST_HOOK_EVENT_US: AtomicU64 = AtomicU64::new(0);
static SELECTION_BUSY: AtomicBool = AtomicBool::new(false);

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
    if origin != EventOrigin::Physical {
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
        let decision = state.process_event(input.vkCode as u16, key_down, key_up);
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

    trace_physical_event(input, key_down, key_up, decision);
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
                if let Err(error) = begin_selection_capture(intent, target, events.clone()) {
                    SELECTION_BUSY.store(false, Ordering::Release);
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id: RequestId::new(),
                        message: error.to_string(),
                    });
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
    thread::Builder::new()
        .name("selection-capture".to_owned())
        .spawn(move || {
            let _busy_guard = SelectionBusyGuard;
            let result = if !wait_for_mask_clear(
                selection_chord_mask(intent),
                Duration::from_millis(1200),
                INPUT_RELEASE_STABILITY,
            ) {
                Err(clipboard::ClipboardError::CopyTimeout)
            } else if inject::is_foreground(target) {
                let copy_shortcut = target::selection_copy_shortcut(target);
                clipboard::capture_selection(|| {
                    inject::copy_shortcut(copy_shortcut)
                        .map_err(|_| clipboard::ClipboardError::InvalidData)
                })
            } else {
                Err(clipboard::ClipboardError::CopyTimeout)
            };
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    let _ = events.send(InputHostEvent::SelectionCaptured {
                        request_id,
                        intent,
                        target,
                        text,
                    });
                }
                Ok(_) => {
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id,
                        message: "The selection is empty.".to_owned(),
                    });
                }
                Err(error) => {
                    let _ = events.send(InputHostEvent::SelectionCaptureFailed {
                        request_id,
                        message: error.to_string(),
                    });
                }
            }
        })
        .map(|_| ())
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
    let started = Instant::now();
    let mut reconciled = false;
    loop {
        if PHYSICAL_INPUT.load(Ordering::Acquire) & mask == 0 {
            let last_change = LAST_PHYSICAL_CHANGE_US.load(Ordering::Acquire);
            let stable_us = stable_for.as_micros().try_into().unwrap_or(u64::MAX);
            if monotonic_us().saturating_sub(last_change) >= stable_us {
                return true;
            }
        }
        // LL hooks do not observe key releases on the secure desktop. Reconcile
        // only after the normal release window has elapsed, and force the
        // hook-thread state to reconcile on its next physical event as well.
        if !reconciled && started.elapsed() >= Duration::from_millis(75) {
            reconciled = true;
            let sampled =
                PhysicalKeys::from_async_except(u16::MAX, &PhysicalKeys::default()).input_mask();
            let previous = PHYSICAL_INPUT.swap(sampled, Ordering::AcqRel);
            if sampled != previous {
                LAST_PHYSICAL_CHANGE_US.store(monotonic_us(), Ordering::Release);
                LAST_HOOK_EVENT_US.store(0, Ordering::Release);
            }
        }
        if started.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
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

    fn process_event(&mut self, virtual_key: u16, key_down: bool, key_up: bool) -> HookDecision {
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

        if virtual_key == VK_K {
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
        if virtual_key == VK_V {
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
        if virtual_key == VK_T {
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
fn trace_physical_event(
    input: &KBDLLHOOKSTRUCT,
    key_down: bool,
    key_up: bool,
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
        origin: EventOrigin::Physical,
        state_before: decision.state_before,
        state_after: decision.state_after,
        repeated: decision.repeated,
        suppressed: decision.suppress,
        gesture_id: decision.action.and_then(action_gesture_id),
        action: decision.action,
    });
}

#[cfg(not(debug_assertions))]
fn trace_physical_event(
    _input: &KBDLLHOOKSTRUCT,
    _key_down: bool,
    _key_up: bool,
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
    use rust_protocol::TextActionIntent;

    use super::{
        EndReason, EventOrigin, GestureState, HookAction, HotkeyState, INPUT_ALT, INPUT_CONTROL,
        INPUT_K, INPUT_T, PhysicalKeys, classify_event, selection_chord_mask,
    };
    use crate::inject::SYNTHETIC_MARKER;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_SPACE,
        },
        WindowsAndMessaging::LLKHF_INJECTED,
    };

    fn event(state: &mut HotkeyState, key: u16, down: bool) -> super::HookDecision {
        state.process_event(key, down, !down)
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
}
