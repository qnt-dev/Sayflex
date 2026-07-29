use std::{
    mem::size_of,
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
        PostThreadMessageW, RegisterClassExW, TranslateMessage, UnregisterClassW, WM_ENDSESSION,
        WM_QUERYENDSESSION, WM_QUIT, WNDCLASSEXW, WS_OVERLAPPED,
    },
};

static SESSION_ENDING: AtomicBool = AtomicBool::new(false);
static MONITOR_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SESSION_EVENT_SENDER: OnceLock<Mutex<Option<mpsc::Sender<MonitorSignal>>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndEvent {
    Query,
    Confirmed,
    Cancelled,
}

enum MonitorSignal {
    Event(SessionEndEvent),
    Shutdown,
}

pub struct SessionEndMonitor {
    thread_id: u32,
    native_thread: Option<thread::JoinHandle<()>>,
    event_thread: Option<thread::JoinHandle<()>>,
}

impl SessionEndMonitor {
    pub fn start(on_event: impl Fn(SessionEndEvent) + Send + 'static) -> Result<Self, String> {
        let (event_sender, event_receiver) = mpsc::channel();
        let sender_slot = SESSION_EVENT_SENDER.get_or_init(|| Mutex::new(None));
        {
            let mut sender = sender_slot
                .lock()
                .map_err(|_| "session-end sender lock was poisoned".to_owned())?;
            if sender.is_some() {
                return Err("session-end monitor is already running".to_owned());
            }
            *sender = Some(event_sender);
        }

        let event_thread = match thread::Builder::new()
            .name("session-end-events".to_owned())
            .spawn(move || {
                while let Ok(signal) = event_receiver.recv() {
                    match signal {
                        MonitorSignal::Event(event) => on_event(event),
                        MonitorSignal::Shutdown => break,
                    }
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                clear_sender();
                return Err(error.to_string());
            }
        };

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let native_thread = match thread::Builder::new()
            .name("session-end-window".to_owned())
            .spawn(move || run_native_window(ready_sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                send_signal(MonitorSignal::Shutdown);
                clear_sender();
                let _ = event_thread.join();
                return Err(error.to_string());
            }
        };

        let thread_id = match ready_receiver.recv() {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = native_thread.join();
                send_signal(MonitorSignal::Shutdown);
                clear_sender();
                let _ = event_thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = native_thread.join();
                send_signal(MonitorSignal::Shutdown);
                clear_sender();
                let _ = event_thread.join();
                return Err("session-end window stopped during startup".to_owned());
            }
        };

        Ok(Self {
            thread_id,
            native_thread: Some(native_thread),
            event_thread: Some(event_thread),
        })
    }
}

impl Drop for SessionEndMonitor {
    fn drop(&mut self) {
        // SAFETY: thread_id belongs to the native window thread, whose message
        // queue exists before the ready notification.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.native_thread.take() {
            let _ = thread.join();
        }
        send_signal(MonitorSignal::Shutdown);
        clear_sender();
        if let Some(thread) = self.event_thread.take() {
            let _ = thread.join();
        }
        SESSION_ENDING.store(false, Ordering::Release);
    }
}

pub fn session_is_ending() -> bool {
    SESSION_ENDING.load(Ordering::Acquire)
}

fn run_native_window(ready: mpsc::SyncSender<Result<u32, String>>) {
    let class_name = format!("RustVoice.SessionEnd.{}", std::process::id())
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let window_name = [0_u16];
    // SAFETY: null requests the module handle of the current executable.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..WNDCLASSEXW::default()
    };
    // SAFETY: class and its string pointers remain valid for the lifetime of
    // this thread and the registered window class.
    if unsafe { RegisterClassExW(&class) } == 0 {
        let _ = ready.send(Err(format!(
            "RegisterClassExW failed for session-end monitor: {}",
            std::io::Error::last_os_error()
        )));
        return;
    }

    // SAFETY: The registered class uses window_proc and all optional handles
    // are deliberately null. The top-level window is never shown.
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: class_name and instance identify the class registered above.
        unsafe {
            let _ = UnregisterClassW(class_name.as_ptr(), instance);
        }
        let _ = ready.send(Err(format!(
            "CreateWindowExW failed for session-end monitor: {error}"
        )));
        return;
    }

    MONITOR_WINDOW.store(window as usize, Ordering::Release);
    // SAFETY: this is the identifier of the current native window thread.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_ok() {
        let mut message = MSG::default();
        loop {
            // SAFETY: message is writable storage and this thread owns the
            // window/message loop.
            let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            // SAFETY: message was initialized by GetMessageW.
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    MONITOR_WINDOW.store(0, Ordering::Release);
    // SAFETY: window and class are owned by this thread and released once.
    unsafe {
        let _ = DestroyWindow(window);
        let _ = UnregisterClassW(class_name.as_ptr(), instance);
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            SESSION_ENDING.store(true, Ordering::Release);
            send_signal(MonitorSignal::Event(SessionEndEvent::Query));
            1
        }
        WM_ENDSESSION => {
            if w_param != 0 {
                SESSION_ENDING.store(true, Ordering::Release);
                send_signal(MonitorSignal::Event(SessionEndEvent::Confirmed));
            } else {
                SESSION_ENDING.store(false, Ordering::Release);
                send_signal(MonitorSignal::Event(SessionEndEvent::Cancelled));
            }
            0
        }
        _ => {
            // SAFETY: Unhandled messages are forwarded to the system default
            // procedure for this valid top-level window.
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
    }
}

fn send_signal(signal: MonitorSignal) {
    let Some(slot) = SESSION_EVENT_SENDER.get() else {
        return;
    };
    if let Ok(sender) = slot.lock()
        && let Some(sender) = sender.as_ref()
    {
        let _ = sender.send(signal);
    }
}

fn clear_sender() {
    if let Some(slot) = SESSION_EVENT_SENDER.get()
        && let Ok(mut sender) = slot.lock()
    {
        *sender = None;
    }
}

#[cfg(test)]
fn test_window_handle() -> HWND {
    MONITOR_WINDOW.load(Ordering::Acquire) as HWND
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageW, WM_ENDSESSION, WM_QUERYENDSESSION,
    };

    use super::{SessionEndEvent, SessionEndMonitor, session_is_ending};

    #[test]
    fn native_messages_drive_query_cancel_and_confirm_states() {
        let (sender, receiver) = mpsc::channel();
        let monitor = SessionEndMonitor::start(move |event| {
            let _ = sender.send(event);
        })
        .unwrap();

        let window = super::test_window_handle();
        assert!(!window.is_null());
        // SAFETY: Messages target only the private hidden monitor window and
        // do not request a real operating-system shutdown.
        assert_eq!(unsafe { SendMessageW(window, WM_QUERYENDSESSION, 0, 0) }, 1);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            SessionEndEvent::Query
        );
        assert!(session_is_ending());

        // SAFETY: wParam=0 explicitly models a cancelled session end.
        unsafe {
            let _ = SendMessageW(window, WM_ENDSESSION, 0, 0);
        }
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            SessionEndEvent::Cancelled
        );
        assert!(!session_is_ending());

        // SAFETY: This models confirmation inside the private monitor only.
        unsafe {
            let _ = SendMessageW(window, WM_QUERYENDSESSION, 0, 0);
        }
        let _ = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        unsafe {
            let _ = SendMessageW(window, WM_ENDSESSION, 1, 0);
        }
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            SessionEndEvent::Confirmed
        );
        assert!(session_is_ending());

        drop(monitor);
        assert!(!session_is_ending());
    }
}
