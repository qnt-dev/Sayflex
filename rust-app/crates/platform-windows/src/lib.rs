#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod single_instance;

#[cfg(windows)]
pub use overlay::{
    configure_action_window, configure_overlay_window,
    restore_target_if_our_action_window_activated,
};
#[cfg(windows)]
pub use single_instance::SingleInstanceGuard;

#[cfg(not(windows))]
pub fn configure_overlay_window(
    _window_title: &str,
    _target_window_handle: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn configure_action_window(_window_title: &str) -> Result<(), String> {
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
