//! Why a provider could not transcribe, and what that means for the next one.
//!
//! Two decisions come out of a failure: whether to try the next provider, and
//! how long to leave this one alone. Both are taken from what the service
//! answered rather than from a local count of what it should have allowed,
//! because a count drifts — a key used from two machines, a request that cost
//! more than expected, an allowance the service quietly changed — while the
//! service always knows.
//!
//! Requests that a service refuses are still passed down the list. Providers
//! are not interchangeable: a clip too long for one is ordinary for the next,
//! and stopping the walk would strand a recording that the local engine, often
//! sitting further down, could have handled. Sending audio nobody will accept
//! to every provider in turn is avoided earlier instead, by [`crate::capability`].

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
    /// The service understood the request and refused it: a format it does not
    /// take, a model name that does not exist, a limit this build did not know
    /// about.
    Rejected(String),
    /// The service heard no speech. Not a fault, and not worth asking anyone
    /// else: the same silence would come back.
    NoSpeech,
}

impl ProviderFailure {
    /// Whether the next provider in the list should be tried.
    ///
    /// Silence is the only reason not to: every other service would hear the
    /// same nothing. A rejection *does* move on, because providers differ in
    /// what they accept — a clip too long for one is ordinary for the next —
    /// and refusing to continue would strand the recording even when the local
    /// engine is sitting further down the list.
    #[must_use]
    pub const fn should_fail_over(&self) -> bool {
        !matches!(self, Self::NoSpeech)
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
            Self::Credential(_) | Self::Rejected(_) | Self::NoSpeech => None,
        }
    }
}

const DEFAULT_QUOTA_COOLDOWN: Duration = Duration::from_secs(60 * 60);
const UNAVAILABLE_COOLDOWN: Duration = Duration::from_secs(30);
/// A service can ask to be left alone for a while; it cannot ask to be left
/// alone until the process restarts, and an unbounded value would also
/// overflow the instant it is added to.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

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
            Self::NoSpeech => formatter.write_str("no speech was recognized"),
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
    let retry_after = retry_after.map(|delay| delay.min(MAX_RETRY_AFTER));
    match status {
        401 => ProviderFailure::Credential(detail),
        // A 403 is not always a bad key: Cloudflare also uses it for account,
        // plan and model restrictions, which replacing the key will not fix.
        403 => {
            if mentions_authentication(body) {
                ProviderFailure::Credential(detail)
            } else {
                ProviderFailure::Rejected(detail)
            }
        }
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

fn mentions_authentication(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "authentication",
        "unauthorized",
        "invalid api key",
        "invalid token",
    ]
    .iter()
    .any(|marker| body.contains(marker))
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

/// Removes every occurrence of a secret from a diagnostic before it is stored
/// or printed.
///
/// A service is trusted to answer, not to answer safely: a custom endpoint is
/// whatever address the user typed, and echoing the key back inside an error
/// body would otherwise put it in the log.
#[must_use]
pub fn redact_secret(text: &str, secret: &str) -> String {
    if secret.len() < MIN_SECRET_FRAGMENT {
        return text.to_owned();
    }
    let mut redacted = text.replace(secret, "<redacted>");
    // `summarize` keeps the first characters of a reply and marks the cut with
    // an ellipsis. A key that straddled that limit survives only as a prefix,
    // which the whole-key replacement above cannot match.
    if let Some(kept) = redacted.strip_suffix('…') {
        let fragment = (MIN_SECRET_FRAGMENT..secret.len())
            .rev()
            .filter(|length| secret.is_char_boundary(*length))
            .find(|length| kept.ends_with(&secret[..*length]));
        if let Some(length) = fragment {
            redacted = format!("{}<redacted>…", &kept[..kept.len() - length]);
        }
    }
    redacted
}

/// The shortest key prefix treated as a secret when a reply was cut inside it.
const MIN_SECRET_FRAGMENT: usize = 4;

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
    fn a_refused_request_still_moves_down_the_list() {
        // Providers differ in what they accept: a clip one refuses is
        // ordinary for the next, and the local engine is often below them.
        let failure = classify_response(400, None, "audio too long");

        assert!(failure.should_fail_over());
        assert!(!failure.disables_provider());
        assert_eq!(failure.cooldown(), None, "and it is not held against them");
    }

    #[test]
    fn silence_is_the_only_answer_that_ends_the_walk() {
        assert!(!ProviderFailure::NoSpeech.should_fail_over());
        assert!(!ProviderFailure::NoSpeech.disables_provider());
    }

    #[test]
    fn a_forbidden_response_is_only_a_key_problem_when_it_says_so() {
        let key = classify_response(403, None, "Authentication error");
        let plan = classify_response(403, None, "This model is not available on your plan");

        assert!(matches!(key, ProviderFailure::Credential(_)));
        assert!(
            matches!(plan, ProviderFailure::Rejected(_)),
            "replacing the key would not fix an account restriction"
        );
    }

    #[test]
    fn a_service_cannot_ask_to_be_left_alone_indefinitely() {
        let failure = classify_response(429, Some(Duration::from_secs(60 * 60 * 24)), "slow down");

        assert_eq!(failure.cooldown(), Some(Duration::from_secs(60 * 60)));
    }

    #[test]
    fn a_secret_echoed_back_by_a_service_is_removed_from_diagnostics() {
        let leaked = "the key gsk_secret_value is not valid";

        assert_eq!(
            super::redact_secret(leaked, "gsk_secret_value"),
            "the key <redacted> is not valid"
        );
        // Too short to match anything meaningful; replacing it would mangle
        // ordinary text instead.
        assert_eq!(super::redact_secret(leaked, "is"), leaked);
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

#[cfg(test)]
mod truncated_secret_tests {
    use super::{classify_response, redact_secret};

    #[test]
    fn a_key_cut_by_the_summary_limit_is_still_redacted() {
        let key = "gsk_0123456789abcdefghijklmnopqrstuv";
        let body = format!("{}{key} was rejected", "x".repeat(180));
        let detail = match classify_response(401, None, &body) {
            crate::ProviderFailure::Credential(detail) => detail,
            other => panic!("unexpected {other:?}"),
        };
        assert!(detail.ends_with('…'), "the summary was cut inside the key");

        let redacted = redact_secret(&detail, key);

        assert!(!redacted.contains("gsk_0123"), "{redacted}");
        assert!(redacted.ends_with("<redacted>…"), "{redacted}");
    }

    #[test]
    fn a_whole_key_is_redacted_and_ordinary_text_is_left_alone() {
        let key = "gsk_0123456789abcdefghij";
        assert_eq!(
            redact_secret(&format!("bad key {key}"), key),
            "bad key <redacted>"
        );
        assert_eq!(redact_secret("quota exceeded…", key), "quota exceeded…");
    }
}
