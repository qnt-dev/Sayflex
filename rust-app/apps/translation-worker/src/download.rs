use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use reqwest::blocking::Client;
use rust_model_manager::{ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError};

pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, ModelInstallError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60 * 6))
            .user_agent("RustVoice/0.3 translation-model-manager")
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
        let mut response = self
            .client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;
        let mut file = File::create(destination)?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        let mut received = 0_u64;
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
        Ok(())
    }
}
