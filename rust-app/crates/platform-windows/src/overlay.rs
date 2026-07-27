use std::{ffi::c_void, mem::size_of};

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Dwm::{
        DWM_BB_ENABLE, DWM_BLURBEHIND, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND, DwmEnableBlurBehindWindow, DwmExtendFrameIntoClientArea,
        DwmSetWindowAttribute,
    },
    Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
    },
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Threading::GetCurrentProcessId,
    },
    UI::{
        Controls::MARGINS,
        Input::KeyboardAndMouse::ReleaseCapture,
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW,
            GetWindowRect, GetWindowThreadProcessId, HTCAPTION, HWND_NOTOPMOST, HWND_TOPMOST,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageW,
            SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WM_NCLBUTTONDOWN, WS_CAPTION,
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX,
            WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
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
const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;

pub fn configure_overlay_window(
    window_handle: u64,
    target_window_handle: Option<u64>,
) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("overlay window handle is not available".to_owned());
    }

    // SAFETY: All pointers refer to initialized buffers for the duration of
    // each Win32 call. The HWND belongs to this process and was resolved by
    // exact title before subsequent calls.
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_style = (current_style & !(WS_EX_APPWINDOW as isize))
            | WS_EX_NOACTIVATE as isize
            | WS_EX_TOOLWINDOW as isize
            | WS_EX_TRANSPARENT as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_style);
        apply_transient_backdrop(hwnd);

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

pub fn configure_action_window(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("action window handle is not available".to_owned());
    }
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

pub fn configure_translation_window(window_handle: u64) -> Result<(), String> {
    configure_action_window(window_handle)?;
    let hwnd = window_handle as usize as HWND;
    // SAFETY: hwnd is the live translation preview window owned by this
    // process. Cursor, monitor and rectangle buffers are fully initialized.
    unsafe {
        // Slint requests a frameless surface, but DWM can restore native
        // non-client chrome when the backdrop changes the frame. Enforce the
        // contract at Win32 level so no system Close button leaks into the
        // compact translation card.
        let current_window_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let native_chrome =
            (WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize;
        SetWindowLongPtrW(hwnd, GWL_STYLE, current_window_style & !native_chrome);

        // Translation is already complete before this window is shown, so it
        // may become interactive after an explicit click. SWP_NOACTIVATE below
        // still guarantees that merely showing it never steals editor focus.
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            current_style & !(WS_EX_NOACTIVATE as isize),
        );
        apply_translation_backdrop(hwnd);

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

pub fn begin_window_drag(window_handle: u64) -> Result<(), String> {
    let hwnd = window_handle as usize as HWND;
    if hwnd.is_null() {
        return Err("drag window handle is not available".to_owned());
    }
    // SAFETY: ReleaseCapture and WM_NCLBUTTONDOWN are the standard Win32
    // contract for initiating a native caption drag from custom chrome.
    unsafe {
        let _ = ReleaseCapture();
        let _ = SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
    }
    Ok(())
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
    Ok(())
}

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

unsafe fn apply_acrylic_backdrop(hwnd: HWND) {
    // The DWM system backdrop above is the supported Windows 11 path. This
    // composition attribute is a best-effort Windows 10/11 fallback that keeps
    // blur visible for a fully custom, frameless no-activate popup.
    let mut policy = AccentPolicy {
        accent_state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
        accent_flags: 2,
        // ABGR: a dark 35% tint. Slint adds one light glass surface over this,
        // leaving the blurred scene visibly present behind the text.
        gradient_color: 0x58_12_10_0E,
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

unsafe fn apply_translation_backdrop(hwnd: HWND) {
    // configure_action_window establishes the common no-activate action-window
    // contract. Translation uses Acrylic instead of stacking the opaque system
    // transient backdrop underneath it.
    let backdrop = DWMSBT_NONE;
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let blur = DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE,
        fEnable: 1,
        hRgnBlur: core::ptr::null_mut(),
        fTransitionOnMaximized: 0,
    };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const backdrop).cast(),
            size_of_val(&backdrop) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(hwnd, &raw const margins);
        let _ = DwmEnableBlurBehindWindow(hwnd, &raw const blur);
        apply_acrylic_backdrop(hwnd);
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
