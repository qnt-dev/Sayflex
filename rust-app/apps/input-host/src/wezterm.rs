use std::{
    io::{Read, Write},
    path::Path,
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

pub fn send_text(target_snapshot: TargetSnapshot, text: &str) -> Result<(), String> {
    if !inject::is_foreground(target_snapshot) {
        return Err("target is no longer foreground before WezTerm dispatch".to_owned());
    }
    let resolved = resolve_client(target_snapshot.process_id)?;

    // Resolve the process and foreground window again after discovery. This
    // closes the race where the user changes applications while list-clients
    // is running.
    if target::process_path(target_snapshot.process_id).as_deref()
        != Some(resolved.gui_path.as_path())
        || !inject::is_foreground(target_snapshot)
    {
        return Err("target changed before WezTerm dispatch".to_owned());
    }

    let pane_id = resolved.pane_id.to_string();
    let result = run_cli(
        &resolved.cli_path,
        &["cli", "--no-auto-start", "send-text", "--pane-id", &pane_id],
        Some(text.as_bytes()),
        SEND_TEXT_TIMEOUT,
    )?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "wezterm cli send-text exited with {}: {}",
            exit_code(&result.status),
            bounded_text(&result.stderr)
        ))
    }
}

pub fn probe(process_id: u32) -> Result<(u64, String), String> {
    let resolved = resolve_client(process_id)?;
    Ok((
        resolved.pane_id,
        resolved.cli_path.to_string_lossy().into_owned(),
    ))
}

struct ResolvedClient {
    gui_path: std::path::PathBuf,
    cli_path: std::path::PathBuf,
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

    let clients = run_cli(
        &cli_path,
        &["cli", "--no-auto-start", "list-clients", "--format", "json"],
        None,
        LIST_CLIENTS_TIMEOUT,
    )?;
    let pane_id = focused_pane_for_pid(&clients.stdout, process_id)?;
    Ok(ResolvedClient {
        gui_path,
        cli_path,
        pane_id,
    })
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
    use super::{focused_pane_for_pid, is_expected_gui_path};
    use std::path::Path;

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
}
