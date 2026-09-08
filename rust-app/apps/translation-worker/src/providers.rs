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

use std::{path::PathBuf, time::Instant};

use rust_app_core::credentials::CredentialStore;
use rust_cloud_translation::{
    Attempt, AttemptOutcome, Chain, DEFAULT_TIMEOUT, HttpExecutor, ProviderEntry, ProviderId,
    RouteOutcome, Router, Translation, Unavailable, redact_secret,
};
use rust_platform_windows::credentials::DpapiCipher;
use rust_protocol::{TranslationProviderEntry, TranslationProviderStatus};

pub struct Providers {
    router: Router,
    credentials: CredentialStore<DpapiCipher>,
    /// `None` when no HTTP client could be built at all. The local engine still
    /// works, which is the point of having a list.
    executor: Option<HttpExecutor>,
}

/// What the walk decided, in the terms the worker acts on.
pub enum Routed {
    /// A service translated it. The worker sends this and stops.
    Translated { provider: String, text: String },
    /// The walk reached the local engine, or never had anywhere else to go.
    UseLocalEngine,
    /// The user stopped waiting.
    Cancelled,
    /// Everyone in the list was asked and nobody answered.
    Exhausted { detail: String },
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
        }
    }

    pub fn set_entries(&mut self, entries: &[TranslationProviderEntry]) {
        let mut chain = Chain::new(entries.iter().map(to_entry).collect());
        // A build that knows a service the stored list has never seen still
        // offers it, switched off and last.
        chain.merge_known();
        *self.router.chain_mut() = chain;
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
        result.map_err(|error| error.to_string())
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

    pub fn translate(
        &mut self,
        text: &str,
        source: Option<&str>,
        target: &str,
        is_current: &impl Fn() -> bool,
    ) -> Routed {
        let Some(executor) = self.executor.as_ref() else {
            return Routed::UseLocalEngine;
        };
        let credentials = &self.credentials;
        let outcome = self.router.translate(
            Translation {
                text,
                source,
                target,
            },
            Instant::now(),
            &|id: &ProviderId| credentials.secret(id.as_str()).map(str::to_owned),
            executor,
            &|| !is_current(),
        );
        self.report_attempts(&outcome);
        match outcome {
            RouteOutcome::Translated { provider, text, .. } => Routed::Translated {
                provider: provider.as_str().to_owned(),
                text,
            },
            RouteOutcome::UseLocalEngine { .. } => Routed::UseLocalEngine,
            RouteOutcome::Cancelled { .. } => Routed::Cancelled,
            RouteOutcome::Exhausted { attempts } => Routed::Exhausted {
                detail: attempts.last().map_or_else(
                    || "Every translation provider is switched off.".to_owned(),
                    |attempt| {
                        format!(
                            "{}: {}",
                            attempt.provider.as_str(),
                            describe_outcome(&attempt.outcome)
                        )
                    },
                ),
            },
        }
    }

    /// Puts every step of the walk in the diagnostics, with any key the service
    /// echoed back removed first.
    fn report_attempts(&self, outcome: &RouteOutcome) {
        for attempt in attempts_of(outcome) {
            let mut reason = describe_outcome(&attempt.outcome);
            if let Some(secret) = self.credentials.secret(attempt.provider.as_str()) {
                reason = redact_secret(&reason, secret);
            }
            // Standard error is the worker's diagnostics channel; standard
            // output carries protocol messages only.
            eprintln!(
                "translation provider={} result=not_used reason={reason}",
                attempt.provider.as_str()
            );
        }
    }
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
    use super::describe;
    use rust_cloud_translation::Unavailable;
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
}
