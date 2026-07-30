use std::time::Duration;

#[derive(Clone, Debug)]
pub struct BlurredBackdrop {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(windows)]
mod job;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod popup_dismiss;
#[cfg(windows)]
mod process_exit;
#[cfg(windows)]
mod session_end;
#[cfg(windows)]
mod single_instance;

#[cfg(windows)]
pub use job::WorkerJob;
#[cfg(windows)]
pub use overlay::{
    begin_window_drag, capture_blurred_overlay_backdrop, configure_action_window,
    configure_blue_glass_overlay_window, configure_overlay_window, configure_standard_window,
    configure_translation_window, configure_transparent_overlay_window,
    restore_target_if_our_action_window_activated,
};
#[cfg(windows)]
pub use popup_dismiss::{PopupDismissMonitor, activate_popup_dismiss, deactivate_popup_dismiss};
#[cfg(windows)]
pub use process_exit::watch_process_exit;
#[cfg(windows)]
pub use session_end::{SessionEndEvent, SessionEndMonitor, session_is_ending};
#[cfg(windows)]
pub use single_instance::SingleInstanceGuard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub exit_code: Option<u32>,
    pub uptime: Duration,
}

#[cfg(windows)]
pub fn configure_safe_dll_search() -> std::io::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LOAD_LIBRARY_SEARCH_USER_DIRS, SetDefaultDllDirectories,
    };

    let flags = LOAD_LIBRARY_SEARCH_APPLICATION_DIR
        | LOAD_LIBRARY_SEARCH_SYSTEM32
        | LOAD_LIBRARY_SEARCH_USER_DIRS;
    if unsafe { SetDefaultDllDirectories(flags) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
pub fn configure_process_error_mode() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };

    // SAFETY: These calls only update the current process error mode. Existing
    // flags are preserved; children inherit the result unless explicitly
    // created with CREATE_DEFAULT_ERROR_MODE.
    unsafe {
        let current = GetErrorMode();
        let _ = SetErrorMode(current | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

#[cfg(not(windows))]
pub fn configure_overlay_window(
    _window_handle: u64,
    _target_window_handle: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_transparent_overlay_window(
    _window_handle: u64,
    _target_window_handle: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_blue_glass_overlay_window(
    _window_handle: u64,
    _target_window_handle: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn capture_blurred_overlay_backdrop(
    _window_handle: u64,
    _blur_radius: u32,
) -> Result<BlurredBackdrop, String> {
    Err("blurred overlay backdrop capture is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn configure_action_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_translation_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn begin_window_drag(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_standard_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn restore_target_if_our_action_window_activated(
    _target_window_handle: u64,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub struct SingleInstanceGuard;

#[cfg(not(windows))]
impl SingleInstanceGuard {
    pub fn acquire(_name: &str) -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(not(windows))]
pub struct WorkerJob;

#[cfg(not(windows))]
impl WorkerJob {
    pub fn new() -> Result<Self, String> {
        Ok(Self)
    }

    pub fn assign(&self, _child: &std::process::Child) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn configure_safe_dll_search() -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_process_error_mode() {}

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndEvent {
    Query,
    Confirmed,
    Cancelled,
}

#[cfg(not(windows))]
pub struct SessionEndMonitor;

#[cfg(not(windows))]
impl SessionEndMonitor {
    pub fn start(_on_event: impl Fn(SessionEndEvent) + Send + 'static) -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(not(windows))]
pub fn session_is_ending() -> bool {
    false
}

#[cfg(not(windows))]
pub fn watch_process_exit(
    _child: &std::process::Child,
    _started_at: std::time::Instant,
    _on_exit: impl FnOnce(ProcessExit) + Send + 'static,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub struct PopupDismissMonitor;

#[cfg(not(windows))]
impl PopupDismissMonitor {
    pub fn start(
        _on_dismiss: impl Fn() + Send + 'static,
        _on_pin_changed: impl Fn(bool) + Send + 'static,
        _on_scroll: impl Fn(i32) + Send + 'static,
    ) -> Result<Self, String> {
        Ok(Self)
    }
}

#[cfg(not(windows))]
pub fn activate_popup_dismiss(_window_handle: u64) {}

#[cfg(not(windows))]
pub fn deactivate_popup_dismiss() {}

#[cfg(all(test, windows))]
mod tests {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };

    use super::configure_process_error_mode;

    #[test]
    fn process_error_mode_preserves_existing_flags_and_suppresses_loader_dialogs() {
        // SAFETY: The original process-wide mode is restored before the test
        // returns. No test in this crate depends on either flag being absent.
        let original = unsafe { GetErrorMode() };
        configure_process_error_mode();
        // SAFETY: This only reads the current process error mode.
        let configured = unsafe { GetErrorMode() };

        assert_eq!(
            configured & (SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX),
            SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX
        );
        assert_eq!(configured & original, original);

        // SAFETY: Restore the exact mode that was active before this test.
        unsafe {
            let _ = SetErrorMode(original);
        }
    }
}
