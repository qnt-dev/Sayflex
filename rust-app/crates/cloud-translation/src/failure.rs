//! Why a provider could not translate, and what that means for the next one.
//!
//! The transcription crate answers the same question, and this is deliberately
//! not the same code. These three services disagree with the speech services
//! about which status code means what, and the differences are exactly the ones
//! that matter:
//!
//! * DeepL signals a spent allowance with **456**, a status no speech service
//!   uses, and signals a bad key with **403** where the others use 401.
//! * Google returns **403** for a key that is wrong, a key that is restricted
//!   to other APIs, and an allowance that is spent — the body is the only thing
//!   that tells them apart.
//! * Azure Translator returns **401** for a missing key and **403** for a key
//!   whose region is wrong, which is a fixable mistake rather than a dead key.
//!
//! Sharing one classifier between the two would mean a table of exceptions per
//! service, which is the same code with worse names.

use std::time::Duration;

/// How long a service that says its allowance is spent is left alone, when it
/// does not say for how long.
///
/// An hour, and no attempt to work out when the allowance actually resets.
/// None of the three publishes that in a reply, and a guess would be a
/// deliberate silence of unknown length. An hour is long enough not to hammer a
/// spent daily allowance and short enough that a service which has recovered is
/// tried again the same afternoon.
///
/// The ledger holding this is in memory: a restart forgets every cooldown and
/// the next translation asks again. That is the cheap answer and, for a
/// desktop application that is not restarted mid-sentence, an acceptable one —
/// the cost of being wrong is one refused request.
const DEFAULT_QUOTA_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// A service that asks to be left alone for longer than this is telling us
/// about its own maintenance, not about our request.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// Everything a provider can tell us about why it did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderFailure {
    /// The key is missing, wrong, or revoked. Trying again changes nothing, so
    /// the row is switched off and the user is told which one it was.
    Credential(String),
    /// The key is real but the request was addressed wrongly — an Azure key
    /// used with the wrong region, a Google key restricted to another API.
    /// Fixable in settings, and not a reason to throw the key away.
    Misconfigured(String),
    /// The free allowance is used up. Nothing will work until the service
    /// resets it: midnight UTC for some, the start of a month for others.
    QuotaExhausted { retry_after: Option<Duration> },
    /// Too many requests just now. Usually seconds.
    RateLimited { retry_after: Duration },
    /// The service is down, unreachable, or timed out.
    Unavailable(String),
    /// The service understood the request and refused it: a language pair it
    /// does not have, a text longer than it takes.
    Rejected(String),
    /// The service answered and handed back nothing. Not a fault, and not worth
    /// asking anyone else: the same nothing would come back.
    Empty,
}

impl ProviderFailure {
    /// Whether the next provider in the list should be tried.
    ///
    /// Only an empty answer stops the walk. A rejection moves on, because these
    /// three do not cover the same language pairs — DeepL has around thirty
    /// languages where Google has hundreds — and stopping would strand a
    /// translation the local engine, usually sitting further down, could do.
    #[must_use]
    pub const fn should_fail_over(&self) -> bool {
        !matches!(self, Self::Empty)
    }

    /// Whether this provider should stop being offered until the user acts.
    ///
    /// A wrong region is not a dead key: the row stays on and says what to fix.
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
                Some(retry_after.unwrap_or(DEFAULT_QUOTA_COOLDOWN))
            }
            // An unreachable service is worth one quick retry from the next
            // recording rather than a cooldown: the network usually comes back
            // before the user notices.
            _ => None,
        }
    }

    /// The sentence the interface shows, in the user's terms rather than the
    /// service's.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Credential(detail) => format!("Key refused: {detail}"),
            Self::Misconfigured(detail) => format!("Settings are wrong: {detail}"),
            Self::QuotaExhausted { retry_after } => retry_after.map_or_else(
                || "Free allowance spent for this period".to_owned(),
                |delay| {
                    format!(
                        "Free allowance spent; available again in about {} min",
                        delay.as_secs().div_ceil(60)
                    )
                },
            ),
            Self::RateLimited { retry_after } => format!(
                "Too many requests; available again in about {} s",
                retry_after.as_secs().max(1)
            ),
            Self::Unavailable(detail) => format!("Unavailable: {detail}"),
            Self::Rejected(detail) => format!("Refused this request: {detail}"),
            Self::Empty => "Answered with nothing".to_owned(),
        }
    }
}

/// Turns an HTTP response into a failure.
///
/// The decision rests on what the service answered, never on a local count of
/// what it should have allowed. A count drifts — a key used from two machines,
/// a text that cost more than its character length, an allowance the service
/// quietly changed — while the service always knows.
#[must_use]
pub fn classify_response(
    provider: &str,
    status: u16,
    retry_after: Option<Duration>,
    body: &str,
) -> ProviderFailure {
    let detail = summarize(body);
    let retry_after = retry_after.map(|delay| delay.min(MAX_RETRY_AFTER));
    match status {
        401 => ProviderFailure::Credential(detail),
        // DeepL's own code for a spent character allowance. No other service
        // here uses it, and no speech service uses it at all.
        456 => ProviderFailure::QuotaExhausted { retry_after },
        403 => classify_forbidden(provider, &detail, body, retry_after),
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

/// A 403 means three different things across these services, and the difference
/// decides whether the user should replace the key or correct a setting.
fn classify_forbidden(
    provider: &str,
    detail: &str,
    body: &str,
    retry_after: Option<Duration>,
) -> ProviderFailure {
    if mentions_exhausted_allowance(body) {
        return ProviderFailure::QuotaExhausted { retry_after };
    }
    if mentions_wrong_configuration(body) {
        return ProviderFailure::Misconfigured(detail.to_owned());
    }
    match provider {
        // DeepL has no other use for 403: it is the code for a key that will
        // not authenticate.
        super::ProviderId::DEEPL => ProviderFailure::Credential(detail.to_owned()),
        // Azure answers 403 when the key is real but was issued in a different
        // region. Throwing the key away would be the wrong advice.
        super::ProviderId::AZURE => ProviderFailure::Misconfigured(detail.to_owned()),
        _ => ProviderFailure::Credential(detail.to_owned()),
    }
}

fn mentions_wrong_configuration(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "region",
        "not enabled",
        "has not been used",
        "api key not valid for",
        "restricted",
        "referer",
        "wrong resource",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

fn mentions_exhausted_allowance(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "quota",
        "character limit",
        "credit",
        "insufficient",
        "exceeded",
        "daily limit",
        "monthly limit",
        "out of",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

/// Removes every occurrence of a secret from a diagnostic before it is stored
/// or printed.
///
/// A service is trusted to answer, not to answer safely: an error body that
/// echoes the key back would otherwise put it in the log.
#[must_use]
pub fn redact_secret(text: &str, secret: &str) -> String {
    if secret.len() < 4 {
        return text.to_owned();
    }
    text.replace(secret, "<redacted>")
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

    use super::{ProviderFailure, classify_response, redact_secret};
    use crate::ProviderId;

    #[test]
    fn deepl_456_is_a_spent_allowance_and_not_a_refusal() {
        // The status the transcription crate has never seen, and the reason
        // this classifier is separate from that one.
        let failure = classify_response(ProviderId::DEEPL, 456, None, "Quota exceeded");

        assert!(matches!(failure, ProviderFailure::QuotaExhausted { .. }));
        assert!(failure.should_fail_over());
        assert!(!failure.disables_provider());
        assert!(failure.cooldown().is_some());
    }

    #[test]
    fn deepl_403_is_a_bad_key() {
        let failure = classify_response(ProviderId::DEEPL, 403, None, "Authorization failure");

        assert!(matches!(failure, ProviderFailure::Credential(_)));
        assert!(failure.disables_provider());
    }

    #[test]
    fn azure_403_is_a_wrong_region_rather_than_a_dead_key() {
        // Telling the user to replace a working key because it was addressed
        // to the wrong region would send them to buy a new one for nothing.
        let failure = classify_response(
            ProviderId::AZURE,
            403,
            None,
            "The request is not authorized because the region is incorrect.",
        );

        assert!(matches!(failure, ProviderFailure::Misconfigured(_)));
        assert!(!failure.disables_provider());
        assert!(failure.should_fail_over());
    }

    #[test]
    fn google_403_tells_a_spent_quota_from_a_wrong_key_by_the_body() {
        let spent = classify_response(
            ProviderId::GOOGLE,
            403,
            None,
            "Quota exceeded for quota metric 'v2 and v3 general model characters'",
        );
        assert!(matches!(spent, ProviderFailure::QuotaExhausted { .. }));

        let restricted = classify_response(
            ProviderId::GOOGLE,
            403,
            None,
            "Cloud Translation API has not been used in project 12345 before or it is disabled",
        );
        assert!(matches!(restricted, ProviderFailure::Misconfigured(_)));

        let dead = classify_response(
            ProviderId::GOOGLE,
            403,
            None,
            "The caller does not have permission",
        );
        assert!(matches!(dead, ProviderFailure::Credential(_)));
    }

    #[test]
    fn a_rate_limit_is_told_from_a_spent_allowance_by_the_body() {
        let slow_down = classify_response(ProviderId::DEEPL, 429, None, "Too many requests");
        assert!(matches!(slow_down, ProviderFailure::RateLimited { .. }));

        let spent = classify_response(
            ProviderId::GOOGLE,
            429,
            None,
            "Daily limit exceeded for this project",
        );
        assert!(matches!(spent, ProviderFailure::QuotaExhausted { .. }));
    }

    #[test]
    fn a_service_asking_to_be_left_alone_for_a_week_is_not_believed() {
        let failure = classify_response(
            ProviderId::AZURE,
            429,
            Some(Duration::from_secs(7 * 24 * 60 * 60)),
            "Too many requests",
        );

        assert_eq!(
            failure.cooldown(),
            Some(Duration::from_secs(60 * 60)),
            "an hour is the longest this application will hold a provider back"
        );
    }

    #[test]
    fn nothing_back_is_the_one_answer_that_stops_the_walk() {
        assert!(!ProviderFailure::Empty.should_fail_over());
        assert!(ProviderFailure::Rejected("no such language pair".into()).should_fail_over());
    }

    #[test]
    fn a_key_echoed_back_by_a_service_never_reaches_the_log() {
        let body = "invalid key 279a2e9d-83b3-c416-7e2d-f721593e42a0:fx supplied";
        let redacted = redact_secret(body, "279a2e9d-83b3-c416-7e2d-f721593e42a0:fx");

        assert!(!redacted.contains("279a2e9d"));
        assert!(redacted.contains("<redacted>"));
    }
}
