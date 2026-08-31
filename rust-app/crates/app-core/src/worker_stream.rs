//! Command transport to a child worker.
//!
//! A worker reads newline-delimited JSON from its standard input. The pipe
//! between the two processes holds only a few kilobytes, and a blocking write
//! into a full pipe never returns on its own. The caller here is usually the
//! desktop's UI thread, so a worker that stops reading — a long installation
//! used to do exactly that — must not be able to freeze the application.
//!
//! Callers therefore only serialize and enqueue. A dedicated thread owns the
//! pipe, and a queue that stays full for longer than the send timeout produces
//! an error naming the problem instead of a hang.

use std::{
    cell::RefCell,
    io::{self, BufWriter, Write},
    sync::mpsc::{self, SyncSender, TrySendError},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

/// Commands buffered for a worker before a sender is told to give up. A worker
/// that is reading drains this in microseconds.
pub const WORKER_COMMAND_QUEUE: usize = 256;
/// How long a caller waits for room in a full queue before failing.
pub const WORKER_COMMAND_TIMEOUT: Duration = Duration::from_millis(100);
const WORKER_COMMAND_RETRY: Duration = Duration::from_millis(5);

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// The message could not be turned into JSON.
    Encode(String),
    /// The worker has not read a full queue of commands within the timeout.
    Backlogged,
    /// The writer thread is gone: the worker exited or its pipe broke.
    Closed,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(formatter, "the command could not be encoded: {error}"),
            Self::Backlogged => write!(
                formatter,
                "the worker has not read {WORKER_COMMAND_QUEUE} queued commands"
            ),
            Self::Closed => write!(formatter, "the worker command stream is closed"),
        }
    }
}

impl std::error::Error for SendError {}

pub struct JsonWriter {
    commands: RefCell<SyncSender<String>>,
}

impl JsonWriter {
    /// Starts a writer thread that owns `sink`.
    ///
    /// `name` appears in the thread name and in diagnostics.
    pub fn start<W>(sink: W, name: &str) -> io::Result<Self>
    where
        W: Write + Send + 'static,
    {
        let (commands, lines) = mpsc::sync_channel::<String>(WORKER_COMMAND_QUEUE);
        let diagnostics_name = name.to_owned();
        thread::Builder::new()
            .name(format!("{name}-commands"))
            .spawn(move || {
                let mut writer = BufWriter::new(sink);
                for line in lines {
                    if writer.write_all(line.as_bytes()).is_err()
                        || writer.write_all(b"\n").is_err()
                        || writer.flush().is_err()
                    {
                        crate::diagnostics::log_line(&format!(
                            "worker={diagnostics_name} command_stream=write_failed"
                        ));
                        break;
                    }
                }
            })?;
        Ok(Self {
            commands: RefCell::new(commands),
        })
    }

    /// Encodes and enqueues one command. Never blocks for longer than
    /// [`WORKER_COMMAND_TIMEOUT`].
    pub fn send<T: Serialize>(&self, message: &T) -> Result<(), SendError> {
        let mut line =
            serde_json::to_string(message).map_err(|error| SendError::Encode(error.to_string()))?;
        let deadline = Instant::now() + WORKER_COMMAND_TIMEOUT;
        loop {
            let outcome = self.commands.borrow().try_send(line);
            match outcome {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(SendError::Closed),
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return Err(SendError::Backlogged);
                    }
                    line = returned;
                }
            }
            thread::sleep(WORKER_COMMAND_RETRY);
        }
    }

    /// Points this writer at a restarted worker. Dropping the previous sender
    /// ends the thread that owned the previous pipe.
    pub fn replace_with(&self, replacement: Self) {
        *self.commands.borrow_mut() = replacement.commands.into_inner();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::{JsonWriter, SendError, WORKER_COMMAND_QUEUE};

    /// A sink that accepts nothing, the way a full pipe to a worker that
    /// stopped reading behaves.
    struct NeverAccepts;

    impl Write for NeverAccepts {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            std::thread::sleep(std::time::Duration::from_secs(3_600));
            Ok(0)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_sink_that_never_accepts_produces_an_error_rather_than_a_hang() {
        let writer = JsonWriter::start(NeverAccepts, "never-accepts").unwrap();
        let started = std::time::Instant::now();

        // The queue absorbs a bounded number of commands and then refuses.
        let outcome = (0..WORKER_COMMAND_QUEUE * 2)
            .map(|index| writer.send(&index))
            .find(Result::is_err);

        assert_eq!(outcome, Some(Err(SendError::Backlogged)));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "sending must fail quickly, took {:?}",
            started.elapsed()
        );
    }
}
