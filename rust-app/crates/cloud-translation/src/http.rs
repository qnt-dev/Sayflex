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
    request::{ReadAnswer, RequestPlan, read_answer},
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

#[derive(Clone)]
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
        // The body is read even for an error status: both services explain
        // themselves there, and the detail distinguishes a wrong key from a
        // spent allowance.
        let mut body = String::new();
        response
            .take(MAX_RESPONSE_BYTES)
            .read_to_string(&mut body)
            .map_err(|error| ProviderFailure::Unavailable(error.to_string()))?;

        // The status decides first. Reading the body for a translation before
        // looking at the status meant a 500 whose body happened to fit the
        // shape was accepted as a success, and — worse — that any unparseable
        // 200 became `Empty`, which stops the walk: one proxy login page from
        // DeepL, Azure, and the local engine were never asked.
        if !(200..300).contains(&status) {
            return Err(classify_response(
                plan.provider.as_str(),
                status,
                retry_after,
                &body,
            ));
        }
        match read_answer(plan.shape, &body) {
            ReadAnswer::Translated(text) => Ok(text),
            // Parsed, understood, and carrying nothing. Only this stops the
            // walk, because only this is the service telling us something true
            // about the text rather than about itself.
            ReadAnswer::Empty => Err(ProviderFailure::Empty),
            // A success status wrapped around something this service does not
            // answer in. That is the service being broken, not the text being
            // untranslatable, so the next one is asked.
            ReadAnswer::Malformed(detail) => Err(ProviderFailure::Unavailable(detail)),
        }
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
    use crate::{
        ProviderFailure,
        request::{ReadAnswer, ResponseShape, read_answer},
    };

    /// What `execute` decides once a reply is in hand.
    ///
    /// The socket is the only part of `execute` that cannot be exercised
    /// without a network, so the decision it makes afterwards is written here
    /// exactly as it is written there.
    fn outcome(status: u16, body: &str) -> Result<String, ProviderFailure> {
        if !(200..300).contains(&status) {
            return Err(crate::failure::classify_response(
                "deepl", status, None, body,
            ));
        }
        match read_answer(ResponseShape::DeepL, body) {
            ReadAnswer::Translated(text) => Ok(text),
            ReadAnswer::Empty => Err(ProviderFailure::Empty),
            ReadAnswer::Malformed(detail) => Err(ProviderFailure::Unavailable(detail)),
        }
    }

    #[test]
    fn a_broken_answer_and_an_empty_one_are_told_apart_and_both_move_on() {
        // The failure this replaces: any 200 that could not be read became
        // `Empty`, and `Empty` ends the walk — so one login page from a captive
        // proxy meant Azure and the local engine were never asked.
        for broken in [
            "<html><body>Sign in to your proxy</body></html>",
            "{\"unexpected\":\"schema\"}",
            "",
        ] {
            let failure = outcome(200, broken).unwrap_err();
            assert!(
                failure.should_fail_over(),
                "a service answering nonsense must not stop the walk: {broken:?}"
            );
        }

        let empty = outcome(200, "{\"translations\":[{\"text\":\"\"}]}").unwrap_err();
        assert_eq!(
            empty,
            ProviderFailure::Empty,
            "a parsed answer carrying no text is not the same as an unreadable one"
        );
        assert!(
            empty.should_fail_over(),
            "and it is still this service failing, so the next one is asked"
        );
    }

    #[test]
    fn a_failing_status_is_never_read_as_a_success() {
        // A 500 whose body happens to fit the shape was accepted as a
        // translation, because the body was read before the status.
        let failure = outcome(500, "{\"translations\":[{\"text\":\"nonsense\"}]}").unwrap_err();

        assert!(
            !matches!(failure, ProviderFailure::Empty),
            "a server error is a server error whatever its body says: {failure:?}"
        );
    }

    #[test]
    fn a_real_answer_is_read() {
        assert_eq!(
            outcome(200, "{\"translations\":[{\"text\":\"привет\"}]}").unwrap(),
            "привет"
        );
    }

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
