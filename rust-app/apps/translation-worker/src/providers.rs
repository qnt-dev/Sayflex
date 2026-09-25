//! The user's ordered list of translation providers, and asking them in turn.
//!
//! The local engine is one row in this list. When it is the only one switched
//! on — the default, and what most people will keep — nothing here runs at all
//! and the worker translates on the machine exactly as it always did.
//!
//! Kept separate from the transcription list on purpose. A key for Azure
//! Translator is not a key for Azure Speech, the free allowances are counted
//! separately, and somebody willing to send a few seconds of audio to a service
//! is not thereby willing to send the contents of their documents.

use std::{collections::HashMap, path::PathBuf, time::Instant};

use rust_app_core::credentials::CredentialStore;
use rust_cloud_translation::{
    Attempt, AttemptOutcome, Chain, DEFAULT_TIMEOUT, HttpExecutor, ProviderEntry, ProviderId,
    RouteOutcome, Router, Translation, Unavailable, redact_secret,
};
use rust_platform_windows::credentials::DpapiCipher;
use rust_protocol::{
    TranslationAttemptReport, TranslationFailureKind, TranslationFailureReport,
    TranslationProviderEntry, TranslationProviderStatus,
};

pub struct Providers {
    router: Router,
    credentials: CredentialStore<DpapiCipher>,
    /// `None` when no HTTP client could be built at all. The local engine still
    /// works, which is the point of having a list.
    executor: Option<HttpExecutor>,
    revision: u64,
}

/// Immutable configuration and private routing state for one network walk.
/// It can spend seconds outside the provider actor without holding the
/// credential/configuration owner hostage.
pub struct ProvidersSnapshot {
    router: Router,
    secrets: HashMap<String, String>,
    executor: Option<HttpExecutor>,
    revision: u64,
}

/// What the walk decided, in the terms the worker acts on.
pub enum Routed {
    /// A service translated it. The worker sends this and stops.
    Translated {
        provider: String,
        text: String,
        attempts: Vec<TranslationAttemptReport>,
    },
    /// The walk reached the local engine, or never had anywhere else to go.
    UseLocalEngine {
        attempts: Vec<TranslationAttemptReport>,
    },
    /// The user stopped waiting.
    Cancelled,
    /// Everyone in the list was asked and nobody answered.
    Exhausted { failure: TranslationFailureReport },
}

impl Providers {
    /// The credentials file is separate from the transcription one, so a key
    /// stored for one is never quietly used for the other.
    #[must_use]
    pub fn new(credentials_path: PathBuf) -> Self {
        Self {
            router: Router::new(Chain::default_order()),
            credentials: CredentialStore::open(credentials_path, DpapiCipher),
            executor: HttpExecutor::new(DEFAULT_TIMEOUT).ok(),
            revision: 0,
        }
    }

    /// What the credential store could not read, once.
    ///
    /// A key file written on another Windows account, or copied with the
    /// profile to another machine, is by design unreadable here. The store
    /// notices and keeps the reason; nothing was collecting it, so every key
    /// silently became no key and the rows just said they needed one.
    pub fn recovery_message(&mut self) -> Option<String> {
        self.credentials.take_recovery_message()
    }

    pub fn set_entries(&mut self, entries: &[TranslationProviderEntry]) {
        let mut chain = Chain::new(entries.iter().map(to_entry).collect());
        // A build that knows a service the stored list has never seen still
        // offers it, switched off and last.
        chain.merge_known();
        *self.router.chain_mut() = chain;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Stores or clears one key. `None` clears it.
    pub fn set_credential(&mut self, id: &str, secret: Option<&str>) -> Result<(), String> {
        let result = match secret {
            Some(secret) => self.credentials.set(id, secret),
            None => self.credentials.remove(id),
        };
        // A key the user has just corrected deserves another attempt without
        // waiting out a cooldown the old one earned.
        self.router.forget(&ProviderId::new(id));
        self.revision = self.revision.wrapping_add(1);
        result.map_err(|error| error.to_string())
    }

    #[must_use]
    pub fn snapshot(&self) -> ProvidersSnapshot {
        let secrets = self
            .credentials
            .providers()
            .into_iter()
            .filter_map(|id| {
                self.credentials
                    .secret(id)
                    .map(|secret| (id.to_owned(), secret.to_owned()))
            })
            .collect();
        ProvidersSnapshot {
            router: self.router.clone(),
            secrets,
            executor: self.executor.clone(),
            revision: self.revision,
        }
    }

    /// Adopts only request-learned cooldown/refusal state, and only if no
    /// configuration or key changed while that request was in flight.
    pub fn adopt(&mut self, snapshot: ProvidersSnapshot) {
        if self.revision == snapshot.revision {
            self.router.replace_ledger(snapshot.router.into_ledger());
        }
    }

    /// Whether the walk has to happen at all, or the worker can go straight to
    /// the local engine.
    #[must_use]
    pub fn routing_required(&self) -> bool {
        self.router
            .chain()
            .routing_required(|id| self.credentials.has(id.as_str()))
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<TranslationProviderStatus> {
        self.router
            .chain()
            .status(self.router.ledger(), Instant::now(), |id| {
                !id.needs_credential() || self.credentials.has(id.as_str())
            })
            .into_iter()
            .map(|status| TranslationProviderStatus {
                detail: describe(status.unavailable.as_ref()),
                enabled: !matches!(status.unavailable, Some(Unavailable::Disabled)),
                available: status.is_available(),
                has_credential: self.credentials.has(status.id.as_str()),
                credential_hint: self.credentials.hint(status.id.as_str()),
                position: status.position,
                id: status.id.as_str().to_owned(),
            })
            .collect()
    }
}

impl ProvidersSnapshot {
    pub fn translate(
        &mut self,
        text: &str,
        source: Option<&str>,
        target: &str,
        is_current: &impl Fn() -> bool,
    ) -> Routed {
        let Some(executor) = self.executor.as_ref() else {
            return if self
                .router
                .chain()
                .entries()
                .iter()
                .any(|entry| entry.enabled && entry.id.is_local())
            {
                Routed::UseLocalEngine {
                    attempts: Vec::new(),
                }
            } else {
                Routed::Exhausted {
                    failure: TranslationFailureReport {
                        kind: TranslationFailureKind::NoProviderAvailable,
                        provider_id: None,
                        retry_after_ms: None,
                        required_catalog_id: None,
                        detail: "No network client could be created, and the local engine is switched off in the translation order."
                            .to_owned(),
                        attempts: Vec::new(),
                    },
                }
            };
        };
        let outcome = self.router.translate(
            Translation {
                text,
                source,
                target,
            },
            Instant::now(),
            &|id: &ProviderId| self.secrets.get(id.as_str()).cloned(),
            executor,
            &|| !is_current(),
        );
        for attempt in attempts_of(&outcome) {
            let mut reason = describe_outcome(&attempt.outcome);
            if let Some(secret) = self.secrets.get(attempt.provider.as_str()) {
                reason = redact_secret(&reason, secret);
            }
            eprintln!(
                "translation provider={} result=not_used reason={reason}",
                attempt.provider.as_str()
            );
        }
        match outcome {
            RouteOutcome::Translated {
                provider,
                text,
                attempts,
            } => Routed::Translated {
                provider: provider.as_str().to_owned(),
                text,
                attempts: attempts.iter().map(report_attempt).collect(),
            },
            RouteOutcome::UseLocalEngine { attempts } => Routed::UseLocalEngine {
                attempts: attempts.iter().map(report_attempt).collect(),
            },
            RouteOutcome::Cancelled { .. } => Routed::Cancelled,
            RouteOutcome::Exhausted { attempts } => Routed::Exhausted {
                failure: failure_report(&attempts),
            },
        }
    }
}

fn failure_report(attempts: &[Attempt]) -> TranslationFailureReport {
    let attempts = attempts.iter().map(report_attempt).collect::<Vec<_>>();
    let last = attempts.last();
    TranslationFailureReport {
        kind: last.map_or(TranslationFailureKind::NoProviderAvailable, |attempt| {
            attempt.kind.clone()
        }),
        provider_id: last.map(|attempt| attempt.provider_id.clone()),
        retry_after_ms: last.and_then(|attempt| attempt.retry_after_ms),
        required_catalog_id: None,
        detail: last.map_or_else(
            || "Every translation provider is switched off or unavailable.".to_owned(),
            |attempt| attempt.detail.clone(),
        ),
        attempts,
    }
}

fn report_attempt(attempt: &Attempt) -> TranslationAttemptReport {
    let (kind, retry_after_ms, detail) = match &attempt.outcome {
        AttemptOutcome::Skipped(reason) => (TranslationFailureKind::Timeout, None, reason.clone()),
        AttemptOutcome::Failed(failure) => report_failure(failure),
    };
    TranslationAttemptReport {
        provider_id: attempt.provider.as_str().to_owned(),
        kind,
        retry_after_ms,
        detail,
    }
}

fn report_failure(
    failure: &rust_cloud_translation::ProviderFailure,
) -> (TranslationFailureKind, Option<u64>, String) {
    use rust_cloud_translation::ProviderFailure;

    match failure {
        ProviderFailure::Credential(detail) => {
            (TranslationFailureKind::Credential, None, detail.clone())
        }
        ProviderFailure::Misconfigured(detail) => {
            (TranslationFailureKind::Misconfigured, None, detail.clone())
        }
        ProviderFailure::QuotaExhausted { retry_after } => (
            TranslationFailureKind::QuotaExhausted,
            retry_after.map(duration_ms),
            String::new(),
        ),
        ProviderFailure::RateLimited { retry_after } => (
            TranslationFailureKind::RateLimited,
            Some(duration_ms(*retry_after)),
            String::new(),
        ),
        ProviderFailure::Timeout(detail) => (TranslationFailureKind::Timeout, None, detail.clone()),
        ProviderFailure::Unavailable(detail) => {
            (TranslationFailureKind::Unavailable, None, detail.clone())
        }
        ProviderFailure::Unsupported(detail) => (
            TranslationFailureKind::UnsupportedRequest,
            None,
            detail.clone(),
        ),
        ProviderFailure::Rejected(detail) => {
            (TranslationFailureKind::Rejected, None, detail.clone())
        }
        ProviderFailure::Empty => (TranslationFailureKind::EmptyResponse, None, String::new()),
    }
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn attempts_of(outcome: &RouteOutcome) -> &[Attempt] {
    match outcome {
        RouteOutcome::Translated { attempts, .. }
        | RouteOutcome::UseLocalEngine { attempts }
        | RouteOutcome::Cancelled { attempts }
        | RouteOutcome::Exhausted { attempts } => attempts,
    }
}

fn describe_outcome(outcome: &AttemptOutcome) -> String {
    match outcome {
        AttemptOutcome::Skipped(reason) => reason.clone(),
        AttemptOutcome::Failed(failure) => failure.describe(),
    }
}

fn to_entry(entry: &TranslationProviderEntry) -> ProviderEntry {
    ProviderEntry {
        id: ProviderId::new(&entry.id),
        enabled: entry.enabled,
        region: entry.region.clone(),
    }
}

/// Why a row cannot run, in words meant for the person reading the list.
fn describe(unavailable: Option<&Unavailable>) -> String {
    match unavailable {
        None => String::new(),
        Some(Unavailable::Disabled) => "Switched off".to_owned(),
        Some(Unavailable::MissingCredential) => "Needs a key".to_owned(),
        Some(Unavailable::CredentialRefused(detail)) => format!("Key refused · {detail}"),
        Some(Unavailable::Misconfigured(detail)) => format!("Needs a correction · {detail}"),
        Some(Unavailable::CoolingDown { remaining, reason }) => {
            format!("Waiting {}s · {reason}", remaining.as_secs().max(1))
        }
        Some(Unavailable::Unsupported) => "This build does not know it".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{describe, report_attempt};
    use rust_cloud_translation::{
        Attempt, AttemptOutcome, ProviderFailure, ProviderId, Unavailable,
    };
    use rust_protocol::TranslationFailureKind;
    use std::time::Duration;

    #[test]
    fn an_available_row_says_nothing() {
        assert_eq!(describe(None), "");
    }

    #[test]
    fn every_reason_a_row_cannot_run_is_worded_for_the_reader() {
        assert_eq!(describe(Some(&Unavailable::Disabled)), "Switched off");
        assert_eq!(
            describe(Some(&Unavailable::MissingCredential)),
            "Needs a key"
        );
        assert!(
            describe(Some(&Unavailable::CoolingDown {
                remaining: Duration::from_millis(400),
                reason: "the allowance is spent".to_owned(),
            }))
            .starts_with("Waiting 1s"),
            "a wait under a second still reads as a wait, not as none"
        );
    }

    #[test]
    fn provider_failures_cross_the_wire_without_parsing_display_text() {
        let report = report_attempt(&Attempt {
            provider: ProviderId::new(ProviderId::DEEPL),
            outcome: AttemptOutcome::Failed(ProviderFailure::QuotaExhausted {
                retry_after: Some(Duration::from_secs(60)),
            }),
        });

        assert_eq!(report.provider_id, ProviderId::DEEPL);
        assert_eq!(report.kind, TranslationFailureKind::QuotaExhausted);
        assert_eq!(report.retry_after_ms, Some(60_000));
    }
}
