use std::time::Duration;
use std::{io, path::Path};

#[derive(Clone, Debug)]
pub struct BlurredBackdrop {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(windows)]
pub mod credentials;
mod job;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod popup_dismiss;
#[cfg(windows)]
mod private_directory;
#[cfg(windows)]
mod process_exit;
#[cfg(windows)]
mod process_resources;
#[cfg(windows)]
pub mod region_selection;
pub mod screen_capture;
#[cfg(windows)]
mod session_end;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod window_snapshot;

#[cfg(windows)]
pub use job::WorkerJob;
#[cfg(windows)]
pub use overlay::{
    capture_blurred_cloaked_overlay_backdrop, capture_blurred_overlay_backdrop,
    cloak_overlay_window, conceal_overlay_window, configure_action_window,
    configure_action_window_for_target, configure_blue_glass_overlay_window,
    configure_overlay_window, configure_standard_window, configure_translation_window,
    configure_translation_window_for_region, configure_translation_window_for_target,
    configure_transparent_overlay_window, disable_translation_live_backdrop,
    enable_translation_live_backdrop, overlay_window_is_cloaked, prepare_action_window,
    prepare_blue_glass_overlay_window, prepare_overlay_window, prepare_translation_window,
    prepare_transparent_overlay_window, primary_pointer_button_is_down,
    reconcile_interactive_overlay_window_contract, reconcile_overlay_window_contract,
    request_repaint, restore_target_if_our_action_window_activated, reveal_overlay_window,
    verify_interactive_overlay_window_contract, verify_overlay_window_contract,
};
#[cfg(windows)]
pub use popup_dismiss::{
    PopupDismissMonitor, activate_popup_dismiss, deactivate_all_popup_dismiss,
    deactivate_popup_dismiss, set_popup_pinned,
};
#[cfg(windows)]
pub use private_directory::protect_private_directory;
#[cfg(windows)]
pub use process_exit::watch_process_exit;
#[cfg(windows)]
pub use process_resources::{
    ProcessNode, ProcessResourceSampler, ProcessResourceUsage, aggregate_resource_usage,
    cpu_percent_between, descendant_process_ids,
};
#[cfg(windows)]
pub use session_end::{SessionEndEvent, SessionEndMonitor, session_is_ending};
#[cfg(windows)]
pub use single_instance::SingleInstanceGuard;
#[cfg(windows)]
pub use window_snapshot::{WindowIdentity, WindowInventory, WindowSnapshot, WindowSnapshotError};

#[cfg(windows)]
pub fn open_https_url(url: &str) -> io::Result<()> {
    use std::{iter, ptr};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let operation = "open"
        .encode_utf16()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let target = url.encode_utf16().chain(iter::once(0)).collect::<Vec<_>>();
    // SAFETY: both strings are NUL-terminated for the duration of the call;
    // null optional arguments ask the Windows shell to use its defaults.
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err(io::Error::other(format!(
            "Windows shell refused the support page (code {result})"
        )));
    }
    Ok(())
}

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

/// Replaces a durable file without exposing a moment where the primary path is
/// absent. When a primary already exists Windows also writes its former bytes
/// to `backup`, which gives startup recovery one known-good predecessor.
#[cfg(windows)]
pub fn replace_file_atomically(
    primary: &Path,
    replacement: &Path,
    backup: &Path,
) -> io::Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let primary_w = wide(primary);
    let replacement_w = wide(replacement);
    let backup_w = wide(backup);
    // SAFETY: all strings are owned, NUL-terminated and remain alive through
    // the call. The optional exclusion/reserved pointers are intentionally
    // null as required by ReplaceFileW.
    unsafe {
        let result = if primary.exists() {
            ReplaceFileW(
                primary_w.as_ptr(),
                replacement_w.as_ptr(),
                backup_w.as_ptr(),
                REPLACEFILE_WRITE_THROUGH,
                ptr::null(),
                ptr::null(),
            )
        } else {
            MoveFileExW(
                replacement_w.as_ptr(),
                primary_w.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(not(windows))]
pub fn replace_file_atomically(
    primary: &Path,
    replacement: &Path,
    backup: &Path,
) -> io::Result<()> {
    if primary.exists() {
        let _ = std::fs::remove_file(backup);
        std::fs::rename(primary, backup)?;
    }
    std::fs::rename(replacement, primary)
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
pub fn prepare_overlay_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn prepare_transparent_overlay_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn prepare_blue_glass_overlay_window(_window_handle: u64) -> Result<(), String> {
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
pub fn capture_blurred_cloaked_overlay_backdrop(
    _window_handle: u64,
    _blur_radius: u32,
) -> Result<BlurredBackdrop, String> {
    Err("blurred overlay backdrop capture is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn conceal_overlay_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn cloak_overlay_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn reveal_overlay_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn request_repaint(_window_handle: u64) {}

#[cfg(not(windows))]
pub fn verify_overlay_window_contract(
    _window_handle: u64,
    _expected_cloaked: bool,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn verify_interactive_overlay_window_contract(
    _window_handle: u64,
    _expected_cloaked: bool,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn reconcile_overlay_window_contract(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn reconcile_interactive_overlay_window_contract(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_action_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_action_window_for_target(
    _window_handle: u64,
    _target_window_handle: u64,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn prepare_action_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_translation_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_translation_window_for_target(
    _window_handle: u64,
    _target_window_handle: u64,
    _target_thread_id: u32,
    _selection_rect: Option<(i32, i32, i32, i32)>,
    _captured_pointer: Option<(i32, i32)>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_translation_window_for_region(
    _window_handle: u64,
    _region: (i32, i32, i32, i32),
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn prepare_translation_window(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn enable_translation_live_backdrop(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn disable_translation_live_backdrop(_window_handle: u64) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn primary_pointer_button_is_down() -> bool {
    false
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
pub fn deactivate_popup_dismiss(_window_handle: u64) {}

#[cfg(not(windows))]
pub fn deactivate_all_popup_dismiss() {}

#[cfg(not(windows))]
pub fn set_popup_pinned(_window_handle: u64, _pinned: bool) {}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };

    use super::{configure_process_error_mode, replace_file_atomically};

    #[test]
    fn atomic_replace_overwrites_an_existing_backup_without_a_missing_backup_window() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let replacement = directory.path().join("settings.tmp");
        let backup = directory.path().join("settings.previous");
        fs::write(&primary, b"old-primary").unwrap();
        fs::write(&replacement, b"new-primary").unwrap();
        fs::write(&backup, b"older-backup").unwrap();

        replace_file_atomically(&primary, &replacement, &backup).unwrap();

        assert_eq!(fs::read(&primary).unwrap(), b"new-primary");
        assert_eq!(fs::read(&backup).unwrap(), b"old-primary");
        assert!(!replacement.exists());
    }

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

#[cfg(all(test, windows))]
mod process_resource_contract_tests {
    use std::time::Duration;

    use super::{
        ProcessNode, ProcessResourceSampler, ProcessResourceUsage, aggregate_resource_usage,
        cpu_percent_between, descendant_process_ids,
    };

    #[test]
    fn a_feature_row_aggregates_its_worker_and_descendant_runtime() {
        let usage = aggregate_resource_usage(&[
            ProcessResourceUsage {
                cpu_percent: Some(12.5),
                ram_bytes: 300,
                gpu_percent: Some(35.0),
                vram_bytes: Some(700),
            },
            ProcessResourceUsage {
                cpu_percent: Some(7.5),
                ram_bytes: 500,
                gpu_percent: Some(80.0),
                vram_bytes: Some(900),
            },
        ]);

        assert_eq!(usage.cpu_percent, Some(20.0));
        assert_eq!(usage.ram_bytes, 800);
        assert_eq!(usage.gpu_percent, Some(100.0));
        assert_eq!(usage.vram_bytes, Some(1_600));
    }

    #[test]
    fn a_translation_group_includes_the_nested_llama_runtime_only() {
        let processes = [
            ProcessNode {
                pid: 10,
                parent_pid: 1,
            },
            ProcessNode {
                pid: 11,
                parent_pid: 10,
            },
            ProcessNode {
                pid: 12,
                parent_pid: 11,
            },
            ProcessNode {
                pid: 20,
                parent_pid: 1,
            },
        ];

        assert_eq!(descendant_process_ids(10, &processes), vec![10, 11, 12]);
    }

    #[test]
    fn cpu_time_is_normalized_by_wall_time_and_logical_processors() {
        assert_eq!(
            cpu_percent_between(1_000_000, 3_000_000, Duration::from_secs(1), 4),
            Some(5.0)
        );
        assert_eq!(
            cpu_percent_between(3_000_000, 2_000_000, Duration::from_secs(1), 4),
            None
        );
    }

    #[test]
    fn live_sampler_reports_the_current_process_working_set() {
        let mut sampler = ProcessResourceSampler::new();
        let groups = sampler
            .sample_groups(&[std::process::id()])
            .expect("sample current process");

        assert_eq!(groups.len(), 1);
        assert!(groups[0].ram_bytes > 0);
    }
}

/// Today, as the design's topline shows it: "Mon — 8 September".
///
/// From `GetLocalTime` rather than a conversion off the system clock, because
/// a UTC date shown to someone three hours east is yesterday's date for part of
/// every evening, and this line exists to orient a person looking at a list of
/// their own dictations.
#[cfg(windows)]
pub fn local_date_line() -> String {
    // SAFETY: GetLocalTime only writes the SYSTEMTIME it is given.
    let now = unsafe {
        let mut now = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&raw mut now);
        now
    };
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let day = DAYS.get(now.wDayOfWeek as usize).copied().unwrap_or("");
    let month = MONTHS
        .get(now.wMonth.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("");
    format!("{day} — {} {month}", now.wDay)
}

/// Formats the settings header date with Windows locale data for the selected
/// SayFlex interface language. This keeps month and weekday names in step with
/// an in-app locale override instead of always leaking English or following a
/// different system language.
#[cfg(windows)]
pub fn local_date_line_for_locale(locale: &str) -> String {
    use std::{os::windows::ffi::OsStrExt, ptr};

    use windows_sys::Win32::{
        Foundation::SYSTEMTIME, Globalization::GetDateFormatEx,
        System::SystemInformation::GetLocalTime,
    };

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    // SAFETY: GetLocalTime only writes the SYSTEMTIME it is given.
    let now = unsafe {
        let mut now = std::mem::zeroed::<SYSTEMTIME>();
        GetLocalTime(&raw mut now);
        now
    };
    let locale = wide(std::ffi::OsStr::new(locale));
    let format = wide(std::ffi::OsStr::new("ddd '—' d MMMM"));
    // SAFETY: all input strings are NUL-terminated and live through both
    // calls. The first call asks Windows for the required buffer length.
    let required = unsafe {
        GetDateFormatEx(
            locale.as_ptr(),
            0,
            &raw const now,
            format.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null(),
        )
    };
    if required <= 1 {
        return local_date_line();
    }
    let mut output = vec![0u16; required as usize];
    // SAFETY: output has exactly the length returned by the sizing call.
    let written = unsafe {
        GetDateFormatEx(
            locale.as_ptr(),
            0,
            &raw const now,
            format.as_ptr(),
            output.as_mut_ptr(),
            required,
            ptr::null(),
        )
    };
    if written <= 1 {
        return local_date_line();
    }
    String::from_utf16_lossy(&output[..written as usize - 1])
}

/// BCP-47 locale selected for the current Windows user.
#[cfg(windows)]
#[must_use]
pub fn user_locale_name() -> String {
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    let mut buffer = [0u16; LOCALE_NAME_MAX_LENGTH];
    let length = unsafe {
        windows_sys::Win32::Globalization::GetUserDefaultLocaleName(
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    if length <= 1 {
        return "en".to_owned();
    }
    String::from_utf16_lossy(&buffer[..length as usize - 1])
}

#[cfg(not(windows))]
pub mod region_selection {
    use rust_protocol::PhysicalRect;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PointerEvent {
        Down { x: i32, y: i32 },
        Move { x: i32, y: i32 },
        Up { x: i32, y: i32 },
        RightDown { x: i32, y: i32 },
        RightUp { x: i32, y: i32 },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MonitorRegion {
        pub bounds: PhysicalRect,
        pub dpi_scale_milli: u32,
    }

    pub struct RegionSelectionMonitor;

    impl RegionSelectionMonitor {
        pub fn start(_callback: impl Fn(PointerEvent) + Send + 'static) -> Result<Self, String> {
            Err("screen-region pointer tracking is only available on Windows".into())
        }
    }

    pub fn attached_monitor_regions() -> Result<Vec<MonitorRegion>, String> {
        Err("monitor enumeration is only available on Windows".into())
    }

    pub fn place_region_selection_window(
        _window_handle: u64,
        _bounds: PhysicalRect,
    ) -> Result<(), String> {
        Err("screen-region overlays are only available on Windows".into())
    }
}

#[cfg(not(windows))]
pub fn local_date_line() -> String {
    String::new()
}

#[cfg(not(windows))]
pub fn local_date_line_for_locale(_locale: &str) -> String {
    local_date_line()
}

#[cfg(not(windows))]
#[must_use]
pub fn user_locale_name() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "en".to_owned())
}

/// Names this process to the shell.
///
/// The taskbar does not ask a window what it looks like. It resolves an
/// application identity — the AppUserModelID — and looks the icon up against
/// that, in a cache Explorer keeps and does not invalidate when an executable
/// changes. Without an explicit id, Windows derives one from the executable's
/// path, so SayFlex inherited the cache entry written when that path first
/// appeared: the yellow mark from before the redesign, which survived every
/// rebuild, a reinstall, `ie4uinit -show`, the window icon being set at both
/// sizes and the window class icon being set as well.
///
/// An explicit id is a new identity with nothing cached against it, so the
/// shell has to ask. It is also what makes the taskbar group this application's
/// windows together and what a pinned shortcut has to carry to be recognised as
/// the same application — the installer sets the same string on its shortcuts.
///
/// Called before any window exists, because the shell reads it when the first
/// one is created.
pub fn name_this_application_to_the_shell() -> Result<(), String> {
    const APP_ID: &[u16] = &[
        b'S' as u16,
        b'a' as u16,
        b'y' as u16,
        b'F' as u16,
        b'l' as u16,
        b'e' as u16,
        b'x' as u16,
        b'.' as u16,
        b'D' as u16,
        b'e' as u16,
        b's' as u16,
        b'k' as u16,
        b't' as u16,
        b'o' as u16,
        b'p' as u16,
        0,
    ];
    // SAFETY: the string is NUL-terminated UTF-16 with a static lifetime, and
    // the call takes nothing else.
    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(APP_ID.as_ptr())
    };
    if result < 0 {
        // Not fatal: the shell falls back to deriving an id from the
        // executable's path, which is what it did before. It is worth saying,
        // because that fallback is the one with the stale icon in it.
        return Err(format!(
            "the shell refused this application's identity: 0x{result:08X}"
        ));
    }
    Ok(())
}
