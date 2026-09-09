use std::{ffi::c_void, mem, path::Path};

use rust_protocol::{ScreenPoint, TargetSnapshot};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::{
        Input::KeyboardAndMouse::GetKeyboardLayout,
        WindowsAndMessaging::{
            ES_PASSWORD, GA_ROOT, GUITHREADINFO, GWL_STYLE, GetAncestor, GetClassNameW,
            GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW,
            GetWindowThreadProcessId,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionCopyShortcut {
    ControlC,
    ControlShiftC,
    ControlInsert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteShortcut {
    ControlV,
    ControlShiftV,
    ShiftInsert,
}

pub fn capture_target() -> TargetSnapshot {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return TargetSnapshot::default();
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let top_level = if root.is_null() { foreground } else { root };
    capture_window(top_level)
}

pub fn capture_window_handle(window_handle: u64) -> TargetSnapshot {
    capture_window(window_handle as usize as *mut c_void)
}

fn capture_window(top_level: *mut c_void) -> TargetSnapshot {
    let root = unsafe { GetAncestor(top_level, GA_ROOT) };
    let top_level = if root.is_null() { top_level } else { root };
    let mut process_id = 0_u32;
    let thread_id = unsafe { GetWindowThreadProcessId(top_level, &mut process_id) };
    let layout = unsafe { GetKeyboardLayout(thread_id) };
    TargetSnapshot {
        window_handle: top_level as usize as u64,
        process_id,
        thread_id,
        keyboard_layout: layout as usize as u64,
        selection_rect: None,
        pointer: captured_pointer(),
    }
}

pub fn capture_text_target() -> Option<TargetSnapshot> {
    let target = capture_target();
    if target.window_handle == 0 || target.process_id == 0 || focused_control_is_password(target) {
        None
    } else {
        Some(target)
    }
}

fn focused_control_is_password(target: TargetSnapshot) -> bool {
    let mut information = GUITHREADINFO {
        cbSize: mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } == 0
        || information.hwndFocus.is_null()
    {
        return false;
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
        return false;
    }
    let class_name = String::from_utf16_lossy(&class_name[..copied as usize]).to_ascii_lowercase();
    let is_edit = class_name == "edit" || class_name.contains("richedit");
    let style = unsafe { GetWindowLongPtrW(information.hwndFocus, GWL_STYLE) };
    is_edit && style & ES_PASSWORD as isize != 0
}

pub fn selection_copy_shortcut(target: TargetSnapshot) -> SelectionCopyShortcut {
    process_name(target.process_id)
        .as_deref()
        .map(selection_copy_shortcut_for_process)
        .unwrap_or(SelectionCopyShortcut::ControlC)
}

pub fn paste_shortcut(target: TargetSnapshot) -> PasteShortcut {
    process_name(target.process_id)
        .as_deref()
        .map(paste_shortcut_for_process)
        .unwrap_or(PasteShortcut::ControlV)
}

pub fn is_wezterm_target(target: TargetSnapshot) -> bool {
    process_name(target.process_id)
        .as_deref()
        .is_some_and(is_wezterm)
}

fn selection_copy_shortcut_for_process(process_name: &str) -> SelectionCopyShortcut {
    if is_wezterm(process_name) || uses_control_insert(process_name) {
        SelectionCopyShortcut::ControlInsert
    } else if uses_control_shift_c(process_name) {
        SelectionCopyShortcut::ControlShiftC
    } else {
        SelectionCopyShortcut::ControlC
    }
}

fn paste_shortcut_for_process(process_name: &str) -> PasteShortcut {
    if is_wezterm(process_name) || uses_control_shift_c(process_name) {
        PasteShortcut::ControlShiftV
    } else if uses_control_insert(process_name) {
        PasteShortcut::ShiftInsert
    } else {
        PasteShortcut::ControlV
    }
}

fn is_wezterm(process_name: &str) -> bool {
    process_name.eq_ignore_ascii_case("wezterm-gui")
}

fn uses_control_shift_c(process_name: &str) -> bool {
    [
        "WindowsTerminal",
        "OpenConsole",
        "alacritty",
        "kitty",
        "tabby",
        "Hyper",
    ]
    .iter()
    .any(|candidate| process_name.eq_ignore_ascii_case(candidate))
}

fn uses_control_insert(process_name: &str) -> bool {
    ["mintty", "putty", "puttytel"]
        .iter()
        .any(|candidate| process_name.eq_ignore_ascii_case(candidate))
}

fn process_name(process_id: u32) -> Option<String> {
    process_path(process_id)?
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

pub fn process_path(process_id: u32) -> Option<std::path::PathBuf> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let success =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) } != 0;
    unsafe {
        let _ = CloseHandle(process);
    }
    if !success {
        return None;
    }
    let full_path = String::from_utf16(&buffer[..length as usize]).ok()?;
    Some(Path::new(&full_path).to_path_buf())
}

#[allow(dead_code)]
pub fn capture_focused_target() -> TargetSnapshot {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return TargetSnapshot::default();
    }
    let mut process_id = 0_u32;
    let mut thread_id = unsafe { GetWindowThreadProcessId(foreground, &mut process_id) };
    let mut information = GUITHREADINFO {
        cbSize: mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let focused = if unsafe { GetGUIThreadInfo(thread_id, &mut information) } != 0
        && !information.hwndFocus.is_null()
    {
        information.hwndFocus
    } else {
        foreground
    };
    thread_id = unsafe { GetWindowThreadProcessId(focused, &mut process_id) };
    let layout = unsafe { GetKeyboardLayout(thread_id) };
    TargetSnapshot {
        window_handle: focused as usize as u64,
        process_id,
        thread_id,
        keyboard_layout: layout as usize as u64,
        selection_rect: None,
        pointer: captured_pointer(),
    }
}

fn captured_pointer() -> Option<ScreenPoint> {
    let mut point = windows_sys::Win32::Foundation::POINT::default();
    (unsafe { GetCursorPos(&mut point) } != 0).then_some(ScreenPoint {
        x: point.x,
        y: point.y,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PasteShortcut, SelectionCopyShortcut, is_wezterm, paste_shortcut_for_process,
        selection_copy_shortcut_for_process,
    };

    #[test]
    fn wezterm_never_uses_interrupt_as_its_copy_shortcut() {
        assert_eq!(
            selection_copy_shortcut_for_process("wezterm-gui"),
            SelectionCopyShortcut::ControlInsert
        );
        assert_eq!(
            selection_copy_shortcut_for_process("WEZTERM-GUI"),
            SelectionCopyShortcut::ControlInsert
        );
        assert_eq!(
            paste_shortcut_for_process("wezterm-gui"),
            PasteShortcut::ControlShiftV
        );
    }

    #[test]
    fn normal_editors_keep_standard_copy_behavior() {
        assert_eq!(
            selection_copy_shortcut_for_process("notepad"),
            SelectionCopyShortcut::ControlC
        );
        assert_eq!(
            paste_shortcut_for_process("notepad"),
            PasteShortcut::ControlV
        );
        assert!(!is_wezterm("notepad"));
    }

    #[test]
    fn modern_terminal_hosts_use_their_non_interrupting_copy_shortcut() {
        for process in [
            "WindowsTerminal",
            "OpenConsole",
            "alacritty",
            "kitty",
            "tabby",
            "Hyper",
        ] {
            assert_eq!(
                selection_copy_shortcut_for_process(process),
                SelectionCopyShortcut::ControlShiftC,
                "{process}"
            );
            assert_eq!(
                paste_shortcut_for_process(process),
                PasteShortcut::ControlShiftV,
                "{process}"
            );
        }
    }

    #[test]
    fn classic_terminal_hosts_use_control_insert() {
        for process in ["mintty", "putty", "puttytel"] {
            assert_eq!(
                selection_copy_shortcut_for_process(process),
                SelectionCopyShortcut::ControlInsert,
                "{process}"
            );
            assert_eq!(
                paste_shortcut_for_process(process),
                PasteShortcut::ShiftInsert,
                "{process}"
            );
        }
    }
}
