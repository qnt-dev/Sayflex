use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError};
use reqwest::{
    Client, StatusCode,
    header::{RANGE, USER_AGENT},
};
use tokio::runtime::Runtime;

const MAX_ATTEMPTS: u32 = 4;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

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
    runtime: Runtime,
    user_agent: String,
    cancellation: CancelToken,
    https_only: bool,
}

impl HttpFetcher {
    /// Requires HTTPS for the initial URL and every redirect.
    pub fn https_only(component: &str) -> Result<Self, ModelInstallError> {
        Self::build(component, true)
    }

    /// `component` names the caller in the user agent, so a server log can tell
    /// a speech model download from a translation pack download.
    ///
    /// Every download requires HTTPS, for the first URL and each redirect. The
    /// SHA-256 pins protect the bytes; HTTPS also keeps which model a user
    /// fetches off the network path.
    pub fn new(component: &str) -> Result<Self, ModelInstallError> {
        Self::build(component, true)
    }

    /// Plain HTTP, for a test server on the loopback interface only.
    #[cfg(test)]
    fn allowing_plain_http(component: &str) -> Result<Self, ModelInstallError> {
        Self::build(component, false)
    }

    fn build(component: &str, https_only: bool) -> Result<Self, ModelInstallError> {
        let client = Client::builder()
            .https_only(https_only)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60 * 60 * 6))
            .build()
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ModelInstallError::Download(error.to_string()))?;
        Ok(Self {
            client,
            runtime,
            user_agent: format!("{} {component}", rust_app_core::product::USER_AGENT_PRODUCT),
            cancellation: CancelToken::default(),
            https_only,
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
        if self.https_only && reqwest::Url::parse(url).map_or(true, |url| url.scheme() != "https") {
            return Err(ModelInstallError::Download(
                "this component requires HTTPS".into(),
            ));
        }
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
                let backoff = Duration::from_millis(350 * 2_u64.pow(attempt));
                self.runtime.block_on(async {
                    tokio::select! {
                        () = tokio::time::sleep(backoff) => Ok(()),
                        () = wait_until_cancelled(&self.cancellation) => {
                            Err(ModelInstallError::Cancelled)
                        }
                    }
                })?;
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
        let mut response = self.runtime.block_on(async {
            tokio::select! {
                response = request.send() => response
                    .map_err(|error| ModelInstallError::Download(error.to_string())),
                () = wait_until_cancelled(&self.cancellation) => {
                    Err(ModelInstallError::Cancelled)
                }
            }
        })?;

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
        loop {
            let chunk = self.runtime.block_on(async {
                tokio::select! {
                    chunk = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, response.chunk()) => {
                        match chunk {
                            Ok(Ok(chunk)) => Ok(chunk),
                            Ok(Err(error)) => Err(ModelInstallError::Download(error.to_string())),
                            Err(_) => Err(ModelInstallError::Download(format!(
                                "download received no data for {} seconds",
                                DOWNLOAD_IDLE_TIMEOUT.as_secs()
                            ))),
                        }
                    }
                    () = wait_until_cancelled(&self.cancellation) => {
                        Err(ModelInstallError::Cancelled)
                    }
                }
            })?;
            let Some(chunk) = chunk else {
                break;
            };
            file.write_all(&chunk)?;
            received = received.saturating_add(chunk.len() as u64);
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

async fn wait_until_cancelled(cancellation: &CancelToken) {
    while !cancellation.is_cancelled() {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::Duration,
    };

    use tempfile::tempdir;

    use super::HttpFetcher;
    use crate::{ArtifactFetcher, ModelInstallError};

    #[test]
    fn https_only_fetcher_rejects_http_even_when_a_partial_file_is_complete() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("complete.bin");
        std::fs::write(&destination, b"complete").unwrap();
        let fetcher = HttpFetcher::https_only("cuda-runtime-test").unwrap();
        let result = fetcher.fetch(
            "http://127.0.0.1:9/runtime.zip",
            &destination,
            8,
            &mut |_| {},
        );
        assert!(
            matches!(result, Err(ModelInstallError::Download(message)) if message.contains("HTTPS"))
        );
    }

    #[test]
    fn every_fetcher_rejects_plain_http() {
        let directory = tempdir().unwrap();
        let fetcher = HttpFetcher::new("model-test").unwrap();
        let result = fetcher.fetch(
            "http://127.0.0.1:9/model.bin",
            &directory.path().join("model.bin"),
            8,
            &mut |_| {},
        );
        assert!(
            matches!(result, Err(ModelInstallError::Download(message)) if message.contains("HTTPS"))
        );
    }

    #[test]
    fn cancellation_interrupts_a_stalled_response_body() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (headers_sent, headers_received) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2_048];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
                .unwrap();
            stream.flush().unwrap();
            headers_sent.send(()).unwrap();
            thread::sleep(Duration::from_secs(5));
        });

        let fetcher = HttpFetcher::allowing_plain_http("cancel-test").unwrap();
        let cancellation = fetcher.cancellation();
        let directory = tempdir().unwrap();
        let destination = directory.path().join("partial.bin");
        let (result_sent, result_received) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = fetcher.fetch(
                &format!("http://{address}/model"),
                &destination,
                10,
                &mut |_| {},
            );
            result_sent.send(result).unwrap();
        });

        headers_received
            .recv_timeout(Duration::from_secs(1))
            .expect("server response headers");
        cancellation.cancel();
        let result = result_received
            .recv_timeout(Duration::from_secs(1))
            .expect("cancelled download must not wait for the network timeout");
        assert!(matches!(result, Err(ModelInstallError::Cancelled)));
    }
}
