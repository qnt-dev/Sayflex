//! Speech transcription through third-party services, alongside the local
//! engine.
//!
//! The crate holds the part that must be right regardless of which service is
//! involved: what a provider is, why one failed, and which one to try next.
//! Nothing here opens a socket, so all of it is tested directly.

pub mod chain;
pub mod failure;
pub mod provider;

pub use chain::{Chain, Ledger, ProviderStatus, Unavailable};
pub use failure::{ProviderFailure, classify_response};
pub use provider::{ProviderEntry, ProviderId};
