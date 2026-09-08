//! The ordered list of translation providers, and what is known about each.
//!
//! The order is the user's. This module answers one question about it — who
//! should be asked next — and remembers just enough about the last answers to
//! avoid asking a service that has already said it has nothing left.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::{ProviderEntry, ProviderId, failure::ProviderFailure};

/// Why a provider is not being offered right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The user switched the row off.
    Disabled,
    /// No key has been stored for it.
    MissingCredential,
    /// The key was refused, and will be until it is replaced.
    CredentialRefused(String),
    /// A setting is wrong — Azure's region, a restricted Google key. The row
    /// stays on, because the fix is in settings rather than in a new key.
    Misconfigured(String),
    /// The service asked to be left alone, and there is time left to wait.
    CoolingDown { remaining: Duration, reason: String },
    /// This build does not know how to talk to it.
    Unsupported,
}

/// What the last few answers from each provider mean for the next request.
#[derive(Debug, Default)]
pub struct Ledger {
    cooldowns: HashMap<ProviderId, (Instant, String)>,
    refusals: HashMap<ProviderId, String>,
    misconfigured: HashMap<ProviderId, String>,
}

impl Ledger {
    pub fn record_failure(&mut self, id: &ProviderId, failure: &ProviderFailure, now: Instant) {
        if let Some(cooldown) = failure.cooldown() {
            self.cooldowns
                .insert(id.clone(), (now + cooldown, failure.describe()));
        }
        match failure {
            ProviderFailure::Credential(detail) => {
                self.refusals.insert(id.clone(), detail.clone());
            }
            ProviderFailure::Misconfigured(detail) => {
                self.misconfigured.insert(id.clone(), detail.clone());
            }
            _ => {}
        }
    }

    /// A provider that answered is a provider with nothing held against it.
    pub fn record_success(&mut self, id: &ProviderId) {
        self.cooldowns.remove(id);
        self.refusals.remove(id);
        self.misconfigured.remove(id);
    }

    /// Called when the user stores a new key or corrects a setting: whatever
    /// the old one did is no longer evidence about the new one.
    pub fn forget(&mut self, id: &ProviderId) {
        self.record_success(id);
    }

    fn unavailable(
        &self,
        entry: &ProviderEntry,
        now: Instant,
        has_credential: &impl Fn(&ProviderId) -> bool,
    ) -> Option<Unavailable> {
        if !entry.enabled {
            return Some(Unavailable::Disabled);
        }
        if !entry.id.is_known() {
            return Some(Unavailable::Unsupported);
        }
        if entry.id.needs_credential() && !has_credential(&entry.id) {
            return Some(Unavailable::MissingCredential);
        }
        if let Some(detail) = self.refusals.get(&entry.id) {
            return Some(Unavailable::CredentialRefused(detail.clone()));
        }
        if let Some(detail) = self.misconfigured.get(&entry.id) {
            return Some(Unavailable::Misconfigured(detail.clone()));
        }
        if let Some((until, reason)) = self.cooldowns.get(&entry.id)
            && *until > now
        {
            return Some(Unavailable::CoolingDown {
                remaining: *until - now,
                reason: reason.clone(),
            });
        }
        None
    }
}

/// One row as the interface should show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub position: usize,
    pub enabled: bool,
    pub unavailable: Option<Unavailable>,
}

impl ProviderStatus {
    /// Whether this row would be asked right now.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.unavailable.is_none()
    }
}

/// The user's ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    entries: Vec<ProviderEntry>,
}

impl Chain {
    /// Builds a chain from stored entries, dropping repeats and appending any
    /// provider this build knows that the file did not mention.
    ///
    /// A duplicated row would otherwise be asked twice for the same
    /// translation, spending two lots of a free allowance on one request.
    #[must_use]
    pub fn new(entries: Vec<ProviderEntry>) -> Self {
        let mut seen = Vec::new();
        let mut unique = Vec::new();
        for entry in entries {
            if seen.contains(&entry.id) {
                continue;
            }
            seen.push(entry.id.clone());
            unique.push(entry);
        }
        let mut chain = Self { entries: unique };
        chain.merge_known();
        chain
    }

    #[must_use]
    pub fn default_order() -> Self {
        Self::new(ProviderEntry::default_chain())
    }

    #[must_use]
    pub fn entries(&self) -> &[ProviderEntry] {
        &self.entries
    }

    pub fn set_entries(&mut self, entries: Vec<ProviderEntry>) {
        *self = Self::new(entries);
    }

    /// Whether the walk has to happen at all.
    ///
    /// It does not when the local engine is switched on and no configured
    /// service sits anywhere in the list: then the worker translates directly
    /// and never builds a request. Asked of the configuration rather than of
    /// the ledger, because an answer that changed as cooldowns came and went
    /// would send the worker down the direct path at the exact moment a
    /// recovered service should have been tried again.
    #[must_use]
    pub fn routing_required(&self, has_credential: impl Fn(&ProviderId) -> bool) -> bool {
        let local_enabled = self
            .entries
            .iter()
            .any(|entry| entry.id.is_local() && entry.enabled);
        let service_configured = self.entries.iter().any(|entry| {
            entry.enabled
                && entry.id.is_known()
                && !entry.id.is_local()
                && has_credential(&entry.id)
        });
        service_configured || !local_enabled
    }

    /// Adds any provider this build knows and the stored list did not mention,
    /// at the end, switched off. An entry the file has that this build does not
    /// know keeps its place and is never selected.
    pub fn merge_known(&mut self) {
        for id in ProviderId::KNOWN {
            let id = ProviderId::new(id);
            if !self.entries.iter().any(|entry| entry.id == id) {
                self.entries.push(ProviderEntry::new(id));
            }
        }
    }

    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
    }

    pub fn set_enabled(&mut self, id: &ProviderId, enabled: bool) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| &entry.id == id) {
            entry.enabled = enabled;
        }
    }

    pub fn set_region(&mut self, id: &ProviderId, region: Option<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| &entry.id == id) {
            entry.region = region.filter(|region| !region.trim().is_empty());
        }
    }

    /// Whether any service would be asked before the local engine is reached.
    ///
    /// Every claim the interface makes about text staying on this machine rests
    /// on this, so it is answered from the list itself rather than from a
    /// separate setting that could disagree with it.
    #[must_use]
    pub fn text_can_leave_device(&self, has_credential: impl Fn(&ProviderId) -> bool) -> bool {
        self.entries
            .iter()
            .filter(|entry| entry.enabled)
            .find_map(|entry| {
                if entry.id.is_local() {
                    // The walk ends at the local engine; nothing below is asked.
                    return Some(false);
                }
                has_credential(&entry.id).then_some(true)
            })
            .unwrap_or(false)
    }

    /// Every row with what the interface should say about it.
    #[must_use]
    pub fn status(
        &self,
        ledger: &Ledger,
        now: Instant,
        has_credential: impl Fn(&ProviderId) -> bool,
    ) -> Vec<ProviderStatus> {
        self.entries
            .iter()
            .enumerate()
            .map(|(position, entry)| ProviderStatus {
                id: entry.id.clone(),
                position,
                enabled: entry.enabled,
                unavailable: ledger.unavailable(entry, now, &has_credential),
            })
            .collect()
    }

    /// Who to ask, in order, for one translation.
    #[must_use]
    pub fn candidates(
        &self,
        ledger: &Ledger,
        now: Instant,
        has_credential: impl Fn(&ProviderId) -> bool,
    ) -> Vec<ProviderId> {
        self.entries
            .iter()
            .filter(|entry| ledger.unavailable(entry, now, &has_credential).is_none())
            .map(|entry| entry.id.clone())
            .collect()
    }

    #[must_use]
    pub fn entry(&self, id: &ProviderId) -> Option<&ProviderEntry> {
        self.entries.iter().find(|entry| &entry.id == id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Chain, Ledger, Unavailable};
    use crate::{ProviderEntry, ProviderId, failure::ProviderFailure};

    fn keyed(id: &str) -> bool {
        id != "never-keyed"
    }

    fn enabled(ids: &[&str]) -> Chain {
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

    #[test]
    fn a_duplicated_row_is_asked_once() {
        // Two rows for one service would spend two lots of a free allowance on
        // a single translation.
        let chain = Chain::new(vec![
            ProviderEntry::new(ProviderId::new(ProviderId::DEEPL)),
            ProviderEntry::new(ProviderId::new(ProviderId::DEEPL)),
        ]);

        assert_eq!(
            chain
                .entries()
                .iter()
                .filter(|entry| entry.id.as_str() == ProviderId::DEEPL)
                .count(),
            1
        );
    }

    #[test]
    fn a_provider_this_build_does_not_know_keeps_its_place_and_is_never_asked() {
        let chain = enabled(&["a-service-from-next-year", "local"]);
        let ledger = Ledger::default();

        assert_eq!(chain.entries()[0].id.as_str(), "a-service-from-next-year");
        assert_eq!(
            chain.candidates(&ledger, Instant::now(), |id| keyed(id.as_str())),
            vec![ProviderId::local()]
        );
    }

    #[test]
    fn a_refused_key_takes_its_row_out_until_it_is_replaced() {
        let chain = enabled(&[ProviderId::DEEPL, ProviderId::LOCAL]);
        let mut ledger = Ledger::default();
        let now = Instant::now();
        ledger.record_failure(
            &ProviderId::new(ProviderId::DEEPL),
            &ProviderFailure::Credential("bad key".into()),
            now,
        );

        assert_eq!(
            chain.candidates(&ledger, now, |id| keyed(id.as_str())),
            vec![ProviderId::local()]
        );

        // Storing a new key is a new fact about it.
        ledger.forget(&ProviderId::new(ProviderId::DEEPL));
        assert_eq!(
            chain
                .candidates(&ledger, now, |id| keyed(id.as_str()))
                .len(),
            2
        );
    }

    #[test]
    fn a_wrong_region_takes_the_row_out_but_leaves_the_key_alone() {
        let chain = enabled(&[ProviderId::AZURE, ProviderId::LOCAL]);
        let mut ledger = Ledger::default();
        let now = Instant::now();
        ledger.record_failure(
            &ProviderId::new(ProviderId::AZURE),
            &ProviderFailure::Misconfigured("wrong region".into()),
            now,
        );

        let status = chain.status(&ledger, now, |id| keyed(id.as_str()));
        assert!(matches!(
            status[0].unavailable,
            Some(Unavailable::Misconfigured(_))
        ));
    }

    #[test]
    fn a_spent_allowance_comes_back_when_the_cooldown_is_over() {
        let chain = enabled(&[ProviderId::GOOGLE, ProviderId::LOCAL]);
        let mut ledger = Ledger::default();
        let now = Instant::now();
        ledger.record_failure(
            &ProviderId::new(ProviderId::GOOGLE),
            &ProviderFailure::QuotaExhausted {
                retry_after: Some(Duration::from_secs(600)),
            },
            now,
        );

        assert_eq!(
            chain
                .candidates(&ledger, now, |id| keyed(id.as_str()))
                .len(),
            1
        );
        assert_eq!(
            chain
                .candidates(&ledger, now + Duration::from_secs(601), |id| keyed(
                    id.as_str()
                ))
                .len(),
            2
        );
    }

    #[test]
    fn text_leaves_the_device_only_when_a_service_is_reached_before_the_local_engine() {
        let service_first = enabled(&[ProviderId::DEEPL, ProviderId::LOCAL]);
        assert!(service_first.text_can_leave_device(|id| keyed(id.as_str())));

        let local_first = enabled(&[ProviderId::LOCAL, ProviderId::DEEPL]);
        assert!(
            !local_first.text_can_leave_device(|id| keyed(id.as_str())),
            "the walk ends at the local engine, so the row below it is never asked"
        );

        let no_key = enabled(&[ProviderId::DEEPL, ProviderId::LOCAL]);
        assert!(!no_key.text_can_leave_device(|_| false));
    }

    #[test]
    fn the_walk_is_skipped_when_only_the_local_engine_is_configured() {
        let local_only = Chain::new(vec![ProviderEntry {
            id: ProviderId::local(),
            enabled: true,
            region: None,
        }]);

        assert!(
            !local_only.routing_required(|_| false),
            "nothing to walk to, so the worker translates directly"
        );
        let mut with_service = local_only;
        with_service.set_enabled(&ProviderId::new(ProviderId::DEEPL), true);
        assert!(
            with_service.routing_required(|id| !id.is_local()),
            "a service that is switched on and keyed means the walk decides"
        );
    }

    #[test]
    fn a_list_read_from_an_older_file_gains_the_providers_it_never_heard_of() {
        let chain = Chain::new(vec![ProviderEntry::new(ProviderId::local())]);

        for known in ProviderId::KNOWN {
            assert!(
                chain
                    .entries()
                    .iter()
                    .any(|entry| entry.id.as_str() == known),
                "{known} should have been added, switched off"
            );
        }
        assert!(
            chain
                .entries()
                .iter()
                .filter(|entry| !entry.id.is_local())
                .all(|entry| !entry.enabled)
        );
    }
}
