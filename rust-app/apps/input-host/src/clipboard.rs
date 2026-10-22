use std::{
    ffi::c_void,
    mem, ptr,
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        Com::{STGMEDIUM, STGMEDIUM_0, TYMED_ENHMF, TYMED_GDI, TYMED_HGLOBAL, TYMED_MFPICT},
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardOwner, GetClipboardSequenceNumber, GetOpenClipboardWindow,
            IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{
            CF_BITMAP, CF_ENHMETAFILE, CF_METAFILEPICT, CF_PALETTE, CF_UNICODETEXT,
            OleDuplicateData, OleInitialize, OleUninitialize, ReleaseStgMedium,
        },
    },
    UI::WindowsAndMessaging::{
        CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GA_ROOT,
        GWLP_USERDATA, GetAncestor, GetForegroundWindow, GetWindowLongPtrW,
        GetWindowThreadProcessId, HWND_MESSAGE, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
        SetWindowLongPtrW, TranslateMessage, WM_NCCREATE, WM_RENDERFORMAT, WNDCLASSW,
    },
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
    #[error("insertion into a password or protected field is blocked")]
    Protected,
    #[error("paste shortcut injection failed")]
    Paste,
    #[error("the target did not request the insertion payload")]
    NotDelivered,
    #[error("paste was dispatched but delivery is unconfirmed; do not retry automatically")]
    DeliveryUnconfirmed,
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

/// A capture owns only fully materialized data. Its private message-only
/// window distinguishes our CloseClipboard format synthesis from a new Copy.
struct ClipboardCaptureOwner {
    hwnd: HWND,
}

impl ClipboardCaptureOwner {
    fn create() -> Result<Self, ClipboardError> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(ClipboardError::Allocation);
        }
        Ok(Self { hwnd })
    }

    fn settled_sequence(&self) -> Result<u32, ClipboardError> {
        // Closing a write can synthesize additional text formats. Reopen to
        // observe the settled token, but only while this exact window still
        // owns the clipboard; a foreign or anonymous publisher is never ours.
        with_open_clipboard_owner(self.hwnd, || {
            settled_capture_sequence(
                self.hwnd as usize as u64,
                unsafe { GetClipboardOwner() } as usize as u64,
                unsafe { GetClipboardSequenceNumber() },
            )
        })
    }
}

impl Drop for ClipboardCaptureOwner {
    fn drop(&mut self) {
        // Every format was provided with a real data handle. Destroying this
        // owner does not require rendering or replacing the restored clipboard.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

const PASTE_READ_ACK_TIMEOUT: Duration = Duration::from_millis(2_500);
static CLIPBOARD_TRANSACTION: Mutex<()> = Mutex::new(());

pub(crate) fn begin_clipboard_transaction() -> Result<MutexGuard<'static, ()>, ClipboardError> {
    // Never wait on the insertion STA: it must keep servicing COM/window
    // messages, including delayed clipboard rendering for other processes.
    CLIPBOARD_TRANSACTION
        .try_lock()
        .map_err(|_| ClipboardError::Busy)
}
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
    generation: u64,
    rendered: bool,
    target_requested: bool,
    render_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClipboardPublication {
    generation: u64,
    replacement_sequence: u32,
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

    fn publish_if_unchanged(
        &mut self,
        expected_sequence: u32,
        payload: &str,
        target_process_id: u32,
        target_window_handle: u64,
    ) -> Result<ClipboardPublication, ClipboardError> {
        let mut private_payloads = private_format_payloads()?;
        let generation = self.state.generation.wrapping_add(1).max(1);
        let publication = with_open_clipboard_owner(self.hwnd, || {
            let current_sequence = unsafe { GetClipboardSequenceNumber() };
            publish_if_sequence_current(expected_sequence, current_sequence, || {
                self.state.payload.clear();
                self.state.payload.push_str(payload);
                self.state.target_process_id = target_process_id;
                self.state.target_window_handle = target_window_handle;
                self.state.generation = generation;
                self.state.rendered = false;
                self.state.target_requested = false;
                self.state.render_failed = false;

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
                Ok(ClipboardPublication {
                    generation,
                    replacement_sequence: unsafe { GetClipboardSequenceNumber() },
                })
            })
        })?
        .ok_or(ClipboardError::Superseded)?;
        if unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) } == 0 {
            return Err(ClipboardError::Allocation);
        }
        Ok(publication)
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
            if let Some(outcome) =
                acknowledged_read_outcome(&self.state, &mut target_contains_payload)
            {
                if outcome == ClipboardReadOutcome::TargetRead {
                    self.wait_until_clipboard_released(deadline);
                }
                return Ok(outcome);
            }
            if !self.owns_clipboard() {
                return Ok(if self.state.rendered {
                    ClipboardReadOutcome::DispatchedUnverified
                } else {
                    ClipboardReadOutcome::Superseded
                });
            }
            if !target_still_focused() {
                return Ok(if self.state.rendered {
                    ClipboardReadOutcome::DispatchedUnverified
                } else {
                    ClipboardReadOutcome::TargetChanged
                });
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

    fn owns_publication(&self, publication: ClipboardPublication) -> bool {
        publication_still_owned(
            publication,
            self.state.generation,
            unsafe { GetClipboardSequenceNumber() },
            unsafe { GetClipboardOwner() } as usize as u64,
            self.hwnd as usize as u64,
        )
    }

    fn restore_snapshot_if_owned(
        &self,
        snapshot: &ClipboardSnapshot,
        publication: ClipboardPublication,
    ) -> Result<bool, ClipboardError> {
        let mut formats = snapshot.prepare_restore()?;
        let active_generation = self.state.generation;
        let host_window = self.hwnd as usize as u64;
        with_open_clipboard(|| {
            restore_preserved_formats_if_publication_owned(
                &mut formats,
                publication,
                active_generation,
                unsafe { GetClipboardSequenceNumber() },
                unsafe { GetClipboardOwner() } as usize as u64,
                host_window,
                || {
                    if unsafe { EmptyClipboard() } == 0 {
                        Err(ClipboardError::Busy)
                    } else {
                        Ok(())
                    }
                },
                |format| {
                    let accepted = unsafe { SetClipboardData(format.format, format.handle) };
                    if accepted.is_null() {
                        return Err(ClipboardError::Allocation);
                    }
                    format.release_to_clipboard();
                    Ok(())
                },
            )
        })
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

fn acknowledged_read_outcome(
    state: &ClipboardRenderState,
    mut target_contains_payload: impl FnMut() -> bool,
) -> Option<ClipboardReadOutcome> {
    if state.rendered && state.target_requested {
        // The identified target already requested this exact payload. An
        // optional UIA provider must not delay that independent proof.
        Some(ClipboardReadOutcome::TargetRead)
    } else if target_contains_payload() {
        Some(ClipboardReadOutcome::TargetVerified)
    } else {
        None
    }
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
    let _transaction = begin_clipboard_transaction()?;
    set_text_unlocked(text)
}

fn set_private_text_if_current(
    owner: &ClipboardCaptureOwner,
    text: &str,
    expected_sequence: u32,
    private_sequence: &mut Option<u32>,
) -> Result<u32, ClipboardError> {
    // Allocate before opening or modifying the clipboard. Compare and write
    // must share one OpenClipboard transaction, including partial failures.
    let mut payloads = vec![(
        u32::from(CF_UNICODETEXT),
        GlobalAllocation::from_utf16(text)?,
    )];
    payloads.extend(private_format_payloads()?);
    let mut changed = false;
    let publication = with_open_clipboard_owner(owner.hwnd, || {
        publish_if_sequence_current(
            expected_sequence,
            unsafe { GetClipboardSequenceNumber() },
            || {
                if unsafe { EmptyClipboard() } == 0 {
                    return Err(ClipboardError::Busy);
                }
                changed = true;
                let result = payloads.iter_mut().try_for_each(|(format, payload)| {
                    if unsafe { SetClipboardData(*format, payload.handle) }.is_null() {
                        return Err(ClipboardError::Allocation);
                    }
                    payload.release_to_clipboard();
                    Ok(())
                });
                // This remains our mutation on partial failure. Capture its
                // sequence while foreign writers are still excluded.
                let owned = unsafe { GetClipboardSequenceNumber() };
                *private_sequence = Some(owned);
                result.map(|()| owned)
            },
        )
    });
    let settled = if changed {
        match owner.settled_sequence() {
            Ok(sequence) => {
                *private_sequence = Some(sequence);
                Some(sequence)
            }
            Err(error) => {
                // Without a proven post-close owner, do not adopt a new token
                // or restore our snapshot over possibly newer clipboard data.
                *private_sequence = None;
                crate::diagnostic_line(&format!(
                    "clipboard_capture settlement=failed error={error}"
                ));
                return Err(ClipboardError::Superseded);
            }
        }
    } else {
        None
    };
    publication?.ok_or(ClipboardError::Superseded)?;
    settled.ok_or(ClipboardError::Superseded)
}

fn set_text_unlocked(text: &str) -> Result<(), ClipboardError> {
    let mut payloads = vec![(
        u32::from(CF_UNICODETEXT),
        GlobalAllocation::from_utf16(text)?,
    )];

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

fn get_text_while_open() -> Result<String, ClipboardError> {
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
}

fn get_target_copied_text(
    original_sequence: u32,
    target: TargetSnapshot,
    owned_sequence: &mut Option<u32>,
) -> Result<(String, u32), ClipboardError> {
    with_open_clipboard(|| {
        let owner = unsafe { GetClipboardOwner() };
        let mut owner_pid = 0;
        let owner_root = if owner.is_null() {
            owner
        } else {
            unsafe {
                GetWindowThreadProcessId(owner, &mut owner_pid);
            }
            unsafe { GetAncestor(owner, GA_ROOT) }
        };
        acquire_target_copy(
            original_sequence,
            unsafe { GetClipboardSequenceNumber() },
            request_is_from_target(
                owner_pid,
                target.process_id,
                owner_root as usize as u64,
                target.window_handle,
            ),
            || {
                let text = get_text_while_open();
                // Delayed rendering can advance this sequence while the
                // clipboard is open, but no foreign Copy can interleave.
                let sequence = unsafe { GetClipboardSequenceNumber() };
                *owned_sequence = Some(sequence);
                text.map(|text| (text, sequence))
            },
        )
    })
}

fn acquire_target_copy<T>(
    original_sequence: u32,
    current_sequence: u32,
    owner_is_target: bool,
    read: impl FnOnce() -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    // The first observed change may be EmptyClipboard while the source still
    // holds the lock. Anchor to its completed publication here, but only when
    // the actual owner identifies the target. Unknown/foreign Copy is never
    // adopted, inspected or later restored over.
    if current_sequence == original_sequence || !owner_is_target {
        return Err(ClipboardError::Superseded);
    }
    read()
}

pub fn capture_selection(
    target: TargetSnapshot,
    send_direct_copy: impl FnOnce() -> Result<(), ClipboardError>,
    send_shortcut_copy: impl FnOnce() -> Result<(), ClipboardError>,
) -> Result<ClipboardCapture, ClipboardError> {
    let _transaction = begin_clipboard_transaction()?;
    let target_still_focused = || crate::inject::is_foreground(target);
    let _apartment = ClipboardApartment::initialize()?;
    let owner = ClipboardCaptureOwner::create()?;
    let (snapshot, original_sequence) = ClipboardSnapshot::capture_consistent()?;
    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }

    if unsafe { GetClipboardSequenceNumber() } != original_sequence {
        return Err(ClipboardError::Superseded);
    }
    let mut retry_sequence = original_sequence;
    let _ = send_direct_copy();

    // WM_COPY is synchronous, so an unchanged sequence can fall through
    // immediately. A changed-but-unreadable clipboard is also not a terminal
    // failure: restore the user's clipboard and still try the application's
    // normal keyboard command.
    if unsafe { GetClipboardSequenceNumber() } != original_sequence {
        match finish_copy_attempt(
            &snapshot,
            &owner,
            target,
            original_sequence,
            ClipboardCaptureMethod::WindowMessage,
            Duration::ZERO,
            Duration::from_millis(250),
            &mut retry_sequence,
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
    if shortcut_sequence != retry_sequence {
        return Err(ClipboardError::Superseded);
    }
    send_shortcut_copy()?;
    finish_copy_attempt(
        &snapshot,
        &owner,
        target,
        shortcut_sequence,
        ClipboardCaptureMethod::Shortcut,
        Duration::from_millis(1_500),
        Duration::from_millis(800),
        &mut retry_sequence,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_copy_attempt(
    snapshot: &ClipboardSnapshot,
    owner: &ClipboardCaptureOwner,
    target: TargetSnapshot,
    original_sequence: u32,
    method: ClipboardCaptureMethod,
    change_timeout: Duration,
    read_timeout: Duration,
    retry_sequence: &mut u32,
) -> Result<ClipboardCapture, ClipboardError> {
    let deadline = Instant::now() + change_timeout;
    let target_still_focused = || crate::inject::is_foreground(target);
    loop {
        let current = unsafe { GetClipboardSequenceNumber() };
        if current != original_sequence {
            break;
        }
        if !target_still_focused() {
            return Err(ClipboardError::TargetChanged);
        }
        if Instant::now() >= deadline {
            return Err(ClipboardError::CopyTimeout);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let stable_for = if method == ClipboardCaptureMethod::Shortcut {
        Duration::from_millis(40)
    } else {
        Duration::from_millis(20)
    };
    let mut private_sequence = None;
    let text = claim_copied_text(
        owner,
        target,
        original_sequence,
        stable_for,
        read_timeout,
        &mut private_sequence,
    );
    match text {
        Ok(text) => {
            if let Some(sequence) = private_sequence {
                *retry_sequence = snapshot
                    .restore_sequence_if_current(owner, sequence)?
                    .ok_or(ClipboardError::Superseded)?;
            }
            Ok(ClipboardCapture { text, method })
        }
        Err(error) => {
            if let Some(sequence) = private_sequence {
                *retry_sequence = snapshot
                    .restore_sequence_if_current(owner, sequence)?
                    .ok_or(ClipboardError::Superseded)?;
            }
            Err(error)
        }
    }
}

fn claim_copied_text(
    owner: &ClipboardCaptureOwner,
    target: TargetSnapshot,
    original_sequence: u32,
    stable_for: Duration,
    timeout: Duration,
    private_sequence: &mut Option<u32>,
) -> Result<String, ClipboardError> {
    claim_copied_text_using(
        &|| crate::inject::is_foreground(target),
        stable_for,
        timeout,
        private_sequence,
        |owned| get_target_copied_text(original_sequence, target, owned),
        |text, sequence, private| set_private_text_if_current(owner, text, sequence, private),
        || unsafe { GetClipboardSequenceNumber() },
    )
}

fn claim_copied_text_using(
    target_still_focused: &impl Fn() -> bool,
    stable_for: Duration,
    timeout: Duration,
    private_sequence: &mut Option<u32>,
    mut read: impl FnMut(&mut Option<u32>) -> Result<(String, u32), ClipboardError>,
    mut publish: impl FnMut(&str, u32, &mut Option<u32>) -> Result<u32, ClipboardError>,
    current_sequence: impl Fn() -> u32,
) -> Result<String, ClipboardError> {
    let deadline = Instant::now() + timeout;
    loop {
        if !target_still_focused() {
            return Err(ClipboardError::TargetChanged);
        }

        // A source commonly increments the sequence at EmptyClipboard, before
        // delayed rendering has made CF_UNICODETEXT readable. Keep polling the
        // actual text instead of treating that transient gap as a failed Copy.
        let (text, copied_sequence) = match read(private_sequence) {
            Ok(text) => text,
            Err(error) if is_transient_read_error(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => {
                if error == ClipboardError::Superseded {
                    *private_sequence = None;
                }
                return Err(error);
            }
        };

        // A foreign Copy command cannot attach the Windows privacy formats.
        // Re-own the same payload when possible, but never discard an already
        // captured selection merely because a privacy format registration or
        // clipboard transaction failed. The original snapshot is restored on
        // both paths.
        *private_sequence = Some(copied_sequence);
        let published = publish(&text, copied_sequence, private_sequence);
        if let Err(error) = published {
            if error == ClipboardError::Superseded {
                *private_sequence = None;
                return Err(error);
            }
            crate::diagnostic_line(&format!(
                "clipboard_capture privacy_marking=failed error={error}"
            ));
            return Ok(text);
        }
        let owned = published.expect("publication succeeded");
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
            if current_sequence() != owned {
                *private_sequence = None;
                return Err(ClipboardError::Superseded);
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

fn clipboard_sequence_still_owned(expected_sequence: u32, current_sequence: u32) -> bool {
    expected_sequence == current_sequence
}

fn settled_capture_sequence(
    owned_window: u64,
    observed_owner: u64,
    settled_sequence: u32,
) -> Result<u32, ClipboardError> {
    if owned_window != 0 && observed_owner == owned_window {
        Ok(settled_sequence)
    } else {
        Err(ClipboardError::Superseded)
    }
}

fn publish_precondition_holds(expected_sequence: u32, current_sequence: u32) -> bool {
    expected_sequence == current_sequence
}

fn publish_if_sequence_current<T>(
    expected_sequence: u32,
    current_sequence: u32,
    publish: impl FnOnce() -> Result<T, ClipboardError>,
) -> Result<Option<T>, ClipboardError> {
    if !publish_precondition_holds(expected_sequence, current_sequence) {
        return Ok(None);
    }
    publish().map(Some)
}

fn publication_still_owned(
    publication: ClipboardPublication,
    active_generation: u64,
    _current_sequence: u32,
    current_owner_window: u64,
    host_window: u64,
) -> bool {
    publication.generation == active_generation
        && current_owner_window != 0
        && current_owner_window == host_window
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPasteDisposition {
    /// The focused target's observable text changed by the exact payload and
    /// the previous IDataObject was restored.
    VerifiedAndRestored,
    /// The target was verified, then a newer clipboard owner took over.
    VerifiedAndSuperseded,
    /// The target was verified; restoring the previous clipboard then failed.
    /// The text is delivered.
    VerifiedRestoreFailed,
    /// The target process requested the exact delayed-rendered payload and the
    /// previous IDataObject was restored without overwriting a newer Copy.
    PayloadReadAndRestored,
    /// The target read the payload; restoring the previous clipboard then
    /// failed. The text is delivered.
    PayloadReadRestoreFailed,
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
#[allow(clippy::too_many_arguments)]
pub fn dispatch_paste(
    host: &mut ClipboardPasteHost,
    target_still_focused: impl Fn() -> bool,
    verification_available: bool,
    target_contains_payload: impl FnMut() -> bool,
    validate_before_dispatch: impl FnOnce() -> Result<(), ClipboardError>,
    send_paste: impl FnOnce() -> Result<(), ClipboardError>,
    target_process_id: u32,
    payload: &str,
) -> Result<ClipboardPasteReceipt, ClipboardError> {
    // The insertion adapter owns the transaction guard across route selection
    // and dispatch, so a capture cannot race a native insertion either.
    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }
    let (snapshot, original_sequence) = ClipboardSnapshot::capture_consistent()?;
    let target_window_handle = crate::inject::foreground_window_handle();
    let publication = match host.publish_if_unchanged(
        original_sequence,
        payload,
        target_process_id,
        target_window_handle,
    ) {
        Ok(publication) => publication,
        Err(error) => {
            // Even failure cleanup must check ownership inside OpenClipboard.
            // Sampling a sequence after an outside owner check could adopt a
            // newer user Copy as if it were our failed publication.
            let failed_publication = ClipboardPublication {
                generation: host.state.generation,
                replacement_sequence: original_sequence,
            };
            let _ = host.restore_snapshot_if_owned(&snapshot, failed_publication)?;
            return Err(error);
        }
    };
    let replacement_sequence = publication.replacement_sequence;

    if !target_still_focused() {
        let _ = host.restore_snapshot_if_owned(&snapshot, publication)?;
        return Err(ClipboardError::TargetChanged);
    }
    let dispatch = validate_before_dispatch().and_then(|()| {
        // A provider check may take time: recheck our publication afterwards,
        // immediately before injecting, so a new Copy never becomes payload.
        if !host.owns_publication(publication) {
            return Err(ClipboardError::Superseded);
        }
        send_paste()
    });
    if let Err(error) = dispatch {
        if let Err(restore_error) = host.restore_snapshot_if_owned(&snapshot, publication) {
            crate::diagnostic_line(&format!(
                "clipboard_transaction restore=failed after=dispatch_error error={restore_error}"
            ));
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
            let _ = host.restore_snapshot_if_owned(&snapshot, publication)?;
            return Err(error);
        }
    };
    let disposition = match outcome {
        ClipboardReadOutcome::Superseded => return Err(ClipboardError::Superseded),
        outcome => settled_disposition(
            outcome,
            host.restore_snapshot_if_owned(&snapshot, publication),
        )?,
    };

    Ok(ClipboardPasteReceipt {
        original_sequence,
        replacement_sequence,
        settled_sequence: unsafe { GetClipboardSequenceNumber() },
        disposition,
    })
}

/// What a finished paste means once the attempt to restore the user's
/// previous clipboard is known.
///
/// Once the target has read the payload the words are delivered. Failing to
/// put the previous clipboard back afterwards is a separate problem: it is
/// logged and reported as such, never as "not delivered" — that told the
/// user to paste again and produced the text twice.
fn settled_disposition(
    outcome: ClipboardReadOutcome,
    restored: Result<bool, ClipboardError>,
) -> Result<ClipboardPasteDisposition, ClipboardError> {
    match outcome {
        ClipboardReadOutcome::TargetVerified => Ok(match restored {
            Ok(true) => ClipboardPasteDisposition::VerifiedAndRestored,
            Ok(false) => ClipboardPasteDisposition::VerifiedAndSuperseded,
            Err(error) => {
                crate::diagnostic_line(&format!(
                    "clipboard_transaction restore=failed after=verified error={error}"
                ));
                ClipboardPasteDisposition::VerifiedRestoreFailed
            }
        }),
        ClipboardReadOutcome::TargetRead => Ok(match restored {
            Ok(true) => ClipboardPasteDisposition::PayloadReadAndRestored,
            // A newer clipboard owner is user/external state. Never overwrite
            // it merely to reconstruct our earlier snapshot.
            Ok(false) => ClipboardPasteDisposition::PayloadReadAndSuperseded,
            Err(error) => {
                crate::diagnostic_line(&format!(
                    "clipboard_transaction restore=failed after=payload_read error={error}"
                ));
                ClipboardPasteDisposition::PayloadReadRestoreFailed
            }
        }),
        ClipboardReadOutcome::DispatchedUnverified => match restored {
            Ok(true) => Ok(ClipboardPasteDisposition::DispatchedUnverifiedAndRestored),
            Ok(false) => Err(ClipboardError::DeliveryUnconfirmed),
            Err(error) => {
                crate::diagnostic_line(&format!(
                    "clipboard_transaction restore=failed after=dispatched_unverified error={error}"
                ));
                Err(ClipboardError::DeliveryUnconfirmed)
            }
        },
        ClipboardReadOutcome::TargetChanged => {
            restored?;
            Err(ClipboardError::TargetChanged)
        }
        ClipboardReadOutcome::TimedOut => {
            if restored? {
                Err(ClipboardError::NotDelivered)
            } else {
                Err(ClipboardError::Superseded)
            }
        }
        ClipboardReadOutcome::Superseded => Err(ClipboardError::Superseded),
    }
}

struct ClipboardFormatSnapshot {
    format: u32,
    handle: HANDLE,
}

impl ClipboardFormatSnapshot {
    fn release_to_clipboard(&mut self) {
        self.handle = ptr::null_mut();
    }
}

impl Drop for ClipboardFormatSnapshot {
    fn drop(&mut self) {
        if self.handle.is_null() {
            return;
        }
        let tymed = match self.format as u16 {
            CF_BITMAP | CF_PALETTE => TYMED_GDI,
            CF_METAFILEPICT => TYMED_MFPICT,
            CF_ENHMETAFILE => TYMED_ENHMF,
            _ => TYMED_HGLOBAL,
        } as u32;
        let mut medium = STGMEDIUM {
            tymed,
            u: STGMEDIUM_0 {
                hGlobal: self.handle,
            },
            pUnkForRelease: ptr::null_mut(),
        };
        unsafe {
            ReleaseStgMedium(&raw mut medium);
        }
        self.handle = ptr::null_mut();
    }
}

struct ClipboardSnapshot {
    formats: Vec<ClipboardFormatSnapshot>,
}

fn collect_preservable_formats<T>(
    advertised_formats: impl IntoIterator<Item = u32>,
    mut preserve: impl FnMut(u32) -> Result<Option<T>, ClipboardError>,
) -> Result<Vec<T>, ClipboardError> {
    let mut advertised_count = 0usize;
    let mut preserved = Vec::new();
    for format in advertised_formats {
        advertised_count += 1;
        if let Some(value) = preserve(format)? {
            preserved.push(value);
        }
    }

    if advertised_count > 0 && preserved.is_empty() {
        return Err(ClipboardError::InvalidData);
    }
    Ok(preserved)
}

fn restore_preserved_formats_if_current<T>(
    formats: &mut [T],
    expected_sequence: u32,
    current_sequence: u32,
    mut empty_clipboard: impl FnMut() -> Result<(), ClipboardError>,
    mut restore_format: impl FnMut(&mut T) -> Result<(), ClipboardError>,
) -> Result<bool, ClipboardError> {
    if !clipboard_sequence_still_owned(expected_sequence, current_sequence) {
        return Ok(false);
    }
    empty_clipboard()?;
    for format in formats {
        restore_format(format)?;
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn restore_preserved_formats_if_publication_owned<T>(
    formats: &mut [T],
    publication: ClipboardPublication,
    active_generation: u64,
    current_sequence: u32,
    current_owner_window: u64,
    host_window: u64,
    mut empty_clipboard: impl FnMut() -> Result<(), ClipboardError>,
    mut restore_format: impl FnMut(&mut T) -> Result<(), ClipboardError>,
) -> Result<bool, ClipboardError> {
    if !publication_still_owned(
        publication,
        active_generation,
        current_sequence,
        current_owner_window,
        host_window,
    ) {
        return Ok(false);
    }
    empty_clipboard()?;
    for format in formats {
        restore_format(format)?;
    }
    Ok(true)
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
        with_open_clipboard(|| {
            let mut advertised_formats = Vec::new();
            let mut previous = 0;
            loop {
                let format = unsafe { EnumClipboardFormats(previous) };
                if format == 0 {
                    break;
                }
                advertised_formats.push(format);
                previous = format;
            }

            let formats = collect_preservable_formats(advertised_formats, |format| {
                let clipboard_handle = unsafe { GetClipboardData(format) };
                if clipboard_handle.is_null() {
                    crate::diagnostic_line(&format!(
                        "clipboard_snapshot format={format} result=skipped reason=unavailable"
                    ));
                    return Ok(None);
                }
                let clipboard_format =
                    u16::try_from(format).map_err(|_| ClipboardError::InvalidData)?;
                let duplicated =
                    unsafe { OleDuplicateData(clipboard_handle, clipboard_format, GMEM_MOVEABLE) };
                if duplicated.is_null() {
                    crate::diagnostic_line(&format!(
                        "clipboard_snapshot format={format} result=failed reason=not_duplicable"
                    ));
                    return Err(ClipboardError::Allocation);
                }
                Ok(Some(ClipboardFormatSnapshot {
                    format,
                    handle: duplicated,
                }))
            })?;
            Ok(Self { formats })
        })
    }

    fn prepare_restore(&self) -> Result<Vec<ClipboardFormatSnapshot>, ClipboardError> {
        self.formats
            .iter()
            .map(|format| {
                let duplicated =
                    unsafe { OleDuplicateData(format.handle, format.format as u16, GMEM_MOVEABLE) };
                if duplicated.is_null() {
                    return Err(ClipboardError::Allocation);
                }
                Ok(ClipboardFormatSnapshot {
                    format: format.format,
                    handle: duplicated,
                })
            })
            .collect()
    }

    fn restore_sequence_if_current(
        &self,
        owner: &ClipboardCaptureOwner,
        expected_sequence: u32,
    ) -> Result<Option<u32>, ClipboardError> {
        let mut formats = self.prepare_restore()?;
        let restored = with_open_clipboard_owner(owner.hwnd, || {
            let current_sequence = unsafe { GetClipboardSequenceNumber() };
            restore_preserved_formats_if_current(
                &mut formats,
                expected_sequence,
                current_sequence,
                || {
                    if unsafe { EmptyClipboard() } == 0 {
                        Err(ClipboardError::Busy)
                    } else {
                        Ok(())
                    }
                },
                |format| {
                    let accepted = unsafe { SetClipboardData(format.format, format.handle) };
                    if accepted.is_null() {
                        return Err(ClipboardError::Allocation);
                    }
                    format.release_to_clipboard();
                    Ok(())
                },
            )
        })?;
        if !restored {
            return Ok(None);
        }
        // Restoration writes all saved formats directly; it does not hand
        // ownership to an OLE data object. Like publication, it can synthesize
        // formats when closed, so the retry token must be settled under owner
        // verification rather than captured inside the preceding write.
        match owner.settled_sequence() {
            Ok(sequence) => Ok(Some(sequence)),
            Err(ClipboardError::Superseded) => Ok(None),
            Err(error) => Err(error),
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
        ClipboardError, ClipboardFormatSnapshot, ClipboardPublication, ClipboardReadOutcome,
        ClipboardRenderState, ClipboardSnapshot, GMEM_MOVEABLE, GlobalAlloc,
        acknowledged_read_outcome, begin_clipboard_transaction, claim_copied_text_using,
        clipboard_sequence_still_owned, collect_preservable_formats, is_retryable_capture_error,
        is_transient_read_error, publication_still_owned, publish_if_sequence_current,
        publish_precondition_holds, request_is_from_target, restore_preserved_formats_if_current,
        restore_preserved_formats_if_publication_owned,
    };
    use std::{cell::Cell, time::Duration};

    #[test]
    fn capture_accepts_its_own_post_close_synthesized_sequence() {
        let mut owned = None;
        let result = claim_copied_text_using(
            &|| true,
            Duration::from_millis(1),
            Duration::from_millis(100),
            &mut owned,
            |_| Ok(("owned synthetic selection".to_owned(), 45_196)),
            |_, _, token| {
                // Native Windows regression: SetClipboardData reported 45202,
                // then CloseClipboard synthesized formats and advanced to 45205.
                *token = Some(45_202);
                super::settled_capture_sequence(500, 500, 45_205)
            },
            || 45_205,
        );
        assert_eq!(result, Ok("owned synthetic selection".to_owned()));
        assert_eq!(owned, Some(45_205));
    }

    #[test]
    fn capture_never_adopts_a_foreign_or_unknown_post_close_sequence() {
        for (own_window, observed_owner) in [(500, 600), (500, 0), (0, 0)] {
            let reads = Cell::new(0);
            let mut owned = None;
            let result = claim_copied_text_using(
                &|| true,
                Duration::ZERO,
                Duration::from_millis(100),
                &mut owned,
                |_| {
                    reads.set(reads.get() + 1);
                    Ok(("owned synthetic selection".to_owned(), 41))
                },
                |_, _, token| {
                    *token = Some(42);
                    super::settled_capture_sequence(own_window, observed_owner, 99)
                },
                || 99,
            );
            assert_eq!(result, Err(ClipboardError::Superseded));
            assert_eq!(
                owned, None,
                "a newer or anonymous Copy is not ours to restore"
            );
            assert_eq!(reads.get(), 1, "do not recapture foreign clipboard text");
        }
    }

    #[test]
    fn a_foreign_copy_before_the_first_completed_read_is_never_claimed() {
        let mut read = false;
        let result = super::acquire_target_copy(40, 41, false, || {
            read = true;
            Ok("foreign value")
        });
        assert_eq!(result, Err(ClipboardError::Superseded));
        assert!(!read);
    }

    #[test]
    fn a_target_completing_empty_then_set_before_open_is_still_capturable() {
        let result = super::acquire_target_copy(40, 42, true, || Ok("selected text"));
        assert_eq!(result, Ok("selected text"));
    }
    #[test]
    fn a_proven_target_read_does_not_wait_on_an_optional_uia_provider() {
        let state = ClipboardRenderState {
            rendered: true,
            target_requested: true,
            ..Default::default()
        };
        let mut provider_called = false;
        let result = acknowledged_read_outcome(&state, || {
            provider_called = true;
            false
        });
        assert_eq!(result, Some(ClipboardReadOutcome::TargetRead));
        assert!(
            !provider_called,
            "the target already supplied a consumption acknowledgement"
        );
    }

    #[test]
    fn anonymous_materialization_requires_either_verification_or_the_full_lease() {
        let state = ClipboardRenderState {
            rendered: true,
            ..Default::default()
        };
        assert_eq!(acknowledged_read_outcome(&state, || false), None);
        assert_eq!(
            acknowledged_read_outcome(&state, || true),
            Some(ClipboardReadOutcome::TargetVerified)
        );
    }

    #[test]
    fn capture_and_insert_transactions_cannot_overlap_or_block_the_sta() {
        let first = begin_clipboard_transaction().expect("first transaction");
        let second = std::thread::spawn(|| begin_clipboard_transaction().map(|_| ()))
            .join()
            .expect("contending operation must return immediately");
        drop(first);
        assert_eq!(second, Err(ClipboardError::Busy));
        assert!(begin_clipboard_transaction().is_ok());
    }

    #[test]
    fn failed_privacy_publication_never_claims_a_new_foreign_sequence() {
        let mut owned = None;
        let result = claim_copied_text_using(
            &|| true,
            Duration::ZERO,
            Duration::from_millis(100),
            &mut owned,
            |_| Ok(("selection".to_owned(), 41)),
            |_, _, _| Err(ClipboardError::Busy),
            || 99,
        );
        assert_eq!(result, Ok("selection".to_owned()));
        assert_ne!(owned, Some(99), "a failed writer cannot own a foreign Copy");
    }

    #[test]
    fn a_foreign_copy_before_privacy_publication_aborts_capture() {
        let mut owned = None;
        let result = claim_copied_text_using(
            &|| true,
            Duration::ZERO,
            Duration::from_millis(100),
            &mut owned,
            |_| Ok(("selection".to_owned(), 41)),
            |_, _, _| Err(ClipboardError::Superseded),
            || 99,
        );
        assert_eq!(result, Err(ClipboardError::Superseded));
        assert_eq!(owned, None);
    }

    #[test]
    fn a_new_copy_after_publication_is_never_recaptured_or_restored() {
        let reads = Cell::new(0);
        let mut owned = None;
        let result = claim_copied_text_using(
            &|| true,
            Duration::from_millis(20),
            Duration::from_millis(30),
            &mut owned,
            |_| {
                reads.set(reads.get() + 1);
                Ok(("selection".to_owned(), 41))
            },
            |_, _, _| Ok(42),
            || 99,
        );
        assert_eq!(result, Err(ClipboardError::Superseded));
        assert_eq!(reads.get(), 1);
        assert_eq!(owned, None);
    }

    #[test]
    fn a_user_copy_between_capture_and_publish_prevents_replacement() {
        assert!(publish_precondition_holds(41, 41));
        assert!(!publish_precondition_holds(41, 42));

        let mut destructive_callback_entered = false;
        let result = publish_if_sequence_current(41, 42, || {
            destructive_callback_entered = true;
            Ok(())
        })
        .expect("a foreign Copy is a non-destructive rejection");

        assert_eq!(result, None);
        assert!(!destructive_callback_entered);
    }

    #[test]
    fn own_delayed_materialization_keeps_publication_owned() {
        let publication = ClipboardPublication {
            generation: 7,
            replacement_sequence: 41,
        };

        assert!(publication_still_owned(publication, 7, 42, 100, 100,));

        let mut formats = vec![13];
        let mut clipboard_was_emptied = false;
        let restored = restore_preserved_formats_if_publication_owned(
            &mut formats,
            publication,
            7,
            42,
            100,
            100,
            || {
                clipboard_was_emptied = true;
                Ok(())
            },
            |_| Ok(()),
        )
        .expect("own materialization remains restorable");

        assert!(restored);
        assert!(clipboard_was_emptied);
    }

    #[test]
    fn a_user_copy_after_materialization_prevents_restore() {
        let publication = ClipboardPublication {
            generation: 7,
            replacement_sequence: 41,
        };

        assert!(!publication_still_owned(publication, 7, 43, 200, 100,));

        let mut formats = vec![13];
        let mut clipboard_was_emptied = false;
        let restored = restore_preserved_formats_if_publication_owned(
            &mut formats,
            publication,
            7,
            43,
            200,
            100,
            || {
                clipboard_was_emptied = true;
                Ok(())
            },
            |_| Ok(()),
        )
        .expect("a foreign Copy is a safe no-op");

        assert!(!restored);
        assert!(!clipboard_was_emptied);
    }

    #[test]
    fn an_unreadable_private_format_does_not_discard_readable_clipboard_data() {
        let formats = collect_preservable_formats([13, 49_858, 17], |format| {
            Ok((format != 49_858).then_some(format))
        })
        .expect("standard clipboard formats remain preservable");

        assert_eq!(formats, vec![13, 17]);
    }

    #[test]
    fn a_nonempty_clipboard_with_no_readable_formats_is_rejected() {
        let result = collect_preservable_formats([49_858], |_| Ok(None::<u32>));

        assert_eq!(result, Err(ClipboardError::InvalidData));
    }

    #[test]
    fn a_duplication_failure_aborts_the_entire_snapshot() {
        let result = collect_preservable_formats([13, 17], |format| {
            if format == 17 {
                Err(ClipboardError::Allocation)
            } else {
                Ok(Some(format))
            }
        });

        assert_eq!(result, Err(ClipboardError::Allocation));
    }

    #[test]
    fn readable_formats_survive_an_unavailable_private_format_and_restore_together() {
        let mut formats = collect_preservable_formats([13, 49_858, 8], |format| {
            Ok((format != 49_858).then_some(format))
        })
        .expect("readable standard formats should form a complete snapshot");
        let mut clipboard_was_emptied = false;
        let mut restored = Vec::new();

        let restored_snapshot = restore_preserved_formats_if_current(
            &mut formats,
            41,
            41,
            || {
                clipboard_was_emptied = true;
                Ok(())
            },
            |format| {
                restored.push(*format);
                Ok(())
            },
        )
        .expect("matching clipboard sequence should restore the snapshot");

        assert!(restored_snapshot);
        assert!(clipboard_was_emptied);
        assert_eq!(restored, vec![13, 8]);
    }

    #[test]
    fn a_newer_user_copy_wins_without_emptying_the_clipboard() {
        let mut formats = vec![13, 8];
        let mut clipboard_was_emptied = false;
        let mut restored = Vec::new();

        let restored_snapshot = restore_preserved_formats_if_current(
            &mut formats,
            41,
            42,
            || {
                clipboard_was_emptied = true;
                Ok(())
            },
            |format| {
                restored.push(*format);
                Ok(())
            },
        )
        .expect("a superseded clipboard is a safe no-op");

        assert!(!restored_snapshot);
        assert!(!clipboard_was_emptied);
        assert!(restored.is_empty());
    }

    #[test]
    fn restore_is_rejected_when_sequence_changes_while_clipboard_open_retries() {
        let sequence_before_open = 41;
        let sequence_after_retry = 42;

        assert!(!clipboard_sequence_still_owned(
            sequence_before_open,
            sequence_after_retry
        ));
    }

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

    #[test]
    fn a_snapshot_can_prepare_two_independent_restores() {
        let original = unsafe { GlobalAlloc(GMEM_MOVEABLE, 32) };
        assert!(!original.is_null());
        let snapshot = ClipboardSnapshot {
            formats: vec![ClipboardFormatSnapshot {
                format: 13,
                handle: original,
            }],
        };

        let first = snapshot.prepare_restore().expect("first restore copy");
        let second = snapshot.prepare_restore().expect("second restore copy");

        assert_eq!(snapshot.formats[0].handle, original);
        assert_ne!(first[0].handle, original);
        assert_ne!(second[0].handle, original);
        assert_ne!(first[0].handle, second[0].handle);
    }
}

#[cfg(test)]
mod settled_disposition_tests {
    use super::{
        ClipboardError, ClipboardPasteDisposition, ClipboardReadOutcome, settled_disposition,
    };

    #[test]
    fn an_unverified_dispatched_paste_is_not_reported_as_not_delivered_on_restore_failure() {
        let result = settled_disposition(
            ClipboardReadOutcome::DispatchedUnverified,
            Err(ClipboardError::Busy),
        );
        assert_eq!(result, Err(ClipboardError::DeliveryUnconfirmed));
    }

    #[test]
    fn a_restore_failure_after_a_verified_paste_is_still_delivered() {
        assert!(matches!(
            settled_disposition(
                ClipboardReadOutcome::TargetVerified,
                Err(ClipboardError::Busy)
            ),
            Ok(ClipboardPasteDisposition::VerifiedRestoreFailed)
        ));
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TargetRead, Err(ClipboardError::Busy)),
            Ok(ClipboardPasteDisposition::PayloadReadRestoreFailed)
        ));
    }

    #[test]
    fn the_other_outcomes_keep_their_meaning() {
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TargetVerified, Ok(true)),
            Ok(ClipboardPasteDisposition::VerifiedAndRestored)
        ));
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TargetRead, Ok(false)),
            Ok(ClipboardPasteDisposition::PayloadReadAndSuperseded)
        ));
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::DispatchedUnverified, Ok(false)),
            Err(ClipboardError::DeliveryUnconfirmed)
        ));
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TimedOut, Ok(true)),
            Err(ClipboardError::NotDelivered)
        ));
        // Without proof of delivery a restore failure is still a failure.
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TimedOut, Err(ClipboardError::Busy)),
            Err(ClipboardError::Busy)
        ));
        assert!(matches!(
            settled_disposition(ClipboardReadOutcome::TargetChanged, Ok(true)),
            Err(ClipboardError::TargetChanged)
        ));
    }
}
