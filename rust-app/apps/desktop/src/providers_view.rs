//! The transcription list in Settings.
//!
//! Turning the arranged order into rows the interface can draw, and turning
//! what the user does to those rows back into a list the worker is told about.
//! The key itself never passes through here in either direction: it goes
//! straight to the worker, and comes back only as a masked tail.

use std::{cell::RefCell, rc::Rc};

use rust_protocol::{SpeechWorkerCommand, TranscriptionProviderEntry};
use slint::{ModelRc, VecModel};

use crate::{
    AppState, ProviderRow, SettingsWindow, set_indicator_status,
    settings_document::save_settings,
    settings_projection::{chain_positions, privacy_claim, provider_row},
    worker_supervisor::Workers,
};

pub(crate) fn provider_display_name(id: &str) -> String {
    match id {
        "local" => "This computer".to_owned(),
        "groq" => "Groq".to_owned(),
        "cloudflare" => "Cloudflare Workers AI".to_owned(),
        "azure" => "Microsoft Azure Speech".to_owned(),
        "deepgram" => "Deepgram".to_owned(),
        "custom" => "Custom endpoint".to_owned(),
        other => other.to_owned(),
    }
}

/// The one extra field a service needs beyond its key, if any.
pub(crate) const fn provider_field_label(id: &str) -> &'static str {
    match id.as_bytes() {
        b"cloudflare" => "Account",
        b"azure" => "Region",
        b"custom" => "Endpoint",
        _ => "",
    }
}

pub(crate) fn provider_field_value(entry: &TranscriptionProviderEntry) -> String {
    match entry.id.as_str() {
        "custom" => entry.endpoint.clone().unwrap_or_default(),
        "cloudflare" | "azure" => entry.account.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

pub(crate) fn set_provider_field_value(entry: &mut TranscriptionProviderEntry, value: String) {
    let value = value.trim().to_owned();
    let value = (!value.is_empty()).then_some(value);
    match entry.id.as_str() {
        "custom" => entry.endpoint = value,
        "cloudflare" | "azure" => entry.account = value,
        _ => {}
    }
}

/// Whether this arrangement can send a recording off the machine.
///
/// Every claim in the interface about audio staying here rests on this, so it
/// walks the list the way the router walks it rather than asking whether a
/// service is switched on somewhere. `Router::transcribe` returns
/// `UseLocalEngine` at the first local candidate and stops, so a keyed service
/// below an enabled local engine is never consulted: it must not make the
/// interface announce that audio is leaving.
///
/// Momentary availability is deliberately not consulted. A service that is
/// cooling down will be used again within the hour, and a privacy claim that
/// flickers with a quota counter teaches the user to ignore it.
pub(crate) fn audio_can_leave_device(
    chain: &[TranscriptionProviderEntry],
    has_credential: impl Fn(&str) -> bool,
) -> bool {
    chain
        .iter()
        .filter(|entry| entry.enabled)
        .find_map(|entry| {
            if entry.id == "local" {
                // The walk ends here; nothing below is reached.
                return Some(false);
            }
            has_credential(&entry.id).then_some(true)
        })
        .unwrap_or(false)
}

pub(crate) fn sync_providers(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let has_credential = |id: &str| {
        state
            .transcription_providers
            .iter()
            .any(|status| status.id == id && status.has_credential)
    };
    // Where the walk starts and where it stops, worked out once for the whole
    // list: a row cannot say "in use" while another row says the same.
    let (first_local, first_usable) = chain_positions(&state.transcription_chain, has_credential);

    let rows: Vec<ProviderRow> = state
        .transcription_chain
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            let status = state
                .transcription_providers
                .iter()
                .find(|status| status.id == entry.id);
            provider_row(entry, status, position, first_local, first_usable)
        })
        .collect();

    let leaves = audio_can_leave_device(&state.transcription_chain, has_credential);
    settings.set_remote_transcription_enabled(leaves);
    settings.set_provider_claim(
        privacy_claim(&state.transcription_chain, leaves, has_credential).into(),
    );
    settings.set_app_version(env!("CARGO_PKG_VERSION").into());
    settings.set_provider_rows(ModelRc::new(VecModel::from(rows)));
}

/// Sends the arranged list to the worker and writes it to settings.
pub(crate) fn apply_provider_chain(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
) {
    let providers = state.borrow().transcription_chain.clone();
    if let Err(error) = workers
        .speech
        .send(&SpeechWorkerCommand::SetTranscriptionProviders { providers })
    {
        set_indicator_status(
            settings,
            &format!("The transcription order could not be applied: {error}"),
        );
        return;
    }
    let _ = save_settings(&state.borrow());
    sync_providers(settings, state);
}

#[cfg(test)]
mod transcription_provider_tests {
    use rust_protocol::TranscriptionProviderEntry;

    use super::{
        audio_can_leave_device, provider_display_name, provider_field_label, provider_field_value,
        set_provider_field_value,
    };

    fn entry(id: &str) -> TranscriptionProviderEntry {
        TranscriptionProviderEntry {
            id: id.to_owned(),
            enabled: true,
            endpoint: None,
            model: None,
            account: None,
        }
    }

    #[test]
    fn each_service_asks_for_the_one_extra_thing_it_needs() {
        assert_eq!(provider_field_label("cloudflare"), "Account");
        assert_eq!(provider_field_label("azure"), "Region");
        assert_eq!(provider_field_label("custom"), "Endpoint");
        assert_eq!(provider_field_label("groq"), "");
        assert_eq!(provider_field_label("local"), "");
    }

    #[test]
    fn the_extra_field_is_written_to_the_place_that_service_reads() {
        let mut cloudflare = entry("cloudflare");
        set_provider_field_value(&mut cloudflare, " abc123 ".to_owned());
        assert_eq!(cloudflare.account.as_deref(), Some("abc123"));
        assert_eq!(provider_field_value(&cloudflare), "abc123");

        let mut custom = entry("custom");
        set_provider_field_value(&mut custom, "https://speech.example.test/v1".to_owned());
        assert_eq!(
            custom.endpoint.as_deref(),
            Some("https://speech.example.test/v1")
        );
        assert!(custom.account.is_none(), "a custom endpoint has no account");
    }

    #[test]
    fn clearing_the_field_removes_it_rather_than_storing_an_empty_string() {
        let mut azure = entry("azure");
        set_provider_field_value(&mut azure, "westeurope".to_owned());
        set_provider_field_value(&mut azure, "   ".to_owned());

        assert_eq!(azure.account, None);
        assert_eq!(provider_field_value(&azure), "");
    }

    #[test]
    fn a_service_this_build_does_not_know_still_shows_its_identifier() {
        assert_eq!(provider_display_name("local"), "This computer");
        assert_eq!(provider_display_name("groq"), "Groq");
        assert_eq!(
            provider_display_name("a-service-from-the-future"),
            "a-service-from-the-future"
        );
    }

    fn chain(rows: &[(&str, bool)]) -> Vec<TranscriptionProviderEntry> {
        rows.iter()
            .map(|(id, enabled)| TranscriptionProviderEntry {
                enabled: *enabled,
                ..entry(id)
            })
            .collect()
    }

    #[test]
    fn a_keyed_service_below_the_local_engine_does_not_send_audio_out() {
        // The router stops at the local engine, so Groq here is never asked,
        // and the interface must not say the recording is leaving.
        let rows = chain(&[("local", true), ("groq", true)]);
        assert!(!audio_can_leave_device(&rows, |id| id == "groq"));
    }

    #[test]
    fn a_keyed_service_above_the_local_engine_does_send_audio_out() {
        let rows = chain(&[("groq", true), ("local", true)]);
        assert!(audio_can_leave_device(&rows, |id| id == "groq"));
    }

    #[test]
    fn a_service_without_a_key_is_not_a_service() {
        let rows = chain(&[("groq", true), ("local", true)]);
        assert!(!audio_can_leave_device(&rows, |_| false));
    }

    #[test]
    fn a_switched_off_local_engine_does_not_end_the_walk() {
        // Switching the local engine off is how a user says "always use a
        // service", and the row below it is then genuinely reached.
        let rows = chain(&[("local", false), ("groq", true)]);
        assert!(audio_can_leave_device(&rows, |id| id == "groq"));
    }

    #[test]
    fn an_empty_or_silent_chain_claims_nothing() {
        assert!(!audio_can_leave_device(&[], |_| true));
        let off = chain(&[("groq", false), ("local", false)]);
        assert!(!audio_can_leave_device(&off, |_| true));
    }
}
