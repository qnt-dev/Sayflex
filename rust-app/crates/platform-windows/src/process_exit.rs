use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
    ptr, thread,
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{GetCurrentProcess, GetExitCodeProcess, INFINITE, WaitForSingleObject},
};

use crate::ProcessExit;

pub fn watch_process_exit(
    child: &Child,
    started_at: Instant,
    on_exit: impl FnOnce(ProcessExit) + Send + 'static,
) -> Result<(), String> {
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: The source handle belongs to the live Child. DuplicateHandle
    // creates an independently owned process handle for the watcher thread.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            child.as_raw_handle().cast(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 {
        return Err(format!(
            "could not duplicate worker process handle: {}",
            std::io::Error::last_os_error()
        ));
    }

    // SAFETY: duplicate is newly owned and transferred exactly once.
    let watched = unsafe { OwnedHandle::from_raw_handle(duplicate.cast()) };
    thread::Builder::new()
        .name(format!("worker-exit-{}", child.id()))
        .spawn(move || {
            let handle = watched.as_raw_handle().cast();
            // SAFETY: handle remains owned by watched throughout this query.
            let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
            let mut code = 0_u32;
            let exit_code = if wait == WAIT_OBJECT_0
                // SAFETY: code is writable and handle is a signaled process.
                && unsafe { GetExitCodeProcess(handle, &mut code) } != 0
            {
                Some(code)
            } else {
                if wait == WAIT_FAILED {
                    eprintln!(
                        "worker process wait failed: {}",
                        std::io::Error::last_os_error()
                    );
                }
                None
            };
            on_exit(ProcessExit {
                exit_code,
                uptime: started_at.elapsed(),
            });
        })
        .map(|_| ())
        .map_err(|error| format!("could not start worker exit watcher: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };

    use super::watch_process_exit;

    #[test]
    fn reports_the_real_process_exit_code() {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/C", "exit 42"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);
        let mut child = command.spawn().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        watch_process_exit(&child, Instant::now(), move |exit| {
            let _ = sender.send(exit);
        })
        .unwrap();

        let exit = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(exit.exit_code, Some(42));
        let _ = child.wait();
    }
}
