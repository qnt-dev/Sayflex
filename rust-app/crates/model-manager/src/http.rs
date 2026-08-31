use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::{ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError};
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{RANGE, USER_AGENT},
};

const MAX_ATTEMPTS: u32 = 4;
const BUFFER_BYTES: usize = 1024 * 1024;

/// Stops an installation that is already running.
///
/// A download of several gigabytes owns its thread for minutes. Without this
/// the only ways out were waiting for it or killing the process, and a command
/// asking it to stop could not even be read.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Clears the flag before starting new work. A token that stays set would
    /// cancel the next installation as well.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Streams a verified artifact over HTTPS, resuming a partial file when the
/// server supports ranges.
pub struct HttpFetcher {
    client: Client,
    user_agent: String,
    cancellation: CancelToken,
}

impl HttpFetcher {
    /// `component` names the caller in the user agent, so a server log can tell
    /// a speech model download from a translation pack download.
    pub fn new(component: &str) -> Result<Self, ModelInstallError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60 * 6))
            .build()
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;
        Ok(Self {
            client,
            user_agent: format!("{} {component}", rust_app_core::product::USER_AGENT_PRODUCT),
            cancellation: CancelToken::default(),
        })
    }

    /// Handle used by another thread to stop an installation in flight.
    #[must_use]
    pub fn cancellation(&self) -> CancelToken {
        self.cancellation.clone()
    }
}

impl ArtifactFetcher for HttpFetcher {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        expected_bytes: u64,
        progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), ModelInstallError> {
        let mut last_error = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            if self.cancellation.is_cancelled() {
                return Err(ModelInstallError::Cancelled);
            }
            match self.fetch_attempt(url, destination, expected_bytes, progress) {
                Ok(()) => return Ok(()),
                Err(ModelInstallError::Download(error)) => last_error = error,
                Err(error) => return Err(error),
            }
            if attempt + 1 < MAX_ATTEMPTS {
                thread::sleep(Duration::from_millis(350 * 2_u64.pow(attempt)));
            }
        }
        Err(ModelInstallError::Download(last_error))
    }
}

impl HttpFetcher {
    fn fetch_attempt(
        &self,
        url: &str,
        destination: &Path,
        expected_bytes: u64,
        progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), ModelInstallError> {
        let mut offset = destination.metadata().map_or(0, |metadata| metadata.len());
        if offset == expected_bytes {
            return Ok(());
        }
        if offset > expected_bytes {
            File::create(destination)?;
            offset = 0;
        }

        let mut request = self.client.get(url).header(USER_AGENT, &self.user_agent);
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let mut response = request
            .send()
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;

        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && offset == expected_bytes {
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(ModelInstallError::Download(format!(
                "server returned {}",
                response.status()
            )));
        }

        let resumed = offset > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
        if offset > 0 && !resumed {
            offset = 0;
        }
        let mut file = if resumed {
            OpenOptions::new().append(true).open(destination)?
        } else {
            File::create(destination)?
        };
        let mut received = offset;
        progress(DownloadProgress {
            phase: InstallPhase::Downloading,
            bytes_received: received,
            total_bytes: expected_bytes,
        });
        let mut buffer = vec![0_u8; BUFFER_BYTES];
        loop {
            let count = response
                .read(&mut buffer)
                .map_err(|error| ModelInstallError::Download(error.to_string()))?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])?;
            if self.cancellation.is_cancelled() {
                // The partial file stays on disk: a later attempt resumes from
                // exactly here instead of starting the download again.
                file.flush()?;
                return Err(ModelInstallError::Cancelled);
            }
            received = received.saturating_add(count as u64);
            if received > expected_bytes {
                return Err(ModelInstallError::SizeMismatch {
                    expected: expected_bytes,
                    actual: received,
                });
            }
            progress(DownloadProgress {
                phase: InstallPhase::Downloading,
                bytes_received: received,
                total_bytes: expected_bytes,
            });
        }
        file.flush()?;
        if received != expected_bytes {
            return Err(ModelInstallError::Download(format!(
                "connection ended after {received} of {expected_bytes} bytes"
            )));
        }
        Ok(())
    }
}
