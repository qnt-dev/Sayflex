//! Walking the user's list until something transcribes the audio.
//!
//! The router owns the rule the whole feature exists for: use the row the user
//! put first, and move to the next one only when the one before it could not
//! do the work. The local engine is a row like any other, but it is not
//! executed here — the speech worker owns the model — so reaching it ends the
//! walk and hands the recording back.

use std::time::{Duration, Instant};

use crate::{
    capability::{TranscriptionRequest, capabilities},
    chain::{Chain, Ledger},
    failure::ProviderFailure,
    provider::{ProviderEntry, ProviderId},
    request::{PlanError, RequestPlan, plan_request},
};

/// Performs a prepared request. Separated so the walk can be tested without a
/// network, and so the transport can be replaced without touching the rule.
pub trait RequestExecutor {
    /// `budget` is what is left of the deadline for the whole recording, not
    /// for this provider: the user is waiting for words, not for a service.
    fn execute(
        &self,
        id: &ProviderId,
        plan: RequestPlan,
        budget: Duration,
    ) -> Result<String, ProviderFailure>;
}

/// Asked between providers so a cancelled dictation stops the walk instead of
/// working its way down a list nobody is waiting for.
pub trait CancelSignal {
    fn is_cancelled(&self) -> bool;
}

impl CancelSignal for () {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// What became of one provider during the walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Never asked: it cannot serve this recording at all.
    Skipped(&'static str),
    /// Asked, and it answered with a failure.
    Failed(ProviderFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub provider: ProviderId,
    pub outcome: AttemptOutcome,
}

impl Attempt {
    #[must_use]
    pub fn reason(&self) -> String {
        match &self.outcome {
            AttemptOutcome::Skipped(reason) => (*reason).to_owned(),
            AttemptOutcome::Failed(failure) => failure.to_string(),
        }
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&ProviderFailure> {
        match &self.outcome {
            AttemptOutcome::Failed(failure) => Some(failure),
            AttemptOutcome::Skipped(_) => None,
        }
    }
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
    /// The dictation was cancelled while the walk was under way.
    Cancelled { attempts: Vec<Attempt> },
    /// Every service heard silence, and the local engine was not in the list.
    NoSpeech { attempts: Vec<Attempt> },
    /// Nothing in the list could do the work.
    Exhausted { attempts: Vec<Attempt> },
}

/// How long the whole walk may take, however many providers it visits.
///
/// A per-provider timeout alone lets a list of five services keep a user
/// waiting for minutes with their words undelivered. This is the number that
/// matters to them, so it is the one that is enforced.
pub const TOTAL_DEADLINE: Duration = Duration::from_secs(25);
/// No single provider may consume the entire budget: there has to be room left
/// for the one after it, and for the local engine at the end.
pub const PER_PROVIDER_LIMIT: Duration = Duration::from_secs(12);

#[derive(Clone)]
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

    pub fn replace_ledger(&mut self, ledger: Ledger) {
        self.ledger = ledger;
    }

    #[must_use]
    pub fn into_ledger(self) -> Ledger {
        self.ledger
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
        request: &TranscriptionRequest,
        started: Instant,
        secret_for: &impl Fn(&ProviderId) -> Option<String>,
        executor: &impl RequestExecutor,
        cancel: &impl CancelSignal,
    ) -> RouteOutcome {
        let candidates = self.chain.candidates(&self.ledger, started, |id| {
            !id.needs_credential() || secret_for(id).is_some()
        });
        let mut attempts = Vec::new();
        let deadline = started + TOTAL_DEADLINE;
        let mut network_budget_exhausted = false;

        for id in candidates {
            if cancel.is_cancelled() {
                return RouteOutcome::Cancelled { attempts };
            }
            if id.is_local() {
                return RouteOutcome::UseLocalEngine { attempts };
            }
            if network_budget_exhausted {
                attempts.push(Attempt {
                    provider: id,
                    outcome: AttemptOutcome::Skipped("no time left for another service"),
                });
                continue;
            }
            if let Some(refusal) = capabilities(&id).refusal(request) {
                // Not a fault of the service and not worth remembering: the
                // next recording may well be one it can take.
                attempts.push(Attempt {
                    provider: id,
                    outcome: AttemptOutcome::Skipped(refusal),
                });
                continue;
            }
            let Some(entry) = self.entry(&id) else {
                continue;
            };
            let Some(secret) = secret_for(&id) else {
                continue;
            };
            let plan = match plan_request(&entry, &secret, wav.to_vec(), request) {
                Ok(plan) => plan,
                Err(PlanError::MissingSetting(setting)) => {
                    attempts.push(Attempt {
                        provider: id,
                        outcome: AttemptOutcome::Skipped(setting_missing(setting)),
                    });
                    continue;
                }
                Err(PlanError::InsecureEndpoint) => {
                    attempts.push(Attempt {
                        provider: id,
                        outcome: AttemptOutcome::Skipped(
                            "the endpoint is not HTTPS, so the key and audio would travel in the clear",
                        ),
                    });
                    continue;
                }
                Err(PlanError::Unsupported) => continue,
            };

            let Some(budget) =
                deadline
                    .checked_duration_since(Instant::now())
                    .filter(|remaining| {
                        // Below this there is no point starting: the request would be
                        // cut off mid-flight and the time spent buys nothing.
                        *remaining >= MINIMUM_BUDGET
                    })
            else {
                attempts.push(Attempt {
                    provider: id,
                    outcome: AttemptOutcome::Skipped("no time left for another service"),
                });
                network_budget_exhausted = true;
                continue;
            };

            match executor.execute(&id, plan, budget.min(PER_PROVIDER_LIMIT)) {
                Ok(text) => {
                    self.ledger.record_success(&id);
                    return RouteOutcome::Transcribed {
                        provider: id,
                        text,
                        attempts,
                    };
                }
                Err(failure) => {
                    // A cancelled dictation often surfaces as a transport
                    // failure; it must not be held against the provider.
                    if cancel.is_cancelled() {
                        return RouteOutcome::Cancelled { attempts };
                    }
                    self.ledger.record_failure(&id, &failure, Instant::now());
                    let heard_silence = matches!(failure, ProviderFailure::NoSpeech);
                    let should_continue = failure.should_fail_over();
                    attempts.push(Attempt {
                        provider: id,
                        outcome: AttemptOutcome::Failed(failure),
                    });
                    if heard_silence {
                        return RouteOutcome::NoSpeech { attempts };
                    }
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

/// Below this a request is not worth starting.
const MINIMUM_BUDGET: Duration = Duration::from_secs(2);

const fn setting_missing(setting: &'static str) -> &'static str {
    match setting.as_bytes() {
        b"endpoint" => "no endpoint is set",
        b"account" => "no account is set",
        b"region" => "no region is set",
        b"language" => "the language is unknown",
        _ => "a required setting is missing",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    use super::{AttemptOutcome, RequestExecutor, RouteOutcome, Router, TOTAL_DEADLINE};
    use crate::{
        capability::{LanguageHint, TranscriptionRequest},
        chain::Chain,
        failure::ProviderFailure,
        provider::{ProviderEntry, ProviderId},
        request::RequestPlan,
    };

    fn request() -> TranscriptionRequest {
        TranscriptionRequest {
            audio_duration_ms: 4_000,
            upload_bytes: 128_000,
            translate_to_english: false,
            language: LanguageHint::new("ru-RU"),
        }
    }

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
        fn execute(
            &self,
            id: &ProviderId,
            _: RequestPlan,
            _: std::time::Duration,
        ) -> Result<String, ProviderFailure> {
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

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

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

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

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

        router.transcribe(b"wav", &request(), now, &every_key, &executor, &());
        router.transcribe(
            b"wav",
            &request(),
            now + Duration::from_secs(1),
            &every_key,
            &executor,
            &(),
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

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

        let RouteOutcome::UseLocalEngine { attempts } = outcome else {
            panic!("the walk must hand back to the local engine");
        };
        assert_eq!(attempts.len(), 1);
    }

    #[test]
    fn a_rejection_still_reaches_the_local_engine_further_down_the_list() {
        let mut router = router_of(&[
            (ProviderId::GROQ, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[
            (
                ProviderId::GROQ,
                Err(ProviderFailure::Rejected("model not found".to_owned())),
            ),
            (
                ProviderId::AZURE,
                Err(ProviderFailure::Rejected("format not accepted".to_owned())),
            ),
        ]);

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

        // Providers differ in what they accept, so a refusal is not a verdict
        // on the recording, and stranding it when the local engine is right
        // there would be the worse mistake.
        assert!(matches!(outcome, RouteOutcome::UseLocalEngine { .. }));
        assert_eq!(executor.asked(), [ProviderId::GROQ, ProviderId::AZURE]);
    }

    #[test]
    fn silence_ends_the_walk_because_every_service_would_hear_the_same() {
        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::AZURE, true)]);
        let executor = ScriptedExecutor::new(&[(ProviderId::GROQ, Err(ProviderFailure::NoSpeech))]);

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

        assert!(matches!(outcome, RouteOutcome::NoSpeech { .. }));
        assert_eq!(executor.asked(), [ProviderId::GROQ]);
    }

    #[test]
    fn a_service_that_cannot_serve_this_recording_is_stepped_over_untried() {
        let mut router = router_of(&[
            (ProviderId::AZURE, true),
            (ProviderId::GROQ, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[(ProviderId::GROQ, Ok("long recording"))]);
        let long = TranscriptionRequest {
            audio_duration_ms: 120_000,
            ..request()
        };

        let outcome = router.transcribe(b"wav", &long, Instant::now(), &every_key, &executor, &());

        let RouteOutcome::Transcribed { attempts, .. } = outcome else {
            panic!("Groq takes a two minute clip");
        };
        assert_eq!(executor.asked(), [ProviderId::GROQ], "Azure is never asked");
        assert!(matches!(attempts[0].outcome, AttemptOutcome::Skipped(_)));
    }

    #[test]
    fn a_skipped_service_carries_no_penalty_into_the_next_recording() {
        let mut router = router_of(&[(ProviderId::AZURE, true), (ProviderId::GROQ, true)]);
        let executor = ScriptedExecutor::new(&[
            (ProviderId::AZURE, Ok("short one")),
            (ProviderId::GROQ, Ok("long one")),
        ]);
        let now = Instant::now();

        let long = TranscriptionRequest {
            audio_duration_ms: 120_000,
            ..request()
        };
        router.transcribe(b"wav", &long, now, &every_key, &executor, &());
        router.transcribe(b"wav", &request(), now, &every_key, &executor, &());

        assert_eq!(
            executor.asked(),
            [ProviderId::GROQ, ProviderId::AZURE],
            "a clip Azure can take goes to Azure again"
        );
    }

    #[test]
    fn a_cancelled_dictation_stops_the_walk() {
        struct AlwaysCancelled;
        impl super::CancelSignal for AlwaysCancelled {
            fn is_cancelled(&self) -> bool {
                true
            }
        }

        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::LOCAL, true)]);
        let executor = ScriptedExecutor::new(&[(ProviderId::GROQ, Ok("never used"))]);

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &AlwaysCancelled,
        );

        assert!(matches!(outcome, RouteOutcome::Cancelled { .. }));
        assert!(executor.asked().is_empty());
    }

    #[test]
    fn the_walk_stops_asking_once_the_deadline_for_the_recording_has_passed() {
        let mut router = router_of(&[
            (ProviderId::GROQ, true),
            (ProviderId::AZURE, true),
            (ProviderId::CLOUDFLARE, true),
        ]);
        let executor = ScriptedExecutor::new(&[]);
        // Started long enough ago that no budget is left for anyone.
        let started = Instant::now() - TOTAL_DEADLINE;

        let outcome = router.transcribe(b"wav", &request(), started, &every_key, &executor, &());

        assert!(matches!(outcome, RouteOutcome::Exhausted { .. }));
        assert!(
            executor.asked().is_empty(),
            "a user already past the deadline is not served by asking one more service"
        );
    }

    #[test]
    fn an_exhausted_network_budget_still_reaches_a_later_local_fallback() {
        let mut router = router_of(&[
            (ProviderId::GROQ, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        let executor = ScriptedExecutor::new(&[]);
        let started = Instant::now() - TOTAL_DEADLINE;

        let outcome = router.transcribe(b"wav", &request(), started, &every_key, &executor, &());

        assert!(matches!(outcome, RouteOutcome::UseLocalEngine { .. }));
        assert!(executor.asked().is_empty());
    }

    #[test]
    fn a_provider_without_a_key_is_never_asked() {
        let mut router = router_of(&[(ProviderId::GROQ, true), (ProviderId::AZURE, true)]);
        let executor = ScriptedExecutor::new(&[(ProviderId::AZURE, Ok("answered"))]);

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &|id: &ProviderId| (id.as_str() == ProviderId::AZURE).then(|| "key".to_owned()),
            &executor,
            &(),
        );

        assert!(matches!(outcome, RouteOutcome::Transcribed { .. }));
        assert_eq!(executor.asked(), [ProviderId::AZURE]);
    }

    #[test]
    fn an_empty_or_fully_spent_list_reports_that_nothing_could_be_tried() {
        let mut router = router_of(&[(ProviderId::GROQ, false), (ProviderId::LOCAL, false)]);
        let executor = ScriptedExecutor::new(&[]);

        let outcome = router.transcribe(
            b"wav",
            &request(),
            Instant::now(),
            &every_key,
            &executor,
            &(),
        );

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
        router.transcribe(b"wav", &request(), now, &every_key, &failing, &());

        let recovered = ScriptedExecutor::new(&[(ProviderId::GROQ, Ok("first again"))]);
        let outcome = router.transcribe(
            b"wav",
            &request(),
            now + Duration::from_secs(60),
            &every_key,
            &recovered,
            &(),
        );

        assert!(matches!(
            outcome,
            RouteOutcome::Transcribed { ref provider, .. } if provider.as_str() == ProviderId::GROQ
        ));
    }
}
