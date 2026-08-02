//! Слоёное окно рабочего стола.
//!
//! Окно создаётся напрямую через `CreateWindowExW` и **никогда не скрывается и
//! не показывается заново**. Это осознанное решение: в основном приложении мы
//! уже ловили баг, когда winit при каждом показе восстанавливал свои
//! extended-стили и затирал прозрачность с no-activate. Владея HWND сами и не
//! выполняя транзакций показа, мы исключаем этот класс ошибок.
//!
//! Содержимое отдаётся в `UpdateLayeredWindow`: DWM получает готовый
//! premultiplied BGRA-битмап, своей поверхности рисования у окна нет. Пока
//! кадр не меняется, не выполняется ничего.

#![cfg(windows)]

use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_FAILED, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetMonitorInfoW, HBITMAP, HDC, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        MonitorFromWindow, ReleaseDC, SelectObject,
    },
    System::LibraryLoader::GetModuleHandleW,
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU},
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GetCursorPos, GetMessageW, GetSystemMetrics, HHOOK, HWND_TOPMOST, IsWindow, MSG,
            MSLLHOOKSTRUCT, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
            PM_REMOVE, PeekMessageW, PostQuitMessage, PostThreadMessageW, QS_ALLINPUT,
            RegisterClassExW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage, ULW_ALPHA,
            UnhookWindowsHookEx, UpdateLayeredWindow, WH_MOUSE_LL, WM_DESTROY, WM_LBUTTONDOWN,
            WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

/// Ошибка создания окна. Строковая — на этом уровне выше нечего различать.
pub type Error = String;

/// GDI-ресурсы одного кадра. Новый набор всегда создаётся целиком до замены
/// старого, поэтому неудачный resize не оставляет `Overlay` с уже удалёнными
/// DC/bitmap и не приводит к двойному освобождению в `Drop`.
struct DibSurface {
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous: *mut c_void,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl DibSurface {
    fn new(width: i32, height: i32) -> Result<Self, Error> {
        if width <= 0 || height <= 0 {
            return Err("размер DIB должен быть положительным".to_owned());
        }
        // SAFETY: создаётся совместимый memory DC и 32-битный top-down DIB;
        // каждый успешно созданный ресурс сразу получает владельца.
        unsafe {
            let screen_dc = GetDC(ptr::null_mut());
            if screen_dc.is_null() {
                return Err("GetDC не вернул экранный DC".to_owned());
            }
            let memory_dc = CreateCompatibleDC(screen_dc);
            ReleaseDC(ptr::null_mut(), screen_dc);
            if memory_dc.is_null() {
                return Err("CreateCompatibleDC не создал memory DC".to_owned());
            }

            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut c_void = ptr::null_mut();
            let bitmap = CreateDIBSection(
                memory_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(memory_dc);
                return Err("CreateDIBSection не выделил секцию".to_owned());
            }
            let previous = SelectObject(memory_dc, bitmap.cast());
            if previous.is_null() {
                DeleteObject(bitmap.cast());
                DeleteDC(memory_dc);
                return Err("SelectObject не выбрал DIB в memory DC".to_owned());
            }
            Ok(Self {
                memory_dc,
                bitmap,
                previous,
                bits: bits.cast(),
                width,
                height,
            })
        }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        // SAFETY: ресурсы принадлежат структуре и освобождаются в обратном
        // порядке; bitmap сначала снимается с DC.
        unsafe {
            SelectObject(self.memory_dc, self.previous);
            DeleteObject(self.bitmap.cast());
            DeleteDC(self.memory_dc);
        }
    }
}

pub struct Overlay {
    hwnd: HWND,
    surface: DibSurface,
    position: (i32, i32),
}

impl Overlay {
    /// Создаёт окно фиксированного размера и сразу показывает его без
    /// активации. Дальше окно только перемещается и обновляет содержимое.
    pub fn new(width: u32, height: u32, x: i32, y: i32) -> Result<Self, Error> {
        let width = width as i32;
        let height = height as i32;
        // SAFETY: имя класса и модуль живут дольше вызова; при повторной
        // регистрации того же класса Windows вернёт 0, что здесь не ошибка.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class_name: Vec<u16> = "PetOverlayWindow\0".encode_utf16().collect();
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                // Кисти фона нет намеренно: у окна нет клиентской отрисовки,
                // всё содержимое приходит через UpdateLayeredWindow.
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: ptr::null_mut(),
            };
            RegisterClassExW(&class);

            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT,
                class_name.as_ptr(),
                ptr::null(),
                WS_POPUP,
                x,
                y,
                width,
                height,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW вернул нулевой HWND".to_owned());
        }

        let surface = match DibSurface::new(width, height) {
            Ok(surface) => surface,
            Err(error) => {
                // SAFETY: HWND создан в этом потоке и ещё не показан.
                unsafe { DestroyWindow(hwnd) };
                return Err(error);
            }
        };

        // SAFETY: hwnd валиден; SW_SHOWNOACTIVATE не забирает фокус.
        unsafe {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(Self {
            hwnd,
            surface,
            position: (x, y),
        })
    }

    /// Меняет размер окна и его битмапа.
    ///
    /// Пересоздаётся только DIB-секция; само окно и его extended-стили
    /// остаются нетронутыми, потому что это не показ, а изменение размера.
    /// Именно поэтому смена размера персонажа не может вернуть белый кадр.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        let width = width as i32;
        let height = height as i32;
        if width == self.surface.width && height == self.surface.height {
            return Ok(());
        }
        if width <= 0 || height <= 0 {
            return Err("размер окна должен быть положительным".to_owned());
        }

        // Сначала полностью создаём замену. Старый DIB остаётся рабочим при
        // любой ошибке выделения или изменения размера HWND.
        let replacement = DibSurface::new(width, height)?;

        // SAFETY: hwnd валиден; активация и переупорядочивание запрещены.
        let resized = unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                self.position.0,
                self.position.1,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        };
        if resized == 0 {
            return Err("SetWindowPos не изменил размер окна".to_owned());
        }
        self.surface = replacement;
        Ok(())
    }

    /// Переносит окно. Стили при этом не трогаются — это не показ, а сдвиг.
    pub fn set_position(&mut self, x: i32, y: i32) -> Result<(), Error> {
        if self.position == (x, y) {
            return Ok(());
        }
        // SAFETY: hwnd валиден, флаги запрещают активацию и изменение размера.
        let moved = unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            )
        };
        if moved == 0 {
            return Err("SetWindowPos не переместил окно".to_owned());
        }
        self.position = (x, y);
        Ok(())
    }

    #[must_use]
    pub const fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Отдаёт кадр композитору. `pixels` — premultiplied BGRA, строки сверху
    /// вниз, длина ровно `width * height * 4`.
    pub fn present(&mut self, pixels: &[u8]) -> Result<(), Error> {
        let expected = (self.surface.width * self.surface.height * 4) as usize;
        if pixels.len() != expected {
            return Err(format!(
                "размер кадра {} не совпадает с окном {expected}",
                pixels.len()
            ));
        }
        // SAFETY: `bits` указывает на секцию DIB того же размера, что и окно;
        // области не пересекаются.
        unsafe {
            ptr::copy_nonoverlapping(pixels.as_ptr(), self.surface.bits, expected);
        }

        let size = SIZE {
            cx: self.surface.width,
            cy: self.surface.height,
        };
        let source = POINT { x: 0, y: 0 };
        let destination = POINT {
            x: self.position.0,
            y: self.position.1,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: все указатели ведут на живые локальные значения, DC и
        // битмап принадлежат этому окну.
        let ok = unsafe {
            let screen_dc = GetDC(ptr::null_mut());
            let result = UpdateLayeredWindow(
                self.hwnd,
                screen_dc,
                &destination,
                &size,
                self.surface.memory_dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            );
            ReleaseDC(ptr::null_mut(), screen_dc);
            result
        };
        if ok == 0 {
            return Err("UpdateLayeredWindow отклонил кадр".to_owned());
        }
        Ok(())
    }

    /// Разбирает накопившиеся сообщения. `false` — пришёл запрос на выход.
    pub fn pump(&self) -> bool {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: message — валидная локальная структура.
        unsafe {
            while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                // WM_DESTROY is sent synchronously from DispatchMessage and
                // therefore never appears as a queued message.  The window
                // procedure turns it into the conventional WM_QUIT marker.
                if message.message == WM_QUIT {
                    return false;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        true
    }

    /// Паркует UI-поток до следующего кадра или входящего оконного сообщения.
    /// В отличие от 1-мс polling это не будит ядро сотни раз в секунду в
    /// режиме ожидания, но WM_CLOSE и системные события обрабатываются сразу.
    pub fn wait(&self, timeout: Duration) -> Result<(), Error> {
        let timeout_ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        // SAFETY: массив handles пуст; функция ждёт только очередь сообщений
        // текущего UI-потока и таймаут.
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                ptr::null(),
                timeout_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if result == WAIT_FAILED {
            return Err(format!(
                "MsgWaitForMultipleObjectsEx: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: освобождаем ровно то, что создали, в обратном порядке.
        unsafe {
            // При ошибке рендера цикл может завершиться до WM_CLOSE. В таком
            // случае окно всё ещё принадлежит этому потоку и должно быть
            // уничтожено явно; после обычного WM_CLOSE IsWindow уже вернёт 0.
            if IsWindow(self.hwnd) != 0 {
                DestroyWindow(self.hwnd);
            }
        }
    }
}

static DRAG_HIT_ENABLED: AtomicBool = AtomicBool::new(false);
static DRAG_HIT_LEFT: AtomicI32 = AtomicI32::new(0);
static DRAG_HIT_TOP: AtomicI32 = AtomicI32::new(0);
static DRAG_HIT_RIGHT: AtomicI32 = AtomicI32::new(0);
static DRAG_HIT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static DRAG_CURSOR_X: AtomicI32 = AtomicI32::new(0);
static DRAG_CURSOR_Y: AtomicI32 = AtomicI32::new(0);
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);
static DRAG_STARTED: AtomicBool = AtomicBool::new(false);
static DRAG_ENDED: AtomicBool = AtomicBool::new(false);
static DRAG_REJECTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierDragEvent {
    Started { x: i32, y: i32 },
    Moving { x: i32, y: i32 },
    Ended { x: i32, y: i32 },
    RejectedOutsideHitRect { x: i32, y: i32 },
}

/// A no-focus drag gesture for click-through overlays.
///
/// The low-level hook sleeps in `GetMessageW`; it does no polling while idle.
/// Only Ctrl+Alt+left-click inside the current hit rectangle is swallowed.
/// Every other mouse event keeps flowing to the application below the overlay.
pub struct ModifierDragMonitor {
    thread_id: u32,
    hook_thread: Option<thread::JoinHandle<()>>,
}

impl ModifierDragMonitor {
    pub fn start() -> Result<Self, Error> {
        DRAG_HIT_ENABLED.store(false, Ordering::Release);
        DRAG_ACTIVE.store(false, Ordering::Release);
        DRAG_STARTED.store(false, Ordering::Release);
        DRAG_ENDED.store(false, Ordering::Release);
        DRAG_REJECTED.store(false, Ordering::Release);

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let hook_thread = thread::Builder::new()
            .name("pet-modifier-drag".to_owned())
            .spawn(move || run_modifier_drag_hook(ready_sender))
            .map_err(|error| error.to_string())?;
        let thread_id = match ready_receiver.recv() {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = hook_thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = hook_thread.join();
                return Err("modifier drag hook stopped during startup".to_owned());
            }
        };
        Ok(Self {
            thread_id,
            hook_thread: Some(hook_thread),
        })
    }

    pub fn set_hit_rect(&self, left: i32, top: i32, right: i32, bottom: i32) {
        DRAG_HIT_ENABLED.store(false, Ordering::Release);
        DRAG_HIT_LEFT.store(left, Ordering::Relaxed);
        DRAG_HIT_TOP.store(top, Ordering::Relaxed);
        DRAG_HIT_RIGHT.store(right, Ordering::Relaxed);
        DRAG_HIT_BOTTOM.store(bottom, Ordering::Relaxed);
        DRAG_HIT_ENABLED.store(right > left && bottom > top, Ordering::Release);
    }

    pub fn clear_hit_rect(&self) {
        DRAG_HIT_ENABLED.store(false, Ordering::Release);
    }

    pub fn cancel_drag(&self) {
        DRAG_HIT_ENABLED.store(false, Ordering::Release);
        DRAG_ACTIVE.store(false, Ordering::Release);
        DRAG_STARTED.store(false, Ordering::Release);
        DRAG_ENDED.store(false, Ordering::Release);
        DRAG_REJECTED.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn poll(&self) -> Option<ModifierDragEvent> {
        let x = DRAG_CURSOR_X.load(Ordering::Acquire);
        let y = DRAG_CURSOR_Y.load(Ordering::Acquire);
        if DRAG_STARTED.swap(false, Ordering::AcqRel) {
            return Some(ModifierDragEvent::Started { x, y });
        }
        if DRAG_ENDED.swap(false, Ordering::AcqRel) {
            return Some(ModifierDragEvent::Ended { x, y });
        }
        if DRAG_REJECTED.swap(false, Ordering::AcqRel) {
            return Some(ModifierDragEvent::RejectedOutsideHitRect { x, y });
        }
        DRAG_ACTIVE
            .load(Ordering::Acquire)
            .then_some(ModifierDragEvent::Moving { x, y })
    }
}

impl Drop for ModifierDragMonitor {
    fn drop(&mut self) {
        DRAG_HIT_ENABLED.store(false, Ordering::Release);
        DRAG_ACTIVE.store(false, Ordering::Release);
        // SAFETY: thread_id belongs to the hook thread created in `start`.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.hook_thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_modifier_drag_hook(ready: mpsc::SyncSender<Result<u32, Error>>) {
    let mut queue_probe = MSG::default();
    // SAFETY: this only creates the hook thread's message queue.
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    // SAFETY: null requests the current executable module.
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    // SAFETY: `modifier_drag_hook` has the required ABI and static lifetime.
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(modifier_drag_hook), module, 0) };
    if hook.is_null() {
        let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
        return;
    }
    let _guard = MouseHookGuard(hook);
    // SAFETY: this is the current hook thread.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: message is valid writable storage.
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

unsafe extern "system" fn modifier_drag_hook(
    code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if code >= 0 {
        // SAFETY: Windows supplies an MSLLHOOKSTRUCT for WH_MOUSE_LL.
        let event = unsafe { &*(l_param as *const MSLLHOOKSTRUCT) };
        // Synthetic input is accepted intentionally: accessibility tools and
        // automated UI verification must have the same gesture contract as a
        // physical mouse. This process never injects mouse input itself, so
        // accepting it cannot create a feedback loop.
        let message = w_param as u32;
        let active = DRAG_ACTIVE.load(Ordering::Acquire);
        if message == WM_LBUTTONDOWN && modifiers_are_down() {
            store_drag_cursor(event.pt);
            if point_is_in_drag_hit_rect(event.pt) {
                DRAG_ENDED.store(false, Ordering::Release);
                DRAG_ACTIVE.store(true, Ordering::Release);
                DRAG_STARTED.store(true, Ordering::Release);
                return 1;
            }
            DRAG_REJECTED.store(true, Ordering::Release);
        }
        if active && message == WM_MOUSEMOVE {
            store_drag_cursor(event.pt);
            if !modifiers_are_down() {
                DRAG_ACTIVE.store(false, Ordering::Release);
                DRAG_ENDED.store(true, Ordering::Release);
            }
            // Do not swallow mouse movement. A non-zero hook result prevents
            // Windows from moving the system cursor at all, which makes a
            // correctly captured drag feel physically stuck. The button-down
            // is still swallowed, so the application below receives hover
            // movement but never receives a click or starts its own drag.
        }
        if active && message == WM_LBUTTONUP {
            store_drag_cursor(event.pt);
            DRAG_ACTIVE.store(false, Ordering::Release);
            DRAG_ENDED.store(true, Ordering::Release);
            return 1;
        }
    }
    // SAFETY: forwarding preserves the global mouse hook chain.
    unsafe { CallNextHookEx(ptr::null_mut(), code, w_param, l_param) }
}

fn modifiers_are_down() -> bool {
    // SAFETY: GetAsyncKeyState has no pointer preconditions.
    unsafe {
        GetAsyncKeyState(i32::from(VK_CONTROL)) < 0 && GetAsyncKeyState(i32::from(VK_MENU)) < 0
    }
}

fn point_is_in_drag_hit_rect(point: POINT) -> bool {
    DRAG_HIT_ENABLED.load(Ordering::Acquire)
        && point.x >= DRAG_HIT_LEFT.load(Ordering::Relaxed)
        && point.x < DRAG_HIT_RIGHT.load(Ordering::Relaxed)
        && point.y >= DRAG_HIT_TOP.load(Ordering::Relaxed)
        && point.y < DRAG_HIT_BOTTOM.load(Ordering::Relaxed)
}

fn store_drag_cursor(point: POINT) {
    DRAG_CURSOR_X.store(point.x, Ordering::Release);
    DRAG_CURSOR_Y.store(point.y, Ordering::Release);
}

struct MouseHookGuard(HHOOK);

impl Drop for MouseHookGuard {
    fn drop(&mut self) {
        // SAFETY: the hook belongs to this guard and is removed once.
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if message == WM_DESTROY {
        // SAFETY: окно живёт в том же UI-потоке; WM_QUIT завершает его цикл
        // сообщений и не адресуется никакому конкретному HWND.
        unsafe {
            PostQuitMessage(0);
        }
        return 0;
    }
    // SAFETY: стандартная обработка по умолчанию для окна без клиентской
    // отрисовки.
    unsafe { DefWindowProcW(hwnd, message, w_param, l_param) }
}

/// Позиция курсора в координатах виртуального экрана.
///
/// Опрос дешевле низкоуровневого хука мыши и не подвержен снятию по
/// `LowLevelHooksTimeout`.
#[must_use]
pub fn cursor_position() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: point — валидная локальная структура.
    unsafe {
        GetCursorPos(&mut point);
    }
    (point.x, point.y)
}

/// Размер виртуального экрана — суммарной области всех мониторов.
#[must_use]
pub fn virtual_screen() -> RECT {
    // SAFETY: GetSystemMetrics не имеет предусловий.
    unsafe {
        RECT {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

#[derive(Clone, Copy)]
pub struct MonitorArea {
    pub bounds: RECT,
    pub work: RECT,
}

/// Геометрия монитора под указанной экранной точкой. Рабочая область
/// учитывает панель задач; полные границы позволяют кэшировать результат и не
/// опрашивать user32 на каждом кадре.
#[must_use]
pub fn monitor_area_at(x: i32, y: i32) -> MonitorArea {
    // SAFETY: MonitorFromPoint с DEFAULTTONEAREST всегда возвращает ближайший
    // монитор; MONITORINFO имеет правильный cbSize.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        if monitor.is_null() || GetMonitorInfoW(monitor, &mut info) == 0 {
            let screen = virtual_screen();
            MonitorArea {
                bounds: screen,
                work: screen,
            }
        } else {
            MonitorArea {
                bounds: info.rcMonitor,
                work: info.rcWork,
            }
        }
    }
}

/// Геометрия монитора, на котором находится целевое окно. Функция только
/// читает HWND и не активирует приложение-источник.
#[must_use]
pub fn monitor_area_for_window(window_handle: u64) -> MonitorArea {
    // SAFETY: MonitorFromWindow только читает значение HWND;
    // MONITOR_DEFAULTTONEAREST возвращает ближайший монитор даже для
    // свёрнутого окна.
    unsafe {
        let monitor = MonitorFromWindow(window_handle as usize as HWND, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        if monitor.is_null() || GetMonitorInfoW(monitor, &mut info) == 0 {
            let screen = virtual_screen();
            MonitorArea {
                bounds: screen,
                work: screen,
            }
        } else {
            MonitorArea {
                bounds: info.rcMonitor,
                work: info.rcWork,
            }
        }
    }
}
