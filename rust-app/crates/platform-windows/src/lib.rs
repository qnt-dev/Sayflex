#[cfg(windows)]
mod job;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod single_instance;

#[cfg(windows)]
pub use job::WorkerJob;
#[cfg(windows)]
pub use overlay::{
    configure_action_window, configure_overlay_window,
    restore_target_if_our_action_window_activated,
};
#[cfg(windows)]
pub use single_instance::SingleInstanceGuard;

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

#[cfg(not(windows))]
pub fn configure_overlay_window(
    _window_handle: u64,
    _target_window_handle: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_action_window(_window_handle: u64) -> Result<(), String> {
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
