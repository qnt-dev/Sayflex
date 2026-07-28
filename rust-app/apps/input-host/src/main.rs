#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod hotkeys;
mod inject;
mod insertion;
mod selection_capture;
mod target;
mod uia_selection;

use std::{
    env,
    ffi::c_void,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use rust_protocol::{InputHostCommand, InputHostEvent, PROTOCOL_VERSION, SessionId, Transcript};
use windows_sys::Win32::{
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT,
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    rust_platform_windows::configure_safe_dll_search()?;
    let _ = rust_app_core::diagnostics::initialize(&data_root().join("logs"), "input-host");
    diagnostic_line("input_host lifecycle=starting");
    if std::env::args().any(|argument| argument == "--selection-capture-smoke") {
        require_smoke_opt_in("QNTFLEX_RUN_SELECTION_SMOKE")?;
        return run_selection_capture_smoke();
    }
    if std::env::args().any(|argument| argument == "--insertion-smoke") {
        require_smoke_opt_in("QNTFLEX_RUN_INSERTION_SMOKE")?;
        return run_insertion_smoke();
    }

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
    diagnostic_line("input_host lifecycle=stopped");
    Ok(())
}

pub(crate) fn diagnostic_line(message: &str) {
    rust_app_core::diagnostics::log_line(message);
    #[cfg(debug_assertions)]
    eprintln!("{message}");
}

fn data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(".rust-voice-data")
        },
        |root| Path::new(&root).join("KeyboopRust"),
    )
}

fn run_selection_capture_smoke() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(delay) = std::env::var("QNTFLEX_SELECTION_SMOKE_DELAY_MS") {
        thread::sleep(std::time::Duration::from_millis(delay.parse()?));
    }
    let target = if let Ok(handle) = std::env::var("QNTFLEX_SELECTION_SMOKE_HWND") {
        target::capture_window_handle(handle.parse()?)
    } else {
        target::capture_text_target()
            .ok_or_else(|| io::Error::other("no unprotected foreground text target"))?
    };
    if let Ok(stable_ms) = std::env::var("QNTFLEX_SELECTION_SMOKE_STABLE_FOCUS_MS") {
        wait_for_stable_foreground(
            target,
            std::time::Duration::from_millis(stable_ms.parse()?),
            std::time::Duration::from_secs(20),
        )?;
    }
    diagnostic_line(&format!(
        "selection_smoke target_pid={} target_hwnd={} foreground_pid={} foreground_hwnd={} \
hkl={:#x} shortcut={:?}",
        target.process_id,
        target.window_handle,
        target::capture_target().process_id,
        target::capture_target().window_handle,
        target.keyboard_layout,
        target::selection_copy_shortcut(target),
    ));
    let started = Instant::now();
    let captured = selection_capture::capture(target)?;
    serde_json::to_writer(
        io::stdout().lock(),
        &serde_json::json!({
            "process_id": target.process_id,
            "method": format!("{:?}", captured.method),
            "elapsed_ms": started.elapsed().as_millis(),
            "text": captured.text,
        }),
    )?;
    Ok(())
}

fn run_insertion_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let handle = std::env::var("QNTFLEX_INSERTION_SMOKE_HWND")?.parse()?;
    let text = std::env::var("QNTFLEX_INSERTION_SMOKE_TEXT")?;
    let target = target::capture_window_handle(handle);
    let (events, receiver) = mpsc::channel();
    let insertion = insertion::InsertionService::start(events)?;
    let session_id = SessionId::new();
    insertion.insert(
        Transcript {
            session_id,
            text: text.clone(),
            language: "en".to_owned(),
            audio_duration_ms: 0,
            processing_duration_ms: 0,
            latency: Default::default(),
        },
        target,
        false,
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut method = None;
    let mut clipboard_restored = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(InputHostEvent::InsertionDispatched {
                session_id: completed,
                method: insertion_method,
                ..
            }) if completed == session_id => {
                method = Some(insertion_method);
                if insertion_method != rust_protocol::InsertionMethod::ClipboardPaste {
                    break;
                }
            }
            Ok(InputHostEvent::ClipboardRestored {
                session_id: restored,
            }) if restored == session_id => {
                clipboard_restored = true;
                break;
            }
            Ok(InputHostEvent::Failed {
                session_id: Some(failed),
                message,
                ..
            }) if failed == session_id => return Err(message.into()),
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("insertion service stopped before completion".into());
            }
        }
    }
    insertion.shutdown();
    let method = method.ok_or("insertion smoke timed out")?;
    serde_json::to_writer(
        io::stdout().lock(),
        &serde_json::json!({
            "session_id": session_id,
            "method": format!("{method:?}"),
            "clipboard_restored": clipboard_restored,
            "chars": text.chars().count(),
        }),
    )?;
    Ok(())
}

fn require_smoke_opt_in(variable: &str) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var(variable).as_deref() == Ok("1") {
        Ok(())
    } else {
        Err(format!("{variable}=1 is required for this isolated smoke mode").into())
    }
}

fn wait_for_stable_foreground(
    target: rust_protocol::TargetSnapshot,
    stable_for: Duration,
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut stable_since = None;
    while Instant::now() < deadline {
        if inject::is_foreground(target) {
            let since = stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= stable_for {
                return Ok(());
            }
        } else {
            stable_since = None;
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "target did not remain foreground for the required interval",
    ))
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
