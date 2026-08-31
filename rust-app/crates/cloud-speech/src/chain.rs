//! The ordered list of providers and the rule for moving down it.
//!
//! The order is the user's, set by dragging rows in Settings. A row that is
//! switched off is skipped. Otherwise the first row is used, and the next one
//! only when the one before it could not do the work — which in practice means
//! its free allowance ran out.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::{
    failure::ProviderFailure,
    provider::{ProviderEntry, ProviderId},
};

/// Why a provider is not being offered right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    Disabled,
    /// Kept out of the way until its allowance resets or the service calms
    /// down. Carries the remaining wait so Settings can show it.
    CoolingDown {
        remaining: Duration,
        reason: ProviderFailure,
    },
    /// The key was refused. Only the user can fix this.
    CredentialRefused(String),
    /// This build has no adapter for the identifier in the list.
    Unsupported,
    /// Able to run, but not for this recording: too long, too large, or a
    /// translation it cannot perform. Carries no penalty and no cooldown.
    Incapable(&'static str),
    /// The provider needs a key and none is stored.
    MissingCredential,
}

#[derive(Debug, Default, Clone)]
struct ProviderState {
    cooling_until: Option<Instant>,
    cooling_reason: Option<ProviderFailure>,
    refused_credential: Option<String>,
}

/// Remembers what each provider did last, so the chain does not keep walking
/// into the same wall.
#[derive(Debug, Default)]
pub struct Ledger {
    states: HashMap<ProviderId, ProviderState>,
}

impl Ledger {
    pub fn record_failure(&mut self, id: &ProviderId, failure: &ProviderFailure, now: Instant) {
        let state = self.states.entry(id.clone()).or_default();
        if failure.disables_provider() {
            state.refused_credential = Some(failure.to_string());
        }
        if let Some(cooldown) = failure.cooldown() {
            state.cooling_until = Some(now + cooldown);
            state.cooling_reason = Some(failure.clone());
        }
    }

    /// A success clears everything: an allowance that reset, a service that
    /// came back, or a key the user corrected.
    pub fn record_success(&mut self, id: &ProviderId) {
        self.states.remove(id);
    }

    /// Clears the refusal recorded for a key the user has just replaced.
    pub fn clear_credential_refusal(&mut self, id: &ProviderId) {
        if let Some(state) = self.states.get_mut(id) {
            state.refused_credential = None;
        }
    }

    fn availability(&self, id: &ProviderId, now: Instant) -> Option<Unavailable> {
        let state = self.states.get(id)?;
        if let Some(detail) = &state.refused_credential {
            return Some(Unavailable::CredentialRefused(detail.clone()));
        }
        let cooling_until = state.cooling_until?;
        let remaining = cooling_until.checked_duration_since(now)?;
        if remaining.is_zero() {
            return None;
        }
        Some(Unavailable::CoolingDown {
            remaining,
            reason: state
                .cooling_reason
                .clone()
                .unwrap_or(ProviderFailure::Unavailable("unknown".to_owned())),
        })
    }
}

/// What the chain decided about one row, for Settings to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub position: usize,
    pub unavailable: Option<Unavailable>,
}

impl ProviderStatus {
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.unavailable.is_none()
    }
}

/// The user's ordered list.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Chain {
    entries: Vec<ProviderEntry>,
}

impl Chain {
    #[must_use]
    pub fn new(entries: Vec<ProviderEntry>) -> Self {
        Self { entries }
    }

    /// The list a fresh installation starts with: the local engine first and
    /// on, every service present, in a fixed order, and off until the user
    /// adds a key.
    #[must_use]
    pub fn default_order() -> Self {
        Self::new(
            ProviderId::KNOWN
                .iter()
                .map(|id| ProviderEntry::new(id, *id == ProviderId::LOCAL))
                .collect(),
        )
    }

    #[must_use]
    pub fn entries(&self) -> &[ProviderEntry] {
        &self.entries
    }

    /// Moves one row to another position, the way dragging it does.
    ///
    /// Out-of-range positions are clamped rather than rejected: a drag that
    /// ends past the end of the list means "put it last".
    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || self.entries.is_empty() {
            return;
        }
        let to = to.min(self.entries.len() - 1);
        if from == to {
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

    /// Whether a recording has to go through the router at all.
    ///
    /// False only when the local engine is enabled and no network service
    /// could be chosen ahead of it whatever the ledger says, which is what a
    /// fresh installation and an installation without keys both look like. The
    /// caller then keeps its direct local path.
    ///
    /// Deliberately independent of the ledger: a value that changed as
    /// cooldowns came and went would need recomputing at moments nobody
    /// observes, and a service that recovered would never be reached again.
    pub fn routing_required(&self, has_credential: impl Fn(&ProviderId) -> bool) -> bool {
        let local_enabled = self
            .entries
            .iter()
            .any(|entry| entry.id.is_local() && entry.enabled);
        let remote_configured = self.entries.iter().any(|entry| {
            entry.enabled
                && entry.id.is_known()
                && !entry.id.is_local()
                && has_credential(&entry.id)
        });
        remote_configured || !local_enabled
    }

    /// Adds any provider this build knows that the stored list does not
    /// mention, so an upgrade does not hide a new service. New rows go last
    /// and switched off, never ahead of the user's own order.
    pub fn merge_known(&mut self) {
        for id in ProviderId::KNOWN {
            if !self.entries.iter().any(|entry| entry.id.as_str() == id) {
                self.entries.push(ProviderEntry::new(id, false));
            }
        }
    }

    /// The full picture for Settings, in the user's order.
    ///
    /// `has_credential` answers whether a key is stored for a provider; the
    /// chain never sees the key itself.
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
                unavailable: self.unavailable_reason(entry, ledger, now, &has_credential),
            })
            .collect()
    }

    /// The providers to try, in order, for one recording.
    pub fn candidates(
        &self,
        ledger: &Ledger,
        now: Instant,
        has_credential: impl Fn(&ProviderId) -> bool,
    ) -> Vec<ProviderId> {
        self.entries
            .iter()
            .filter(|entry| {
                self.unavailable_reason(entry, ledger, now, &has_credential)
                    .is_none()
            })
            .map(|entry| entry.id.clone())
            .collect()
    }

    fn unavailable_reason(
        &self,
        entry: &ProviderEntry,
        ledger: &Ledger,
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
        ledger.availability(&entry.id, now)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Chain, Ledger, Unavailable};
    use crate::{
        failure::ProviderFailure,
        provider::{ProviderEntry, ProviderId},
    };

    fn everything_has_a_key(_: &ProviderId) -> bool {
        true
    }

    fn chain_of(ids: &[(&str, bool)]) -> Chain {
        Chain::new(
            ids.iter()
                .map(|(id, enabled)| ProviderEntry::new(id, *enabled))
                .collect(),
        )
    }

    #[test]
    fn the_users_order_is_the_order_of_attempts() {
        let chain = chain_of(&[
            (ProviderId::AZURE, true),
            (ProviderId::GROQ, true),
            (ProviderId::LOCAL, true),
        ]);

        let candidates = chain.candidates(&Ledger::default(), Instant::now(), everything_has_a_key);

        assert_eq!(
            candidates
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::AZURE, ProviderId::GROQ, ProviderId::LOCAL]
        );
    }

    #[test]
    fn a_switched_off_row_is_skipped_and_keeps_its_place() {
        let mut chain = chain_of(&[
            (ProviderId::CLOUDFLARE, true),
            (ProviderId::AZURE, true),
            (ProviderId::LOCAL, true),
        ]);
        chain.set_enabled(&ProviderId::new(ProviderId::CLOUDFLARE), false);

        let candidates = chain.candidates(&Ledger::default(), Instant::now(), everything_has_a_key);

        assert_eq!(
            candidates
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::AZURE, ProviderId::LOCAL]
        );
        assert_eq!(
            chain.entries()[0].id.as_str(),
            ProviderId::CLOUDFLARE,
            "the row stays where the user put it"
        );
    }

    #[test]
    fn a_spent_allowance_moves_work_to_the_next_row_and_back_again_later() {
        let chain = chain_of(&[(ProviderId::CLOUDFLARE, true), (ProviderId::AZURE, true)]);
        let mut ledger = Ledger::default();
        let now = Instant::now();

        ledger.record_failure(
            &ProviderId::new(ProviderId::CLOUDFLARE),
            &ProviderFailure::QuotaExhausted {
                retry_after: Some(Duration::from_secs(600)),
            },
            now,
        );

        assert_eq!(
            chain
                .candidates(&ledger, now, everything_has_a_key)
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::AZURE],
            "Cloudflare is out, Azure takes over"
        );

        let later = now + Duration::from_secs(601);
        assert_eq!(
            chain
                .candidates(&ledger, later, everything_has_a_key)
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::CLOUDFLARE, ProviderId::AZURE],
            "once the allowance resets the first choice returns"
        );
    }

    #[test]
    fn a_refused_key_takes_a_provider_out_until_the_user_replaces_it() {
        let chain = chain_of(&[(ProviderId::GROQ, true), (ProviderId::LOCAL, true)]);
        let mut ledger = Ledger::default();
        let now = Instant::now();
        let groq = ProviderId::new(ProviderId::GROQ);

        ledger.record_failure(&groq, &ProviderFailure::Credential("bad key".into()), now);
        let much_later = now + Duration::from_secs(60 * 60 * 24);

        assert_eq!(
            chain
                .candidates(&ledger, much_later, everything_has_a_key)
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::LOCAL],
            "waiting does not fix a wrong key"
        );

        ledger.clear_credential_refusal(&groq);
        assert_eq!(
            chain
                .candidates(&ledger, much_later, everything_has_a_key)
                .len(),
            2
        );
    }

    #[test]
    fn a_provider_without_a_stored_key_is_never_attempted() {
        let chain = chain_of(&[(ProviderId::DEEPGRAM, true), (ProviderId::LOCAL, true)]);

        let candidates = chain.candidates(&Ledger::default(), Instant::now(), |id| id.is_local());

        assert_eq!(
            candidates
                .iter()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            [ProviderId::LOCAL]
        );
    }

    #[test]
    fn an_identifier_this_build_does_not_know_is_kept_but_never_used() {
        let chain = chain_of(&[("some-future-service", true), (ProviderId::LOCAL, true)]);

        let status = chain.status(&Ledger::default(), Instant::now(), everything_has_a_key);

        assert_eq!(status[0].unavailable, Some(Unavailable::Unsupported));
        assert!(status[1].is_available());
    }

    #[test]
    fn every_provider_being_out_leaves_nothing_to_try() {
        let chain = chain_of(&[(ProviderId::GROQ, false), (ProviderId::LOCAL, false)]);

        assert!(
            chain
                .candidates(&Ledger::default(), Instant::now(), everything_has_a_key)
                .is_empty()
        );
    }

    #[test]
    fn dragging_a_row_puts_it_exactly_where_it_was_dropped() {
        let mut chain = chain_of(&[
            (ProviderId::LOCAL, true),
            (ProviderId::GROQ, true),
            (ProviderId::AZURE, true),
        ]);

        chain.move_entry(2, 0);

        assert_eq!(
            chain
                .entries()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [ProviderId::AZURE, ProviderId::LOCAL, ProviderId::GROQ]
        );

        // A drop past the end means last, and an impossible source is ignored.
        chain.move_entry(0, 99);
        chain.move_entry(99, 0);
        assert_eq!(
            chain
                .entries()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [ProviderId::LOCAL, ProviderId::GROQ, ProviderId::AZURE]
        );
    }

    #[test]
    fn an_upgrade_adds_new_services_last_without_disturbing_the_order() {
        let mut chain = chain_of(&[(ProviderId::AZURE, true), (ProviderId::LOCAL, true)]);

        chain.merge_known();

        let ids: Vec<_> = chain
            .entries()
            .iter()
            .map(|entry| entry.id.as_str().to_owned())
            .collect();
        assert_eq!(&ids[..2], &[ProviderId::AZURE, ProviderId::LOCAL]);
        assert_eq!(ids.len(), ProviderId::KNOWN.len());
        assert!(
            chain.entries()[2..].iter().all(|entry| !entry.enabled),
            "a service the user has never seen is off"
        );
    }

    #[test]
    fn the_default_list_starts_local_and_leaves_the_services_off() {
        let chain = Chain::default_order();

        assert_eq!(chain.entries()[0].id.as_str(), ProviderId::LOCAL);
        assert!(chain.entries()[0].enabled);
        assert!(chain.entries()[1..].iter().all(|entry| !entry.enabled));
    }
}
