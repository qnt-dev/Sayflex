//! The protocol contract, checked against the worker the product ships.
//!
//! The other cross-process test uses a stand-in worker to prove the transport
//! carries commands whole and in order. This one starts *this* binary — the
//! same executable the desktop spawns — and checks what it actually answers:
//! that it announces the protocol version both sides must agree on, that it
//! names a model from its own catalogue, that an unreadable command produces a
//! failure rather than silence or a crash, and that it exits when told to.
//!
//! It runs in the diagnostic mode that keeps the protocol and leaves out the
//! hardware, so it needs no microphone, no GPU and no downloaded model, and
//! belongs in the ordinary test run rather than in a manual checklist.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const WORKER: &str = env!("CARGO_BIN_EXE_sayflex-speech-worker");

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Worker {
    fn start() -> Self {
        let mut child = Command::new(WORKER)
            .arg("--protocol-smoke")
            .env("SAYFLEX_RUN_PROTOCOL_SMOKE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the speech worker is built by cargo test");
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            input,
            output,
        }
    }

    fn send_raw(&mut self, line: &str) {
        writeln!(self.input, "{line}").expect("the worker is reading its input");
    }

    fn send(&mut self, command: &serde_json::Value) {
        self.send_raw(&command.to_string());
    }

    fn next_event(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.output
            .read_line(&mut line)
            .expect("the worker writes one JSON document per line");
        assert!(
            !line.is_empty(),
            "the worker closed its output unexpectedly"
        );
        serde_json::from_str(&line).expect("every line is a JSON document")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_worker_announces_the_protocol_version_and_a_model_it_knows() {
    let mut worker = Worker::start();

    worker.send(&serde_json::json!({
        "type": "initialize",
        "payload": {
            "selected_model_id": "a-model-that-does-not-exist",
            "output_mode": "original_language",
            "history_enabled": false,
            "fast_release_previews": true
        }
    }));
    let event = worker.next_event();

    assert_eq!(event["type"], "ready");
    assert_eq!(
        event["payload"]["protocol_version"],
        rust_protocol::PROTOCOL_VERSION,
        "both sides must agree on this before anything else is exchanged"
    );
    let selected = event["payload"]["selected_model_id"]
        .as_str()
        .expect("a model is always named");
    let models = event["payload"]["models"]
        .as_array()
        .expect("the catalogue travels with the handshake");
    assert!(
        models.iter().any(|model| model["catalog_id"] == selected),
        "an unknown request falls back to a model the worker actually has"
    );
}

#[test]
fn an_unreadable_command_is_answered_rather_than_ignored() {
    let mut worker = Worker::start();

    worker.send_raw("{ this is not a command }");
    let event = worker.next_event();

    assert_eq!(event["type"], "failed");
    assert_eq!(event["payload"]["code"], "invalid_command");
    assert_eq!(
        event["payload"]["retryable"], false,
        "sending the same broken line again would not help"
    );

    // And the worker is still listening afterwards.
    worker.send(&serde_json::json!({
        "type": "initialize",
        "payload": {
            "selected_model_id": "whisper-large-v3-turbo",
            "output_mode": "original_language",
            "history_enabled": false,
            "fast_release_previews": true
        }
    }));
    assert_eq!(worker.next_event()["type"], "ready");
}

#[test]
fn shutdown_ends_the_process() {
    let mut worker = Worker::start();

    worker.send(&serde_json::json!({ "type": "shutdown" }));

    let status = worker.child.wait().expect("the worker exits on its own");
    assert!(
        status.success(),
        "a worker asked to stop exits cleanly, not by being killed"
    );
}

#[test]
fn a_diagnostic_mode_is_refused_without_its_opt_in() {
    // The mode is present in a release build, so what keeps it out of the
    // ordinary lifecycle is the explicit environment opt-in.
    let output = Command::new(WORKER)
        .arg("--protocol-smoke")
        .env_remove("SAYFLEX_RUN_PROTOCOL_SMOKE")
        .output()
        .expect("the worker runs");

    assert!(!output.status.success());
}
