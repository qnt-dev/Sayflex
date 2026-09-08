//! Identity and configuration of a translation provider.
//!
//! A provider is anything that can turn one language into another: the local
//! llama.cpp engine is one entry in the same list as the network services, so
//! the user can put it anywhere in the order or switch it off entirely.
//!
//! Deliberately a separate list from the transcription one. They look alike and
//! are not: a key for Azure Translator is not a key for Azure Speech, the free
//! allowances are counted separately, and a user who is happy to send audio to
//! a service is not thereby happy to send the contents of their documents.

use serde::{Deserialize, Serialize};

/// Stable identifier written into settings.
///
/// A string rather than an enum, for the same reason the transcription list
/// uses one: an ordered list a user built must survive being read by a build
/// that does not know one of its entries. The unknown identifier keeps its
/// place in the file and in the order, and is simply never selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub const LOCAL: &'static str = "local";
    pub const DEEPL: &'static str = "deepl";
    pub const AZURE: &'static str = "azure";
    pub const GOOGLE: &'static str = "google";

    /// Every provider this build understands, in the order the settings list
    /// offers them before the user rearranges anything. The local engine comes
    /// first because nothing leaves the machine while it does.
    pub const KNOWN: [&'static str; 4] = [Self::LOCAL, Self::DEEPL, Self::AZURE, Self::GOOGLE];

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

    /// The second thing a provider needs beyond its key, if it needs one.
    ///
    /// Azure will not answer without knowing which region issued the key.
    /// DeepL and Google carry everything they need in the key itself.
    #[must_use]
    pub fn required_setting(&self) -> Option<&'static str> {
        match self.0.as_str() {
            Self::AZURE => Some("region"),
            _ => None,
        }
    }

    /// What the free allowance is, for the row that offers it.
    ///
    /// Stated once here rather than in the interface, so the page and the code
    /// that meets the limit cannot describe it differently.
    #[must_use]
    pub fn free_allowance(&self) -> &'static str {
        match self.0.as_str() {
            Self::LOCAL => "Runs on this computer · nothing leaves it",
            // Free keys end in `:fx`. New DeepL customers are directed to a
            // one-off million characters rather than a monthly allowance, but
            // keys issued before that keep working, which is why this row still
            // exists.
            Self::DEEPL => "Free tier: 500 000 characters a month · key ends in :fx",
            Self::AZURE => "Free tier F0: 2 million characters a month · needs a region",
            Self::GOOGLE => "Free tier: 500 000 characters a month",
            _ => "",
        }
    }
}

/// One row of the ordered translation list, as it is stored and sent between
/// processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub id: ProviderId,
    pub enabled: bool,
    /// Azure's region. Empty for the providers that do not take one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl ProviderEntry {
    #[must_use]
    pub fn new(id: ProviderId) -> Self {
        Self {
            enabled: id.is_local(),
            id,
            region: None,
        }
    }

    /// The list a fresh installation starts with: the local engine on, every
    /// service present but switched off and without a key.
    #[must_use]
    pub fn default_chain() -> Vec<Self> {
        ProviderId::KNOWN
            .iter()
            .map(|id| Self::new(ProviderId::new(id)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderEntry, ProviderId};

    #[test]
    fn a_provider_this_build_does_not_know_keeps_its_place() {
        let future = ProviderId::new("a-service-from-next-year");

        assert!(!future.is_known());
        assert!(!future.is_local());
        // It still needs a key, so it is never silently tried without one.
        assert!(future.needs_credential());
    }

    #[test]
    fn a_new_installation_translates_locally_and_sends_nothing_out() {
        let chain = ProviderEntry::default_chain();

        assert_eq!(chain.first().map(|entry| entry.id.as_str()), Some("local"));
        assert!(chain[0].enabled);
        assert!(
            chain[1..].iter().all(|entry| !entry.enabled),
            "no service is on until the user turns it on"
        );
    }

    #[test]
    fn only_azure_asks_for_a_second_setting() {
        assert_eq!(
            ProviderId::new(ProviderId::AZURE).required_setting(),
            Some("region")
        );
        assert_eq!(ProviderId::new(ProviderId::DEEPL).required_setting(), None);
        assert_eq!(ProviderId::new(ProviderId::GOOGLE).required_setting(), None);
    }
}
