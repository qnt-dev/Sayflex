//! Cross-process contract for the worker command transport.
//!
//! Everything else in the workspace tests one process in isolation. These
//! tests start a real child, hand it a real pipe, and check the two properties
//! the desktop depends on: commands arrive complete and in order, and a worker
//! that stops reading cannot block the process that talks to it.

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use rust_app_core::worker_stream::{JsonWriter, SendError, WORKER_COMMAND_QUEUE};
use rust_protocol::{
    PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext, TranslationLanguage,
    TranslationLanguagePolicy, TranslationWorkerCommand,
};

const CONTRACT_WORKER: &str = env!("CARGO_BIN_EXE_contract_worker");

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(mode: &str) -> ChildGuard {
    let mut command = Command::new(CONTRACT_WORKER);
    command
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    ChildGuard(
        command
            .spawn()
            .expect("the contract worker binary is built by cargo test"),
    )
}

#[test]
fn commands_reach_a_reading_worker_complete_and_in_order() {
    let mut worker = spawn("echo");
    let stdin = worker.0.stdin.take().expect("piped stdin");
    let stdout = worker.0.stdout.take().expect("piped stdout");
    let writer = JsonWriter::start(stdin, "contract-echo").expect("writer thread");

    let sent = 500;
    for index in 0..sent {
        writer
            .send(&format!("command-{index}"))
            .expect("a reading worker never backlogs");
    }
    writer.send(&"shutdown").expect("shutdown is deliverable");

    let mut received = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("the worker writes valid lines");
        let reply: serde_json::Value = serde_json::from_str(&line).expect("one JSON document");
        received.push(reply);
    }

    assert_eq!(received.len(), sent, "no command may be dropped");
    for (index, reply) in received.iter().enumerate() {
        assert_eq!(
            reply["index"].as_u64(),
            Some(index as u64),
            "commands must keep their order"
        );
        assert_eq!(
            reply["echoed"].as_str(),
            Some(format!("\"command-{index}\"").as_str()),
            "each command must arrive whole"
        );
    }
}

#[test]
fn a_worker_that_stops_reading_cannot_block_the_sender() {
    let mut worker = spawn("deaf");
    let stdin = worker.0.stdin.take().expect("piped stdin");
    let writer = JsonWriter::start(stdin, "contract-deaf").expect("writer thread");

    let started = Instant::now();
    // Enough bytes to overrun any plausible pipe buffer as well as the queue:
    // a kilobyte per command, several thousand commands.
    let payload = "x".repeat(1024);
    let outcome = (0..WORKER_COMMAND_QUEUE * 16)
        .map(|index| writer.send(&format!("command-{index}-{payload}")))
        .find(Result::is_err);

    assert_eq!(
        outcome,
        Some(Err(SendError::Backlogged)),
        "the sender must be told the worker is not reading"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the sender must never wait on the pipe, took {:?}",
        started.elapsed()
    );
}

#[test]
fn contextual_screen_commands_cross_the_worker_pipe_whole() {
    let mut worker = spawn("echo");
    let stdin = worker.0.stdin.take().expect("piped stdin");
    let stdout = worker.0.stdout.take().expect("piped stdout");
    let writer = JsonWriter::start(stdin, "contextual-screen-contract").expect("writer thread");
    let command = TranslationWorkerCommand::TranslateBlocks {
        request_id: RequestId::new(),
        base_revision: 19,
        blocks: vec![ScreenTextBlock {
            block_id: ScreenBlockId::from("column-2-line-3"),
            source_text: "Open settings".to_owned(),
            bounds: PhysicalRect {
                x: -800,
                y: 120,
                width: 240,
                height: 38,
            },
            confidence_milli: 944,
        }],
        context: SourceContext {
            application_id: Some("org.example.game".to_owned()),
            partial: false,
            ..SourceContext::default()
        },
        source_language: Some(TranslationLanguage::English),
        target_language: Some(TranslationLanguage::Russian),
        language_policy: TranslationLanguagePolicy::default(),
    };

    writer.send(&command).expect("command is deliverable");
    writer.send(&"shutdown").expect("shutdown is deliverable");

    let line = BufReader::new(stdout)
        .lines()
        .next()
        .expect("one response")
        .expect("valid response line");
    let reply: serde_json::Value = serde_json::from_str(&line).expect("one JSON document");
    let echoed = reply["echoed"].as_str().expect("serialized command");

    assert_eq!(
        serde_json::from_str::<TranslationWorkerCommand>(echoed).unwrap(),
        command
    );
}
