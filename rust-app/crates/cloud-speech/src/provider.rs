//! Identity and configuration of a transcription provider.
//!
//! A provider is anything that can turn recorded audio into text: the local
//! speech engine is one entry in the same list as the network services, so the
//! user can put it anywhere in the order or switch it off entirely.

use serde::{Deserialize, Serialize};

/// Stable identifier written into settings.
///
/// Deliberately a string rather than an enum. The ordered list a user builds
/// must survive reading by a build that does not know one of its entries: an
/// unknown identifier is kept in the file and in its place in the order, and is
/// simply never selected. An enum would make the whole list unreadable instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub const LOCAL: &'static str = "local";
    pub const GROQ: &'static str = "groq";
    pub const CLOUDFLARE: &'static str = "cloudflare";
    pub const AZURE: &'static str = "azure";
    pub const DEEPGRAM: &'static str = "deepgram";
    pub const CUSTOM: &'static str = "custom";

    /// Every provider this build understands, in the order the settings list
    /// offers them before the user rearranges anything.
    pub const KNOWN: [&'static str; 6] = [
        Self::LOCAL,
        Self::GROQ,
        Self::CLOUDFLARE,
        Self::AZURE,
        Self::DEEPGRAM,
        Self::CUSTOM,
    ];

    #[must_use]
    pub fn new(value: &str) -> Self {
        Self(value.to_owned())
    }

    #[must_use]
    pub fn local() -> Self {
        Self::new(Self::LOCAL)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The local engine needs no key and no network, so it is never held back
    /// by a quota and never fails over to anything else.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.0 == Self::LOCAL
    }

    #[must_use]
    pub fn is_known(&self) -> bool {
        Self::KNOWN.contains(&self.0.as_str())
    }

    /// Whether this provider cannot run without a stored key.
    #[must_use]
    pub fn needs_credential(&self) -> bool {
        !self.is_local()
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One row of the ordered list in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub id: ProviderId,
    /// A row the user switched off is skipped without being forgotten: its
    /// position and its settings are kept for when it is switched back on.
    pub enabled: bool,
    /// Endpoint for `custom`, and an override for the others when a service
    /// offers a regional or self-hosted address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Model or deployment name where the service takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Region for Azure, account identifier for Cloudflare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

impl ProviderEntry {
    #[must_use]
    pub fn new(id: &str, enabled: bool) -> Self {
        Self {
            id: ProviderId::new(id),
            enabled,
            endpoint: None,
            model: None,
            account: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderEntry, ProviderId};

    #[test]
    fn an_identifier_from_a_newer_build_survives_a_round_trip() {
        let entry = ProviderEntry::new("a-service-from-the-future", true);
        let document = serde_json::to_string(&entry).unwrap();
        let restored: ProviderEntry = serde_json::from_str(&document).unwrap();

        assert_eq!(restored, entry);
        assert!(!restored.id.is_known(), "it is kept without being usable");
    }

    #[test]
    fn only_the_local_engine_runs_without_a_key() {
        assert!(!ProviderId::local().needs_credential());
        assert!(ProviderId::new(ProviderId::GROQ).needs_credential());
        assert!(ProviderId::new(ProviderId::CUSTOM).needs_credential());
    }

    #[test]
    fn optional_fields_stay_out_of_the_document_when_unset() {
        let document = serde_json::to_string(&ProviderEntry::new(ProviderId::GROQ, true)).unwrap();

        assert_eq!(document, r#"{"id":"groq","enabled":true}"#);
    }
}
