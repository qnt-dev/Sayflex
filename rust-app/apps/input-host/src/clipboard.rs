use std::{
    mem, ptr, thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{GlobalFree, HANDLE},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_UNICODETEXT, OleDuplicateData},
    },
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
}

pub fn set_text(text: &str) -> Result<(), ClipboardError> {
    let utf16 = text.encode_utf16().chain([0]).collect::<Vec<_>>();
    let bytes = utf16.len() * mem::size_of::<u16>();
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
    if handle.is_null() {
        return Err(ClipboardError::Allocation);
    }

    let pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
    if pointer.is_null() {
        unsafe {
            GlobalFree(handle);
        }
        return Err(ClipboardError::Allocation);
    }
    unsafe {
        ptr::copy_nonoverlapping(utf16.as_ptr(), pointer, utf16.len());
        let _ = GlobalUnlock(handle);
    }

    if let Err(error) = with_open_clipboard(|| {
        if unsafe { EmptyClipboard() } == 0 {
            return Err(ClipboardError::Busy);
        }
        let accepted = unsafe { SetClipboardData(CF_UNICODETEXT.into(), handle) };
        if accepted.is_null() {
            return Err(ClipboardError::Allocation);
        }
        Ok(())
    }) {
        unsafe {
            GlobalFree(handle);
        }
        return Err(error);
    }
    Ok(())
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
    let mut snapshot = ClipboardSnapshot::capture()?;
    let before = unsafe { GetClipboardSequenceNumber() };
    send_copy()?;
    let deadline = Instant::now() + Duration::from_millis(900);
    while unsafe { GetClipboardSequenceNumber() } == before {
        if Instant::now() >= deadline {
            return Err(ClipboardError::CopyTimeout);
        }
        thread::sleep(Duration::from_millis(12));
    }
    let copied_sequence = unsafe { GetClipboardSequenceNumber() };
    let text = get_text()?;
    if unsafe { GetClipboardSequenceNumber() } == copied_sequence {
        snapshot.restore()?;
    }
    Ok(text)
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
    set_text(&complete)?;
    let replacement_sequence = unsafe { GetClipboardSequenceNumber() };
    if !target_still_focused() {
        if unsafe { GetClipboardSequenceNumber() } == replacement_sequence {
            snapshot.restore()?;
        }
        return Err(ClipboardError::TargetChanged);
    }
    send_paste()?;
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
    pub fn restore_after(mut self, delay: Duration) -> Result<(), ClipboardError> {
        thread::sleep(delay);
        if unsafe { GetClipboardSequenceNumber() } == self.replacement_sequence {
            self.snapshot.restore()?;
        }
        Ok(())
    }
}

struct ClipboardSnapshot {
    formats: Vec<(u32, HANDLE)>,
}

impl ClipboardSnapshot {
    fn capture() -> Result<Self, ClipboardError> {
        with_open_clipboard(|| {
            let mut formats = Vec::new();
            let mut format = 0_u32;
            loop {
                format = unsafe { EnumClipboardFormats(format) };
                if format == 0 {
                    break;
                }
                let source = unsafe { GetClipboardData(format) };
                if source.is_null() {
                    continue;
                }
                let duplicate = unsafe { OleDuplicateData(source, format as u16, 0) };
                if !duplicate.is_null() {
                    formats.push((format, duplicate));
                }
            }
            Ok(Self { formats })
        })
    }

    fn restore(&mut self) -> Result<(), ClipboardError> {
        with_open_clipboard(|| {
            if unsafe { EmptyClipboard() } == 0 {
                return Err(ClipboardError::Busy);
            }
            for (format, handle) in &mut self.formats {
                if handle.is_null() {
                    continue;
                }
                if !unsafe { SetClipboardData(*format, *handle) }.is_null() {
                    *handle = ptr::null_mut();
                }
            }
            Ok(())
        })
    }
}

impl Drop for ClipboardSnapshot {
    fn drop(&mut self) {
        for (_, handle) in &mut self.formats {
            if !handle.is_null() {
                unsafe {
                    GlobalFree(*handle);
                }
                *handle = ptr::null_mut();
            }
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
