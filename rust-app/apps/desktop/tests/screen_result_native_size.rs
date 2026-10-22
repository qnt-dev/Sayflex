//! Native regression for the production screenshot window's layout constraints.
//!
//! The testing backend accepts sizes below Slint's minimum, so it cannot catch
//! the 230x35 crop becoming a 380x340 native window. This opt-in test uses real,
//! off-screen Winit windows and synthetic pixels, without starting the app or
//! opening its profile, workers, clipboard, or screenshot store.
//! Run: cargo test -p sayflex-desktop --test screen_result_native_size -- --ignored --nocapture

#![cfg(windows)]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use slint::{
    ComponentHandle, LogicalSize, PhysicalPosition,
    winit_030::{
        WinitWindowAccessor,
        winit::{
            platform::windows::WindowAttributesExtWindows,
            raw_window_handle::{HasWindowHandle, RawWindowHandle},
        },
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, MINMAXINFO, SendMessageW, WM_GETMINMAXINFO,
    },
};

slint::slint! {
    import { ScreenTranslationWindow, ScreenBlockOverlay } from "../ui/screen_translation.slint";
    export { ScreenTranslationWindow, ScreenBlockOverlay }
}

fn native_handle(window: &ScreenTranslationWindow) -> HWND {
    window
        .window()
        .with_winit_window(|native| match native.window_handle().unwrap().as_raw() {
            RawWindowHandle::Win32(handle) => handle.hwnd.get() as HWND,
            _ => panic!("expected a Windows result surface"),
        })
        .expect("Winit must create a native window")
}

fn check_size(
    errors: &RefCell<Vec<String>>,
    name: &str,
    window: &ScreenTranslationWindow,
    expected: (f32, f32),
) {
    let scale = window.window().scale_factor();
    let hwnd = native_handle(window);
    for (kind, measure) in [
        (
            "client",
            GetClientRect as unsafe extern "system" fn(HWND, *mut RECT) -> i32,
        ),
        (
            "outer",
            GetWindowRect as unsafe extern "system" fn(HWND, *mut RECT) -> i32,
        ),
    ] {
        let mut rect = RECT::default();
        assert_ne!(unsafe { measure(hwnd, &mut rect) }, 0);
        let actual = (
            (rect.right - rect.left) as f32 / scale,
            (rect.bottom - rect.top) as f32 / scale,
        );
        eprintln!("{name}: {kind}={actual:?}, expected={expected:?}, scale={scale}");
        if (actual.0 - expected.0).abs() > 1.0 || (actual.1 - expected.1).abs() > 1.0 {
            errors.borrow_mut().push(format!(
                "{name}: native {kind} size {actual:?}, expected {expected:?} logical pixels"
            ));
        }
    }
    let expected_viewport = expected.1 - 38.0 - window.get_screen_notice_height();
    if (window.get_screen_visible_height() - expected_viewport).abs() > 1.0 {
        errors.borrow_mut().push(format!(
            "{name}: viewport={}, expected={expected_viewport}",
            window.get_screen_visible_height()
        ));
    }
}

fn check_minimum_height(
    errors: &RefCell<Vec<String>>,
    window: &ScreenTranslationWindow,
    expected: f32,
) {
    let mut constraints = MINMAXINFO::default();
    unsafe {
        SendMessageW(
            native_handle(window),
            WM_GETMINMAXINFO,
            0,
            &mut constraints as *mut MINMAXINFO as isize,
        );
    }
    let actual = constraints.ptMinTrackSize.y as f32 / window.window().scale_factor();
    if (actual - expected).abs() > 1.0 {
        errors.borrow_mut().push(format!(
            "native minimum height={actual}, expected={expected} logical pixels"
        ));
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop and native Winit renderer"]
fn native_screen_result_fits_small_crops_and_preserves_resize_and_pin() {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(|attributes| {
            attributes.with_active(false).with_skip_taskbar(true)
        })
        .select()
        .unwrap();

    // Expected heights are hand-derived: crop viewport + 38px toolbar + the
    // optional 52px notice. DUAL and unplaceable rows must not add blank space.
    let cases = [
        ("tiny", 230.0, 35.0, "", 3, false, (380.0, 73.0)),
        (
            "notice",
            230.0,
            35.0,
            "Translation unavailable",
            3,
            false,
            (380.0, 125.0),
        ),
        ("dual", 230.0, 35.0, "", 2, false, (380.0, 73.0)),
        ("unplaceable", 230.0, 35.0, "", 3, true, (380.0, 73.0)),
        ("medium", 640.0, 240.0, "", 1, false, (640.0, 278.0)),
        ("large", 960.0, 576.0, "", 3, false, (960.0, 614.0)),
    ];
    let windows = cases
        .iter()
        .map(|(_, width, height, notice, mode, unplaceable, _)| {
            let window = ScreenTranslationWindow::new().unwrap();
            window.set_screen_content_width(*width);
            window.set_screen_content_height(*height);
            window.set_screen_image_content_width(*width);
            window.set_screen_image_content_height(*height);
            window.set_screen_total_content_height(*height);
            window.set_screen_view_mode(*mode);
            window.set_screen_translation_notice((*notice).into());
            let pixels =
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(*width as u32, *height as u32);
            window.set_screen_crop_image(slint::Image::from_rgba8(pixels));
            if *unplaceable {
                window.set_screen_blocks(slint::ModelRc::new(slint::VecModel::from(vec![
                    ScreenBlockOverlay {
                        source: "Source row".into(),
                        translation: "A translation that cannot fit inside this crop".into(),
                        not_placeable: true,
                        source_width: 230.0,
                        source_height: 35.0,
                        ..Default::default()
                    },
                ])));
            }
            // Match the production present_screen_result request. The native
            // backend must still apply the actual component's min constraints.
            window.window().set_size(LogicalSize::new(
                *width,
                *height + 38.0 + window.get_screen_notice_height(),
            ));
            window
                .window()
                .set_position(PhysicalPosition::new(-32_000, -32_000));
            window.show().unwrap();
            window
        })
        .collect::<Vec<_>>();
    let windows = Rc::new(windows);
    let errors = Rc::new(RefCell::new(Vec::new()));
    let observed_errors = errors.clone();
    let observed_windows = windows.clone();
    let phase = Cell::new(0);
    let original_handle = Cell::new(std::ptr::null_mut());
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let small = &observed_windows[0];
            let notice = &observed_windows[1];
            let large = &observed_windows[5];
            match phase.get() {
                0 => {
                    for (case, window) in cases.iter().zip(observed_windows.iter()) {
                        check_size(&observed_errors, case.0, window, case.6);
                    }
                    check_minimum_height(&observed_errors, small, 73.0);
                    check_minimum_height(&observed_errors, large, 340.0);
                    original_handle.set(native_handle(small));
                    small.window().set_size(LogicalSize::new(640.0, 420.0));
                    large.window().set_size(LogicalSize::new(640.0, 400.0));
                    rust_platform_windows::set_screen_result_window_pinned(
                        original_handle.get() as usize as u64,
                        true,
                    )
                    .unwrap();
                    small.set_pinned(true);
                }
                1 => {
                    check_size(
                        &observed_errors,
                        "small resized and pinned",
                        small,
                        (640.0, 420.0),
                    );
                    check_size(&observed_errors, "large resized", large, (640.0, 400.0));
                    assert_eq!(native_handle(small), original_handle.get());
                    rust_platform_windows::set_screen_result_window_pinned(
                        original_handle.get() as usize as u64,
                        false,
                    )
                    .unwrap();
                    small.set_pinned(false);
                    check_size(&observed_errors, "small unpinned", small, (640.0, 420.0));
                    small.window().set_size(LogicalSize::new(380.0, 73.0));
                }
                2 => {
                    check_size(&observed_errors, "small restored", small, (380.0, 73.0));
                    small.set_screen_editor_visible(true);
                    notice.set_screen_editor_visible(true);
                }
                3 => {
                    check_size(&observed_errors, "small editor open", small, (380.0, 340.0));
                    check_size(
                        &observed_errors,
                        "notice editor open",
                        notice,
                        (380.0, 392.0),
                    );
                    assert_eq!(native_handle(small), original_handle.get());
                    small.set_screen_editor_visible(false);
                    notice.set_screen_editor_visible(false);
                }
                4 => {
                    check_size(
                        &observed_errors,
                        "small editor closed",
                        small,
                        (380.0, 73.0),
                    );
                    check_size(
                        &observed_errors,
                        "notice editor closed",
                        notice,
                        (380.0, 125.0),
                    );
                    small.window().set_size(LogicalSize::new(640.0, 420.0));
                }
                5 => {
                    check_size(
                        &observed_errors,
                        "manual size before editor",
                        small,
                        (640.0, 420.0),
                    );
                    small.set_screen_editor_visible(true);
                }
                6 => {
                    check_size(
                        &observed_errors,
                        "manual size editor open",
                        small,
                        (640.0, 420.0),
                    );
                    small.window().set_size(LogicalSize::new(640.0, 450.0));
                }
                7 => {
                    check_size(
                        &observed_errors,
                        "resized while editor open",
                        small,
                        (640.0, 450.0),
                    );
                    small.set_screen_editor_visible(false);
                }
                8 => {
                    check_size(
                        &observed_errors,
                        "resized editor closed",
                        small,
                        (640.0, 450.0),
                    );
                    // A user edit of the last failed block clears its notice
                    // during snapshot refresh, before the editor is closed.
                    notice.set_screen_translation_notice("".into());
                    notice.set_screen_failed_blocks(1);
                    small.set_screen_failed_blocks(1);
                    notice.set_screen_editor_visible(true);
                    small.set_screen_editor_visible(true);
                }
                9 => {
                    check_size(
                        &observed_errors,
                        "failed block editor open",
                        notice,
                        (380.0, 392.0),
                    );
                    check_size(
                        &observed_errors,
                        "manual failed block editor open",
                        small,
                        (640.0, 450.0),
                    );
                    notice.set_screen_failed_blocks(0);
                    small.set_screen_failed_blocks(0);
                }
                10 => {
                    assert_eq!(notice.get_screen_notice_height(), 0.0);
                    assert!(notice.get_screen_editor_visible());
                    notice.set_screen_editor_visible(false);
                    small.set_screen_editor_visible(false);
                }
                11 => {
                    check_size(
                        &observed_errors,
                        "last failed block fixed",
                        notice,
                        (380.0, 73.0),
                    );
                    check_size(
                        &observed_errors,
                        "manual size after last failed block fixed",
                        small,
                        (640.0, 450.0),
                    );
                    for window in observed_windows.iter() {
                        rust_platform_windows::clear_screen_result_window_mode(
                            native_handle(window) as usize as u64,
                        );
                        window.hide().unwrap();
                    }
                    slint::quit_event_loop().unwrap();
                }
                _ => unreachable!(),
            }
            phase.set(phase.get() + 1);
        },
    );
    slint::run_event_loop().unwrap();
    assert!(errors.borrow().is_empty(), "{}", errors.borrow().join("\n"));
}
