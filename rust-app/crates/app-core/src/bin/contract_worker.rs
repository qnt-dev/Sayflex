//! A stand-in worker for the process-contract tests.
//!
//! It speaks the same newline-delimited JSON discipline as the real workers —
//! commands on standard input, one JSON document per line on standard output —
//! without a model, a microphone or a GPU, so the tests that exercise the
//! transport can run anywhere in a second.
//!
//! Modes, chosen by the first argument:
//!
//! - `echo`: replies to every line with `{"echoed": <line>, "index": n}`,
//!   which shows both that nothing was lost and that order was preserved.
//! - `deaf`: never reads standard input, the way a worker busy with a
//!   multi-gigabyte installation used to behave.

use std::io::{BufRead, BufReader, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "echo".to_owned());
    match mode.as_str() {
        "echo" => echo(),
        "deaf" => deaf(),
        other => {
            eprintln!("unknown contract worker mode: {other}");
            std::process::exit(2);
        }
    }
}

fn echo() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for (index, line) in BufReader::new(stdin.lock()).lines().enumerate() {
        let Ok(line) = line else {
            break;
        };
        if line == "\"shutdown\"" {
            break;
        }
        let reply = serde_json::json!({ "echoed": line, "index": index });
        if serde_json::to_writer(&mut stdout, &reply).is_err()
            || stdout.write_all(b"\n").is_err()
            || stdout.flush().is_err()
        {
            break;
        }
    }
}

fn deaf() {
    // Standard input is deliberately never read; the process stays alive so
    // its pipe stays open and fills.
    std::thread::sleep(std::time::Duration::from_secs(30));
}
