use std::mem::size_of;

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::{
        DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
    },
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowThreadProcessId, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    },
};

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

unsafe fn apply_transient_backdrop(hwnd: HWND) {
    // DWM system backdrops are available from Windows 11 build 22621. These
    // calls are deliberately best-effort: Slint's dark translucent surface is
    // the deterministic fallback when the OS, accessibility settings, battery
    // saver, or remote rendering disables Acrylic.
    let dark_mode: i32 = 1;
    let corner_preference = DWMWCP_ROUNDSMALL;
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
