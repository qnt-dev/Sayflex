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

use std::{ffi::c_void, ptr, time::Duration};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_FAILED, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetMonitorInfoW, HBITMAP, HDC, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        ReleaseDC, SelectObject,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
        GetSystemMetrics, HWND_TOPMOST, IsWindow, MSG, MWMO_INPUTAVAILABLE,
        MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT,
        RegisterClassExW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_DESTROY,
        WM_QUIT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_POPUP,
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
