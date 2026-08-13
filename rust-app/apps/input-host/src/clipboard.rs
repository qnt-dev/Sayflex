use std::{
    ffi::c_void,
    mem, ptr, thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use windows_sys::{
    Win32::{
        Foundation::{GlobalFree, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
                GetClipboardSequenceNumber, GetOpenClipboardWindow, IsClipboardFormatAvailable,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, OleGetClipboard, OleInitialize, OleSetClipboard, OleUninitialize,
            },
        },
        UI::WindowsAndMessaging::{
            CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GA_ROOT, GWLP_USERDATA, GetAncestor, GetForegroundWindow, GetWindowLongPtrW,
            GetWindowThreadProcessId, HWND_MESSAGE, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
            SetWindowLongPtrW, TranslateMessage, WM_NCCREATE, WM_RENDERFORMAT, WNDCLASSW,
        },
    },
    core::IUnknown_Vtbl,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClipboardError {
    #[error("clipboard is busy")]
    Busy,
    #[error("clipboard allocation failed")]
    Allocation,
    #[error("clipboard does not contain Unicode text")]
    NoText,
    #[error("selected text was not copied before the timeout")]
    CopyTimeout,
    #[error("clipboard data is invalid")]
    InvalidData,
    #[error("the target application changed before paste")]
    TargetChanged,
    #[error("paste shortcut injection failed")]
    Paste,
    #[error("the target did not request the insertion payload")]
    NotDelivered,
    #[error("the clipboard was superseded before the target requested the insertion payload")]
    Superseded,
    #[error("OLE clipboard operation failed with HRESULT {0:#010x}")]
    Ole(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardCaptureMethod {
    WindowMessage,
    Shortcut,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ClipboardCapture {
    pub text: String,
    pub method: ClipboardCaptureMethod,
}

pub struct ClipboardApartment {
    initialized: bool,
}

const PASTE_READ_ACK_TIMEOUT: Duration = Duration::from_millis(2_500);
// Clipboard brokers and synchronizers can materialize delayed data before an
// Electron/Chromium editor consumes the paste command. Keep the transaction
// alive long enough for that editor to read the already-materialized payload;
// restoring immediately after an anonymous read races the actual target.
const PASTE_READ_POLL_INTERVAL: Duration = Duration::from_millis(2);
const RENDER_WINDOW_CLASS: &[u16] = &[
    b'S' as u16,
    b'a' as u16,
    b'y' as u16,
    b'F' as u16,
    b'l' as u16,
    b'e' as u16,
    b'x' as u16,
    b'.' as u16,
    b'C' as u16,
    b'l' as u16,
    b'i' as u16,
    b'p' as u16,
    b'b' as u16,
    b'o' as u16,
    b'a' as u16,
    b'r' as u16,
    b'd' as u16,
    b'T' as u16,
    b'r' as u16,
    b'a' as u16,
    b'n' as u16,
    b's' as u16,
    b'a' as u16,
    b'c' as u16,
    b't' as u16,
    b'i' as u16,
    b'o' as u16,
    b'n' as u16,
    0,
];

#[derive(Default)]
struct ClipboardRenderState {
    payload: String,
    target_process_id: u32,
    target_window_handle: u64,
    rendered: bool,
    target_requested: bool,
    render_failed: bool,
}

/// Long-lived delayed-rendering owner for automatic insertion. The payload is
/// not materialized until a consumer asks for CF_UNICODETEXT, which lets the
/// insertion worker distinguish a target read from mere SendInput acceptance.
pub struct ClipboardPasteHost {
    hwnd: HWND,
    state: Box<ClipboardRenderState>,
}

impl ClipboardPasteHost {
    pub fn create() -> Result<Self, ClipboardError> {
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        if instance.is_null() {
            return Err(ClipboardError::Allocation);
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(clipboard_render_window_proc),
            hInstance: instance,
            lpszClassName: RENDER_WINDOW_CLASS.as_ptr(),
            ..Default::default()
        };
        unsafe {
            // RegisterClassW can legitimately report an existing class when a
            // smoke fixture starts a second host in the same process. Window
            // creation below remains the authoritative success check.
            let _ = RegisterClassW(&raw const class);
        }

        let mut state = Box::<ClipboardRenderState>::default();
        let state_pointer = (&mut *state as *mut ClipboardRenderState).cast::<c_void>();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                RENDER_WINDOW_CLASS.as_ptr(),
                RENDER_WINDOW_CLASS.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                instance,
                state_pointer,
            )
        };
        if hwnd.is_null() {
            return Err(ClipboardError::Allocation);
        }
        Ok(Self { hwnd, state })
    }

    fn publish(
        &mut self,
        payload: &str,
        target_process_id: u32,
        target_window_handle: u64,
    ) -> Result<u32, ClipboardError> {
        self.state.payload.clear();
        self.state.payload.push_str(payload);
        self.state.target_process_id = target_process_id;
        self.state.target_window_handle = target_window_handle;
        self.state.rendered = false;
        self.state.target_requested = false;
        self.state.render_failed = false;

        let mut private_payloads = private_format_payloads()?;
        with_open_clipboard_owner(self.hwnd, || {
            if unsafe { EmptyClipboard() } == 0 {
                return Err(ClipboardError::Busy);
            }
            // A null handle advertises delayed rendering. Windows sends
            // WM_RENDERFORMAT only when a consumer actually requests text.
            unsafe {
                let _ = SetClipboardData(u32::from(CF_UNICODETEXT), ptr::null_mut());
            }
            for (format, payload) in &mut private_payloads {
                let accepted = unsafe { SetClipboardData(*format, payload.handle) };
                if accepted.is_null() {
                    unsafe {
                        let _ = EmptyClipboard();
                    }
                    return Err(ClipboardError::Allocation);
                }
                payload.release_to_clipboard();
            }
            Ok(())
        })?;
        if unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) } == 0 {
            return Err(ClipboardError::Allocation);
        }
        Ok(unsafe { GetClipboardSequenceNumber() })
    }

    fn await_target_read(
        &mut self,
        target_still_focused: impl Fn() -> bool,
        verification_available: bool,
        mut target_contains_payload: impl FnMut() -> bool,
    ) -> Result<ClipboardReadOutcome, ClipboardError> {
        let started = Instant::now();
        let deadline = Instant::now() + PASTE_READ_ACK_TIMEOUT;
        loop {
            self.pump_messages();
            if self.state.render_failed {
                return Err(ClipboardError::Allocation);
            }
            if target_contains_payload() {
                return Ok(ClipboardReadOutcome::TargetVerified);
            }
            if self.state.rendered {
                if self.state.target_requested {
                    self.wait_until_clipboard_released(deadline);
                    return Ok(ClipboardReadOutcome::TargetRead);
                }
            }
            if !self.owns_clipboard() {
                return Ok(ClipboardReadOutcome::Superseded);
            }
            if !target_still_focused() {
                return Ok(ClipboardReadOutcome::TargetChanged);
            }
            if self.state.rendered
                && !verification_available
                && started.elapsed() >= PASTE_READ_ACK_TIMEOUT
            {
                return Ok(ClipboardReadOutcome::DispatchedUnverified);
            }
            if Instant::now() >= deadline {
                return Ok(if self.state.rendered {
                    ClipboardReadOutcome::DispatchedUnverified
                } else {
                    ClipboardReadOutcome::TimedOut
                });
            }
            thread::sleep(PASTE_READ_POLL_INTERVAL);
        }
    }

    fn materialize_if_owned(&mut self) -> Result<(), ClipboardError> {
        if !self.owns_clipboard() || self.state.rendered {
            return Ok(());
        }
        with_open_clipboard_owner(self.hwnd, || render_payload(&mut self.state))
    }

    fn owns_clipboard(&self) -> bool {
        (unsafe { GetClipboardOwner() }) == self.hwnd
    }

    fn wait_until_clipboard_released(&mut self, deadline: Instant) {
        while Instant::now() < deadline {
            if unsafe { OpenClipboard(self.hwnd) } != 0 {
                unsafe {
                    let _ = CloseClipboard();
                }
                return;
            }
            self.pump_messages();
            thread::sleep(PASTE_READ_POLL_INTERVAL);
        }
    }

    fn pump_messages(&mut self) {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}

impl Drop for ClipboardPasteHost {
    fn drop(&mut self) {
        let _ = self.materialize_if_owned();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardReadOutcome {
    TargetVerified,
    TargetRead,
    DispatchedUnverified,
    Superseded,
    TargetChanged,
    TimedOut,
}

unsafe extern "system" fn clipboard_render_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if !create.is_null() {
            let state = unsafe { (*create).lpCreateParams } as isize;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state);
            }
        }
    }
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ClipboardRenderState;
    if message == WM_RENDERFORMAT && wparam as u32 == u32::from(CF_UNICODETEXT) && !state.is_null()
    {
        let requester = unsafe { GetOpenClipboardWindow() };
        let mut requester_process_id = 0;
        if !requester.is_null() {
            unsafe {
                GetWindowThreadProcessId(requester, &mut requester_process_id);
            }
        }
        let state = unsafe { &mut *state };
        let foreground = unsafe { GetForegroundWindow() };
        let foreground_root = if foreground.is_null() {
            foreground
        } else {
            let root = unsafe { GetAncestor(foreground, GA_ROOT) };
            if root.is_null() { foreground } else { root }
        };
        let foreground_is_target = foreground_root as usize as u64 == state.target_window_handle;
        let requester_root = if requester.is_null() {
            requester
        } else {
            let root = unsafe { GetAncestor(requester, GA_ROOT) };
            if root.is_null() { requester } else { root }
        };
        // OpenClipboard(NULL) deliberately provides no requester identity.
        // Foreground equality is not proof that the target read the payload:
        // clipboard brokers and WSL/RDP synchronizers can read while that same
        // window remains foreground. Such reads merely materialize the data;
        // the transaction lease below keeps it available to the real target.
        state.target_requested = request_is_from_target(
            requester_process_id,
            state.target_process_id,
            requester_root as usize as u64,
            state.target_window_handle,
        );
        crate::diagnostic_line(&format!(
            "clipboard_transaction action=render requester_pid={requester_process_id} target_pid={} requester_root_hwnd={} foreground_hwnd={} target_hwnd={} foreground_match={} target_match={}",
            state.target_process_id,
            requester_root as usize as u64,
            foreground_root as usize as u64,
            state.target_window_handle,
            foreground_is_target,
            state.target_requested,
        ));
        if render_payload(state).is_err() {
            state.render_failed = true;
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn render_payload(state: &mut ClipboardRenderState) -> Result<(), ClipboardError> {
    if state.rendered {
        return Ok(());
    }
    let mut payload = GlobalAllocation::from_utf16(&state.payload)?;
    let accepted = unsafe { SetClipboardData(u32::from(CF_UNICODETEXT), payload.handle) };
    if accepted.is_null() {
        return Err(ClipboardError::Allocation);
    }
    payload.release_to_clipboard();
    state.rendered = true;
    Ok(())
}

fn request_is_from_target(
    requester_process_id: u32,
    target_process_id: u32,
    requester_root_window: u64,
    target_window: u64,
) -> bool {
    (requester_process_id != 0 && requester_process_id == target_process_id)
        || (requester_root_window != 0 && requester_root_window == target_window)
}

impl ClipboardApartment {
    pub fn initialize() -> Result<Self, ClipboardError> {
        let result = unsafe { OleInitialize(ptr::null()) };
        if result < 0 {
            return Err(ClipboardError::Ole(result));
        }
        Ok(Self { initialized: true })
    }
}

impl Drop for ClipboardApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                OleUninitialize();
            }
        }
    }
}

pub fn set_text(text: &str) -> Result<(), ClipboardError> {
    set_text_with_policy(text, ClipboardPolicy::Normal)
}

fn set_private_text(text: &str) -> Result<(), ClipboardError> {
    set_text_with_policy(text, ClipboardPolicy::EphemeralPrivate)
}

#[derive(Clone, Copy)]
enum ClipboardPolicy {
    Normal,
    EphemeralPrivate,
}

fn set_text_with_policy(text: &str, policy: ClipboardPolicy) -> Result<(), ClipboardError> {
    let mut payloads = vec![(
        u32::from(CF_UNICODETEXT),
        GlobalAllocation::from_utf16(text)?,
    )];
    if matches!(policy, ClipboardPolicy::EphemeralPrivate) {
        payloads.extend(private_format_payloads()?);
    }

    with_open_clipboard(|| {
        if unsafe { EmptyClipboard() } == 0 {
            return Err(ClipboardError::Busy);
        }
        for (format, payload) in &mut payloads {
            let accepted = unsafe { SetClipboardData(*format, payload.handle) };
            if accepted.is_null() {
                // Remove any formats already transferred during this transaction.
                // EmptyClipboard owns and releases those handles now.
                unsafe {
                    let _ = EmptyClipboard();
                }
                return Err(ClipboardError::Allocation);
            }
            payload.release_to_clipboard();
        }
        Ok(())
    })
}

fn private_format_payloads() -> Result<Vec<(u32, GlobalAllocation)>, ClipboardError> {
    let definitions = [
        ("ExcludeClipboardContentFromMonitorProcessing", 1_u32),
        ("CanIncludeInClipboardHistory", 0_u32),
        ("CanUploadToCloudClipboard", 0_u32),
    ];
    definitions
        .into_iter()
        .map(|(name, value)| {
            let wide = name.encode_utf16().chain([0]).collect::<Vec<_>>();
            let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
            if format == 0 {
                return Err(ClipboardError::Allocation);
            }
            Ok((format, GlobalAllocation::from_u32(value)?))
        })
        .collect()
}

struct GlobalAllocation {
    handle: *mut c_void,
}

impl GlobalAllocation {
    fn from_utf16(text: &str) -> Result<Self, ClipboardError> {
        let utf16 = text.encode_utf16().chain([0]).collect::<Vec<_>>();
        Self::from_bytes(
            utf16.as_ptr().cast::<u8>(),
            utf16.len() * mem::size_of::<u16>(),
        )
    }

    fn from_u32(value: u32) -> Result<Self, ClipboardError> {
        Self::from_bytes((&raw const value).cast::<u8>(), mem::size_of::<u32>())
    }

    fn from_bytes(source: *const u8, bytes: usize) -> Result<Self, ClipboardError> {
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if handle.is_null() {
            return Err(ClipboardError::Allocation);
        }
        let allocation = Self { handle };
        let destination = unsafe { GlobalLock(allocation.handle) }.cast::<u8>();
        if destination.is_null() {
            return Err(ClipboardError::Allocation);
        }
        unsafe {
            ptr::copy_nonoverlapping(source, destination, bytes);
            let _ = GlobalUnlock(allocation.handle);
        }
        Ok(allocation)
    }

    fn release_to_clipboard(&mut self) {
        self.handle = ptr::null_mut();
    }
}

impl Drop for GlobalAllocation {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = GlobalFree(self.handle);
            }
        }
    }
}

pub fn get_text() -> Result<String, ClipboardError> {
    with_open_clipboard(|| {
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT.into()) };
        if handle.is_null() {
            return Err(ClipboardError::NoText);
        }
        let pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
        if pointer.is_null() {
            return Err(ClipboardError::InvalidData);
        }
        let units = unsafe { GlobalSize(handle) } / mem::size_of::<u16>();
        let slice = unsafe { std::slice::from_raw_parts(pointer, units) };
        let length = slice.iter().position(|unit| *unit == 0).unwrap_or(units);
        let text = String::from_utf16(&slice[..length]).map_err(|_| ClipboardError::InvalidData);
        unsafe {
            let _ = GlobalUnlock(handle);
        }
        text
    })
}

pub fn capture_selection(
    target_still_focused: impl Fn() -> bool,
    send_direct_copy: impl FnOnce() -> Result<(), ClipboardError>,
    send_shortcut_copy: impl FnOnce() -> Result<(), ClipboardError>,
) -> Result<ClipboardCapture, ClipboardError> {
    let _apartment = ClipboardApartment::initialize()?;
    let mut snapshot = ClipboardSnapshot::capture()?;
    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }

    let original_sequence = unsafe { GetClipboardSequenceNumber() };
    let _ = send_direct_copy();

    // WM_COPY is synchronous, so an unchanged sequence can fall through
    // immediately. A changed-but-unreadable clipboard is also not a terminal
    // failure: restore the user's clipboard and still try the application's
    // normal keyboard command.
    if unsafe { GetClipboardSequenceNumber() } != original_sequence {
        match finish_copy_attempt(
            &mut snapshot,
            &target_still_focused,
            original_sequence,
            ClipboardCaptureMethod::WindowMessage,
            Duration::ZERO,
            Duration::from_millis(250),
        ) {
            Ok(captured) => return Ok(captured),
            Err(error) if is_retryable_capture_error(&error) => {
                crate::diagnostic_line(&format!(
                    "clipboard_capture method=window_message result=retry error={error}"
                ));
            }
            Err(error) => return Err(error),
        }
    }

    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }
    let shortcut_sequence = unsafe { GetClipboardSequenceNumber() };
    send_shortcut_copy()?;
    finish_copy_attempt(
        &mut snapshot,
        &target_still_focused,
        shortcut_sequence,
        ClipboardCaptureMethod::Shortcut,
        Duration::from_millis(1_500),
        Duration::from_millis(800),
    )
}

fn finish_copy_attempt(
    snapshot: &mut ClipboardSnapshot,
    target_still_focused: &impl Fn() -> bool,
    original_sequence: u32,
    method: ClipboardCaptureMethod,
    change_timeout: Duration,
    read_timeout: Duration,
) -> Result<ClipboardCapture, ClipboardError> {
    let deadline = Instant::now() + change_timeout;
    while unsafe { GetClipboardSequenceNumber() } == original_sequence {
        if !target_still_focused() {
            return Err(ClipboardError::TargetChanged);
        }
        if Instant::now() >= deadline {
            return Err(ClipboardError::CopyTimeout);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let copied_sequence = unsafe { GetClipboardSequenceNumber() };
    let stable_for = if method == ClipboardCaptureMethod::Shortcut {
        Duration::from_millis(40)
    } else {
        Duration::from_millis(20)
    };
    let mut private_sequence = None;
    let text = claim_copied_text(
        target_still_focused,
        stable_for,
        read_timeout,
        &mut private_sequence,
    );
    match text {
        Ok(text) => {
            if let Some(sequence) = private_sequence {
                restore_if_current(snapshot, sequence)?;
            }
            Ok(ClipboardCapture { text, method })
        }
        Err(error) => {
            if let Some(sequence) = private_sequence {
                restore_if_current(snapshot, sequence)?;
            } else if target_still_focused()
                && unsafe { GetClipboardSequenceNumber() } == copied_sequence
            {
                snapshot.restore()?;
            }
            Err(error)
        }
    }
}

fn claim_copied_text(
    target_still_focused: &impl Fn() -> bool,
    stable_for: Duration,
    timeout: Duration,
    private_sequence: &mut Option<u32>,
) -> Result<String, ClipboardError> {
    let deadline = Instant::now() + timeout;
    loop {
        if !target_still_focused() {
            return Err(ClipboardError::TargetChanged);
        }

        // A source commonly increments the sequence at EmptyClipboard, before
        // delayed rendering has made CF_UNICODETEXT readable. Keep polling the
        // actual text instead of treating that transient gap as a failed Copy.
        let text = match get_text() {
            Ok(text) => text,
            Err(error) if is_transient_read_error(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };

        // A foreign Copy command cannot attach the Windows privacy formats.
        // Re-own the same payload when possible, but never discard an already
        // captured selection merely because a privacy format registration or
        // clipboard transaction failed. The original snapshot is restored on
        // both paths.
        if let Err(error) = set_private_text(&text) {
            let current = unsafe { GetClipboardSequenceNumber() };
            *private_sequence = Some(current);
            crate::diagnostic_line(&format!(
                "clipboard_capture privacy_marking=failed error={error}"
            ));
            return Ok(text);
        }
        let owned = unsafe { GetClipboardSequenceNumber() };
        *private_sequence = Some(owned);
        if stable_for.is_zero() {
            return Ok(text);
        }

        let stable_since = Instant::now();
        loop {
            if !target_still_focused() {
                return Err(ClipboardError::TargetChanged);
            }
            if stable_since.elapsed() >= stable_for || Instant::now() >= deadline {
                return Ok(text);
            }
            thread::sleep(Duration::from_millis(3));
            if unsafe { GetClipboardSequenceNumber() } != owned {
                *private_sequence = None;
                break;
            }
        }
    }
}

fn is_transient_read_error(error: &ClipboardError) -> bool {
    matches!(
        error,
        ClipboardError::Busy | ClipboardError::NoText | ClipboardError::InvalidData
    )
}

fn is_retryable_capture_error(error: &ClipboardError) -> bool {
    matches!(
        error,
        ClipboardError::Busy
            | ClipboardError::NoText
            | ClipboardError::CopyTimeout
            | ClipboardError::InvalidData
    )
}

fn restore_if_current(
    snapshot: &mut ClipboardSnapshot,
    expected_sequence: u32,
) -> Result<(), ClipboardError> {
    if unsafe { GetClipboardSequenceNumber() } == expected_sequence {
        snapshot.restore()?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPasteDisposition {
    /// The focused target's observable text changed by the exact payload and
    /// the previous IDataObject was restored.
    VerifiedAndRestored,
    /// The target was verified, then a newer clipboard owner took over.
    VerifiedAndSuperseded,
    /// The target process requested the exact delayed-rendered payload and the
    /// previous IDataObject was restored without overwriting a newer Copy.
    PayloadReadAndRestored,
    /// The target requested the payload, then another clipboard owner took
    /// over. Its newer contents are preserved and must never be overwritten.
    PayloadReadAndSuperseded,
    /// The paste shortcut was dispatched and the delayed payload remained
    /// available for the compatibility lease, but the target exposes no safe
    /// acknowledgement channel. The previous IDataObject was restored.
    DispatchedUnverifiedAndRestored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardPasteReceipt {
    pub original_sequence: u32,
    pub replacement_sequence: u32,
    pub settled_sequence: u32,
    pub disposition: ClipboardPasteDisposition,
}

/// Executes one serialized compatibility transaction for targets that expose
/// no acknowledged native adapter. The previous OLE IDataObject is captured
/// before the private delayed-rendered payload is published. Restoration is
/// attempted only while this host is still the clipboard owner; a concurrent
/// user Copy always wins.
pub fn dispatch_paste(
    host: &mut ClipboardPasteHost,
    target_still_focused: impl Fn() -> bool,
    verification_available: bool,
    target_contains_payload: impl FnMut() -> bool,
    send_paste: impl FnOnce() -> Result<(), ClipboardError>,
    target_process_id: u32,
    payload: &str,
) -> Result<ClipboardPasteReceipt, ClipboardError> {
    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }
    let (mut snapshot, original_sequence) = ClipboardSnapshot::capture_consistent()?;
    let target_window_handle = crate::inject::foreground_window_handle();
    let replacement_sequence = match host.publish(payload, target_process_id, target_window_handle)
    {
        Ok(sequence) => sequence,
        Err(error) => {
            if host.owns_clipboard() {
                snapshot.restore()?;
            }
            return Err(error);
        }
    };

    if !target_still_focused() {
        if host.owns_clipboard() {
            snapshot.restore()?;
        }
        return Err(ClipboardError::TargetChanged);
    }
    if let Err(error) = send_paste() {
        if host.owns_clipboard() {
            snapshot.restore()?;
        }
        return Err(error);
    }

    let outcome = match host.await_target_read(
        target_still_focused,
        verification_available,
        target_contains_payload,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            if host.owns_clipboard() {
                snapshot.restore()?;
            }
            return Err(error);
        }
    };
    let disposition = match outcome {
        ClipboardReadOutcome::TargetVerified if host.owns_clipboard() => {
            snapshot.restore()?;
            ClipboardPasteDisposition::VerifiedAndRestored
        }
        ClipboardReadOutcome::TargetVerified => ClipboardPasteDisposition::VerifiedAndSuperseded,
        ClipboardReadOutcome::TargetRead if host.owns_clipboard() => {
            snapshot.restore()?;
            ClipboardPasteDisposition::PayloadReadAndRestored
        }
        ClipboardReadOutcome::TargetRead => {
            // A newer clipboard owner is user/external state. Never overwrite
            // it merely to reconstruct our earlier snapshot.
            ClipboardPasteDisposition::PayloadReadAndSuperseded
        }
        ClipboardReadOutcome::DispatchedUnverified if host.owns_clipboard() => {
            snapshot.restore()?;
            ClipboardPasteDisposition::DispatchedUnverifiedAndRestored
        }
        ClipboardReadOutcome::DispatchedUnverified => return Err(ClipboardError::Superseded),
        ClipboardReadOutcome::TargetChanged => {
            if host.owns_clipboard() {
                snapshot.restore()?;
            }
            return Err(ClipboardError::TargetChanged);
        }
        ClipboardReadOutcome::TimedOut => {
            if host.owns_clipboard() {
                snapshot.restore()?;
                return Err(ClipboardError::NotDelivered);
            }
            return Err(ClipboardError::Superseded);
        }
        ClipboardReadOutcome::Superseded => return Err(ClipboardError::Superseded),
    };

    Ok(ClipboardPasteReceipt {
        original_sequence,
        replacement_sequence,
        settled_sequence: unsafe { GetClipboardSequenceNumber() },
        disposition,
    })
}

struct ClipboardSnapshot {
    data_object: *mut c_void,
    fallback_text: Option<String>,
}

impl ClipboardSnapshot {
    fn capture_consistent() -> Result<(Self, u32), ClipboardError> {
        for _ in 0..4 {
            let before = unsafe { GetClipboardSequenceNumber() };
            let snapshot = Self::capture()?;
            let after = unsafe { GetClipboardSequenceNumber() };
            if before == after {
                return Ok((snapshot, before));
            }
        }
        Err(ClipboardError::Busy)
    }

    fn capture() -> Result<Self, ClipboardError> {
        let fallback_text = get_text().ok();
        let mut last_error = 0;
        for attempt in 0..12 {
            let mut data_object = ptr::null_mut();
            // SAFETY: OLE is initialized on the current worker thread and the
            // out-pointer is valid for the duration of the call.
            let result = unsafe { OleGetClipboard(&mut data_object) };
            if result >= 0 {
                return Ok(Self {
                    data_object,
                    fallback_text,
                });
            }
            last_error = result;
            if attempt < 11 {
                thread::sleep(Duration::from_millis(15));
            }
        }
        Err(ClipboardError::Ole(last_error))
    }

    fn restore(&mut self) -> Result<(), ClipboardError> {
        let mut last_error = 0;
        for attempt in 0..12 {
            // SAFETY: data_object is the AddRef'd IDataObject returned by
            // OleGetClipboard while this apartment remains initialized.
            let result = unsafe { OleSetClipboard(self.data_object) };
            if result >= 0 {
                if let Some(fallback) = &self.fallback_text {
                    let text_was_restored = get_text().is_ok_and(|restored| restored == *fallback);
                    if !text_was_restored {
                        // Some transient clipboard owners expose an IDataObject
                        // whose delayed text becomes unavailable after the owner
                        // exits. Preserve at least the original text instead of
                        // silently leaving the clipboard empty.
                        set_text(fallback)?;
                    }
                }
                return Ok(());
            }
            last_error = result;
            if attempt < 11 {
                thread::sleep(Duration::from_millis(15));
            }
        }
        Err(ClipboardError::Ole(last_error))
    }
}

impl Drop for ClipboardSnapshot {
    fn drop(&mut self) {
        if !self.data_object.is_null() {
            unsafe {
                // SAFETY: OleGetClipboard returned an AddRef'd IDataObject.
                // OleSetClipboard retains its own reference when restoration
                // succeeds, so this releases exactly our snapshot reference.
                let vtable = *(self.data_object.cast::<*const IUnknown_Vtbl>());
                ((*vtable).Release)(self.data_object);
            }
            self.data_object = ptr::null_mut();
        }
    }
}

fn with_open_clipboard<T>(
    operation: impl FnOnce() -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    with_open_clipboard_owner(ptr::null_mut(), operation)
}

fn with_open_clipboard_owner<T>(
    owner: HWND,
    operation: impl FnOnce() -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    for _ in 0..12 {
        if unsafe { OpenClipboard(owner) } != 0 {
            let result = operation();
            unsafe {
                let _ = CloseClipboard();
            }
            return result;
        }
        thread::sleep(Duration::from_millis(15));
    }
    Err(ClipboardError::Busy)
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardError, is_retryable_capture_error, is_transient_read_error, request_is_from_target,
    };

    #[test]
    fn delayed_rendering_errors_are_retryable() {
        assert!(is_transient_read_error(&ClipboardError::Busy));
        assert!(is_transient_read_error(&ClipboardError::NoText));
        assert!(is_transient_read_error(&ClipboardError::InvalidData));
        assert!(!is_transient_read_error(&ClipboardError::TargetChanged));
        assert!(!is_transient_read_error(&ClipboardError::CopyTimeout));
    }

    #[test]
    fn failed_direct_copy_can_escalate_to_the_application_shortcut() {
        assert!(is_retryable_capture_error(&ClipboardError::Busy));
        assert!(is_retryable_capture_error(&ClipboardError::NoText));
        assert!(is_retryable_capture_error(&ClipboardError::CopyTimeout));
        assert!(is_retryable_capture_error(&ClipboardError::InvalidData));
        assert!(!is_retryable_capture_error(&ClipboardError::TargetChanged));
    }

    #[test]
    fn anonymous_clipboard_read_is_not_delivery_acknowledgement() {
        assert!(!request_is_from_target(0, 42, 0, 100));
        assert!(request_is_from_target(42, 42, 0, 100));
        assert!(request_is_from_target(7, 42, 100, 100));
        assert!(!request_is_from_target(7, 42, 200, 100));
    }
}
