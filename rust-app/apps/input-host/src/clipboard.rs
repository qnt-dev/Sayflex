use std::{
    ffi::c_void,
    mem, ptr, thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use windows_sys::{
    Win32::{
        Foundation::GlobalFree,
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

#[derive(Debug, Error)]
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
    #[error("paste injection failed")]
    Paste,
    #[error("OLE clipboard operation failed with HRESULT {0:#010x}")]
    Ole(i32),
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
            unsafe { OleUninitialize() };
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
    send_copy: impl FnOnce() -> Result<(), ClipboardError>,
) -> Result<String, ClipboardError> {
    let _apartment = ClipboardApartment::initialize()?;
    let mut snapshot = ClipboardSnapshot::capture()?;
    let before = unsafe { GetClipboardSequenceNumber() };
    if let Err(error) = send_copy() {
        if unsafe { GetClipboardSequenceNumber() } != before {
            snapshot.restore()?;
        }
        return Err(error);
    }
    let deadline = Instant::now() + Duration::from_millis(900);
    while unsafe { GetClipboardSequenceNumber() } == before {
        if Instant::now() >= deadline {
            return Err(ClipboardError::CopyTimeout);
        }
        thread::sleep(Duration::from_millis(12));
    }
    let copied_sequence = unsafe { GetClipboardSequenceNumber() };
    let text = get_text();
    if unsafe { GetClipboardSequenceNumber() } == copied_sequence {
        snapshot.restore()?;
    }
    text
}

pub fn dispatch_paste(
    target_still_focused: impl Fn() -> bool,
    send_paste: impl FnOnce() -> Result<(), ClipboardError>,
    text: &str,
    add_trailing_space: bool,
) -> Result<PendingClipboardRestore, ClipboardError> {
    if !target_still_focused() {
        return Err(ClipboardError::TargetChanged);
    }
    let mut snapshot = ClipboardSnapshot::capture()?;
    let complete = if add_trailing_space {
        format!("{text} ")
    } else {
        text.to_owned()
    };
    set_private_text(&complete)?;
    let replacement_sequence = unsafe { GetClipboardSequenceNumber() };
    if !target_still_focused() {
        if unsafe { GetClipboardSequenceNumber() } == replacement_sequence {
            snapshot.restore()?;
        }
        return Err(ClipboardError::TargetChanged);
    }
    if let Err(error) = send_paste() {
        if unsafe { GetClipboardSequenceNumber() } == replacement_sequence {
            snapshot.restore()?;
        }
        return Err(error);
    }
    Ok(PendingClipboardRestore {
        snapshot,
        replacement_sequence,
    })
}

pub struct PendingClipboardRestore {
    snapshot: ClipboardSnapshot,
    replacement_sequence: u32,
}

impl PendingClipboardRestore {
    pub fn restore(mut self) -> Result<(), ClipboardError> {
        if unsafe { GetClipboardSequenceNumber() } == self.replacement_sequence {
            self.snapshot.restore()?;
        }
        Ok(())
    }
}

struct ClipboardSnapshot {
    data_object: *mut c_void,
}

impl ClipboardSnapshot {
    fn capture() -> Result<Self, ClipboardError> {
        let mut data_object = ptr::null_mut();
        let result = unsafe { OleGetClipboard(&mut data_object) };
        if result < 0 {
            return Err(ClipboardError::Ole(result));
        }
        Ok(Self { data_object })
    }

    fn restore(&mut self) -> Result<(), ClipboardError> {
        let result = unsafe { OleSetClipboard(self.data_object) };
        if result < 0 {
            return Err(ClipboardError::Ole(result));
        }
        Ok(())
    }
}

impl Drop for ClipboardSnapshot {
    fn drop(&mut self) {
        if !self.data_object.is_null() {
            unsafe {
                // OleGetClipboard returns an AddRef'd IDataObject interface.
                // Release exactly that reference; OleSetClipboard keeps its own.
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
    for _ in 0..12 {
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
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
