use std::{ptr, thread::JoinHandle};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0},
    System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
        WaitForMultipleObjects,
    },
    UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
};

pub struct SingleInstanceGuard(HANDLE);

impl SingleInstanceGuard {
    /// Takes the per-user instance mutex. When another instance holds it, that
    /// instance is asked to show its window before this one exits: a click on
    /// the shortcut used to do nothing visible while SayFlex sat in the tray.
    pub fn acquire(name: &str) -> Result<Self, String> {
        let wide = wide(name);
        let handle = unsafe { CreateMutexW(ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err("Windows could not create the single-instance mutex".to_owned());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            signal_running_instance(name);
            return Err("SayFlex is already running".to_owned());
        }
        Ok(Self(handle))
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
            self.0 = ptr::null_mut();
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn activation_event_name(instance_name: &str) -> Vec<u16> {
    wide(&format!("{instance_name}.Activate"))
}

fn signal_running_instance(instance_name: &str) {
    let name = activation_event_name(instance_name);
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if event.is_null() {
            return;
        }
        // This process was started by the user and may bring a window to the
        // front; pass that right on, or Windows only flashes the taskbar.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let _ = SetEvent(event);
        let _ = CloseHandle(event);
    }
}

/// Owned event handles moved to the listener thread.
struct ThreadHandles {
    activate: usize,
    stop: usize,
}

/// Runs `on_activate` on its own thread each time a second instance starts.
/// Dropping the listener stops the thread.
pub struct ActivationListener {
    activate: HANDLE,
    stop: HANDLE,
    thread: Option<JoinHandle<()>>,
}

pub fn listen_for_activation(
    instance_name: &str,
    on_activate: impl Fn() + Send + 'static,
) -> Result<ActivationListener, String> {
    let name = activation_event_name(instance_name);
    let (activate, stop) = unsafe {
        // Auto-reset: one wake-up per signal.
        let activate = CreateEventW(ptr::null(), 0, 0, name.as_ptr());
        let stop = CreateEventW(ptr::null(), 1, 0, ptr::null());
        if activate.is_null() || stop.is_null() {
            if !activate.is_null() {
                let _ = CloseHandle(activate);
            }
            if !stop.is_null() {
                let _ = CloseHandle(stop);
            }
            return Err("Windows could not create the activation event".to_owned());
        }
        (activate, stop)
    };
    let handles = ThreadHandles {
        activate: activate as usize,
        stop: stop as usize,
    };
    let thread = std::thread::Builder::new()
        .name("instance-activation".into())
        .spawn(move || {
            let handles = [handles.activate as HANDLE, handles.stop as HANDLE];
            loop {
                let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                if woke == WAIT_OBJECT_0 {
                    on_activate();
                } else {
                    return;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(ActivationListener {
        activate,
        stop,
        thread: Some(thread),
    })
}

impl Drop for ActivationListener {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.activate);
            let _ = CloseHandle(self.stop);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use super::{SingleInstanceGuard, listen_for_activation};

    #[test]
    fn a_second_instance_wakes_the_first_and_is_refused() {
        let name = format!("Local\\SayFlex.Test.{}", std::process::id());
        let _first = SingleInstanceGuard::acquire(&name).unwrap();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let listener = listen_for_activation(&name, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        assert!(SingleInstanceGuard::acquire(&name).is_err());
        let deadline = Instant::now() + Duration::from_secs(5);
        while woken.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        drop(listener);
    }
}
