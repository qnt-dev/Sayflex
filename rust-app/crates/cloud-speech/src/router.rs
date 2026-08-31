//! Walking the user's list until something transcribes the audio.
//!
//! The router owns the rule the whole feature exists for: use the row the user
//! put first, and move to the next one only when the one before it could not
//! do the work. The local engine is a row like any other, but it is not
//! executed here — the speech worker owns the model — so reaching it ends the
//! walk and hands the recording back.

use std::time::Instant;

use crate::{
    chain::{Chain, Ledger},
    failure::ProviderFailure,
    provider::{ProviderEntry, ProviderId},
    request::{PlanError, RequestPlan, plan_request},
};

/// Performs a prepared request. Separated so the walk can be tested without a
/// network, and so the transport can be replaced without touching the rule.
pub trait RequestExecutor {
    fn execute(&self, id: &ProviderId, plan: RequestPlan) -> Result<String, ProviderFailure>;
}

/// What one provider did with the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub provider: ProviderId,
    pub failure: ProviderFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOutcome {
    /// A service returned text.
    Transcribed {
        provider: ProviderId,
        text: String,
        attempts: Vec<Attempt>,
    },
    /// The walk reached the local engine. The caller runs the model.
    UseLocalEngine { attempts: Vec<Attempt> },
    /// Nothing in the list could do the work.
    Exhausted { attempts: Vec<Attempt> },
}

pub struct Router {
    chain: Chain,
    ledger: Ledger,
}

impl Router {
    #[must_use]
    pub fn new(chain: Chain) -> Self {
        Self {
            chain,
            ledger: Ledger::default(),
        }
    }

    #[must_use]
    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    pub fn chain_mut(&mut self) -> &mut Chain {
        &mut self.chain
    }

    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut Ledger {
        &mut self.ledger
    }

    /// Walks the list for one recording.
    ///
    /// `secret_for` hands over the key for a provider; the router keeps no
    /// copy. A provider with no key is skipped before a request is built, so a
    /// missing key never becomes a failed request against someone's quota.
    pub fn transcribe(
        &mut self,
        wav: &[u8],
        language: Option<&str>,
        now: Instant,
        secret_for: &impl Fn(&ProviderId) -> Option<String>,
        executor: &impl RequestExecutor,
    ) -> RouteOutcome {
        let candidates = self.chain.candidates(&self.ledger, now, |id| {
            !id.needs_credential() || secret_for(id).is_some()
        });
        let mut attempts = Vec::new();

        for id in candidates {
            if id.is_local() {
                return RouteOutcome::UseLocalEngine { attempts };
            }
            let Some(entry) = self.entry(&id) else {
                continue;
            };
            let Some(secret) = secret_for(&id) else {
                continue;
            };
            let plan = match plan_request(&entry, &secret, wav.to_vec(), language) {
                Ok(plan) => plan,
                Err(PlanError::MissingSetting(setting)) => {
                    // Not the service's fault and not worth a request: the row
                    // is incomplete until the user fills the field in.
                    attempts.push(Attempt {
                        provider: id.clone(),
                        failure: ProviderFailure::Rejected(format!("{setting} is not set")),
                    });
                    continue;
                }
                Err(PlanError::Unsupported) => continue,
            };

            match executor.execute(&id, plan) {
                Ok(text) => {
                    self.ledger.record_success(&id);
                    return RouteOutcome::Transcribed {
                        provider: id,
                        text,
                        attempts,
                    };
                }
                Err(failure) => {
                    self.ledger.record_failure(&id, &failure, now);
                    let should_continue = failure.should_fail_over();
                    attempts.push(Attempt {
                        provider: id,
                        failure,
                    });
                    if !should_continue {
                        break;
                    }
                }
            }
        }

        RouteOutcome::Exhausted { attempts }
    }

    fn entry(&self, id: &ProviderId) -> Option<ProviderEntry> {
        self.chain
            .entries()
            .iter()
            .find(|entry| &entry.id == id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    use super::{RequestExecutor, RouteOutcome, Router};
    use crate::{
        chain::Chain,
        failure::ProviderFailure,
        provider::{ProviderEntry, ProviderId},
        request::RequestPlan,
    };

    /// Answers each provider with a prepared result and records who was asked.
    struct ScriptedExecutor {
        answers: Vec<(String, Result<String, ProviderFailure>)>,
        asked: RefCell<Vec<String>>,
    }

    impl ScriptedExecutor {
        fn new(answers: &[(&str, Result<&str, ProviderFailure>)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(id, result)| {
                        (
                            (*id).to_owned(),
                            result.clone().map(std::borrow::ToOwned::to_owned),
                        )
                    })
                    .collect(),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl RequestExecutor for ScriptedExecutor {
        fn execute(&self, id: &ProviderId, _: RequestPlan) -> Result<String, ProviderFailure> {
            self.asked.borrow_mut().push(id.to_string());
            self.answers
                .iter()
                .find(|(candidate, _)| candidate == id.as_str())
                .map(|(_, result)| result.clone())
                .unwrap_or_else(|| Err(ProviderFailure::Unavailable("not scripted".to_owned())))
        }
    }

    fn router_of(ids: &[(&str, bool)]) -> Router {
        let mut router = Router::new(Chain::new(
            ids.iter()
                .map(|(id, enabled)| {
                    let mut entry = ProviderEntry::new(id, *enabled);
                    // Fill the settings the adapters demand so the walk, not a
                    // missing field, is what these tests exercise.
                    entry.account = Some("account-or-region".to_owned());
                    entry.endpoint = Some("https://example.test/v1".to_owned());
                    entry
                })
                .collect(),
        ));
        router.ledger_mut().record_success(&ProviderId::local());
        router
    }

    fn every_key(_: &ProviderId) -> Option<String> {
        Some("key".to_owned())
    }

    #[test]
    fn the_first_row_transcribes_and_nothing_else_is_asked() {
        let mut router = router_of(&[
            (ProviderId::CLOUDFLARE, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[(ProviderId::CLOUDFLARE, Ok("first result"))]);

        let outcome = router.transcribe(b"wav", None, Instant::now(), &every_key, &executor);

        assert!(matches!(
            outcome,
            RouteOutcome::Transcribed { ref text, .. } if text == "first result"
        ));
        assert_eq!(executor.asked(), [ProviderId::CLOUDFLARE]);
    }

    #[test]
    fn a_spent_allowance_moves_the_work_to_the_next_row() {
        let mut router = router_of(&[
            (ProviderId::CLOUDFLARE, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[
            (
                ProviderId::CLOUDFLARE,
                Err(ProviderFailure::QuotaExhausted { retry_after: None }),
            ),
            (ProviderId::AZURE, Ok("second result")),
        ]);

        let outcome = router.transcribe(b"wav", None, Instant::now(), &every_key, &executor);

        let RouteOutcome::Transcribed {
            provider,
            text,
            attempts,
        } = outcome
        else {
            panic!("Azure should have answered");
        };
        assert_eq!(provider.as_str(), ProviderId::AZURE);
        assert_eq!(text, "second result");
        assert_eq!(attempts.len(), 1, "the failure is reported, not hidden");
        assert_eq!(
            executor.asked(),
            [ProviderId::CLOUDFLARE, ProviderId::AZURE]
        );
    }

    #[test]
    fn the_exhausted_provider_is_skipped_on_the_next_recording() {
        let mut router = router_of(&[(ProviderId::CLOUDFLARE, true), (ProviderId::AZURE, true)]);
        let executor = ScriptedExecutor::new(&[
            (
                ProviderId::CLOUDFLARE,
                Err(ProviderFailure::QuotaExhausted { retry_after: None }),
            ),
            (ProviderId::AZURE, Ok("second result")),
        ]);
        let now = Instant::now();

        router.transcribe(b"wav", None, now, &every_key, &executor);
        router.transcribe(
            b"wav",
            None,
            now + Duration::from_secs(1),
            &every_key,
            &executor,
        );

        assert_eq!(
            executor.asked(),
            [ProviderId::CLOUDFLARE, ProviderId::AZURE, ProviderId::AZURE],
            "the second recording does not retry the spent allowance"
        );
    }

    #[test]
    fn reaching_the_local_row_ends_the_walk_for_the_worker_to_finish() {
        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::LOCAL, true)]);
        let executor = ScriptedExecutor::new(&[(
            ProviderId::GROQ,
            Err(ProviderFailure::QuotaExhausted { retry_after: None }),
        )]);

        let outcome = router.transcribe(b"wav", None, Instant::now(), &every_key, &executor);

        let RouteOutcome::UseLocalEngine { attempts } = outcome else {
            panic!("the walk must hand back to the local engine");
        };
        assert_eq!(attempts.len(), 1);
    }

    #[test]
    fn a_rejected_request_stops_the_walk_instead_of_spending_every_quota() {
        let mut router = router_of(&[
            (ProviderId::GROQ, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[(
            ProviderId::GROQ,
            Err(ProviderFailure::Rejected("audio too long".to_owned())),
        )]);

        let outcome = router.transcribe(b"wav", None, Instant::now(), &every_key, &executor);

        assert!(matches!(outcome, RouteOutcome::Exhausted { .. }));
        assert_eq!(
            executor.asked(),
            [ProviderId::GROQ],
            "the same audio would be refused by everyone"
        );
    }

    #[test]
    fn a_provider_without_a_key_is_never_asked() {
        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::AZURE, true)]);
        let executor = ScriptedExecutor::new(&[(ProviderId::AZURE, Ok("answered"))]);

        let outcome = router.transcribe(
            b"wav",
            None,
            Instant::now(),
            &|id: &ProviderId| (id.as_str() == ProviderId::AZURE).then(|| "key".to_owned()),
            &executor,
        );

        assert!(matches!(outcome, RouteOutcome::Transcribed { .. }));
        assert_eq!(executor.asked(), [ProviderId::AZURE]);
    }

    #[test]
    fn an_empty_or_fully_spent_list_reports_that_nothing_could_be_tried() {
        let mut router = router_of(&[(ProviderId::GROQ, false), (ProviderId::LOCAL, false)]);
        let executor = ScriptedExecutor::new(&[]);

        let outcome = router.transcribe(b"wav", None, Instant::now(), &every_key, &executor);

        assert_eq!(outcome, RouteOutcome::Exhausted { attempts: vec![] });
        assert!(executor.asked().is_empty());
    }

    #[test]
    fn a_success_clears_an_earlier_failure_so_the_row_returns_to_the_front() {
        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::AZURE, true)]);
        let now = Instant::now();

        let failing = ScriptedExecutor::new(&[
            (
                ProviderId::GROQ,
                Err(ProviderFailure::Unavailable("timeout".to_owned())),
            ),
            (ProviderId::AZURE, Ok("second")),
        ]);
        router.transcribe(b"wav", None, now, &every_key, &failing);

        let recovered = ScriptedExecutor::new(&[(ProviderId::GROQ, Ok("first again"))]);
        let outcome = router.transcribe(
            b"wav",
            None,
            now + Duration::from_secs(60),
            &every_key,
            &recovered,
        );

        assert!(matches!(
            outcome,
            RouteOutcome::Transcribed { ref provider, .. } if provider.as_str() == ProviderId::GROQ
        ));
    }
}
