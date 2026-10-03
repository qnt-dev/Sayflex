use std::{
    backtrace::Backtrace,
    fs::{self, OpenOptions},
    io::{self, Write},
    panic,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::diagnostics;

const MARKER_FILE_NAME: &str = "desktop.running";

/// A durable desktop run marker.
///
/// A clean exit removes the marker after flushing its final log line. A hard
/// kill, abort, OOM, or power loss leaves it behind so the next launch can
/// report that the previous run ended unexpectedly.
pub struct RunSession {
    marker_path: PathBuf,
    instance_id: String,
    finished: bool,
}

impl RunSession {
    pub fn begin(data_root: &Path, version: &str) -> io::Result<Self> {
        fs::create_dir_all(data_root)?;
        let marker_path = data_root.join(MARKER_FILE_NAME);
        let previous_unexpected = marker_path.exists();
        if previous_unexpected {
            let previous = fs::read_to_string(&marker_path)
                .unwrap_or_else(|error| format!("unreadable:{error}"));
            diagnostics::log_line(&format!(
                "desktop lifecycle=previous_session_ended_unexpectedly marker={previous}"
            ));
        }

        let started_unix_ms = unix_ms();
        let process_id = process::id();
        let instance_id = format!("{process_id}-{started_unix_ms}");
        write_marker(
            &marker_path,
            &format!(
                "pid={process_id} version={version} instance={instance_id} started_unix_ms={started_unix_ms}"
            ),
        )?;
        diagnostics::log_line(&format!(
            "desktop lifecycle=starting pid={process_id} version={version} instance={instance_id} \
previous_unexpected={previous_unexpected}"
        ));
        diagnostics::flush()?;

        Ok(Self {
            marker_path,
            instance_id,
            finished: false,
        })
    }

    pub fn finish(&mut self, reason: &str) {
        if self.finished {
            return;
        }
        diagnostics::log_line(&format!(
            "desktop lifecycle=exiting reason={reason} instance={}",
            self.instance_id
        ));
        let _ = diagnostics::flush();
        if let Err(error) = fs::remove_file(&self.marker_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            diagnostics::log_line(&format!(
                "desktop lifecycle=marker_remove_failed error={error}"
            ));
            let _ = diagnostics::flush();
            return;
        }
        self.finished = true;
    }
}

impl Drop for RunSession {
    fn drop(&mut self) {
        if self.finished || std::thread::panicking() {
            return;
        }
        self.finish("early_return");
    }
}

pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        // A panic in the writer itself cannot synchronously flush its own
        // channel. Delegate directly to Rust's previous hook instead of
        // deadlocking the process during crash reporting.
        if std::thread::current().name() == Some("diagnostic-log-writer") {
            previous(information);
            return;
        }
        let location = information
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let payload = information
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                information
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
            })
            .unwrap_or("non-string panic payload");
        diagnostics::log_line(&format!(
            "desktop lifecycle=panic location={location} payload={payload} frames={} backtrace={}",
            module_relative_frames(),
            Backtrace::force_capture()
        ));
        let _ = diagnostics::flush();
        previous(information);
    }));
}

/// The panicking thread's return addresses as `module+0xoffset`.
///
/// Release binaries carry no symbols, so `Backtrace` prints only `<unknown>`
/// frames — no names and no addresses to look up later. A module-relative
/// offset resolves against the PDBs kept from the same build (publish.ps1
/// copies them to dist\symbols), e.g. `llvm-symbolizer --obj=SayFlex.exe`.
#[cfg(windows)]
fn module_relative_frames() -> String {
    use windows_sys::Win32::System::{
        Diagnostics::Debug::RtlCaptureStackBackTrace,
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW,
        },
    };

    let mut frames = [std::ptr::null_mut(); 48];
    // SAFETY: the buffer holds `frames.len()` pointers; the hash is optional.
    let captured = usize::from(unsafe {
        RtlCaptureStackBackTrace(
            0,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    });
    frames[..captured.min(frames.len())]
        .iter()
        .map(|&address| {
            let mut module = std::ptr::null_mut();
            // SAFETY: FROM_ADDRESS treats the name argument as an address inside
            // a loaded module; UNCHANGED_REFCOUNT leaves no handle to release.
            let found = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    address.cast_const().cast(),
                    &raw mut module,
                )
            } != 0;
            if !found {
                return format!("{address:p}");
            }
            let mut name = [0_u16; 260];
            // SAFETY: the buffer length is passed; the module handle is live.
            let length = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) }
                as usize;
            let path = String::from_utf16_lossy(&name[..length.min(name.len())]);
            let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            format!(
                "{file}+0x{:x}",
                (address as usize).wrapping_sub(module as usize)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(not(windows))]
fn module_relative_frames() -> String {
    String::new()
}

fn write_marker(path: &Path, contents: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{MARKER_FILE_NAME, RunSession};

    #[test]
    fn clean_finish_removes_marker() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = RunSession::begin(directory.path(), "test").unwrap();
        assert!(directory.path().join(MARKER_FILE_NAME).exists());
        session.finish("test_complete");
        assert!(!directory.path().join(MARKER_FILE_NAME).exists());
    }

    #[test]
    fn stale_marker_is_replaced_by_current_run() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join(MARKER_FILE_NAME);
        std::fs::write(&marker, "pid=old").unwrap();

        let mut session = RunSession::begin(directory.path(), "test").unwrap();
        let current = std::fs::read_to_string(&marker).unwrap();
        assert!(current.contains("version=test"));
        assert!(!current.contains("pid=old"));
        session.finish("test_complete");
    }
}

#[cfg(all(test, windows))]
mod frame_tests {
    #[test]
    fn panic_frames_are_module_relative_offsets() {
        let frames = super::module_relative_frames();
        let first = frames.split(' ').next().unwrap_or_default();
        assert!(
            first.contains(".exe+0x") || first.contains(".dll+0x"),
            "expected module+offset, got {frames}"
        );
    }
}
