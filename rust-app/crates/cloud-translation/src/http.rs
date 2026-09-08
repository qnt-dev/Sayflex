//! Sending a plan over HTTPS.
//!
//! The only part of the crate that opens a socket, and the only part that
//! knows nothing about any particular service: it turns a plan into a request
//! and an answer into a status and a body, and hands both to the code that
//! does know which service replied. Everything decided here is transport —
//! how long to wait, and what a network fault means.

use std::{io::Read, time::Duration};

use reqwest::blocking::Client;

use crate::{
    failure::{ProviderFailure, classify_response},
    request::{RequestPlan, read_translation},
    router::RequestExecutor,
};

/// An upper bound on any single request. The walk hands down a smaller budget
/// whenever less of the translation's deadline is left.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// A reply larger than this is not a translation of a sentence or a paragraph.
/// It is read with a limit because how much a stranger sends is not the
/// stranger's decision to make.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

pub struct HttpExecutor {
    client: Client,
}

impl HttpExecutor {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .user_agent(format!(
                "{} translation",
                rust_app_core::product::USER_AGENT_PRODUCT
            ))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }
}

impl RequestExecutor for HttpExecutor {
    fn execute(&self, plan: &RequestPlan, budget: Duration) -> Result<String, ProviderFailure> {
        let mut request = self
            .client
            .post(&plan.url)
            .timeout(budget)
            .body(plan.body.clone());
        for (name, value) in &plan.headers {
            request = request.header(name, value);
        }

        let response = request.send().map_err(network_failure)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        // The body is read even for an error status: all three services explain
        // themselves there, and for Google the body is the only thing that
        // separates a wrong key from a spent allowance.
        let mut body = String::new();
        response
            .take(MAX_RESPONSE_BYTES)
            .read_to_string(&mut body)
            .map_err(|error| ProviderFailure::Unavailable(error.to_string()))?;

        if let Some(text) = read_translation(plan.shape, &body) {
            return Ok(text);
        }
        // A service that answered successfully and handed back nothing has
        // nothing to hand back. Asking the next one for the same text would
        // spend another allowance on the same answer, which is why `Empty`
        // stops the walk — and why it has to be reachable from here at all.
        // Without this the reply became `Rejected` and the walk carried on,
        // and the rule the router tests describe was one the product never
        // reached.
        if (200..300).contains(&status) {
            return Err(ProviderFailure::Empty);
        }
        Err(classify_response(
            plan.provider.as_str(),
            status,
            retry_after,
            &body,
        ))
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
/// date, and none of the three sends it.
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
            DEFAULT_TIMEOUT <= crate::router::TOTAL_DEADLINE,
            "one request cannot outlast the whole walk"
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
