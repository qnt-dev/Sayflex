use std::{
    mem::size_of,
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use rust_protocol::PhysicalRect;
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, RECT, TRUE, WPARAM},
    Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GWL_STYLE, GetMessageW, GetWindowLongPtrW, HHOOK,
            HWND_TOPMOST, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW,
            PostThreadMessageW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SetWindowLongPtrW, SetWindowPos,
            SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN,
            WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WS_CAPTION,
            WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
        },
    },
};

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

static MONITOR_ACTIVE: AtomicBool = AtomicBool::new(false);
static POINTER_SENDER: OnceLock<Mutex<Option<mpsc::Sender<PointerEvent>>>> = OnceLock::new();

pub struct RegionSelectionMonitor {
    thread_id: u32,
    hook_thread: Option<thread::JoinHandle<()>>,
    event_thread: Option<thread::JoinHandle<()>>,
}

impl RegionSelectionMonitor {
    pub fn start(callback: impl Fn(PointerEvent) + Send + 'static) -> Result<Self, String> {
        if MONITOR_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("a screen-region pointer monitor is already active".into());
        }
        let mut active_guard = ActiveStartGuard(true);
        let (event_sender, event_receiver) = mpsc::channel();
        *pointer_sender()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(event_sender);

        let event_thread = thread::Builder::new()
            .name("screen-region-pointer-events".into())
            .spawn(move || {
                while let Ok(event) = event_receiver.recv() {
                    callback(event);
                }
            })
            .map_err(|error| error.to_string())?;
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let hook_thread = match thread::Builder::new()
            .name("screen-region-mouse-hook".into())
            .spawn(move || run_mouse_hook(ready_sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                pointer_sender()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let _ = event_thread.join();
                return Err(error.to_string());
            }
        };
        let thread_id = match ready_receiver.recv() {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = hook_thread.join();
                pointer_sender()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let _ = event_thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = hook_thread.join();
                pointer_sender()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let _ = event_thread.join();
                return Err("screen-region mouse hook stopped during startup".into());
            }
        };
        active_guard.0 = false;
        Ok(Self {
            thread_id,
            hook_thread: Some(hook_thread),
            event_thread: Some(event_thread),
        })
    }
}

impl Drop for RegionSelectionMonitor {
    fn drop(&mut self) {
        // SAFETY: thread_id belongs to the live hook thread, whose message
        // queue was created before start() returned.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.hook_thread.take() {
            let _ = thread.join();
        }
        pointer_sender()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(thread) = self.event_thread.take() {
            let _ = thread.join();
        }
        MONITOR_ACTIVE.store(false, Ordering::Release);
    }
}

struct ActiveStartGuard(bool);

impl Drop for ActiveStartGuard {
    fn drop(&mut self) {
        if self.0 {
            pointer_sender()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            MONITOR_ACTIVE.store(false, Ordering::Release);
        }
    }
}

fn pointer_sender() -> &'static Mutex<Option<mpsc::Sender<PointerEvent>>> {
    POINTER_SENDER.get_or_init(|| Mutex::new(None))
}

fn run_mouse_hook(ready: mpsc::SyncSender<Result<u32, String>>) {
    let mut queue_probe = MSG::default();
    // SAFETY: creates this thread's message queue before another thread may
    // post WM_QUIT.
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) };
    if hook.is_null() {
        let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
        return;
    }
    let _guard = MouseHookGuard(hook);
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        return;
    }
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn mouse_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code >= 0 {
        let message = w_param as u32;
        if matches!(
            message,
            WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
        ) {
            let raw = unsafe { &*(l_param as *const MSLLHOOKSTRUCT) };
            if raw.flags & LLMHF_INJECTED == 0 {
                let event = match message {
                    WM_LBUTTONDOWN => PointerEvent::Down {
                        x: raw.pt.x,
                        y: raw.pt.y,
                    },
                    WM_MOUSEMOVE => PointerEvent::Move {
                        x: raw.pt.x,
                        y: raw.pt.y,
                    },
                    WM_LBUTTONUP => PointerEvent::Up {
                        x: raw.pt.x,
                        y: raw.pt.y,
                    },
                    WM_RBUTTONDOWN => PointerEvent::RightDown {
                        x: raw.pt.x,
                        y: raw.pt.y,
                    },
                    WM_RBUTTONUP => PointerEvent::RightUp {
                        x: raw.pt.x,
                        y: raw.pt.y,
                    },
                    _ => unreachable!(),
                };
                let _ = pointer_sender()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .map(|sender| sender.send(event));
                // Selection owns button gestures. Suppressing both edges keeps
                // the application beneath the non-activating overlay unchanged.
                if message != WM_MOUSEMOVE {
                    return 1;
                }
            }
        }
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, w_param, l_param) }
}

struct MouseHookGuard(HHOOK);

impl Drop for MouseHookGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

pub fn attached_monitor_regions() -> Result<Vec<MonitorRegion>, String> {
    let mut monitors: Vec<MonitorRegion> = Vec::new();
    let parameter = (&mut monitors as *mut Vec<MonitorRegion>) as LPARAM;
    let succeeded = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect_monitor),
            parameter,
        )
    };
    if succeeded == 0 {
        return Err(format!(
            "EnumDisplayMonitors failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if monitors.is_empty() {
        return Err("Windows reported no attached monitors".into());
    }
    monitors.sort_by_key(|monitor| {
        (
            monitor.bounds.x,
            monitor.bounds.y,
            monitor.bounds.width,
            monitor.bounds.height,
        )
    });
    Ok(monitors)
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _device_context: HDC,
    _monitor_rect: *mut RECT,
    parameter: LPARAM,
) -> windows_sys::core::BOOL {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return 0;
    }
    let width = info.rcMonitor.right - info.rcMonitor.left;
    let height = info.rcMonitor.bottom - info.rcMonitor.top;
    if width <= 0 || height <= 0 {
        return TRUE;
    }
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    let dpi_scale_milli =
        if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } == 0 {
            dpi_x.saturating_mul(1_000) / 96
        } else {
            1_000
        };
    let monitors = unsafe { &mut *(parameter as *mut Vec<MonitorRegion>) };
    monitors.push(MonitorRegion {
        bounds: PhysicalRect {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            width: width as u32,
            height: height as u32,
        },
        dpi_scale_milli,
    });
    TRUE
}

pub fn place_region_selection_window(
    window_handle: u64,
    bounds: PhysicalRect,
) -> Result<(), String> {
    crate::prepare_transparent_overlay_window(window_handle)?;
    let hwnd = window_handle as usize as *mut core::ffi::c_void;
    let width = i32::try_from(bounds.width).map_err(|_| "selection surface is too wide")?;
    let height = i32::try_from(bounds.height).map_err(|_| "selection surface is too tall")?;
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let style = (current_style
            & !(WS_CAPTION as isize)
            & !(WS_SYSMENU as isize)
            & !(WS_THICKFRAME as isize)
            & !(WS_MINIMIZEBOX as isize)
            & !(WS_MAXIMIZEBOX as isize))
            | WS_POPUP as isize;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style);
        if SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            bounds.x,
            bounds.y,
            width,
            height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
        ) == 0
        {
            return Err(format!(
                "SetWindowPos failed for screen-region overlay: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::{MonitorRegion, place_region_selection_window};
    use rust_protocol::PhysicalRect;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetForegroundWindow, WS_POPUP,
    };

    #[test]
    fn monitor_region_preserves_negative_physical_coordinates_and_dpi_metadata() {
        let region = MonitorRegion {
            bounds: PhysicalRect {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
            dpi_scale_milli: 1500,
        };
        assert_eq!(region.bounds.x, -1920);
        assert_eq!(region.dpi_scale_milli, 1500);
    }

    #[test]
    #[ignore = "requires the interactive Windows compositor"]
    fn live_region_surface_does_not_take_foreground_activation() {
        let class = "STATIC\0".encode_utf16().collect::<Vec<_>>();
        let title = "SayFlex focus probe\0".encode_utf16().collect::<Vec<_>>();
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                -32_000,
                -32_000,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!window.is_null(), "CreateWindowExW failed");
        let _guard = WindowGuard(window);
        let foreground_before = unsafe { GetForegroundWindow() };

        place_region_selection_window(
            window as usize as u64,
            PhysicalRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        )
        .unwrap();
        crate::reveal_overlay_window(window as usize as u64).unwrap();

        assert_eq!(unsafe { GetForegroundWindow() }, foreground_before);
    }

    struct WindowGuard(windows_sys::Win32::Foundation::HWND);

    impl Drop for WindowGuard {
        fn drop(&mut self) {
            let _ = crate::conceal_overlay_window(self.0 as usize as u64);
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
