use std::{mem, thread};

use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, SendInput, VK_C, VK_CONTROL,
            VK_INSERT, VK_LCONTROL, VK_LMENU, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{
            ES_PASSWORD, GA_ROOT, GUITHREADINFO, GWL_STYLE, GetAncestor, GetClassNameW,
            GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW, GetWindowThreadProcessId,
            SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_COPY,
        },
    },
};

use crate::target::{PasteShortcut, SelectionCopyShortcut};

pub const SYNTHETIC_MARKER: usize = 0x5255_5354_564f_4943;
pub const E2E_HOTKEY_MARKER: usize = 0x514e_5446_4c45_5845;

#[derive(Debug, Error)]
pub enum InjectionError {
    #[error("insertion into a password or protected field is blocked")]
    Protected,
    #[error("the target application is no longer focused")]
    TargetChanged,
    #[error("physical modifier keys are still held")]
    PhysicalModifiersHeld,
    #[error("SendInput accepted {sent} of {expected} events (Win32 {win32})")]
    Partial {
        sent: u32,
        expected: u32,
        win32: u32,
    },
}

const EM_REPLACESEL: u32 = 0x00c2;

/// Inserts text through the synchronous Win32 edit-control contract without
/// touching the system clipboard. Returns `Ok(false)` when the focused target
/// is not a standard Edit/RichEdit control and another adapter should be used.
pub fn insert_native_edit(
    target: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<bool, InjectionError> {
    ensure_injection_safe()?;
    if !is_foreground(target) {
        return Err(InjectionError::TargetChanged);
    }
    let mut information = GUITHREADINFO {
        cbSize: mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } == 0
        || information.hwndFocus.is_null()
    {
        return Ok(false);
    }
    let mut focused_process_id = 0;
    unsafe {
        GetWindowThreadProcessId(information.hwndFocus, &mut focused_process_id);
    }
    if focused_process_id != target.process_id {
        return Ok(false);
    }
    let mut class_name = [0_u16; 128];
    let copied = unsafe {
        GetClassNameW(
            information.hwndFocus,
            class_name.as_mut_ptr(),
            class_name.len() as i32,
        )
    };
    if copied <= 0 {
        return Ok(false);
    }
    let style = unsafe { GetWindowLongPtrW(information.hwndFocus, GWL_STYLE) };
    if !native_edit_supported(
        &String::from_utf16_lossy(&class_name[..copied as usize]),
        style,
    )? {
        return Ok(false);
    }
    let complete = if add_trailing_space {
        format!("{text} ")
    } else {
        text.to_owned()
    };
    let wide = complete.encode_utf16().chain([0]).collect::<Vec<_>>();
    let mut ignored = 0_usize;
    if ensure_target_not_protected(target)? != information.hwndFocus as usize as u64 {
        return Err(InjectionError::TargetChanged);
    }
    // EM_REPLACESEL is a system-defined message below WM_USER, so Windows
    // marshals its UTF-16 pointer across the process boundary. The call returns
    // only after the edit control has processed the replacement.
    let delivered = unsafe {
        SendMessageTimeoutW(
            information.hwndFocus,
            EM_REPLACESEL,
            1,
            wide.as_ptr() as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            250,
            &mut ignored,
        )
    };
    if delivered == 0 {
        return Err(InjectionError::Partial {
            sent: 0,
            expected: 1,
            win32: unsafe { GetLastError() },
        });
    }
    Ok(true)
}

fn is_native_edit_class(class_name: &str) -> bool {
    let normalized = class_name.to_ascii_lowercase();
    normalized == "edit"
        || normalized.contains("richedit")
        || normalized.starts_with("windowsforms10.edit.")
}

fn native_edit_supported(class_name: &str, style: isize) -> Result<bool, InjectionError> {
    if !is_native_edit_class(class_name) {
        return Ok(false);
    }
    if style & ES_PASSWORD as isize != 0 {
        return Err(InjectionError::Protected);
    }
    Ok(true)
}

/// Recheck the current focused native control immediately before delivery.
/// An unknown provider stays eligible; a positive password bit never does.
pub fn ensure_target_not_protected(target: TargetSnapshot) -> Result<u64, InjectionError> {
    if !is_foreground(target) {
        return Err(InjectionError::TargetChanged);
    }
    let mut information = GUITHREADINFO {
        cbSize: mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } == 0
        || information.hwndFocus.is_null()
    {
        return Ok(0);
    }
    let mut class_name = [0_u16; 128];
    let copied = unsafe {
        GetClassNameW(
            information.hwndFocus,
            class_name.as_mut_ptr(),
            class_name.len() as i32,
        )
    };
    if copied > 0 {
        let style = unsafe { GetWindowLongPtrW(information.hwndFocus, GWL_STYLE) };
        native_edit_supported(
            &String::from_utf16_lossy(&class_name[..copied as usize]),
            style,
        )?;
    }
    Ok(information.hwndFocus as usize as u64)
}

pub fn copy_shortcut(shortcut: SelectionCopyShortcut) -> Result<(), InjectionError> {
    match shortcut {
        SelectionCopyShortcut::ControlC => send_control_shortcut(VK_C, false),
        SelectionCopyShortcut::ControlShiftC => send_control_shortcut(VK_C, true),
        SelectionCopyShortcut::ControlInsert => send_control_shortcut(VK_INSERT, false),
    }
}

pub fn paste_shortcut(shortcut: PasteShortcut) -> Result<(), InjectionError> {
    match shortcut {
        PasteShortcut::ControlV => send_control_shortcut(VK_V, false),
        PasteShortcut::ControlShiftV => send_control_shortcut(VK_V, true),
        PasteShortcut::ShiftInsert => send_shift_shortcut(VK_INSERT),
    }
}

pub fn copy_message(target: TargetSnapshot) -> Result<(), InjectionError> {
    if !is_foreground(target) {
        return Err(InjectionError::TargetChanged);
    }
    let mut information = GUITHREADINFO {
        cbSize: mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: information has the required cbSize and target.thread_id came
    // from the captured foreground window.
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } == 0
        || information.hwndFocus.is_null()
    {
        return Err(InjectionError::TargetChanged);
    }
    let mut ignored = 0_usize;
    // SAFETY: hwndFocus is the focused target-thread window returned above.
    // WM_COPY carries no pointer payload, and SendMessageTimeout bounds a hung
    // target to 100 ms.
    let result = unsafe {
        SendMessageTimeoutW(
            information.hwndFocus,
            WM_COPY,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            100,
            &mut ignored,
        )
    };
    if result == 0 {
        // SAFETY: reads the calling thread's last-error value immediately
        // after the failed Win32 call.
        Err(InjectionError::Partial {
            sent: 0,
            expected: 1,
            win32: unsafe { GetLastError() },
        })
    } else {
        Ok(())
    }
}

pub fn is_foreground(target: TargetSnapshot) -> bool {
    if target.window_handle == 0 || target.process_id == 0 {
        return false;
    }
    foreground_window_handle() == target.window_handle
}

pub fn foreground_window_handle() -> u64 {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return 0;
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let actual = if root.is_null() { foreground } else { root };
    actual as usize as u64
}

fn scan_code_key(virtual_key: u16, key_up: bool) -> INPUT {
    marked_scan_code_key(virtual_key, key_up, SYNTHETIC_MARKER)
}

fn marked_scan_code_key(virtual_key: u16, key_up: bool, marker: usize) -> INPUT {
    // SAFETY: MapVirtualKeyW has no pointer parameters. VK-to-scan translation
    // produces the physical metadata required by Raw Input applications.
    let mapped = unsafe { MapVirtualKeyW(u32::from(virtual_key), MAPVK_VK_TO_VSC_EX) };
    let extended = matches!(mapped & 0xff00, 0xe000 | 0xe100);
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: (mapped & 0xff) as u16,
                dwFlags: KEYEVENTF_SCANCODE
                    | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }
                    | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: marker,
            },
        },
    }
}

pub fn send_e2e_hotkey(trigger: u16) -> Result<(), InjectionError> {
    let inputs = [
        marked_scan_code_key(VK_LCONTROL, false, E2E_HOTKEY_MARKER),
        marked_scan_code_key(VK_LMENU, false, E2E_HOTKEY_MARKER),
        marked_scan_code_key(trigger, false, E2E_HOTKEY_MARKER),
        marked_scan_code_key(trigger, true, E2E_HOTKEY_MARKER),
        marked_scan_code_key(VK_LMENU, true, E2E_HOTKEY_MARKER),
        marked_scan_code_key(VK_LCONTROL, true, E2E_HOTKEY_MARKER),
    ];
    send(&inputs)
}

fn send_control_shortcut(key: u16, with_shift: bool) -> Result<(), InjectionError> {
    ensure_injection_safe()?;
    let inputs = if with_shift {
        vec![
            scan_code_key(VK_CONTROL, false),
            scan_code_key(VK_SHIFT, false),
            scan_code_key(key, false),
            scan_code_key(key, true),
            scan_code_key(VK_SHIFT, true),
            scan_code_key(VK_CONTROL, true),
        ]
    } else {
        vec![
            scan_code_key(VK_CONTROL, false),
            scan_code_key(key, false),
            scan_code_key(key, true),
            scan_code_key(VK_CONTROL, true),
        ]
    };
    match send(&inputs) {
        Ok(()) => Ok(()),
        Err(error @ InjectionError::Partial { sent, .. }) => {
            // Recover only events that belong to this exact synthetic batch.
            // Physical modifiers are never released by the application.
            if (1..inputs.len() as u32).contains(&sent) {
                let mut owned_key_ups = Vec::with_capacity(3);
                let key_down_at = if with_shift { 3 } else { 2 };
                let key_up_at = key_down_at + 1;
                if sent >= key_down_at && sent < key_up_at {
                    owned_key_ups.push(scan_code_key(key, true));
                }
                if with_shift && (2..5).contains(&sent) {
                    owned_key_ups.push(scan_code_key(VK_SHIFT, true));
                }
                owned_key_ups.push(scan_code_key(VK_CONTROL, true));
                release_owned_keys_best_effort(&owned_key_ups);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn send_shift_shortcut(key: u16) -> Result<(), InjectionError> {
    ensure_injection_safe()?;
    let inputs = [
        scan_code_key(VK_SHIFT, false),
        scan_code_key(key, false),
        scan_code_key(key, true),
        scan_code_key(VK_SHIFT, true),
    ];
    match send(&inputs) {
        Ok(()) => Ok(()),
        Err(error @ InjectionError::Partial { sent, .. }) => {
            if (1..inputs.len() as u32).contains(&sent) {
                let mut owned_key_ups = Vec::with_capacity(2);
                if (2..3).contains(&sent) {
                    owned_key_ups.push(scan_code_key(key, true));
                }
                owned_key_ups.push(scan_code_key(VK_SHIFT, true));
                release_owned_keys_best_effort(&owned_key_ups);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn release_owned_keys_best_effort(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    for _ in 0..3 {
        if send(inputs).is_ok() {
            break;
        }
        thread::yield_now();
    }
}

fn ensure_injection_safe() -> Result<(), InjectionError> {
    if crate::hotkeys::injection_is_safe() {
        Ok(())
    } else {
        Err(InjectionError::PhysicalModifiersHeld)
    }
}

fn send(inputs: &[INPUT]) -> Result<(), InjectionError> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(InjectionError::Partial {
            sent,
            expected: inputs.len() as u32,
            win32: unsafe { GetLastError() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ES_PASSWORD, is_native_edit_class, native_edit_supported};

    #[test]
    fn a_native_password_is_a_terminal_denial_not_an_unsupported_adapter() {
        for class in ["Edit", "RICHEDIT50W", "WindowsForms10.EDIT.app"] {
            assert!(
                native_edit_supported(class, ES_PASSWORD as isize).is_err(),
                "{class}"
            );
        }
        assert!(matches!(native_edit_supported("Edit", 0), Ok(true)));
        assert!(matches!(
            native_edit_supported("Chrome_RenderWidgetHostHWND", 0),
            Ok(false)
        ));
    }

    #[test]
    fn recognizes_native_edit_control_classes() {
        assert!(is_native_edit_class("Edit"));
        assert!(is_native_edit_class("RichEditD2DPT"));
        assert!(is_native_edit_class("RICHEDIT50W"));
        assert!(is_native_edit_class(
            "WindowsForms10.EDIT.app.0.141b42a_r8_ad1"
        ));
        assert!(!is_native_edit_class("Chrome_RenderWidgetHostHWND"));
        assert!(!is_native_edit_class("CASCADIA_HOSTING_WINDOW_CLASS"));
    }
}
