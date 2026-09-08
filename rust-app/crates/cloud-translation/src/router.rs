//! Walking the list until something translates.
//!
//! The walk has a deadline. A translation the user is waiting for is worth a
//! few seconds, not a few minutes: past that the local engine — which is
//! usually somewhere in the same list — will have finished anyway, and a
//! preview that arrives after the user has moved on is worse than one that
//! never came.

use std::time::{Duration, Instant};

use crate::{
    ProviderEntry, ProviderId,
    chain::{Chain, Ledger},
    failure::{ProviderFailure, redact_secret},
    request::{PlanError, RequestPlan, plan_request},
};

/// The whole walk, however many providers it touches.
pub const TOTAL_DEADLINE: Duration = Duration::from_secs(12);
/// No single service holds up the rest of the list for longer than this.
pub const PER_PROVIDER_LIMIT: Duration = Duration::from_secs(6);
/// Below this there is no point starting another request; the answer would
/// arrive after the deadline it was meant to beat.
const MINIMUM_BUDGET: Duration = Duration::from_millis(700);

/// Somewhere to ask whether the user has given up waiting.
pub trait CancelSignal {
    fn is_cancelled(&self) -> bool;
}

impl<F: Fn() -> bool> CancelSignal for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// What a plan turns into when it is actually sent.
pub trait RequestExecutor {
    /// Returns the translated text, or why this provider did not produce one.
    fn execute(&self, plan: &RequestPlan, budget: Duration) -> Result<String, ProviderFailure>;
}

/// What one provider did when it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Never asked, and why.
    Skipped(String),
    Failed(ProviderFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub provider: ProviderId,
    pub outcome: AttemptOutcome,
}

/// How the walk ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOutcome {
    Translated {
        provider: ProviderId,
        text: String,
        attempts: Vec<Attempt>,
    },
    /// The walk reached the local engine. It is not asked from here — the
    /// worker owns it — so the walk stops and hands back.
    UseLocalEngine { attempts: Vec<Attempt> },
    /// The user stopped waiting.
    Cancelled { attempts: Vec<Attempt> },
    /// Everyone was asked and nobody answered.
    Exhausted { attempts: Vec<Attempt> },
}

/// The translation being asked for.
///
/// The three of these travel together through every provider in the walk, and
/// naming them once is clearer than repeating a text, an optional source and a
/// target down the call chain.
#[derive(Debug, Clone, Copy)]
pub struct Translation<'a> {
    pub text: &'a str,
    /// `None` asks the service to detect it, which all three can do.
    pub source: Option<&'a str>,
    pub target: &'a str,
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

    pub fn forget(&mut self, id: &ProviderId) {
        self.ledger.forget(id);
    }

    /// Asks each provider in turn until one answers.
    ///
    /// A provider with no key is skipped before a request is built, so a
    /// missing key never becomes a failed request against somebody's quota.
    pub fn translate(
        &mut self,
        asked: Translation<'_>,
        started: Instant,
        secret_for: &impl Fn(&ProviderId) -> Option<String>,
        executor: &impl RequestExecutor,
        cancel: &impl CancelSignal,
    ) -> RouteOutcome {
        let Translation {
            text,
            source,
            target,
        } = asked;
        let candidates = self.chain.candidates(&self.ledger, started, |id| {
            !id.needs_credential() || secret_for(id).is_some()
        });
        let mut attempts = Vec::new();
        let deadline = started + TOTAL_DEADLINE;

        for id in candidates {
            if cancel.is_cancelled() {
                return RouteOutcome::Cancelled { attempts };
            }
            if id.is_local() {
                return RouteOutcome::UseLocalEngine { attempts };
            }
            let Some(entry) = self.chain.entry(&id).cloned() else {
                continue;
            };
            let Some(secret) = secret_for(&id) else {
                continue;
            };
            let plan = match plan_request(&entry, &secret, text, source, target) {
                Ok(plan) => plan,
                Err(error) => {
                    // A setting that is missing is the user's to fix, and
                    // remembering it keeps the row out of the walk until they
                    // do rather than failing on every translation.
                    let failure = plan_failure(&entry, &error);
                    self.ledger.record_failure(&id, &failure, Instant::now());
                    attempts.push(Attempt {
                        provider: id,
                        outcome: AttemptOutcome::Failed(failure),
                    });
                    continue;
                }
            };

            let now = Instant::now();
            let Some(budget) = remaining_budget(now, deadline) else {
                attempts.push(Attempt {
                    provider: id,
                    outcome: AttemptOutcome::Skipped(
                        "not enough time left before the deadline".to_owned(),
                    ),
                });
                continue;
            };

            match executor.execute(&plan, budget) {
                Ok(translated) => {
                    self.ledger.record_success(&id);
                    return RouteOutcome::Translated {
                        provider: id,
                        text: translated,
                        attempts,
                    };
                }
                Err(failure) => {
                    // Whatever the service said about itself must not carry the
                    // key into the log on its way to being recorded.
                    let failure = redact(failure, &secret);
                    self.ledger.record_failure(&id, &failure, Instant::now());
                    let should_continue = failure.should_fail_over();
                    attempts.push(Attempt {
                        provider: id,
                        outcome: AttemptOutcome::Failed(failure),
                    });
                    if !should_continue {
                        return RouteOutcome::Exhausted { attempts };
                    }
                }
            }
        }
        RouteOutcome::Exhausted { attempts }
    }
}

fn plan_failure(entry: &ProviderEntry, error: &PlanError) -> ProviderFailure {
    match error {
        PlanError::MissingSetting(_) => ProviderFailure::Misconfigured(error.to_string()),
        PlanError::Unsupported(_) => ProviderFailure::Rejected(error.to_string()),
        PlanError::NoTargetLanguage => {
            ProviderFailure::Rejected(format!("{} was asked for no language", entry.id.as_str()))
        }
    }
}

fn redact(failure: ProviderFailure, secret: &str) -> ProviderFailure {
    match failure {
        ProviderFailure::Credential(detail) => {
            ProviderFailure::Credential(redact_secret(&detail, secret))
        }
        ProviderFailure::Misconfigured(detail) => {
            ProviderFailure::Misconfigured(redact_secret(&detail, secret))
        }
        ProviderFailure::Unavailable(detail) => {
            ProviderFailure::Unavailable(redact_secret(&detail, secret))
        }
        ProviderFailure::Rejected(detail) => {
            ProviderFailure::Rejected(redact_secret(&detail, secret))
        }
        other => other,
    }
}

fn remaining_budget(now: Instant, deadline: Instant) -> Option<Duration> {
    let left = deadline.checked_duration_since(now)?;
    (left >= MINIMUM_BUDGET).then(|| left.min(PER_PROVIDER_LIMIT))
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    use super::{AttemptOutcome, RequestExecutor, RouteOutcome, Router, Translation};
    use crate::{
        ProviderEntry, ProviderId, chain::Chain, failure::ProviderFailure, request::RequestPlan,
    };

    struct Scripted {
        answers: RefCell<Vec<Result<String, ProviderFailure>>>,
        seen: RefCell<Vec<String>>,
    }

    impl Scripted {
        fn new(answers: Vec<Result<String, ProviderFailure>>) -> Self {
            Self {
                answers: RefCell::new(answers),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl RequestExecutor for Scripted {
        fn execute(
            &self,
            plan: &RequestPlan,
            _budget: Duration,
        ) -> Result<String, ProviderFailure> {
            self.seen.borrow_mut().push(plan.url.clone());
            if self.answers.borrow().is_empty() {
                return Err(ProviderFailure::Unavailable("no answer scripted".into()));
            }
            self.answers.borrow_mut().remove(0)
        }
    }

    fn chain(ids: &[&str]) -> Chain {
        Chain::new(
            ids.iter()
                .map(|id| ProviderEntry {
                    id: ProviderId::new(id),
                    enabled: true,
                    region: Some("westeurope".to_owned()),
                })
                .collect(),
        )
    }

    fn key(_id: &ProviderId) -> Option<String> {
        Some("279a2e9d-83b3-c416-7e2d-f721593e42a0:fx".to_owned())
    }

    #[test]
    fn the_first_service_that_answers_ends_the_walk() {
        let mut router = Router::new(chain(&[
            ProviderId::DEEPL,
            ProviderId::GOOGLE,
            ProviderId::LOCAL,
        ]));
        let executor = Scripted::new(vec![Ok("привет".to_owned())]);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: Some("en"),
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| false,
        );

        match outcome {
            RouteOutcome::Translated {
                provider,
                text,
                attempts,
            } => {
                assert_eq!(provider.as_str(), ProviderId::DEEPL);
                assert_eq!(text, "привет");
                assert!(attempts.is_empty());
            }
            other => panic!("expected a translation, got {other:?}"),
        }
        assert_eq!(executor.seen.borrow().len(), 1, "nobody else was asked");
    }

    #[test]
    fn a_spent_allowance_moves_on_to_the_next_service() {
        let mut router = Router::new(chain(&[
            ProviderId::DEEPL,
            ProviderId::GOOGLE,
            ProviderId::LOCAL,
        ]));
        let executor = Scripted::new(vec![
            Err(ProviderFailure::QuotaExhausted { retry_after: None }),
            Ok("привет".to_owned()),
        ]);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: Some("en"),
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| false,
        );

        match outcome {
            RouteOutcome::Translated {
                provider, attempts, ..
            } => {
                assert_eq!(provider.as_str(), ProviderId::GOOGLE);
                assert_eq!(attempts.len(), 1);
            }
            other => panic!("expected the second service to answer, got {other:?}"),
        }
    }

    #[test]
    fn the_walk_stops_at_the_local_engine_and_hands_back() {
        // Everything below the local engine is unreachable, which is what the
        // interface has to say about those rows.
        let mut router = Router::new(chain(&[ProviderId::LOCAL, ProviderId::DEEPL]));
        let executor = Scripted::new(vec![Ok("never asked".to_owned())]);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: None,
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| false,
        );

        assert!(matches!(outcome, RouteOutcome::UseLocalEngine { .. }));
        assert!(executor.seen.borrow().is_empty());
    }

    #[test]
    fn an_empty_answer_stops_the_walk_rather_than_asking_everyone_for_the_same_nothing() {
        let mut router = Router::new(chain(&[
            ProviderId::DEEPL,
            ProviderId::GOOGLE,
            ProviderId::LOCAL,
        ]));
        let executor = Scripted::new(vec![Err(ProviderFailure::Empty)]);

        let outcome = router.translate(
            Translation {
                text: "   ",
                source: None,
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| false,
        );

        assert!(matches!(outcome, RouteOutcome::Exhausted { .. }));
        assert_eq!(executor.seen.borrow().len(), 1);
    }

    #[test]
    fn a_missing_region_takes_azure_out_without_spending_a_request() {
        let mut router = Router::new(Chain::new(vec![
            ProviderEntry {
                id: ProviderId::new(ProviderId::AZURE),
                enabled: true,
                region: None,
            },
            ProviderEntry {
                id: ProviderId::new(ProviderId::GOOGLE),
                enabled: true,
                region: None,
            },
        ]));
        let executor = Scripted::new(vec![Ok("привет".to_owned())]);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: None,
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| false,
        );

        match outcome {
            RouteOutcome::Translated {
                provider, attempts, ..
            } => {
                assert_eq!(provider.as_str(), ProviderId::GOOGLE);
                assert!(matches!(
                    attempts[0].outcome,
                    AttemptOutcome::Failed(ProviderFailure::Misconfigured(_))
                ));
            }
            other => panic!("expected Google to answer, got {other:?}"),
        }
        assert_eq!(
            executor.seen.borrow().len(),
            1,
            "Azure was never sent a request it could not have answered"
        );
    }

    #[test]
    fn giving_up_waiting_stops_the_walk() {
        let mut router = Router::new(chain(&[ProviderId::DEEPL, ProviderId::LOCAL]));
        let executor = Scripted::new(vec![Ok("never asked".to_owned())]);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: None,
                target: "ru",
            },
            Instant::now(),
            &key,
            &executor,
            &|| true,
        );

        assert!(matches!(outcome, RouteOutcome::Cancelled { .. }));
        assert!(executor.seen.borrow().is_empty());
    }

    #[test]
    fn a_deadline_already_past_asks_nobody_and_falls_back_to_the_local_engine() {
        // The local engine is always in the list, even when the stored order
        // does not mention it, so a walk with no time left does not end in
        // nothing — it ends where it would have ended anyway, one step sooner.
        let mut router = Router::new(chain(&[ProviderId::DEEPL, ProviderId::GOOGLE]));
        let executor = Scripted::new(vec![Ok("too late".to_owned())]);
        let long_ago = Instant::now() - super::TOTAL_DEADLINE - Duration::from_secs(1);

        let outcome = router.translate(
            Translation {
                text: "hello",
                source: None,
                target: "ru",
            },
            long_ago,
            &key,
            &executor,
            &|| false,
        );

        match outcome {
            RouteOutcome::UseLocalEngine { attempts } => {
                assert!(
                    attempts
                        .iter()
                        .all(|attempt| matches!(attempt.outcome, AttemptOutcome::Skipped(_))),
                    "every service was skipped rather than asked: {attempts:?}"
                );
            }
            other => panic!("expected the walk to hand back to the local engine, got {other:?}"),
        }
        assert!(executor.seen.borrow().is_empty());
    }
}
