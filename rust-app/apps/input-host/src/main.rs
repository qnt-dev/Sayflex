#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod hotkeys;
mod inject;
mod insertion;
mod target;

use std::{
    ffi::c_void,
    io::{self, BufRead, BufReader, BufWriter, Write},
    ptr,
    sync::mpsc,
    thread,
};

use rust_protocol::{InputHostCommand, InputHostEvent, PROTOCOL_VERSION};
use windows_sys::Win32::{
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT,
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    rust_platform_windows::configure_safe_dll_search()?;
    let (events, event_receiver) = mpsc::channel::<InputHostEvent>();
    let insertion = insertion::InsertionService::start(events.clone())?;
    hotkeys::initialize(events.clone())?;
    thread::Builder::new()
        .name("input-host-json-output".to_owned())
        .spawn(move || write_events(event_receiver))?;

    // PostThreadMessage requires the destination thread to already own a message
    // queue. Force its creation before any worker capable of requesting shutdown
    // is started.
    let mut queue_probe = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    let hook_thread_id = unsafe { GetCurrentThreadId() };
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hotkeys::keyboard_hook), module, 0) };
    if hook.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    let _hook_guard = HookGuard(hook);
    let command_events = events.clone();
    thread::Builder::new()
        .name("input-host-json-input".to_owned())
        .spawn(move || read_commands(hook_thread_id, command_events, insertion))?;
    let _ = events.send(InputHostEvent::Ready {
        protocol_version: PROTOCOL_VERSION,
    });

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if result == 0 {
            break;
        }
        if result == -1 {
            return Err(io::Error::last_os_error().into());
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

struct HookGuard(*mut c_void);

impl Drop for HookGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

fn write_events(receiver: mpsc::Receiver<InputHostEvent>) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    for event in receiver {
        if serde_json::to_writer(&mut writer, &event).is_err()
            || writer.write_all(b"\n").is_err()
            || writer.flush().is_err()
        {
            break;
        }
    }
}

fn read_commands(
    hook_thread_id: u32,
    events: mpsc::Sender<InputHostEvent>,
    insertion: insertion::InsertionService,
) {
    let stdin = io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let command = match serde_json::from_str::<InputHostCommand>(&line) {
            Ok(command) => command,
            Err(error) => {
                let _ = events.send(InputHostEvent::Failed {
                    session_id: None,
                    code: "invalid_command".to_owned(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        match command {
            InputHostCommand::InsertTranscript {
                transcript,
                target,
                add_trailing_space,
            } => insertion.insert(transcript, target, add_trailing_space),
            InputHostCommand::CopyTranscript { transcript } => {
                insertion.copy(transcript);
            }
            InputHostCommand::CopyText { request_id, text } => {
                insertion.copy_text(request_id, text);
            }
            InputHostCommand::ReplaceSelection {
                request_id,
                target,
                text,
            } => {
                insertion.replace_selection(request_id, target, text);
            }
            InputHostCommand::CancelSelection { .. } => {}
            InputHostCommand::Shutdown => {
                insertion.shutdown();
                break;
            }
        }
    }
    insertion.shutdown();
    unsafe {
        if PostThreadMessageW(hook_thread_id, WM_QUIT, 0, 0) == 0 {
            let _ = events.send(InputHostEvent::Failed {
                session_id: None,
                code: "input_host_shutdown".to_owned(),
                message: io::Error::last_os_error().to_string(),
            });
        }
    }
}
