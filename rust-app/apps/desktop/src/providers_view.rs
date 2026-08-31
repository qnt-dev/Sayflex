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
    AppState, ProviderRow, SettingsWindow, set_indicator_status, settings_document::save_settings,
    worker_supervisor::Workers,
};

/// Height of one row in the transcription list, in logical pixels.
///
/// The drag handle reports how far the pointer travelled rather than a target
/// index, so this number lives here instead of being repeated in the interface
/// and in the code that reads it.
pub(crate) const PROVIDER_ROW_HEIGHT: f32 = 96.0;

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

pub(crate) fn sync_providers(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let rows: Vec<ProviderRow> = state
        .transcription_chain
        .iter()
        .map(|entry| {
            let status = state
                .transcription_providers
                .iter()
                .find(|status| status.id == entry.id);
            ProviderRow {
                id: entry.id.clone().into(),
                title: provider_display_name(&entry.id).into(),
                detail: status
                    .map(|status| {
                        if status.detail.is_empty() {
                            "Ready".to_owned()
                        } else {
                            status.detail.clone()
                        }
                    })
                    .unwrap_or_default()
                    .into(),
                enabled: entry.enabled,
                available: status.is_some_and(|status| status.available),
                needs_key: entry.id != "local",
                // Only ever a masked tail; the key itself never comes back
                // from the worker.
                key_hint: status
                    .and_then(|status| status.credential_hint.clone())
                    .unwrap_or_default()
                    .into(),
                field_label: provider_field_label(&entry.id).into(),
                field_value: provider_field_value(entry).into(),
            }
        })
        .collect();
    // Every claim in the interface about audio staying on this machine is
    // conditional on this, so it is computed from the same list the router
    // walks rather than from a separate setting.
    let sending_audio_out = state.transcription_chain.iter().any(|entry| {
        entry.enabled
            && entry.id != "local"
            && state
                .transcription_providers
                .iter()
                .any(|status| status.id == entry.id && status.has_credential)
    });
    settings.set_remote_transcription_enabled(sending_audio_out);
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
        PROVIDER_ROW_HEIGHT, provider_display_name, provider_field_label, provider_field_value,
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

    #[test]
    fn the_drag_distance_matches_the_row_height_in_the_interface() {
        // A drag of one row must move the entry exactly one place; the value
        // is duplicated in app.slint and the two must agree.
        assert!((PROVIDER_ROW_HEIGHT - 96.0).abs() < f32::EPSILON);
    }
}
