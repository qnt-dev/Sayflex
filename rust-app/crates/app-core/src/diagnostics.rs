use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024;
const LOG_QUEUE_CAPACITY: usize = 2_048;
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(250);

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

pub fn initialize(log_directory: &Path, component: &str) -> io::Result<()> {
    if LOGGER.get().is_some() {
        return Ok(());
    }

    fs::create_dir_all(log_directory)?;
    let path = log_directory.join(format!("{component}.log"));
    let logger = FileLogger::open(path)?;
    let _ = LOGGER.set(logger);
    Ok(())
}

pub fn log_line(message: &str) {
    if let Some(logger) = LOGGER.get() {
        let _ = logger.write(message);
    }
}

struct FileLogger {
    sender: SyncSender<LogCommand>,
}

enum LogCommand {
    Line(String),
    Flush(SyncSender<()>),
}

impl FileLogger {
    fn open(path: PathBuf) -> io::Result<Self> {
        let mut state = LogState {
            path,
            file: None,
            bytes_written: 0,
        };
        state.open_current()?;
        let (sender, receiver) = sync_channel(LOG_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("diagnostic-log-writer".to_owned())
            .spawn(move || run_writer(state, &receiver))
            .map_err(|error| io::Error::other(format!("could not start log writer: {error}")))?;
        Ok(Self { sender })
    }

    fn write(&self, message: &str) -> io::Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sanitized = sanitize(message);
        let line = format!("{timestamp} {sanitized}\n");
        match self.sender.try_send(LogCommand::Line(line)) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "log writer stopped",
            )),
        }
    }

    fn flush(&self) -> io::Result<()> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
        self.sender
            .send(LogCommand::Flush(sender))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped"))?;
        receiver
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped"))
    }
}

fn run_writer(mut state: LogState, receiver: &Receiver<LogCommand>) {
    loop {
        match receiver.recv_timeout(LOG_FLUSH_INTERVAL) {
            Ok(LogCommand::Line(line)) => {
                let _ = state.write_line(&line);
            }
            Ok(LogCommand::Flush(acknowledge)) => {
                let _ = state.flush();
                let _ = acknowledge.send(());
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = state.flush();
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = state.flush();
                return;
            }
        }
    }
}

/// Durably drains all diagnostic lines queued before this call.
///
/// Lifecycle and panic paths use this explicitly because a process-wide static
/// logger is not dropped during normal process teardown.
pub fn flush() -> io::Result<()> {
    LOGGER.get().map_or(Ok(()), FileLogger::flush)
}

struct LogState {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    bytes_written: u64,
}

impl LogState {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.bytes_written.saturating_add(line.len() as u64) > MAX_LOG_BYTES {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("diagnostic log is not open"))?;
        file.write_all(line.as_bytes())?;
        self.bytes_written = self.bytes_written.saturating_add(line.len() as u64);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("diagnostic log is not open"))?
            .flush()
    }

    fn open_current(&mut self) -> io::Result<()> {
        self.bytes_written = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if self.bytes_written >= MAX_LOG_BYTES {
            self.rotate_files()?;
            self.bytes_written = 0;
        }
        self.file = Some(BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        ));
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        self.file.take();
        self.rotate_files()?;
        self.file = Some(BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        ));
        self.bytes_written = 0;
        Ok(())
    }

    fn rotate_files(&self) -> io::Result<()> {
        let first = rotated_path(&self.path, 1);
        let second = rotated_path(&self.path, 2);
        if second.exists() {
            fs::remove_file(&second)?;
        }
        if first.exists() {
            fs::rename(&first, &second)?;
        }
        if self.path.exists() {
            fs::rename(&self.path, &first)?;
        }
        Ok(())
    }
}

fn rotated_path(path: &Path, generation: u8) -> PathBuf {
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(format!(".{generation}"));
    PathBuf::from(rotated)
}

fn sanitize(message: &str) -> String {
    let mut result = String::with_capacity(message.len().min(MAX_LINE_BYTES));
    for character in message.chars() {
        if result.len() >= MAX_LINE_BYTES {
            result.push('…');
            break;
        }
        match character {
            '\r' | '\n' | '\0' => result.push(' '),
            character if character.is_control() => result.push('?'),
            character => result.push(character),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{FileLogger, MAX_LOG_BYTES, rotated_path, sanitize};

    #[test]
    fn sanitizes_control_characters_and_newlines() {
        assert_eq!(sanitize("one\r\ntwo\0\u{7}"), "one  two ?");
    }

    #[test]
    fn rotates_without_writing_outside_the_requested_log_family() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input-host.log");
        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();

        let logger = FileLogger::open(path.clone()).unwrap();
        logger.write("ready").unwrap();
        logger.flush().unwrap();

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(!rotated_path(&path, 2).exists());
        assert!(std::fs::read_to_string(path).unwrap().ends_with(" ready\n"));
    }
}
