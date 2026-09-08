use std::{ffi::c_void, mem::size_of, ptr::null_mut, slice, sync::OnceLock};

use crate::BlurredBackdrop;

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Dwm::{
        DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DWM_CLOAKED_APP, DWMSBT_NONE,
        DWMSBT_TRANSIENTWINDOW, DWMWA_CLOAK, DWMWA_CLOAKED, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DwmEnableBlurBehindWindow, DwmFlush, DwmGetWindowAttribute, DwmSetWindowAttribute,
    },
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BitBlt, CAPTUREBLT, ClientToScreen, CreateCompatibleDC,
        CreateDIBSection, CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
    },
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Threading::GetCurrentProcessId,
    },
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::{
            GUITHREADINFO, GWL_EXSTYLE, GWL_STYLE, GetCursorPos, GetForegroundWindow,
            GetGUIThreadInfo, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
            GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, ICON_BIG, ICON_SMALL,
            IMAGE_ICON, IsWindowVisible, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
            SM_CYICON, SM_CYSMICON, SW_HIDE, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
            SetWindowPos, ShowWindow, WM_SETICON, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
            WS_SYSMENU, WS_THICKFRAME,
        },
    },
};

#[repr(C)]
struct AccentPolicy {
    accent_state: i32,
    accent_flags: i32,
    gradient_color: u32,
    animation_id: i32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: i32,
    data: *mut c_void,
    size: usize,
}

const WCA_ACCENT_POLICY: i32 = 19;
const ACCENT_DISABLED: i32 = 0;
const PREWARM_WINDOW_COORDINATE: i32 = -32_000;

pub fn configure_overlay_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    configure_overlay_window_impl(window_handle, target_window_handle, true)
}

pub fn configure_transparent_overlay_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    configure_overlay_window_impl(window_handle, target_window_handle, false)
}

pub fn configure_blue_glass_overlay_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    configure_overlay_window_impl(window_handle, target_window_handle, false)?;
    let hwnd = window_handle as usize as HWND;
    apply_blue_glass_popup_style(hwnd);
    Ok(())
}

pub fn prepare_overlay_window(window_handle: u64) -> Result<(), String> {
    prepare_overlay_window_impl(window_handle, true)
}

pub fn prepare_transparent_overlay_window(window_handle: u64) -> Result<(), String> {
    prepare_overlay_window_impl(window_handle, false)
}

pub fn prepare_blue_glass_overlay_window(window_handle: u64) -> Result<(), String> {
    prepare_overlay_window_impl(window_handle, false)?;
    apply_blue_glass_popup_style(window_handle as usize as HWND);
    Ok(())
}

pub fn capture_blurred_overlay_backdrop(
    window_handle: u64,
    blur_radius: u32,
) -> Result<BlurredBackdrop, String> {
    capture_blurred_overlay_backdrop_impl(window_handle, blur_radius, true)
}

pub fn capture_blurred_cloaked_overlay_backdrop(
    window_handle: u64,
    blur_radius: u32,
) -> Result<BlurredBackdrop, String> {
    if !overlay_window_is_app_cloaked(window_handle)? {
        return Err("overlay backdrop capture requires an app-cloaked window".to_owned());
    }
    capture_blurred_overlay_backdrop_impl(window_handle, blur_radius, false)
}

fn capture_blurred_overlay_backdrop_impl(
    window_handle: u64,
    blur_radius: u32,
    hide_visible_window: bool,
) -> Result<BlurredBackdrop, String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }

    let mut rect = RECT::default();
    // SAFETY: hwnd is the live preview HWND owned by this process and rect is
    // writable for the duration of the Win32 call.
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err("GetWindowRect failed before backdrop capture".to_owned());
    }

    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    if width <= 0 || height <= 0 {
        return Err("overlay window has no capturable area".to_owned());
    }
    let byte_len = width as usize * height as usize * 4;

    // Interactive previews still need the legacy visibility guard. The
    // recording HUD is app-cloaked by its presenter, so hiding/showing it here
    // would violate the single-lifetime overlay contract and reintroduce the
    // exact visibility race that this capture path is designed to avoid.
    let _visibility = hide_visible_window.then(|| WindowVisibilityGuard::hide(hwnd));

    // SAFETY: Every GDI handle is validated before use and released below.
    unsafe {
        let screen_dc = GetDC(null_mut());
        if screen_dc.is_null() {
            return Err("GetDC failed during backdrop capture".to_owned());
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.is_null() {
            let _ = ReleaseDC(null_mut(), screen_dc);
            return Err("CreateCompatibleDC failed during backdrop capture".to_owned());
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
            return Err("CreateDIBSection failed during backdrop capture".to_owned());
        }

        let previous = SelectObject(memory_dc, bitmap);
        if previous.is_null() || previous as isize == -1 {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(memory_dc);
            let _ = ReleaseDC(null_mut(), screen_dc);
            return Err("SelectObject failed during backdrop capture".to_owned());
        }

        let copied = BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            screen_dc,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        let result = if copied == 0 {
            Err("BitBlt failed during backdrop capture".to_owned())
        } else {
            let bgra = slice::from_raw_parts(bits.cast::<u8>(), byte_len);
            let mut rgba = vec![0_u8; byte_len];
            for (source, target) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                target[0] = source[2];
                target[1] = source[1];
                target[2] = source[0];
                target[3] = u8::MAX;
            }
            blur_rgba(&mut rgba, width as usize, height as usize, blur_radius);
            Ok(BlurredBackdrop {
                width: width as u32,
                height: height as u32,
                rgba,
            })
        };

        let _ = SelectObject(memory_dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory_dc);
        let _ = ReleaseDC(null_mut(), screen_dc);
        result
    }
}

struct WindowVisibilityGuard {
    hwnd: HWND,
    was_visible: bool,
}

impl WindowVisibilityGuard {
    fn hide(hwnd: HWND) -> Self {
        // SAFETY: The HWND belongs to the current process. DwmFlush only waits
        // for the compositor to apply the visibility change.
        unsafe {
            let was_visible = IsWindowVisible(hwnd) != 0;
            if was_visible {
                let _ = ShowWindow(hwnd, SW_HIDE);
                let _ = DwmFlush();
            }
            Self { hwnd, was_visible }
        }
    }
}

impl Drop for WindowVisibilityGuard {
    fn drop(&mut self) {
        // SAFETY: The window remains alive while the preview owns this guard.
        // SW_SHOWNOACTIVATE preserves the user's foreground application.
        if self.was_visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = DwmFlush();
            }
        }
    }
}

fn blur_rgba(pixels: &mut [u8], width: usize, height: usize, radius: u32) {
    if width == 0 || height == 0 || pixels.len() != width * height * 4 {
        return;
    }
    let radius = radius.clamp(1, 24) as usize;
    let mut scratch = vec![0_u8; pixels.len()];
    for _ in 0..3 {
        blur_horizontal(pixels, &mut scratch, width, height, radius);
        blur_vertical(&scratch, pixels, width, height, radius);
    }
}

fn blur_horizontal(source: &[u8], target: &mut [u8], width: usize, height: usize, radius: usize) {
    for y in 0..height {
        for x in 0..width {
            let start = x.saturating_sub(radius);
            let end = (x + radius).min(width - 1);
            let count = (end - start + 1) as u32;
            for channel in 0..3 {
                let mut sum = 0_u32;
                for sample_x in start..=end {
                    sum += source[(y * width + sample_x) * 4 + channel] as u32;
                }
                target[(y * width + x) * 4 + channel] = (sum / count) as u8;
            }
            target[(y * width + x) * 4 + 3] = u8::MAX;
        }
    }
}

fn blur_vertical(source: &[u8], target: &mut [u8], width: usize, height: usize, radius: usize) {
    for y in 0..height {
        let start = y.saturating_sub(radius);
        let end = (y + radius).min(height - 1);
        let count = (end - start + 1) as u32;
        for x in 0..width {
            for channel in 0..3 {
                let mut sum = 0_u32;
                for sample_y in start..=end {
                    sum += source[(sample_y * width + x) * 4 + channel] as u32;
                }
                target[(y * width + x) * 4 + channel] = (sum / count) as u8;
            }
            target[(y * width + x) * 4 + 3] = u8::MAX;
        }
    }
}

fn configure_overlay_window_impl(
    window_handle: u64,
    target_window_handle: Option<u64>,
    use_backdrop: bool,
) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    dress_icon(hwnd);
    apply_overlay_contract(hwnd, use_backdrop)?;

    // SAFETY: All pointers refer to initialized buffers for the duration of
    // each Win32 call. The HWND belongs to this process and was resolved by
    // exact title before subsequent calls.
    unsafe {
        let monitor_target = target_window_handle
            .filter(|handle| *handle != 0)
            .map(|handle| handle as usize as *mut core::ffi::c_void)
            .unwrap_or(hwnd);
        let monitor = MonitorFromWindow(monitor_target, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Err("Windows could not resolve the overlay monitor".to_owned());
        }

        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
            return Err("GetMonitorInfoW failed".to_owned());
        }

        let mut window_rect = RECT::default();
        if GetWindowRect(hwnd, &mut window_rect) == 0 {
            return Err("GetWindowRect failed".to_owned());
        }

        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        let work = monitor_info.rcWork;
        let x = work.left + ((work.right - work.left - width) / 2);
        let y = work.bottom - height - 22;

        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed".to_owned());
        }
    }

    Ok(())
}

fn prepare_overlay_window_impl(window_handle: u64, use_backdrop: bool) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    apply_overlay_contract(hwnd, use_backdrop)?;
    set_overlay_cloaked(hwnd, true)?;
    unsafe {
        let result = DwmFlush();
        if result < 0 {
            return Err(format!(
                "DwmFlush failed while prewarming overlay: 0x{result:08X}"
            ));
        }
    }
    // SAFETY: The live HWND belongs to this process. Parking it outside every
    // practical virtual desktop prevents a native background frame from ever
    // becoming visible while Winit and DWM finish their first composition.
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            PREWARM_WINDOW_COORDINATE,
            PREWARM_WINDOW_COORDINATE,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    } == 0
    {
        return Err("SetWindowPos failed while prewarming overlay".to_owned());
    }
    Ok(())
}

pub fn conceal_overlay_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    set_overlay_cloaked(hwnd, true)?;
    // Wait until DWM has removed the window before moving it. The retirement
    // path is not latency-sensitive, and this ordering prevents a visible
    // off-screen jump on a heavily loaded compositor.
    unsafe {
        let _ = DwmFlush();
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            PREWARM_WINDOW_COORDINATE,
            PREWARM_WINDOW_COORDINATE,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        ) == 0
        {
            return Err("SetWindowPos failed while concealing overlay".to_owned());
        }
    }
    Ok(())
}

pub fn cloak_overlay_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    set_overlay_cloaked(hwnd, true)?;
    unsafe {
        let result = DwmFlush();
        if result < 0 {
            return Err(format!(
                "DwmFlush failed while cloaking overlay: 0x{result:08X}"
            ));
        }
    }
    Ok(())
}

/// Asks the compositor to repaint a window.
///
/// The same thing Slint's `request_redraw` reaches in the end, but callable
/// with nothing but the handle — a timer cannot hold a `slint::Window`. Used to
/// ask a second time for the first frame of an overlay that is waiting to be
/// shown, because a request issued while the window is still settling after a
/// move is sometimes coalesced away.
pub fn request_repaint(window_handle: u64) {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return;
    }
    // SAFETY: hwnd belongs to this process; invalidating its whole client area
    // only schedules a paint.
    unsafe {
        let _ = InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

pub fn reveal_overlay_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    // Uncloaking a window that has lost WS_VISIBLE leaves it invisible and
    // says nothing about it. The overlay is meant to be hidden by the DWM
    // cloak alone, so if the style has gone missing, put it back rather than
    // uncloak into silence.
    //
    // SAFETY: hwnd belongs to this process and both calls only read or set
    // this window's own visibility.
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
    set_overlay_cloaked(hwnd, false)?;
    // DwmFlush is a compositor acknowledgement, not a timing workaround. It
    // guarantees that the already-rendered frame has crossed the native
    // visibility boundary before this transition is reported as Presented.
    unsafe {
        let result = DwmFlush();
        if result < 0 {
            return Err(format!(
                "DwmFlush failed while revealing overlay: 0x{result:08X}"
            ));
        }
    }
    Ok(())
}

pub fn verify_overlay_window_contract(
    window_handle: u64,
    expected_cloaked: bool,
) -> Result<(), String> {
    verify_overlay_window_contract_impl(window_handle, expected_cloaked, true)
}

pub fn verify_interactive_overlay_window_contract(
    window_handle: u64,
    expected_cloaked: bool,
) -> Result<(), String> {
    verify_overlay_window_contract_impl(window_handle, expected_cloaked, false)
}

/// Reassert the native styles that make a Slint window a click-through HUD.
///
/// Winit remains responsible for the HWND lifetime and can rewrite extended
/// styles while applying a toolkit-side size or visibility change. Overlay
/// presentation therefore treats the Win32 contract as a reconciled
/// invariant: apply it immediately before every verify/reveal boundary rather
/// than assuming the values written during initial attachment are permanent.
pub fn reconcile_overlay_window_contract(window_handle: u64) -> Result<(), String> {
    reconcile_overlay_ex_style(window_handle, true)
}

/// Reassert the native styles for an interactive, non-activating overlay.
pub fn reconcile_interactive_overlay_window_contract(window_handle: u64) -> Result<(), String> {
    reconcile_overlay_ex_style(window_handle, false)
}

fn reconcile_overlay_ex_style(window_handle: u64, click_through: bool) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    apply_overlay_ex_style(hwnd, click_through)
}

fn verify_overlay_window_contract_impl(
    window_handle: u64,
    expected_cloaked: bool,
    click_through: bool,
) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    unsafe {
        let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let required = WS_EX_NOACTIVATE as isize
            | WS_EX_TOOLWINDOW as isize
            | if click_through {
                WS_EX_TRANSPARENT as isize
            } else {
                0
            };
        let missing = required & !extended_style;
        let has_app_window = extended_style & WS_EX_APPWINDOW as isize != 0;
        let has_unexpected_click_through =
            !click_through && extended_style & WS_EX_TRANSPARENT as isize != 0;
        if missing != 0 || has_app_window || has_unexpected_click_through {
            return Err(format!(
                "overlay ex-style contract mismatch: exstyle=0x{extended_style:X} \
missing=0x{missing:X} app_window={has_app_window} \
unexpected_click_through={has_unexpected_click_through}"
            ));
        }
    }
    let actual_cloaked = overlay_window_is_app_cloaked(window_handle)?;
    if actual_cloaked != expected_cloaked {
        return Err(format!(
            "overlay cloak contract mismatch: expected={expected_cloaked} actual={actual_cloaked}"
        ));
    }
    Ok(())
}

fn set_overlay_cloaked(hwnd: HWND, cloaked: bool) -> Result<(), String> {
    let value: i32 = i32::from(cloaked);
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK as u32,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        )
    };
    if result < 0 {
        return Err(format!(
            "DwmSetWindowAttribute(DWMWA_CLOAK) failed: 0x{result:08X}"
        ));
    }
    Ok(())
}

fn overlay_window_is_app_cloaked(window_handle: u64) -> Result<bool, String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    let mut flags = 0_u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            (&raw mut flags).cast(),
            size_of::<u32>() as u32,
        )
    };
    if result < 0 {
        return Err(format!(
            "DwmGetWindowAttribute(DWMWA_CLOAKED) failed: 0x{result:08X}"
        ));
    }
    Ok(flags & DWM_CLOAKED_APP != 0)
}

fn apply_overlay_contract(hwnd: HWND, use_backdrop: bool) -> Result<(), String> {
    // SAFETY: hwnd is a live top-level window owned by this process. The style
    // and compositor calls do not retain pointers beyond their invocation.
    apply_overlay_ex_style(hwnd, true)?;
    unsafe {
        if use_backdrop {
            apply_transient_backdrop(hwnd);
        } else {
            apply_transparent_composition(hwnd);
        }
    }
    Ok(())
}

fn apply_overlay_ex_style(hwnd: HWND, click_through: bool) -> Result<(), String> {
    // SAFETY: hwnd is a live top-level window owned by this process. The
    // FRAMECHANGED call makes the rewritten extended style effective before
    // the following contract verification and does not activate the window.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let base_style = (current_style & !(WS_EX_APPWINDOW as isize))
            | WS_EX_NOACTIVATE as isize
            | WS_EX_TOOLWINDOW as isize;
        let desired_style = if click_through {
            base_style | WS_EX_TRANSPARENT as isize
        } else {
            base_style & !(WS_EX_TRANSPARENT as isize)
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style);
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed while reconciling overlay contract".to_owned());
        }
    }
    Ok(())
}

fn apply_blue_glass_popup_style(hwnd: HWND) {
    // SAFETY: The common overlay contract has validated and configured this
    // process-owned HWND. Removing non-client styles prevents a caption glyph
    // from appearing when DWM extends glass into the client area.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let desired_style = (current_style
            & !(WS_CAPTION as isize)
            & !(WS_SYSMENU as isize)
            & !(WS_THICKFRAME as isize)
            & !(WS_MINIMIZEBOX as isize)
            & !(WS_MAXIMIZEBOX as isize))
            | WS_POPUP as isize;
        SetWindowLongPtrW(hwnd, GWL_STYLE, desired_style);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

pub fn configure_action_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("action window handle is not available".to_owned());
    }
    dress_icon(hwnd);
    // An action window must accept mouse clicks without becoming foreground.
    // Keeping the editor foreground preserves both its selection and the
    // immutable target captured before the palette was shown.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_style =
            (current_style & !(WS_EX_APPWINDOW as isize) & !(WS_EX_TRANSPARENT as isize))
                | WS_EX_NOACTIVATE as isize
                | WS_EX_TOOLWINDOW as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style);
        apply_transient_backdrop(hwnd);
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for action window".to_owned());
        }
    }
    Ok(())
}

pub fn configure_action_window_for_target(
    window_handle: u64,
    target_window_handle: u64,
) -> Result<(), String> {
    configure_action_window(window_handle)?;
    position_action_window(window_handle, target_window_handle)
}

pub fn prepare_action_window(window_handle: u64) -> Result<(), String> {
    configure_action_window(window_handle)?;
    prepare_interactive_window(window_handle)
}

/// Places the translation preview next to the text it is translating.
///
/// The old version put it at the cursor, on whatever monitor the cursor
/// happened to be over. With the text on one screen and the pointer on
/// another — which is where a pointer usually is after a keyboard shortcut —
/// the translation opened on the wrong screen, and to the user it had not
/// opened at all.
///
/// Three places to anchor to, best first:
///
/// 1. the caret in the target application, which is where the user is looking;
/// 2. the target window, which is at least on the right screen;
/// 3. the pointer, which is only right by coincidence.
pub fn configure_translation_window_for_target(
    window_handle: u64,
    target_window_handle: u64,
    target_thread_id: u32,
) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    apply_translation_window_contract(hwnd);

    let anchor = caret_anchor(target_thread_id)
        .or_else(|| window_anchor(target_window_handle))
        .or_else(cursor_anchor);
    let Some(anchor) = anchor else {
        return Ok(());
    };
    place_near(hwnd, anchor)
}

/// Where the caret is on screen, if the target thread has one.
///
/// `GetGUIThreadInfo` gives it in the client coordinates of whichever window
/// owns it, so it has to be mapped out. A thread with no caret — a browser, a
/// canvas — reports an empty rectangle, and that is not an anchor.
fn caret_anchor(thread_id: u32) -> Option<POINT> {
    if thread_id == 0 {
        return None;
    }
    // SAFETY: the struct is initialised with its own size, which is what the
    // call requires, and is only read afterwards.
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..std::mem::zeroed()
        };
        if GetGUIThreadInfo(thread_id, &raw mut info) == 0 {
            return None;
        }
        let caret = info.rcCaret;
        if caret.right <= caret.left && caret.bottom <= caret.top {
            return None;
        }
        let mut point = POINT {
            x: caret.left,
            y: caret.bottom,
        };
        if !info.hwndCaret.is_null() && ClientToScreen(info.hwndCaret, &raw mut point) == 0 {
            return None;
        }
        Some(point)
    }
}

/// The top-left of the target window, which is at least on the right screen.
fn window_anchor(target_window_handle: u64) -> Option<POINT> {
    if target_window_handle == 0 {
        return None;
    }
    let target = target_window_handle as usize as HWND;
    // SAFETY: the rectangle is written by the call and read after it succeeds.
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(target, &mut rect) == 0 {
            return None;
        }
        Some(POINT {
            x: rect.left + 24,
            y: rect.top + 24,
        })
    }
}

fn cursor_anchor() -> Option<POINT> {
    // SAFETY: the point is written by the call.
    unsafe {
        let mut cursor = POINT::default();
        (GetCursorPos(&mut cursor) != 0).then_some(cursor)
    }
}

/// Puts the window just below and right of an anchor, kept inside the work area
/// of whichever monitor the anchor is on.
fn place_near(hwnd: HWND, anchor: POINT) -> Result<(), String> {
    // SAFETY: every buffer is initialised before use and the handles belong to
    // this process.
    unsafe {
        let monitor = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Ok(());
        }
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let mut window_rect = RECT::default();
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Ok(());
        }
        let (x, y) = offer_position(
            anchor,
            window_rect.right - window_rect.left,
            window_rect.bottom - window_rect.top,
            monitor_info.rcWork,
        );
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for translation window".to_owned());
        }
    }
    Ok(())
}

/// Where to put a window of this size next to an anchor, without letting it
/// leave the monitor's work area.
///
/// Separated from the Win32 around it because the arithmetic is the part that
/// goes wrong: a window wider than the work area used to produce a maximum
/// below the minimum, which `clamp` panics on.
fn offer_position(anchor: POINT, width: i32, height: i32, work: RECT) -> (i32, i32) {
    const GAP_X: i32 = 18;
    const GAP_Y: i32 = 22;
    const MARGIN: i32 = 8;

    let left = work.left + MARGIN;
    let top = work.top + MARGIN;
    let x = (anchor.x + GAP_X).clamp(left, (work.right - width - MARGIN).max(left));
    let y = (anchor.y + GAP_Y).clamp(top, (work.bottom - height - MARGIN).max(top));
    (x, y)
}

/// Places the preview with nothing to go on but the pointer.
///
/// Kept for the paths that have no target — the translator window opened from
/// Settings, and the preview modes.
pub fn configure_translation_window(window_handle: u64) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    apply_translation_window_contract(hwnd);
    // SAFETY: hwnd is the live translation preview window owned by this
    // process. Cursor, monitor and rectangle buffers are fully initialized.
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) == 0 {
            return Ok(());
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Ok(());
        }
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let mut window_rect = RECT::default();
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Ok(());
        }
        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        let work = monitor_info.rcWork;
        let x = (cursor.x + 18).clamp(work.left + 8, work.right - width - 8);
        let y = (cursor.y + 22).clamp(work.top + 8, work.bottom - height - 8);
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for translation window".to_owned());
        }
    }
    Ok(())
}

pub fn prepare_translation_window(window_handle: u64) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    apply_translation_window_contract(hwnd);
    prepare_interactive_window(window_handle)
}

fn prepare_interactive_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("interactive overlay window handle is not available".to_owned());
    }
    set_overlay_cloaked(hwnd, true)?;
    unsafe {
        let result = DwmFlush();
        if result < 0 {
            return Err(format!(
                "DwmFlush failed while prewarming interactive overlay: 0x{result:08X}"
            ));
        }
    }
    // SAFETY: The HWND is process-owned and remains app-cloaked while this
    // prewarming position is applied.
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            PREWARM_WINDOW_COORDINATE,
            PREWARM_WINDOW_COORDINATE,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    } == 0
    {
        return Err("SetWindowPos failed while prewarming interactive overlay".to_owned());
    }
    Ok(())
}

fn position_action_window(window_handle: u64, target_window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("action window handle is not available".to_owned());
    }
    let target = if target_window_handle == 0 {
        hwnd
    } else {
        target_window_handle as usize as HWND
    };
    unsafe {
        let monitor = MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Err("Windows could not resolve the action-window monitor".to_owned());
        }
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let mut window_rect = RECT::default();
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Err("Windows could not measure the action window".to_owned());
        }
        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        let work = monitor_info.rcWork;
        let x = work.left + (work.right - work.left - width) / 2;
        let y = work.top + (work.bottom - work.top - height) / 2;
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for action window placement".to_owned());
        }
    }
    Ok(())
}

fn apply_translation_window_contract(hwnd: HWND) {
    // SAFETY: Slint created this process-owned top-level HWND. Resetting the
    // compositor to per-pixel transparency lets the captured, radius-two
    // glass layer be the only material behind the compact card.
    unsafe {
        let current_window_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let native_chrome =
            (WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize;
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            (current_window_style & !native_chrome) | WS_POPUP as isize,
        );
        apply_transparent_composition(hwnd);
    }
}

pub fn enable_translation_live_backdrop(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("translation window handle is not available".to_owned());
    }
    // SAFETY: DWM owns this process-local HWND's composition material. A
    // transient system backdrop is continuously recomposed by Windows while
    // the window moves, unlike a captured bitmap.
    unsafe {
        apply_transient_backdrop(hwnd);
    }
    flush_composition("enabling live translation backdrop")?;
    Ok(())
}

pub fn disable_translation_live_backdrop(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("translation window handle is not available".to_owned());
    }
    apply_translation_window_contract(hwnd);
    flush_composition("restoring captured translation backdrop")
}

#[must_use]
pub fn primary_pointer_button_is_down() -> bool {
    // SAFETY: GetAsyncKeyState does not retain pointers and VK_LBUTTON is a
    // documented virtual-key constant. This query runs outside input hooks.
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
}

fn flush_composition(operation: &str) -> Result<(), String> {
    let result = unsafe { DwmFlush() };
    if result < 0 {
        Err(format!("DwmFlush failed while {operation}: 0x{result:08X}"))
    } else {
        Ok(())
    }
}

pub fn configure_standard_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("standard window handle is not available".to_owned());
    }
    // Settings and the full translator belong to the normal application
    // z-order. Explicitly undo every overlay-only style before presenting them.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_style =
            (current_style & !(WS_EX_NOACTIVATE as isize) & !(WS_EX_TOOLWINDOW as isize))
                | WS_EX_APPWINDOW as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style);
        if SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for standard window".to_owned());
        }
    }
    dress_caption(hwnd);
    dress_icon(hwnd);
    Ok(())
}

/// The resource id `winresource` gives the icon it embeds from `build.rs`.
const APPLICATION_ICON: u16 = 1;

/// Hands the window its own icon at both sizes Windows asks for.
///
/// Slint sets only the small icon, so `WM_GETICON` with `ICON_BIG` answered
/// zero and the taskbar fell back to Explorer's cached copy of the executable's
/// icon — which was the yellow mark from before the redesign and stayed yellow
/// however many times the executable was rebuilt. Setting both sizes from the
/// icon compiled into this binary means the button shows what is actually
/// shipping, with no cache in the way.
///
/// Every window gets it, overlays included. The taskbar groups this process
/// into one button and takes the icon from whichever window it decides
/// represents the group; leaving the overlays without one meant the group could
/// still be represented by a window that had nothing to offer, and the stale
/// copy won again.
///
/// A window keeps the handles it is given here for as long as it lives, and the
/// process owns them until it exits; they are loaded from a resource rather
/// than created, so there is nothing to destroy.
fn dress_icon(hwnd: HWND) {
    for (which, icon) in [
        (ICON_BIG, application_icon(true)),
        (ICON_SMALL, application_icon(false)),
    ] {
        if icon == 0 {
            continue;
        }
        // SAFETY: the window belongs to this process and the handle is one of
        // the two the process owns for its lifetime.
        unsafe {
            SendMessageW(hwnd, WM_SETICON, which as usize, icon);
        }
    }
}

/// The process's own icon at one of the two sizes Windows asks for.
///
/// Loaded once each and kept for as long as the process runs. Every window is
/// handed the same two handles: this used to load a fresh pair on every window
/// configuration, without `LR_SHARED` and without ever calling `DestroyIcon`,
/// so each reconfiguration — and the overlays are reconfigured constantly —
/// leaked two icons.
///
/// Returned as an `isize` because that is what `WM_SETICON` takes and because
/// a raw `HICON` is not `Send`, which a `OnceLock` requires. The handles are
/// only ever passed straight back to Win32.
fn application_icon(large: bool) -> isize {
    static LARGE: OnceLock<isize> = OnceLock::new();
    static SMALL: OnceLock<isize> = OnceLock::new();

    let cell = if large { &LARGE } else { &SMALL };
    *cell.get_or_init(|| {
        let (cx, cy) = if large {
            (SM_CXICON, SM_CYICON)
        } else {
            (SM_CXSMICON, SM_CYSMICON)
        };
        // SAFETY: the module handle is this executable's, the resource id is
        // the one `build.rs` embeds, and the metrics calls take no pointers.
        unsafe {
            let module = GetModuleHandleW(null_mut());
            if module.is_null() {
                return 0;
            }
            LoadImageW(
                module,
                APPLICATION_ICON as usize as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(cx),
                GetSystemMetrics(cy),
                LR_DEFAULTCOLOR,
            ) as isize
        }
    })
}

/// Paints the title bar in the window's own material.
///
/// A window drawn in graphite on warm grey paper, wearing whatever dark or
/// light chrome Windows felt like, is two designs stacked on top of each other.
/// These attributes arrive with Windows 11 build 22000; on anything older DWM
/// refuses them and the caption stays as it was, which is why the results are
/// deliberately not checked.
fn dress_caption(hwnd: HWND) {
    // The canvas, the ink and the hairline the interface itself uses. COLORREF
    // is 0x00BBGGRR, so these read backwards from the hex in the stylesheet.
    const CAPTION: u32 = 0x00C6_C2BF; // #bfc2c6
    const TEXT: u32 = 0x0017_1311; // #111317
    const BORDER: u32 = 0x00A5_A29F; // a shade under the canvas

    // SAFETY: hwnd belongs to this process, and each call passes a u32 by
    // reference with its own size, which is what these attributes expect.
    unsafe {
        for (attribute, value) in [
            (DWMWA_CAPTION_COLOR, CAPTION),
            (DWMWA_TEXT_COLOR, TEXT),
            (DWMWA_BORDER_COLOR, BORDER),
        ] {
            let result = DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&raw const value).cast(),
                size_of::<u32>() as u32,
            );
            if result < 0 {
                eprintln!("caption attribute={attribute} rejected=0x{result:08X}");
            }
        }
        // Light-mode chrome for the buttons DWM draws itself, so the minimise
        // and close glyphs are ink on paper rather than white on paper.
        let dark: i32 = 0;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
    }
}

/// Attributes that arrived with Windows 11 and are not in the `windows-sys`
/// constant list this crate imports.
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;

unsafe fn apply_transient_backdrop(hwnd: HWND) {
    // DWM system backdrops are available from Windows 11 build 22621. These
    // calls are deliberately best-effort: Slint's dark translucent surface is
    // the deterministic fallback when the OS, accessibility settings, battery
    // saver, or remote rendering disables Acrylic.
    let dark_mode: i32 = 1;
    let corner_preference = DWMWCP_ROUND;
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark_mode).cast(),
            size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corner_preference).cast(),
            size_of_val(&corner_preference) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const backdrop).cast(),
            size_of_val(&backdrop) as u32,
        );
    }
}

unsafe fn apply_transparent_composition(hwnd: HWND) {
    let backdrop = DWMSBT_NONE;
    unsafe {
        // Clear a composition attribute that may have been installed by an
        // Acrylic action window. Without this reset Windows keeps painting a
        // second full-size material rectangle behind the transparent Slint
        // surface.
        apply_accent_policy(hwnd, ACCENT_DISABLED, 0, 0);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const backdrop).cast(),
            size_of_val(&backdrop) as u32,
        );

        // Winit enables per-pixel alpha through DWM blur-behind when the
        // transparent surface is first created. A system backdrop applied by a
        // previous visualizer replaces that composition state. Re-apply the
        // empty blur region whenever the shared indicator HWND switches back
        // to Circular Spectrum so transparent pixels reveal the target app
        // instead of the swapchain's black clear color.
        let region = CreateRectRgn(0, 0, -1, -1);
        if !region.is_null() {
            let blur = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                fEnable: 1,
                hRgnBlur: region,
                fTransitionOnMaximized: 0,
            };
            let _ = DwmEnableBlurBehindWindow(hwnd, &raw const blur);
            let _ = DeleteObject(region);
        }
    }
}

unsafe fn apply_accent_policy(
    hwnd: HWND,
    accent_state: i32,
    accent_flags: i32,
    gradient_color: u32,
) {
    let mut policy = AccentPolicy {
        accent_state,
        accent_flags,
        gradient_color,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: (&raw mut policy).cast(),
        size: size_of::<AccentPolicy>(),
    };
    let module_name = "user32.dll\0".encode_utf16().collect::<Vec<_>>();
    unsafe {
        let module = GetModuleHandleW(module_name.as_ptr());
        if module.is_null() {
            return;
        }
        let Some(procedure) =
            GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr().cast())
        else {
            return;
        };
        let set_window_composition_attribute: unsafe extern "system" fn(
            HWND,
            *mut WindowCompositionAttributeData,
        ) -> i32 = std::mem::transmute(procedure);
        let _ = set_window_composition_attribute(hwnd, &raw mut data);
    }
}

pub fn restore_target_if_our_action_window_activated(
    target_window_handle: u64,
) -> Result<(), String> {
    if target_window_handle == 0 {
        return Err("action target is missing".to_owned());
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Err("Windows has no foreground window".to_owned());
    }
    let mut foreground_process = 0;
    unsafe {
        GetWindowThreadProcessId(foreground, &raw mut foreground_process);
    }
    if foreground_process != unsafe { GetCurrentProcessId() } {
        // A different application owns focus, so the user moved away while
        // asynchronous work was running. Never pull focus back in that case.
        return Ok(());
    }
    let target = target_window_handle as usize as HWND;
    if target.is_null() || unsafe { SetForegroundWindow(target) } == 0 {
        return Err("Windows refused to restore the action target".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod backdrop_tests {
    use super::blur_rgba;

    #[test]
    fn blur_spreads_color_and_preserves_opaque_alpha() {
        let mut pixels = vec![0_u8; 5 * 4];
        pixels[2 * 4] = u8::MAX;
        pixels[2 * 4 + 1] = u8::MAX;
        pixels[2 * 4 + 2] = u8::MAX;
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = u8::MAX;
        }

        blur_rgba(&mut pixels, 5, 1, 1);

        assert!(pixels[4] > 0);
        assert!(pixels[2 * 4] < u8::MAX);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX));
    }
}

#[cfg(test)]
mod placement_tests {
    use super::{POINT, RECT, offer_position};

    /// A second monitor to the right of the first, as Windows lays them out.
    fn second_screen() -> RECT {
        RECT {
            left: 3840,
            top: 0,
            right: 7680,
            bottom: 2160,
        }
    }

    #[test]
    fn a_window_is_placed_beside_its_anchor() {
        let anchor = POINT { x: 4000, y: 500 };
        let (x, y) = offer_position(anchor, 400, 200, second_screen());

        assert_eq!((x, y), (4018, 522));
    }

    #[test]
    fn an_anchor_near_the_edge_pulls_the_window_back_onto_the_screen() {
        // The caret at the far right of a monitor would otherwise put the
        // preview half onto the next one, or off the desktop entirely.
        let anchor = POINT { x: 7600, y: 2100 };
        let (x, y) = offer_position(anchor, 400, 200, second_screen());

        assert_eq!(x, 7680 - 400 - 8);
        assert_eq!(y, 2160 - 200 - 8);
    }

    #[test]
    fn a_window_larger_than_the_screen_is_pinned_rather_than_panicking() {
        // `clamp` panics when the maximum is below the minimum, which is what a
        // window wider than the work area produces.
        let anchor = POINT { x: 4000, y: 500 };
        let (x, y) = offer_position(anchor, 9000, 9000, second_screen());

        assert_eq!((x, y), (3848, 8));
    }

    #[test]
    fn an_anchor_on_one_screen_never_places_the_window_on_another() {
        // The failure this replaces: the text was at x=52 on the first monitor
        // and the translation opened at x=6319 on the third, because the
        // placement followed the pointer.
        let first = RECT {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2160,
        };
        let (x, _) = offer_position(POINT { x: 52, y: 400 }, 400, 200, first);

        assert!(
            (first.left..first.right).contains(&x),
            "the window stayed on the monitor its anchor was on"
        );
    }
}
