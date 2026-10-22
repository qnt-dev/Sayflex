use std::{
    collections::HashMap,
    ffi::c_void,
    mem::size_of,
    ptr::null_mut,
    slice,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

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
        HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::{
            GCLP_HICON, GCLP_HICONSM, GUITHREADINFO, GWL_EXSTYLE, GWL_STYLE, GetCursorPos,
            GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics, GetWindowLongPtrW,
            GetWindowRect, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, ICON_BIG,
            ICON_SMALL, IMAGE_ICON, IsIconic, IsWindowVisible, LR_DEFAULTCOLOR, LoadImageW,
            SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SW_HIDE, SW_RESTORE, SW_SHOWNOACTIVATE,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
            SetClassLongPtrW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            WM_SETICON, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
            WS_THICKFRAME,
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

/// Makes a cloaked surface eligible for WM_PAINT without revealing it.
///
/// A reused preview can lose WS_VISIBLE while its DWM app cloak stays set.
/// Invalidating that hidden HWND cannot produce the frame required by the
/// presenter. Repairing visibility only in `reveal_overlay_window` is too late:
/// the frame gate prevents that function from ever being reached.
pub fn prepare_overlay_frame(window_handle: u64) -> Result<bool, String> {
    if !overlay_window_is_app_cloaked(window_handle)? {
        return Err("overlay frame preparation requires an app-cloaked window".to_owned());
    }
    let hwnd = window_handle as usize as HWND;
    // SAFETY: DwmGetWindowAttribute above validated the live HWND. Keep the
    // app cloak in place and restore native visibility without activation.
    let restored = unsafe {
        let hidden = IsWindowVisible(hwnd) == 0;
        if hidden {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        if IsWindowVisible(hwnd) == 0 {
            return Err("overlay remained hidden while preparing its frame".to_owned());
        }
        hidden
    };
    if !overlay_window_is_app_cloaked(window_handle)? {
        return Err("overlay lost its app cloak while preparing its frame".to_owned());
    }
    Ok(restored)
}

pub fn reveal_overlay_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }
    // A prepared window is parked outside the virtual desktop until its owner
    // positions it. Uncloaking that HWND is not a successful presentation:
    // the compositor can acknowledge the frame while the user sees nothing.
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err("GetWindowRect failed before revealing overlay".to_owned());
    }
    if rect.left == PREWARM_WINDOW_COORDINATE && rect.top == PREWARM_WINDOW_COORDINATE {
        return Err(format!(
            "overlay is still parked off-screen at ({}, {}) before reveal",
            rect.left, rect.top
        ));
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
    if let Some(pinned) = screen_result_window_mode(window_handle) {
        return verify_screen_result_window_contract(window_handle, expected_cloaked, pinned);
    }
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
    if let Some(pinned) = screen_result_window_mode(window_handle) {
        return apply_screen_result_window_contract(window_handle as usize as HWND, pinned);
    }
    reconcile_overlay_ex_style(window_handle, false)
}

fn screen_result_modes() -> &'static Mutex<HashMap<u64, bool>> {
    static MODES: OnceLock<Mutex<HashMap<u64, bool>>> = OnceLock::new();
    MODES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn screen_result_window_mode(window_handle: u64) -> Option<bool> {
    screen_result_modes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&window_handle)
        .copied()
}

fn remember_screen_result_window_mode(window_handle: u64, pinned: bool) {
    screen_result_modes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(window_handle, pinned);
}

pub fn clear_screen_result_window_mode(window_handle: u64) {
    screen_result_modes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&window_handle);
}

fn verify_screen_result_window_contract(
    window_handle: u64,
    expected_cloaked: bool,
    pinned: bool,
) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    unsafe {
        let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let missing = WS_EX_TOOLWINDOW as isize & !extended_style;
        let forbidden = (WS_EX_APPWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE) as isize;
        let unexpected = extended_style & forbidden;
        let actual_topmost = extended_style & WS_EX_TOPMOST as isize != 0;
        if missing != 0 || unexpected != 0 || actual_topmost != pinned {
            return Err(format!(
                "screen-result ex-style contract mismatch: exstyle=0x{extended_style:X} \
missing=0x{missing:X} unexpected=0x{unexpected:X} expected_topmost={pinned} \
actual_topmost={actual_topmost}"
            ));
        }
    }
    let actual_cloaked = overlay_window_is_app_cloaked(window_handle)?;
    if actual_cloaked != expected_cloaked {
        return Err(format!(
            "screen-result cloak contract mismatch: expected={expected_cloaked} actual={actual_cloaked}"
        ));
    }
    Ok(())
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

/// Whether the compositor is currently hiding this window.
///
/// Public so that a probe can state which case it is measuring rather than
/// assume it: the first attempt at the staging probe called an interval
/// "not cloaked" when `prepare_translation_window` had cloaked it, measured the
/// same case twice, and was reported as proof of something it had not tested.
pub fn overlay_window_is_cloaked(window_handle: u64) -> Result<bool, String> {
    overlay_window_is_app_cloaked(window_handle)
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

/// Turns a translated screenshot into an ordinary, focusable tool window.
///
/// Unlike the compact text popup it must be possible to place this surface
/// behind other applications, drag it, and focus its inline correction field.
/// It remains a tool window so the temporary result does not create a taskbar
/// or Alt+Tab entry. Pinning changes only z-order; it never swaps compositor
/// material or rewrites resizability.
pub fn configure_screen_result_window(window_handle: u64, pinned: bool) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    dress_icon(hwnd);
    apply_screen_result_window_contract(hwnd, pinned)?;
    remember_screen_result_window_mode(window_handle, pinned);
    Ok(())
}

fn apply_screen_result_window_contract(hwnd: HWND, pinned: bool) -> Result<(), String> {
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_style = (current_style
            & !(WS_EX_APPWINDOW as isize)
            & !(WS_EX_TRANSPARENT as isize)
            & !(WS_EX_NOACTIVATE as isize))
            | WS_EX_TOOLWINDOW as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style);
        // Slint's `no-frame` property is implemented by winit's window
        // procedure. Rewriting GWL_STYLE here competes with that owner: winit
        // may restore its caption-capable styles on a later state update,
        // briefly exposing native chrome while this window moves.
        if SetWindowPos(
            hwnd,
            if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for screen-result window".to_owned());
        }
    }
    Ok(())
}

/// Moves the dedicated screen-result HWND entirely through client-owned
/// positioning. This deliberately avoids winit's/native caption drag loop,
/// which can temporarily restore non-client chrome while the pointer is down.
pub fn move_screen_result_window_client(window_handle: u64, x: i32, y: i32) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    if unsafe {
        SetWindowPos(
            hwnd,
            null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } == 0
    {
        return Err("SetWindowPos failed while moving screen-result window".to_owned());
    }
    Ok(())
}

pub fn set_screen_result_window_pinned(window_handle: u64, pinned: bool) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    unsafe {
        if SetWindowPos(
            hwnd,
            if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) == 0
        {
            return Err("SetWindowPos failed while changing screen-result pin".to_owned());
        }
    }
    remember_screen_result_window_mode(window_handle, pinned);
    Ok(())
}

fn borderless_drag_position(
    window_origin: (i32, i32),
    pointer_at_press: (i32, i32),
    pointer_now: (i32, i32),
) -> (i32, i32) {
    (
        window_origin
            .0
            .saturating_add(pointer_now.0.saturating_sub(pointer_at_press.0)),
        window_origin
            .1
            .saturating_add(pointer_now.1.saturating_sub(pointer_at_press.1)),
    )
}

/// Moves a frameless result window without entering Windows' non-client drag
/// loop. `WM_NCLBUTTONDOWN/HTCAPTION` can make DWM paint a temporary system
/// caption over a popup even though the HWND has no `WS_CAPTION`; moving the
/// client-owned popup directly keeps the native and Slint frame contracts in
/// agreement for the whole gesture.
pub fn begin_borderless_window_drag(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("borderless window handle is not available".to_owned());
    }
    let mut pointer_at_press = POINT::default();
    let mut window_rect = RECT::default();
    unsafe {
        if GetCursorPos(&raw mut pointer_at_press) == 0
            || GetWindowRect(hwnd, &raw mut window_rect) == 0
        {
            return Err("Windows could not measure the borderless drag origin".to_owned());
        }
    }

    static ACTIVE_DRAGS: OnceLock<Mutex<HashMap<u64, ()>>> = OnceLock::new();
    let active_drags = ACTIVE_DRAGS.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let mut active = active_drags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.insert(window_handle, ()).is_some() {
            return Ok(());
        }
    }

    let spawn_result = std::thread::Builder::new()
        .name("sayflex-borderless-window-drag".to_owned())
        .spawn(move || {
            let pressed_deadline = Instant::now() + Duration::from_millis(120);
            while !primary_pointer_button_is_down() && Instant::now() < pressed_deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            while primary_pointer_button_is_down() {
                let mut pointer_now = POINT::default();
                if unsafe { GetCursorPos(&raw mut pointer_now) } == 0 {
                    break;
                }
                let (x, y) = borderless_drag_position(
                    (window_rect.left, window_rect.top),
                    (pointer_at_press.x, pointer_at_press.y),
                    (pointer_now.x, pointer_now.y),
                );
                if move_screen_result_window_client(window_handle, x, y).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            ACTIVE_DRAGS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&window_handle);
        });
    if let Err(error) = spawn_result {
        active_drags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&window_handle);
        return Err(format!("could not start borderless window drag: {error}"));
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

/// Places a notice in the lower-right corner of the work area of the monitor
/// with the pointer, where Windows shows its own notifications. It does not
/// cover the text the user is working on and never takes the keyboard focus.
pub fn configure_notice_window(window_handle: u64) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    let anchor = cursor_anchor().unwrap_or_default();
    unsafe {
        let monitor = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let mut window_rect = RECT::default();
        if monitor.is_null()
            || GetMonitorInfoW(monitor, &mut monitor_info) == 0
            || GetWindowRect(hwnd, &mut window_rect) == 0
        {
            return Err("Windows could not measure the notice window".to_owned());
        }
        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        let work = monitor_info.rcWork;
        let margin = 16 * GetDpiForWindow(hwnd).max(96) as i32 / 96;
        let x = (work.right - width - margin).max(work.left);
        let y = (work.bottom - height - margin).max(work.top);
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
            return Err("SetWindowPos failed for notice window placement".to_owned());
        }
    }
    Ok(())
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
/// Five places to anchor to, best first:
///
/// 1. the selected text itself, when UI Automation exposes its rectangles;
/// 2. the caret in the target application, which is where the user is looking;
/// 3. the pointer captured with the translation gesture;
/// 4. the target window, which is at least on the right screen;
/// 5. the current pointer, as a last-resort fallback only.
pub fn configure_translation_window_for_target(
    window_handle: u64,
    target_window_handle: u64,
    target_thread_id: u32,
    selection_rect: Option<(i32, i32, i32, i32)>,
    captured_pointer: Option<(i32, i32)>,
) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    apply_translation_window_contract(hwnd);

    let (anchor, source) = if let Some(anchor) = selection_anchor(selection_rect) {
        (Some(anchor), "selection")
    } else if let Some(anchor) = caret_anchor(target_thread_id) {
        (Some(anchor), "caret")
    } else if let Some((x, y)) = captured_pointer {
        (Some(POINT { x, y }), "captured_pointer")
    } else if let Some(anchor) = window_anchor(target_window_handle) {
        (Some(anchor), "target_window")
    } else {
        (cursor_anchor(), "current_pointer")
    };
    let Some(anchor) = anchor else {
        return Ok(());
    };
    rust_app_core::diagnostics::log_line(&format!(
        "translation_preview placement=anchor source={source} x={} y={}",
        anchor.x, anchor.y
    ));
    place_near(hwnd, anchor)
}

/// Places a screen-translation card beside the physical rectangle selected by
/// the user. Prefer below the selection, flip above when below would leave the
/// work area, and always remain on the monitor that owns the selection.
pub fn configure_translation_window_for_region(
    window_handle: u64,
    region: (i32, i32, i32, i32),
) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    apply_translation_window_contract(hwnd);
    let region = RECT {
        left: region.0,
        top: region.1,
        right: region.2,
        bottom: region.3,
    };
    if region.right <= region.left || region.bottom <= region.top {
        return configure_translation_window(window_handle);
    }
    let anchor = POINT {
        x: region.left.saturating_add((region.right - region.left) / 2),
        y: region.top.saturating_add((region.bottom - region.top) / 2),
    };
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
        let fitted = fit_window_for_region(
            region,
            window_rect.right - window_rect.left,
            window_rect.bottom - window_rect.top,
            monitor_info.rcWork,
        );
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            fitted.x,
            fitted.y,
            fitted.width,
            fitted.height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed for screen-translation window".to_owned());
        }
    }
    Ok(())
}

/// Positions a translated-screenshot result without changing its z-order.
/// The screen-result contract is established before presentation and remains
/// authoritative through prepare, placement and reveal.
pub fn position_screen_result_window_for_region(
    window_handle: u64,
    region: (i32, i32, i32, i32),
) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("screen-result window handle is not available".to_owned());
    }
    let region = RECT {
        left: region.0,
        top: region.1,
        right: region.2,
        bottom: region.3,
    };
    if region.right <= region.left || region.bottom <= region.top {
        return Ok(());
    }
    let anchor = POINT {
        x: region.left.saturating_add((region.right - region.left) / 2),
        y: region.top.saturating_add((region.bottom - region.top) / 2),
    };
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
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
            return Err("GetMonitorInfoW failed while positioning screen-result window".to_owned());
        }
        if GetWindowRect(hwnd, &mut window_rect) == 0 {
            return Err("GetWindowRect failed while positioning screen-result window".to_owned());
        }
        let fitted = fit_window_for_region(
            region,
            window_rect.right - window_rect.left,
            window_rect.bottom - window_rect.top,
            monitor_info.rcWork,
        );
        if SetWindowPos(
            hwnd,
            null_mut(),
            fitted.x,
            fitted.y,
            fitted.width,
            fitted.height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err("SetWindowPos failed while positioning screen-result window".to_owned());
        }
        let mut actual = RECT::default();
        if GetWindowRect(hwnd, &mut actual) == 0 {
            return Err("GetWindowRect failed after positioning screen-result window".to_owned());
        }
        rust_app_core::diagnostics::log_line(&format!(
            "screen_result_window position=applied hwnd={window_handle} \
             anchor=({}, {}, {}, {}) actual=({}, {}, {}, {})",
            region.left,
            region.top,
            region.right,
            region.bottom,
            actual.left,
            actual.top,
            actual.right,
            actual.bottom
        ));
    }
    Ok(())
}

/// Returns the monitor work area in logical pixels for the selected physical
/// screen region. Layout is computed before the result HWND exists, so the
/// monitor is resolved from the frozen capture geometry rather than from a
/// transient window handle.
pub fn screen_result_logical_work_area(region: (i32, i32, u32, u32)) -> Result<(f32, f32), String> {
    let center = POINT {
        x: i64::from(region.0)
            .saturating_add(i64::from(region.2) / 2)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: i64::from(region.1)
            .saturating_add(i64::from(region.3) / 2)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
    };
    unsafe {
        let monitor = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Err("Windows could not resolve the screen-result monitor".to_owned());
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        if GetMonitorInfoW(monitor, &raw mut info) == 0 {
            return Err("Windows could not measure the screen-result work area".to_owned());
        }
        let mut dpi_x = 96_u32;
        let mut dpi_y = 96_u32;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) != 0 {
            dpi_x = 96;
            dpi_y = 96;
        }
        let width =
            (info.rcWork.right - info.rcWork.left).max(1) as f32 * 96.0 / dpi_x.max(96) as f32;
        let height =
            (info.rcWork.bottom - info.rcWork.top).max(1) as f32 * 96.0 / dpi_y.max(96) as f32;
        Ok((width, height))
    }
}

fn selection_anchor(rect: Option<(i32, i32, i32, i32)>) -> Option<POINT> {
    let (left, top, right, bottom) = rect?;
    (right > left && bottom > top).then_some(POINT { x: left, y: bottom })
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
        let fitted = fit_window_near_anchor(
            anchor,
            window_rect.right - window_rect.left,
            window_rect.bottom - window_rect.top,
            monitor_info.rcWork,
        );
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            fitted.x,
            fitted.y,
            fitted.width,
            fitted.height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
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
#[cfg(test)]
fn offer_position(anchor: POINT, width: i32, height: i32, work: RECT) -> (i32, i32) {
    const GAP_X: i32 = 18;
    const GAP_Y: i32 = 22;

    let fitted = fit_window_to_work_area(
        (width, height),
        work,
        POINT {
            x: anchor.x.saturating_add(GAP_X),
            y: anchor.y.saturating_add(GAP_Y),
        },
    );
    (fitted.x, fitted.y)
}

#[cfg(test)]
fn offer_position_for_region(region: RECT, width: i32, height: i32, work: RECT) -> (i32, i32) {
    let fitted = fit_window_for_region(region, width, height, work);
    (fitted.x, fitted.y)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FittedWindow {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn fit_window_near_anchor(anchor: POINT, width: i32, height: i32, work: RECT) -> FittedWindow {
    const GAP_X: i32 = 18;
    const GAP_Y: i32 = 22;

    fit_window_to_work_area(
        (width, height),
        work,
        POINT {
            x: anchor.x.saturating_add(GAP_X),
            y: anchor.y.saturating_add(GAP_Y),
        },
    )
}

fn fit_window_for_region(region: RECT, width: i32, height: i32, work: RECT) -> FittedWindow {
    const GAP: i32 = 12;
    let provisional = fit_window_to_work_area(
        (width, height),
        work,
        POINT {
            x: region.left,
            y: region.bottom.saturating_add(GAP),
        },
    );
    let work_height = work.bottom.saturating_sub(work.top).max(1);
    let margin_y = 8.min(work_height.saturating_sub(1) / 2);
    let min_y = work.top.saturating_add(margin_y);
    let max_y = work
        .bottom
        .saturating_sub(provisional.height)
        .saturating_sub(8)
        .max(min_y);
    let below = region.bottom.saturating_add(GAP);
    let above = region
        .top
        .saturating_sub(GAP)
        .saturating_sub(provisional.height);
    let y = if below <= max_y {
        below.max(min_y)
    } else if above >= min_y {
        above.min(max_y)
    } else {
        below.clamp(min_y, max_y)
    };
    fit_window_to_work_area((width, height), work, POINT { x: region.left, y })
}

fn fit_window_to_work_area(
    window_size: (i32, i32),
    work: RECT,
    desired_origin: POINT,
) -> FittedWindow {
    let work_width = work.right.saturating_sub(work.left).max(1);
    let work_height = work.bottom.saturating_sub(work.top).max(1);
    let right = work.left.saturating_add(work_width);
    let bottom = work.top.saturating_add(work_height);
    let margin_x = 8.min(work_width.saturating_sub(1) / 2);
    let margin_y = 8.min(work_height.saturating_sub(1) / 2);
    let left = work.left.saturating_add(margin_x);
    let top = work.top.saturating_add(margin_y);
    let inner_right = right.saturating_sub(margin_x);
    let inner_bottom = bottom.saturating_sub(margin_y);
    let available_width = inner_right.saturating_sub(left).max(1);
    let available_height = inner_bottom.saturating_sub(top).max(1);
    let width = window_size.0.max(1).min(available_width);
    let height = window_size.1.max(1).min(available_height);
    let max_x = inner_right.saturating_sub(width).max(left);
    let max_y = inner_bottom.saturating_sub(height).max(top);

    FittedWindow {
        x: desired_origin.x.clamp(left, max_x),
        y: desired_origin.y.clamp(top, max_y),
        width,
        height,
    }
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
        let fitted = fit_window_near_anchor(
            cursor,
            window_rect.right - window_rect.left,
            window_rect.bottom - window_rect.top,
            monitor_info.rcWork,
        );
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            fitted.x,
            fitted.y,
            fitted.width,
            fitted.height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
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

/// Restores and brings a window of this process to the front. Windows allows
/// it when the user's last input went to this process, or when the process
/// that received it passed the right on (`AllowSetForegroundWindow`).
pub fn activate_window(window_handle: u64) {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
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
    let large = application_icon(true);
    let small = application_icon(false);

    // The window class as well as the window.
    //
    // The taskbar does not always ask the window. Where a button represents a
    // group rather than one window it resolves the icon from the class, and
    // failing that from the executable — which Explorer keeps a cached copy of,
    // keyed by path. That cache still held the yellow mark from before the
    // redesign, and no number of rebuilds displaced it. Setting the class icon
    // as well leaves nothing for the cache to answer.
    //
    // SAFETY: the window belongs to this process, and both handles are the
    // process-lifetime ones from `application_icon`.
    unsafe {
        if large != 0 {
            SetClassLongPtrW(hwnd, GCLP_HICON, large);
        }
        if small != 0 {
            SetClassLongPtrW(hwnd, GCLP_HICONSM, small);
        }
    }

    for (which, icon) in [(ICON_BIG, large), (ICON_SMALL, small)] {
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
/// Loaded without `LR_SHARED`, so these two handles do belong to this process
/// and would have to be destroyed — except that they are wanted until it exits,
/// and Windows reclaims them then. Two handles for the life of the application
/// is the point: the leak this replaces created a fresh pair on every window
/// configuration, and the overlays are reconfigured constantly.
///
/// Returned as an `isize` because that is what `WM_SETICON` takes and because a
/// raw `HICON` is not `Send`, which a `OnceLock` requires. The handles are only
/// ever passed straight back to Win32.
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
    use super::{
        POINT, RECT, fit_window_to_work_area, offer_position, offer_position_for_region,
        selection_anchor,
    };

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
    fn a_selection_anchors_below_its_last_visible_line() {
        let anchor = selection_anchor(Some((120, 240, 380, 266))).expect("valid selection");
        assert_eq!((anchor.x, anchor.y), (120, 266));
        assert!(selection_anchor(Some((120, 240, 120, 266))).is_none());
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
    fn a_window_larger_than_the_work_area_is_resized_before_placement() {
        let fitted =
            fit_window_to_work_area((9000, 9000), second_screen(), POINT { x: 4000, y: 500 });

        assert_eq!((fitted.x, fitted.y), (3848, 8));
        assert_eq!((fitted.width, fitted.height), (3824, 2144));
    }

    #[test]
    fn even_a_tiny_work_area_has_valid_bounds() {
        let fitted = fit_window_to_work_area(
            (400, 200),
            RECT {
                left: 100,
                top: 200,
                right: 108,
                bottom: 206,
            },
            POINT {
                x: i32::MAX,
                y: i32::MAX,
            },
        );

        assert_eq!((fitted.x, fitted.y), (103, 202));
        assert_eq!((fitted.width, fitted.height), (2, 2));
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

    #[test]
    fn screen_region_prefers_below_and_flips_above_near_the_bottom() {
        let middle = RECT {
            left: 4200,
            top: 400,
            right: 4700,
            bottom: 520,
        };
        assert_eq!(
            offer_position_for_region(middle, 400, 200, second_screen()),
            (4200, 532)
        );

        let bottom = RECT {
            left: 4200,
            top: 2020,
            right: 4700,
            bottom: 2100,
        };
        assert_eq!(
            offer_position_for_region(bottom, 400, 200, second_screen()),
            (4200, 1808)
        );
    }

    #[test]
    fn screen_region_card_is_clamped_inside_its_monitor() {
        let edge = RECT {
            left: 7600,
            top: 5,
            right: 7675,
            bottom: 30,
        };
        assert_eq!(
            offer_position_for_region(edge, 400, 200, second_screen()),
            (7272, 42)
        );
    }
}

#[cfg(all(test, windows))]
mod screen_result_contract_tests {
    use std::ptr::null_mut;

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GWL_EXSTYLE, GWL_STYLE, GetForegroundWindow,
        GetWindowLongPtrW, IsWindowVisible, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
    };

    use super::{
        borderless_drag_position, clear_screen_result_window_mode, conceal_overlay_window,
        configure_screen_result_window, move_screen_result_window_client,
        overlay_window_is_app_cloaked, position_screen_result_window_for_region,
        prepare_overlay_frame, reconcile_interactive_overlay_window_contract,
        reveal_overlay_window, set_screen_result_window_pinned,
    };

    #[test]
    fn borderless_drag_keeps_the_pointer_grab_offset_without_native_caption_dragging() {
        let window_origin = (500, 300);
        let pointer_at_press = (620, 325);
        let pointer_now = (900, 700);

        assert_eq!(
            borderless_drag_position(window_origin, pointer_at_press, pointer_now),
            (780, 675)
        );
    }

    struct TestWindow(windows_sys::Win32::Foundation::HWND);

    impl TestWindow {
        fn create() -> Self {
            let class = "STATIC\0".encode_utf16().collect::<Vec<_>>();
            let title = "SayFlex screen result contract test\0"
                .encode_utf16()
                .collect::<Vec<_>>();
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    320,
                    180,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            assert!(!hwnd.is_null());
            Self(hwnd)
        }

        fn handle(&self) -> u64 {
            self.0 as usize as u64
        }

        fn is_topmost(&self) -> bool {
            unsafe { GetWindowLongPtrW(self.0, GWL_EXSTYLE) & WS_EX_TOPMOST as isize != 0 }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            clear_screen_result_window_mode(self.handle());
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn prepare_position_and_reconcile_never_promote_an_unpinned_result() {
        let window = TestWindow::create();
        configure_screen_result_window(window.handle(), false).unwrap();
        assert!(!window.is_topmost());

        reconcile_interactive_overlay_window_contract(window.handle()).unwrap();
        assert!(!window.is_topmost());

        position_screen_result_window_for_region(window.handle(), (20, 20, 200, 100)).unwrap();
        assert!(!window.is_topmost());

        set_screen_result_window_pinned(window.handle(), true).unwrap();
        assert!(window.is_topmost());
        position_screen_result_window_for_region(window.handle(), (30, 30, 210, 110)).unwrap();
        assert!(window.is_topmost());

        set_screen_result_window_pinned(window.handle(), false).unwrap();
        reconcile_interactive_overlay_window_contract(window.handle()).unwrap();
        assert!(!window.is_topmost());
    }

    #[test]
    fn parked_result_cannot_be_reported_as_revealed() {
        let window = TestWindow::create();
        configure_screen_result_window(window.handle(), false).unwrap();
        conceal_overlay_window(window.handle()).unwrap();

        let error = reveal_overlay_window(window.handle())
            .expect_err("an off-screen parked result must not count as presented");
        assert!(error.contains("parked"), "unexpected error: {error}");
    }

    #[test]
    fn frame_preparation_restores_native_visibility_without_uncloaking_or_activating() {
        let window = TestWindow::create();
        configure_screen_result_window(window.handle(), false).unwrap();
        conceal_overlay_window(window.handle()).unwrap();
        assert_eq!(unsafe { IsWindowVisible(window.0) }, 0);
        let foreground = unsafe { GetForegroundWindow() };

        prepare_overlay_frame(window.handle()).unwrap();

        assert_ne!(
            unsafe { IsWindowVisible(window.0) },
            0,
            "a hidden native window cannot acknowledge the frame needed for its reveal"
        );
        assert!(overlay_window_is_app_cloaked(window.handle()).unwrap());
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }

    #[test]
    fn frame_preparation_refuses_to_show_a_window_without_the_cloak() {
        let window = TestWindow::create();

        assert!(prepare_overlay_frame(window.handle()).is_err());

        assert_eq!(unsafe { IsWindowVisible(window.0) }, 0);
    }

    #[test]
    fn positioning_result_reports_an_invalid_native_window() {
        let error = position_screen_result_window_for_region(u64::MAX, (20, 20, 200, 100))
            .expect_err("an invalid HWND cannot be positioned successfully");
        assert!(error.contains("GetWindowRect"), "unexpected error: {error}");
    }

    #[test]
    fn repeated_client_moves_preserve_the_toolkit_owned_style() {
        let window = TestWindow::create();
        let original = unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) };
        configure_screen_result_window(window.handle(), false).unwrap();

        for (x, y) in [(20, 30), (150, 90), (5, 7), (600, 320)] {
            move_screen_result_window_client(window.handle(), x, y).unwrap();
            let style = unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) };
            assert_eq!(style, original);
        }
    }

    #[test]
    fn configuring_result_preserves_the_toolkit_owned_window_style() {
        let window = TestWindow::create();
        let original = unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) };

        configure_screen_result_window(window.handle(), false).unwrap();
        reconcile_interactive_overlay_window_contract(window.handle()).unwrap();

        assert_eq!(unsafe { GetWindowLongPtrW(window.0, GWL_STYLE) }, original);
    }
}
