use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    thread,
    time::Duration,
};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{RANGE, USER_AGENT},
};
use rust_model_manager::{ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError};

const MAX_ATTEMPTS: u32 = 4;
const BUFFER_BYTES: usize = 1024 * 1024;

pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, ModelInstallError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60 * 6))
            .build()
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;
        Ok(Self { client })
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

        let mut request = self
            .client
            .get(url)
            .header(USER_AGENT, "RustVoice/0.5 model-manager");
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
