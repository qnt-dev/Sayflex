//! Performing a request plan over HTTPS.
//!
//! This is the only part of the crate that opens a socket, and it knows
//! nothing about any particular service: it turns a plan into a request, a
//! response into a status and a body, and hands both to the reader that does
//! know. Everything decided here is transport — timeouts, proxies, and how a
//! network fault maps onto a provider failure.

use std::time::Duration;

use reqwest::blocking::{Client, multipart};

use crate::{
    failure::ProviderFailure,
    provider::ProviderId,
    request::{RequestBody, RequestPlan, read_transcript},
    router::RequestExecutor,
};

/// A dictation clip is seconds long. A service that has not answered within
/// this is not going to answer usefully: the user is waiting with their words
/// undelivered, and the next provider in the list deserves the turn.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HttpExecutor {
    client: Client,
}

impl HttpExecutor {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .user_agent(format!(
                "{} transcription",
                rust_app_core::product::USER_AGENT_PRODUCT
            ))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }
}

impl RequestExecutor for HttpExecutor {
    fn execute(&self, id: &ProviderId, plan: RequestPlan) -> Result<String, ProviderFailure> {
        let mut request = self.client.post(&plan.url);
        for (name, value) in &plan.headers {
            request = request.header(name, value);
        }
        request = match plan.body {
            RequestBody::Raw { bytes, .. } => request.body(bytes),
            RequestBody::Form {
                file_name,
                file_content_type,
                audio,
                fields,
            } => {
                let part = multipart::Part::bytes(audio)
                    .file_name(file_name)
                    .mime_str(file_content_type)
                    .map_err(|error| ProviderFailure::Rejected(error.to_string()))?;
                let mut form = multipart::Form::new().part("file", part);
                for (name, value) in fields {
                    form = form.text(name, value);
                }
                request.multipart(form)
            }
        };

        let response = request.send().map_err(network_failure)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        // The body is read even for an error status: every service in the
        // catalogue explains itself there, and one of them reports failures
        // with a success status.
        let body = response.text().map_err(network_failure)?;

        read_transcript(id, status, retry_after, &body)
    }
}

fn network_failure(error: reqwest::Error) -> ProviderFailure {
    if error.is_timeout() {
        return ProviderFailure::Unavailable("the service did not answer in time".to_owned());
    }
    if error.is_connect() {
        return ProviderFailure::Unavailable("the service could not be reached".to_owned());
    }
    ProviderFailure::Unavailable(error.to_string())
}

/// Reads the seconds form of `Retry-After`. The HTTP-date form is ignored
/// rather than parsed: the fallback cooldown is a better answer than a wrong
/// date, and no service in the catalogue sends it.
fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DEFAULT_TIMEOUT, HttpExecutor, parse_retry_after};

    #[test]
    fn a_client_is_built_with_a_bounded_wait() {
        assert!(HttpExecutor::new(DEFAULT_TIMEOUT).is_ok());
        assert!(
            DEFAULT_TIMEOUT <= Duration::from_secs(30),
            "a dictation cannot wait longer"
        );
    }

    #[test]
    fn retry_after_is_read_in_seconds_and_otherwise_left_to_the_default() {
        assert_eq!(parse_retry_after("12"), Some(Duration::from_secs(12)));
        assert_eq!(parse_retry_after("  3 "), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
