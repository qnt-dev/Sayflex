use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use rust_protocol::TargetSnapshot;

use crate::{inject, target};

const LIST_CLIENTS_TIMEOUT: Duration = Duration::from_millis(500);
const SEND_TEXT_TIMEOUT: Duration = Duration::from_millis(900);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_DIAGNOSTIC_OUTPUT: usize = 16 * 1024;

#[derive(Debug)]
pub struct WezTermError {
    message: String,
    ambiguous: bool,
    superseded: bool,
}

impl WezTermError {
    fn not_delivered(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ambiguous: false,
            superseded: false,
        }
    }

    fn ambiguous(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ambiguous: true,
            superseded: false,
        }
    }

    fn superseded(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ambiguous: false,
            superseded: true,
        }
    }

    pub fn delivery_is_ambiguous(&self) -> bool {
        self.ambiguous
    }

    pub fn target_was_superseded(&self) -> bool {
        self.superseded
    }
}

impl std::fmt::Display for WezTermError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WezTermError {}

pub fn send_text(target_snapshot: TargetSnapshot, text: &str) -> Result<(), WezTermError> {
    if !inject::is_foreground(target_snapshot) {
        return Err(WezTermError::superseded(
            "target is no longer foreground before WezTerm dispatch",
        ));
    }
    let resolved =
        resolve_client(target_snapshot.process_id).map_err(WezTermError::not_delivered)?;

    // Resolve the process and foreground window again after discovery. This
    // closes the race where the user changes applications while list-clients
    // is running.
    if target::process_path(target_snapshot.process_id).as_deref()
        != Some(resolved.gui_path.as_path())
        || !inject::is_foreground(target_snapshot)
    {
        return Err(WezTermError::superseded(
            "target changed before WezTerm dispatch",
        ));
    }

    let pane_id = resolved.pane_id.to_string();
    let result = run_cli(
        &resolved.cli_path,
        &["cli", "--no-auto-start", "send-text", "--pane-id", &pane_id],
        Some(text.as_bytes()),
        SEND_TEXT_TIMEOUT,
        Some(&resolved.socket_path),
    )
    .map_err(WezTermError::ambiguous)?;
    if result.status.success() {
        Ok(())
    } else {
        Err(WezTermError::ambiguous(format!(
            "wezterm cli send-text exited with {}: {}",
            exit_code(&result.status),
            bounded_text(&result.stderr)
        )))
    }
}

pub fn probe(process_id: u32) -> Result<(u64, String, String), String> {
    let resolved = resolve_client(process_id)?;
    Ok((
        resolved.pane_id,
        resolved.cli_path.to_string_lossy().into_owned(),
        resolved.socket_path.to_string_lossy().into_owned(),
    ))
}

struct ResolvedClient {
    gui_path: std::path::PathBuf,
    cli_path: std::path::PathBuf,
    socket_path: std::path::PathBuf,
    pane_id: u64,
}

fn resolve_client(process_id: u32) -> Result<ResolvedClient, String> {
    let gui_path = target::process_path(process_id)
        .ok_or_else(|| "could not resolve the WezTerm process path".to_owned())?;
    if !is_expected_gui_path(&gui_path) {
        return Err("target process is not wezterm-gui.exe".to_owned());
    }
    let cli_path = gui_path
        .parent()
        .map(|directory| directory.join("wezterm.exe"))
        .filter(|path| path.is_file())
        .ok_or_else(|| "wezterm.exe is not adjacent to wezterm-gui.exe".to_owned())?;
    let socket_path = resolve_socket_path(process_id)?;

    let clients = run_cli(
        &cli_path,
        &["cli", "--no-auto-start", "list-clients", "--format", "json"],
        None,
        LIST_CLIENTS_TIMEOUT,
        Some(&socket_path),
    )?;
    let pane_id = focused_pane_for_pid(&clients.stdout, process_id)?;
    Ok(ResolvedClient {
        gui_path,
        cli_path,
        socket_path,
        pane_id,
    })
}

fn resolve_socket_path(process_id: u32) -> Result<PathBuf, String> {
    socket_candidates(
        process_id,
        std::env::var_os("WEZTERM_UNIX_SOCKET"),
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")),
        std::env::var_os("LOCALAPPDATA"),
    )
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or_else(|| {
        format!(
            "could not locate the WezTerm gui socket for process {process_id}; expected gui-sock-{process_id}"
        )
    })
}

fn socket_candidates(
    process_id: u32,
    inherited_socket: Option<std::ffi::OsString>,
    xdg_runtime: Option<std::ffi::OsString>,
    xdg_data: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
    local_app_data: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let socket_name = format!("gui-sock-{process_id}");
    let mut candidates = Vec::new();
    if let Some(socket) = inherited_socket {
        let path = PathBuf::from(socket);
        if path
            .file_name()
            .is_some_and(|name| name == socket_name.as_str())
        {
            candidates.push(path);
        }
    }
    if let Some(root) = xdg_runtime {
        candidates.push(PathBuf::from(root).join("wezterm").join(&socket_name));
    }
    if let Some(root) = xdg_data {
        candidates.push(PathBuf::from(root).join("wezterm").join(&socket_name));
    }
    if let Some(root) = user_profile {
        candidates.push(
            PathBuf::from(root)
                .join(".local")
                .join("share")
                .join("wezterm")
                .join(&socket_name),
        );
    }
    if let Some(root) = local_app_data {
        candidates.push(PathBuf::from(root).join("wezterm").join(&socket_name));
    }
    candidates.dedup();
    candidates
}

struct CliOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_cli(
    executable: &Path,
    arguments: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
    socket_path: Option<&Path>,
) -> Result<CliOutput, String> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(socket_path) = socket_path {
        // InputHost is normally started by SayFlex, not by WezTerm, so it does
        // not inherit WEZTERM_UNIX_SOCKET. Pin every CLI operation to the
        // socket whose name contains the captured GUI process id; otherwise a
        // multi-instance setup can target the wrong terminal or fail entirely.
        command.env("WEZTERM_UNIX_SOCKET", socket_path);
    }
    configure_hidden_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start wezterm cli: {error}"))?;

    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .ok_or_else(|| "wezterm cli stdin is unavailable".to_owned())?
            .write_all(input)
            .map_err(|error| {
                let _ = child.kill();
                format!("could not write text to wezterm cli: {error}")
            })?;
    }

    let status = wait_with_timeout(&mut child, timeout)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(stream) = child.stdout.take() {
        stream
            .take(MAX_DIAGNOSTIC_OUTPUT as u64)
            .read_to_end(&mut stdout)
            .map_err(|error| format!("could not read wezterm cli stdout: {error}"))?;
    }
    if let Some(stream) = child.stderr.take() {
        stream
            .take(MAX_DIAGNOSTIC_OUTPUT as u64)
            .read_to_end(&mut stderr)
            .map_err(|error| format!("could not read wezterm cli stderr: {error}"))?;
    }
    if !status.success() && arguments.contains(&"list-clients") {
        return Err(format!(
            "wezterm cli list-clients exited with {}: {}",
            exit_code(&status),
            bounded_text(&stderr)
        ));
    }
    Ok(CliOutput {
        status,
        stdout,
        stderr,
    })
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "wezterm cli timed out after {} ms",
                    timeout.as_millis()
                ));
            }
            Err(error) => return Err(format!("could not wait for wezterm cli: {error}")),
        }
    }
}

fn focused_pane_for_pid(payload: &[u8], process_id: u32) -> Result<u64, String> {
    let clients: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| format!("invalid wezterm client list: {error}"))?;
    let clients = clients
        .as_array()
        .ok_or_else(|| "wezterm client list is not an array".to_owned())?;
    clients
        .iter()
        .find(|client| value_as_u64(&client["pid"]) == Some(u64::from(process_id)))
        .and_then(|client| value_as_u64(&client["focused_pane_id"]))
        .ok_or_else(|| format!("wezterm has no focused pane for client pid {process_id}"))
}

fn value_as_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

fn is_expected_gui_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("wezterm-gui.exe"))
}

fn bounded_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .replace(['\r', '\n'], " ")
}

fn exit_code(status: &ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "terminated".to_owned(), |code| code.to_string())
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::{focused_pane_for_pid, is_expected_gui_path, socket_candidates};
    use std::{ffi::OsString, path::Path};

    #[test]
    fn resolves_the_focused_pane_for_the_exact_gui_client() {
        let payload = br#"[
            {"pid": 100, "focused_pane_id": 7},
            {"pid": "200", "focused_pane_id": "11"}
        ]"#;
        assert_eq!(focused_pane_for_pid(payload, 100).unwrap(), 7);
        assert_eq!(focused_pane_for_pid(payload, 200).unwrap(), 11);
        assert!(focused_pane_for_pid(payload, 300).is_err());
    }

    #[test]
    fn only_accepts_the_expected_adjacent_gui_binary() {
        assert!(is_expected_gui_path(Path::new(
            r"C:\Apps\WezTerm\wezterm-gui.exe"
        )));
        assert!(is_expected_gui_path(Path::new(
            r"C:\Apps\WezTerm\WEZTERM-GUI.EXE"
        )));
        assert!(!is_expected_gui_path(Path::new(
            r"C:\Apps\WezTerm\wezterm.exe"
        )));
    }

    #[test]
    fn socket_discovery_is_bound_to_the_target_gui_pid() {
        let candidates = socket_candidates(
            13500,
            Some(OsString::from(
                r"C:\Users\n\.local\share\wezterm\gui-sock-999",
            )),
            None,
            None,
            Some(OsString::from(r"C:\Users\n")),
            None,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0],
            Path::new(r"C:\Users\n\.local\share\wezterm\gui-sock-13500")
        );
    }

    #[test]
    fn exact_inherited_socket_has_priority() {
        let candidates = socket_candidates(
            42,
            Some(OsString::from(r"D:\runtime\gui-sock-42")),
            Some(OsString::from(r"D:\xdg-runtime")),
            None,
            Some(OsString::from(r"C:\Users\n")),
            None,
        );

        assert_eq!(candidates[0], Path::new(r"D:\runtime\gui-sock-42"));
        assert_eq!(
            candidates[1],
            Path::new(r"D:\xdg-runtime\wezterm\gui-sock-42")
        );
    }
}
