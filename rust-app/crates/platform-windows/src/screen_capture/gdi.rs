use std::{ffi::c_void, mem::size_of, ptr::null_mut, slice};

use windows_sys::Win32::{
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
    },
    UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

use rust_protocol::PhysicalRect;

use super::{
    CaptureError, CapturedOutput, DesktopCapture, DesktopCaptureBackend, Rotation,
    next_capture_generation,
};

#[derive(Debug, Default)]
pub struct GdiDesktopCaptureBackend;

impl DesktopCaptureBackend for GdiDesktopCaptureBackend {
    fn capture(&self) -> Result<DesktopCapture, CaptureError> {
        capture_virtual_desktop()
    }
}

fn capture_virtual_desktop() -> Result<DesktopCapture, CaptureError> {
    // GetSystemMetrics reports the virtual desktop in the calling process's
    // physical coordinate space. SayFlex opts into per-monitor DPI awareness
    // before constructing its UI, so no logical-pixel conversion belongs here.
    let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(CaptureError::SecureDesktop);
    }
    let bounds = PhysicalRect {
        x,
        y,
        width: width as u32,
        height: height as u32,
    };
    let stride = bounds
        .width
        .checked_mul(4)
        .ok_or(CaptureError::StrideOverflow {
            width: bounds.width,
        })?;
    let byte_len = usize::try_from(
        u64::from(stride)
            .checked_mul(u64::from(bounds.height))
            .ok_or(CaptureError::InvalidGeometry)?,
    )
    .map_err(|_| CaptureError::InvalidGeometry)?;
    if byte_len > super::MAX_CAPTURE_BYTES {
        return Err(CaptureError::AllocationTooLarge {
            requested: byte_len as u64,
            limit: super::MAX_CAPTURE_BYTES,
        });
    }

    // SAFETY: Every acquired GDI object is checked and released on each path.
    // The selected bitmap is restored before it is deleted.
    unsafe {
        let screen_dc = GetDC(null_mut());
        if screen_dc.is_null() {
            return Err(CaptureError::BackendFailure(
                "GDI GetDC returned null".into(),
            ));
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.is_null() {
            let _ = ReleaseDC(null_mut(), screen_dc);
            return Err(CaptureError::BackendFailure(
                "GDI CreateCompatibleDC returned null".into(),
            ));
        }

        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize =
            size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut::<c_void>();
        let bitmap = CreateDIBSection(screen_dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            if !bitmap.is_null() {
                let _ = DeleteObject(bitmap);
            }
            let _ = DeleteDC(memory_dc);
            let _ = ReleaseDC(null_mut(), screen_dc);
            return Err(CaptureError::BackendFailure(
                "GDI CreateDIBSection failed".into(),
            ));
        }
        let previous = SelectObject(memory_dc, bitmap);
        if previous.is_null() || previous as isize == -1 {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(memory_dc);
            let _ = ReleaseDC(null_mut(), screen_dc);
            return Err(CaptureError::BackendFailure(
                "GDI SelectObject failed".into(),
            ));
        }

        let copied = BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            screen_dc,
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );
        let result = if copied == 0 {
            Err(CaptureError::BackendFailure("GDI BitBlt failed".into()))
        } else {
            let bgra = slice::from_raw_parts(bits.cast::<u8>(), byte_len);
            let mut rgba = vec![0_u8; byte_len];
            for (source, target) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                target.copy_from_slice(&[source[2], source[1], source[0], u8::MAX]);
            }
            CapturedOutput::try_new(bounds, Rotation::Identity, 1_000, stride, rgba)
                .and_then(|output| DesktopCapture::try_new(next_capture_generation(), vec![output]))
        };

        let _ = SelectObject(memory_dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory_dc);
        let _ = ReleaseDC(null_mut(), screen_dc);
        result
    }
}
