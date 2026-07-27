use std::{mem, thread};

use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
            SendInput, VK_C, VK_CONTROL, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{GA_ROOT, GetAncestor, GetForegroundWindow},
    },
};

use crate::target::SelectionCopyShortcut;

pub const SYNTHETIC_MARKER: usize = 0x5255_5354_564f_4943;

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
    send_control_shortcut(
        VK_C,
        matches!(shortcut, SelectionCopyShortcut::ControlShiftC),
    )
}

pub fn paste_shortcut() -> Result<(), InjectionError> {
    send_control_shortcut(VK_V, false)
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

fn virtual_key(virtual_key: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: SYNTHETIC_MARKER,
            },
        },
    }
}

fn send_control_shortcut(key: u16, with_shift: bool) -> Result<(), InjectionError> {
    ensure_injection_safe()?;
    let inputs = if with_shift {
        vec![
            virtual_key(VK_CONTROL, false),
            virtual_key(VK_SHIFT, false),
            virtual_key(key, false),
            virtual_key(key, true),
            virtual_key(VK_SHIFT, true),
            virtual_key(VK_CONTROL, true),
        ]
    } else {
        vec![
            virtual_key(VK_CONTROL, false),
            virtual_key(key, false),
            virtual_key(key, true),
            virtual_key(VK_CONTROL, true),
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
                    owned_key_ups.push(virtual_key(key, true));
                }
                if with_shift && (2..5).contains(&sent) {
                    owned_key_ups.push(virtual_key(VK_SHIFT, true));
                }
                owned_key_ups.push(virtual_key(VK_CONTROL, true));
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
