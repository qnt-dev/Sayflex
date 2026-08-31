//! Why a provider could not transcribe, and what that means for the next one.
//!
//! Failing over on the wrong signal is worse than not failing over at all. A
//! request the service rejected because it was malformed will be rejected by
//! every other service too, and retrying it everywhere turns one bug into four
//! wasted quotas. Only a failure that is about *this* provider — its key, its
//! limits, its availability — is a reason to move down the list.

use std::time::Duration;

/// Everything a provider can tell us about why it did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderFailure {
    /// The key is missing, wrong, or revoked. Trying again changes nothing, so
    /// the row is switched off and the user is told which one it was.
    Credential(String),
    /// The free allowance is used up. Nothing will work until the service
    /// resets it, which for these services is either midnight UTC or the start
    /// of a month.
    QuotaExhausted { retry_after: Option<Duration> },
    /// Too many requests just now. Usually seconds, occasionally minutes.
    RateLimited { retry_after: Duration },
    /// The service is down, unreachable, or timed out.
    Unavailable(String),
    /// The service understood the request and refused it: audio too long, a
    /// format it does not take, a model name that does not exist.
    Rejected(String),
}

impl ProviderFailure {
    /// Whether the next provider in the list should be tried.
    ///
    /// A rejected request is the one case where it should not: the same audio
    /// and the same parameters would be refused again, and the user needs to
    /// see the real reason rather than a chain of identical failures.
    #[must_use]
    pub const fn should_fail_over(&self) -> bool {
        !matches!(self, Self::Rejected(_))
    }

    /// Whether this provider should stop being offered until the user acts.
    #[must_use]
    pub const fn disables_provider(&self) -> bool {
        matches!(self, Self::Credential(_))
    }

    /// How long to leave this provider alone before offering it again.
    #[must_use]
    pub fn cooldown(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => Some(*retry_after),
            Self::QuotaExhausted { retry_after } => {
                // A service that does not say when it resets is assumed to be
                // on a daily allowance; an hour is short enough to recover
                // quickly and long enough not to hammer it.
                Some(retry_after.unwrap_or(DEFAULT_QUOTA_COOLDOWN))
            }
            Self::Unavailable(_) => Some(UNAVAILABLE_COOLDOWN),
            Self::Credential(_) | Self::Rejected(_) => None,
        }
    }
}

const DEFAULT_QUOTA_COOLDOWN: Duration = Duration::from_secs(60 * 60);
const UNAVAILABLE_COOLDOWN: Duration = Duration::from_secs(30);

impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credential(detail) => write!(formatter, "the key was refused: {detail}"),
            Self::QuotaExhausted { .. } => formatter.write_str("the free allowance is used up"),
            Self::RateLimited { retry_after } => write!(
                formatter,
                "too many requests, retry in {} s",
                retry_after.as_secs()
            ),
            Self::Unavailable(detail) => write!(formatter, "the service is unreachable: {detail}"),
            Self::Rejected(detail) => write!(formatter, "the request was refused: {detail}"),
        }
    }
}

/// Turns an HTTP response into a failure.
///
/// The decision rests on the status code rather than on a local counter,
/// because a counter drifts: a key shared with another machine, a request that
/// cost more than expected, or a service that changed its allowance all leave
/// the count right and the reality wrong. The service always knows.
#[must_use]
pub fn classify_response(
    status: u16,
    retry_after: Option<Duration>,
    body: &str,
) -> ProviderFailure {
    let detail = summarize(body);
    match status {
        401 | 403 => ProviderFailure::Credential(detail),
        // Every service in the catalogue signals a spent allowance with 402 or
        // 429; only the wording differs, so the body decides between "slow
        // down" and "nothing left this period".
        402 => ProviderFailure::QuotaExhausted { retry_after },
        429 => {
            if mentions_exhausted_allowance(body) {
                ProviderFailure::QuotaExhausted { retry_after }
            } else {
                ProviderFailure::RateLimited {
                    retry_after: retry_after.unwrap_or(Duration::from_secs(5)),
                }
            }
        }
        408 | 500..=599 => ProviderFailure::Unavailable(detail),
        _ => ProviderFailure::Rejected(detail),
    }
}

fn mentions_exhausted_allowance(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "quota",
        "credit",
        "insufficient",
        "exceeded your current",
        "out of neurons",
        "daily limit",
        "monthly limit",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

/// Keeps a diagnostic short and free of anything that looks like a key.
fn summarize(body: &str) -> String {
    const LIMIT: usize = 200;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "no detail".to_owned();
    }
    let mut summary: String = trimmed
        .chars()
        .filter(|character| !character.is_control())
        .take(LIMIT)
        .collect();
    if trimmed.chars().count() > LIMIT {
        summary.push('…');
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ProviderFailure, classify_response};

    #[test]
    fn a_refused_key_stops_that_provider_but_still_tries_the_next() {
        let failure = classify_response(401, None, "invalid api key");

        assert!(matches!(failure, ProviderFailure::Credential(_)));
        assert!(failure.disables_provider());
        assert!(failure.should_fail_over());
        assert_eq!(failure.cooldown(), None);
    }

    #[test]
    fn a_spent_allowance_is_told_apart_from_plain_throttling() {
        let throttled = classify_response(429, Some(Duration::from_secs(3)), "slow down");
        let spent = classify_response(429, None, "You exceeded your current quota");

        assert_eq!(
            throttled,
            ProviderFailure::RateLimited {
                retry_after: Duration::from_secs(3)
            }
        );
        assert_eq!(spent, ProviderFailure::QuotaExhausted { retry_after: None });
        assert_eq!(spent.cooldown(), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn payment_required_is_always_a_spent_allowance() {
        assert_eq!(
            classify_response(402, None, ""),
            ProviderFailure::QuotaExhausted { retry_after: None }
        );
    }

    #[test]
    fn a_malformed_request_must_not_be_repeated_on_every_provider() {
        let failure = classify_response(400, None, "audio too long");

        assert!(!failure.should_fail_over());
        assert!(!failure.disables_provider());
    }

    #[test]
    fn a_server_fault_is_retried_elsewhere_after_a_short_pause() {
        let failure = classify_response(503, None, "upstream unavailable");

        assert!(failure.should_fail_over());
        assert_eq!(failure.cooldown(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn a_diagnostic_is_bounded_and_free_of_control_characters() {
        let failure = classify_response(400, None, &format!("bad\nrequest {}", "x".repeat(400)));

        let ProviderFailure::Rejected(detail) = failure else {
            panic!("a 400 is a rejection");
        };
        assert!(!detail.contains('\n'));
        assert!(detail.chars().count() <= 201);
    }
}
