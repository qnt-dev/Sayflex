use std::{ffi::c_void, io, path::PathBuf, ptr};

use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

const MAX_PATH_CHARS: usize = 32_768;

pub fn choose_png_save_path(
    owner_window_handle: Option<u64>,
    suggested_name: &str,
    title: &str,
) -> io::Result<Option<PathBuf>> {
    let mut file = vec![0_u16; MAX_PATH_CHARS];
    let suggested_name = suggested_name.replace(['\0', '/', '\\'], "_");
    let encoded = suggested_name.encode_utf16().collect::<Vec<_>>();
    let copy_len = encoded.len().min(file.len().saturating_sub(1));
    file[..copy_len].copy_from_slice(&encoded[..copy_len]);

    let filter = "PNG image (*.png)\0*.png\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let title = title
        .replace('\0', " ")
        .encode_utf16()
        .chain([0])
        .collect::<Vec<_>>();
    let extension = "png\0".encode_utf16().collect::<Vec<_>>();
    let mut dialog = OPENFILENAMEW {
        lStructSize: u32::try_from(std::mem::size_of::<OPENFILENAMEW>())
            .expect("OPENFILENAMEW size fits in u32"),
        hwndOwner: owner_window_handle
            .map_or(ptr::null_mut(), |handle| handle as usize as *mut c_void),
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: u32::try_from(file.len()).expect("dialog buffer length fits in u32"),
        lpstrTitle: if title.len() <= 1 {
            ptr::null()
        } else {
            title.as_ptr()
        },
        Flags: OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        lpstrDefExt: extension.as_ptr(),
        ..Default::default()
    };

    // SAFETY: every pointer in `dialog` refers to a live, writable or immutable
    // UTF-16 buffer for the duration of the modal call. The owner handle is a
    // window identity obtained from the live Slint surface.
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        // SAFETY: this reads thread-local common-dialog status immediately
        // after the failed call. Zero means that the user cancelled.
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(io::Error::other(format!(
                "Windows save dialog failed with code {error}"
            )))
        };
    }

    let length = file
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(file.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(
        &file[..length],
    ))))
}
