//! Translating text through a service instead of, or before, the local engine.
//!
//! The product translates on the machine by default and always has. This crate
//! is the other option: an ordered list the user arranges themselves, in which
//! the local engine is one entry among DeepL, Azure Translator and Google
//! Cloud Translation, and can be put first, put last, or switched off.
//!
//! Three services and no more. Each has a free allowance a person can sign up
//! for without a contract — DeepL 500,000 characters a month, Azure two
//! million, Google 500,000 — which is the line drawn for what belongs here.
//!
//! Nothing in this crate sends anything by itself. [`request::plan_request`]
//! turns a translation into the shape one service takes, and the worker hands
//! that plan to something that can reach the network; the plan is data, so a
//! test can read exactly what would have left the machine without a key and
//! without a connection. What text leaves at all is [`Chain::text_can_leave_device`],
//! which answers from the list rather than from a setting, because the list is
//! what actually decides.

pub mod chain;
pub mod failure;
pub mod http;
pub mod provider;
pub mod request;
pub mod router;

pub use chain::{Chain, Ledger, ProviderStatus, Unavailable};
pub use failure::{ProviderFailure, classify_response, redact_secret};
pub use http::{DEFAULT_TIMEOUT, HttpExecutor};
pub use provider::{ProviderEntry, ProviderId};
pub use request::{
    PlanError, ReadAnswer, RequestPlan, ResponseShape, plan_request, read_answer, read_translation,
};
pub use router::{
    Attempt, AttemptOutcome, CancelSignal, PER_PROVIDER_LIMIT, RequestExecutor, RouteOutcome,
    Router, TOTAL_DEADLINE, Translation,
};
