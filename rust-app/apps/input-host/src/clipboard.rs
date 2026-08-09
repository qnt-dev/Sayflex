use std::{
    ffi::c_void,
    mem, ptr, thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use windows_sys::{
    Win32::{
        Foundation::{GlobalFree, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, OleGetClipboard, OleInitialize, OleSetClipboard, OleUninitialize,
            },
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

struct ClipboardSnapshot {
    data_object: *mut c_void,
    fallback_text: Option<String>,
}

impl ClipboardSnapshot {
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
    use super::{ClipboardError, is_retryable_capture_error, is_transient_read_error};

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
}
