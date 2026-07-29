use std::{mem, thread};

use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, SendInput,
            VK_C, VK_CONTROL, VK_INSERT, VK_LCONTROL, VK_LMENU, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{
            GA_ROOT, GUITHREADINFO, GetAncestor, GetForegroundWindow, GetGUIThreadInfo,
            SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_COPY,
        },
    },
};

use crate::target::{PasteShortcut, SelectionCopyShortcut};

pub const SYNTHETIC_MARKER: usize = 0x5255_5354_564f_4943;
pub const E2E_HOTKEY_MARKER: usize = 0x514e_5446_4c45_5845;

#[derive(Debug, Error)]
pub enum InjectionError {
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

pub fn insert_text(
    target: TargetSnapshot,
    text: &str,
    add_trailing_space: bool,
) -> Result<(), InjectionError> {
    ensure_injection_safe()?;
    if !is_foreground(target) {
        return Err(InjectionError::TargetChanged);
    }
    let complete = if add_trailing_space {
        format!("{text} ")
    } else {
        text.to_owned()
    };
    let units = complete.encode_utf16().collect::<Vec<_>>();
    for chunk in units.chunks(128) {
        ensure_injection_safe()?;
        if !is_foreground(target) {
            return Err(InjectionError::TargetChanged);
        }
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for unit in chunk {
            inputs.push(unicode_input(*unit, false));
            inputs.push(unicode_input(*unit, true));
        }
        send(&inputs)?;
    }
    Ok(())
}

pub fn replace_selection(target: TargetSnapshot, text: &str) -> Result<(), InjectionError> {
    insert_text(target, text, false)
}

pub fn copy_shortcut(shortcut: SelectionCopyShortcut) -> Result<(), InjectionError> {
    match shortcut {
        SelectionCopyShortcut::ControlC => send_control_shortcut(VK_C, false),
        SelectionCopyShortcut::ControlShiftC => send_control_shortcut(VK_C, true),
        SelectionCopyShortcut::ControlInsert => send_control_shortcut(VK_INSERT, false),
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

pub fn paste_shortcut(shortcut: PasteShortcut) -> Result<(), InjectionError> {
    match shortcut {
        PasteShortcut::ControlV => send_control_shortcut(VK_V, false),
        PasteShortcut::ControlShiftV => send_control_shortcut(VK_V, true),
        PasteShortcut::ShiftInsert => send_shift_shortcut(VK_INSERT),
    }
}

pub fn is_foreground(target: TargetSnapshot) -> bool {
    if target.window_handle == 0 || target.process_id == 0 {
        return false;
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let actual = if root.is_null() { foreground } else { root };
    actual as usize as u64 == target.window_handle
}

fn unicode_input(unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: SYNTHETIC_MARKER,
            },
        },
    }
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
