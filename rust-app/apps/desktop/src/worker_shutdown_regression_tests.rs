use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use super::finish_worker_children;

const FIXTURE_MODE: &str = "SAYFLEX_SHUTDOWN_FIXTURE_MODE";
const FIXTURE_MARKER: &str = "SAYFLEX_SHUTDOWN_FIXTURE_MARKER";

// Reuses this test binary solely as a synthetic worker, never the installation.
#[test]
fn shutdown_child_fixture() {
    let Ok(mode) = std::env::var(FIXTURE_MODE) else {
        return;
    };
    println!("shutdown-fixture-ready");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "shutdown");
    if mode == "flush" {
        std::thread::sleep(Duration::from_millis(180));
        fs::write(std::env::var_os(FIXTURE_MARKER).unwrap(), "history-drained").unwrap();
    } else {
        std::thread::sleep(Duration::from_secs(30));
    }
}

fn fixture(mode: &str, marker: &std::path::Path) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "worker_supervisor::worker_shutdown_regression_tests::shutdown_child_fixture",
            "--nocapture",
        ])
        .env(FIXTURE_MODE, mode)
        .env(FIXTURE_MARKER, marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(super::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "fixture failed to start"
        );
        if line.contains("shutdown-fixture-ready") {
            break;
        }
    }
    child.stdout = Some(output.into_inner());
    writeln!(child.stdin.as_mut().unwrap(), "shutdown").unwrap();
    child
}

#[test]
fn delayed_history_flush_finishes_before_worker_is_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("history.txt");
    let mut child = fixture("flush", &marker);
    finish_worker_children(&mut [&mut child], Duration::from_secs(2));
    let exited_at_return = child.try_wait().unwrap().is_some();
    let status = child.wait().unwrap();
    assert_eq!(
        fs::read_to_string(marker).ok().as_deref(),
        Some("history-drained")
    );
    assert!(
        exited_at_return,
        "shutdown must reap even cooperative children"
    );
    assert!(status.success(), "cooperative worker must exit cleanly");
}

#[test]
fn stuck_workers_share_one_grace_deadline_and_are_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("unused.txt");
    let mut first = fixture("stuck", &marker);
    let mut second = fixture("stuck", &marker);
    let mut third = fixture("stuck", &marker);
    let started = Instant::now();
    finish_worker_children(
        &mut [&mut first, &mut second, &mut third],
        Duration::from_millis(200),
    );
    let elapsed = started.elapsed();
    let all_reaped = first.try_wait().unwrap().is_some()
        && second.try_wait().unwrap().is_some()
        && third.try_wait().unwrap().is_some();
    let _ = first.wait();
    let _ = second.wait();
    let _ = third.wait();
    assert!(all_reaped, "every uncooperative child must be reaped");
    assert!(
        elapsed < Duration::from_millis(500),
        "per-child grace multiplied shutdown time: {elapsed:?}"
    );
}
