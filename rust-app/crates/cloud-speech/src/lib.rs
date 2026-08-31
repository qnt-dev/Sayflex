//! Speech transcription through third-party services, alongside the local
//! engine.
//!
//! The crate holds the part that must be right regardless of which service is
//! involved: what a provider is, why one failed, and which one to try next.
//! Nothing here opens a socket, so all of it is tested directly.

pub mod audio;
pub mod capability;
pub mod chain;
pub mod failure;
pub mod http;
pub mod provider;
pub mod request;
pub mod router;

pub use capability::{Capabilities, LanguageHint, TranscriptionRequest, capabilities};
pub use chain::{Chain, Ledger, ProviderStatus, Unavailable};
pub use failure::{ProviderFailure, classify_response, redact_secret};
pub use http::{DEFAULT_TIMEOUT, HttpExecutor};
pub use provider::{ProviderEntry, ProviderId};
pub use request::{PlanError, RequestBody, RequestPlan, plan_request, read_transcript};
pub use router::{Attempt, RequestExecutor, RouteOutcome, Router};
