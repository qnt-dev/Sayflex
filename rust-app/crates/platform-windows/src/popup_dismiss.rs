use std::{
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowRect, HHOOK, LLMHF_INJECTED,
            MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
            TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
            WM_MBUTTONDOWN, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
        },
    },
};

static ACTIVE_POPUP: AtomicUsize = AtomicUsize::new(0);
static POPUP_PINNED: AtomicBool = AtomicBool::new(false);
static PIN_CLICK_CAPTURED: AtomicBool = AtomicBool::new(false);
static DISMISS_SENDER: OnceLock<mpsc::Sender<DismissSignal>> = OnceLock::new();

enum DismissSignal {
    Dismiss,
    PinChanged(bool),
    Scroll(i32),
    Shutdown,
}

pub struct PopupDismissMonitor {
    thread_id: u32,
    hook_thread: Option<thread::JoinHandle<()>>,
    event_thread: Option<thread::JoinHandle<()>>,
}

impl PopupDismissMonitor {
    pub fn start(
        on_dismiss: impl Fn() + Send + 'static,
        on_pin_changed: impl Fn(bool) + Send + 'static,
        on_scroll: impl Fn(i32) + Send + 'static,
    ) -> Result<Self, String> {
        let (dismiss_sender, dismiss_receiver) = mpsc::channel();
        DISMISS_SENDER
            .set(dismiss_sender)
            .map_err(|_| "popup dismiss monitor was already initialized".to_owned())?;

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let hook_thread = thread::Builder::new()
            .name("translation-popup-mouse-hook".to_owned())
            .spawn(move || run_mouse_hook(ready_sender))
            .map_err(|error| error.to_string())?;
        let thread_id = match ready_receiver.recv() {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = hook_thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = hook_thread.join();
                return Err("popup mouse hook stopped during startup".to_owned());
            }
        };
        let event_thread = match thread::Builder::new()
            .name("translation-popup-dismiss-events".to_owned())
            .spawn(move || {
                while let Ok(signal) = dismiss_receiver.recv() {
                    match signal {
                        DismissSignal::Dismiss => on_dismiss(),
                        DismissSignal::PinChanged(pinned) => on_pin_changed(pinned),
                        DismissSignal::Scroll(delta) => on_scroll(delta),
                        DismissSignal::Shutdown => break,
                    }
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                // SAFETY: thread_id belongs to hook_thread, whose message queue
                // was created before the ready notification.
                unsafe {
                    let _ = PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
                }
                let _ = hook_thread.join();
                return Err(error.to_string());
            }
        };

        Ok(Self {
            thread_id,
            hook_thread: Some(hook_thread),
            event_thread: Some(event_thread),
        })
    }
}

impl Drop for PopupDismissMonitor {
    fn drop(&mut self) {
        deactivate_popup_dismiss();
        // SAFETY: thread_id belongs to the hook thread created by start().
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.hook_thread.take() {
            let _ = thread.join();
        }
        let _ = DISMISS_SENDER
            .get()
            .map(|sender| sender.send(DismissSignal::Shutdown));
        if let Some(thread) = self.event_thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn activate_popup_dismiss(window_handle: u64) {
    POPUP_PINNED.store(false, Ordering::Release);
    ACTIVE_POPUP.store(window_handle as usize, Ordering::Release);
}

pub fn deactivate_popup_dismiss() {
    ACTIVE_POPUP.store(0, Ordering::Release);
    POPUP_PINNED.store(false, Ordering::Release);
    PIN_CLICK_CAPTURED.store(false, Ordering::Release);
}

fn run_mouse_hook(ready: mpsc::SyncSender<Result<u32, String>>) {
    let mut queue_probe = MSG::default();
    // SAFETY: queue_probe is initialized and this call only creates the thread's
    // message queue before another thread may post WM_QUIT.
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    // SAFETY: null asks Windows for the current executable module.
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    // SAFETY: mouse_hook has the required ABI and remains valid for the process.
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) };
    if hook.is_null() {
        let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
        return;
    }
    let _guard = MouseHookGuard(hook);
    // SAFETY: this is the identifier of the current hook thread.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message points to valid storage for the duration of the call.
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        // SAFETY: message was produced by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn mouse_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code >= 0 {
        let active = ACTIVE_POPUP.load(Ordering::Acquire);
        if active != 0 {
            // SAFETY: Windows supplies an MSLLHOOKSTRUCT pointer for WH_MOUSE_LL.
            let event = unsafe { &*(l_param as *const MSLLHOOKSTRUCT) };
            let message = w_param as u32;
            if event.flags & LLMHF_INJECTED == 0
                && message == WM_LBUTTONDOWN
                && point_is_in_pin(active, event.pt)
            {
                let pinned = !POPUP_PINNED.fetch_xor(true, Ordering::AcqRel);
                PIN_CLICK_CAPTURED.store(true, Ordering::Release);
                let _ = DISMISS_SENDER
                    .get()
                    .map(|sender| sender.send(DismissSignal::PinChanged(pinned)));
                return 1;
            }
            if message == WM_LBUTTONUP && PIN_CLICK_CAPTURED.swap(false, Ordering::AcqRel) {
                return 1;
            }
            if event.flags & LLMHF_INJECTED == 0
                && message == WM_MOUSEWHEEL
                && !point_is_outside(active, event.pt)
            {
                let delta = (event.mouseData >> 16) as u16 as i16 as i32;
                let _ = DISMISS_SENDER
                    .get()
                    .map(|sender| sender.send(DismissSignal::Scroll(delta)));
                // The preview owns this gesture. Do not scroll the editor
                // beneath the non-activating popup at the same time.
                return 1;
            }
            if event.flags & LLMHF_INJECTED == 0
                && matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
                && !POPUP_PINNED.load(Ordering::Acquire)
                && point_is_outside(active, event.pt)
                && ACTIVE_POPUP
                    .compare_exchange(active, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let _ = DISMISS_SENDER
                    .get()
                    .map(|sender| sender.send(DismissSignal::Dismiss));
            }
        }
    }
    // SAFETY: forwarding preserves the global mouse hook chain.
    unsafe { CallNextHookEx(ptr::null_mut(), code, w_param, l_param) }
}

fn point_is_in_pin(window_handle: usize, point: POINT) -> bool {
    let mut rect = RECT::default();
    let hwnd = window_handle as *mut core::ffi::c_void;
    // SAFETY: hwnd is the live popup HWND and rect is initialized storage.
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return false;
    }
    // Slint lays the button out in logical pixels. Convert the exact hitbox to
    // physical pixels so the native path remains correct on scaled monitors.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = dpi as f32 / 96.0;
    let left = rect.right - (52.0 * scale).round() as i32;
    let right = rect.right - (8.0 * scale).round() as i32;
    let top = rect.top + (7.0 * scale).round() as i32;
    let bottom = rect.top + (31.0 * scale).round() as i32;
    point.x >= left && point.x < right && point.y >= top && point.y < bottom
}

fn point_is_outside(window_handle: usize, point: POINT) -> bool {
    let mut rect = RECT::default();
    // SAFETY: window_handle is an HWND supplied by the Slint/winit window and
    // rect is valid writable storage.
    if unsafe { GetWindowRect(window_handle as *mut core::ffi::c_void, &mut rect) } == 0 {
        return false;
    }
    point.x < rect.left || point.x >= rect.right || point.y < rect.top || point.y >= rect.bottom
}

struct MouseHookGuard(HHOOK);

impl Drop for MouseHookGuard {
    fn drop(&mut self) {
        // SAFETY: the hook belongs to this guard and is removed exactly once.
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}
