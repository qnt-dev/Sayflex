//! The transcription list in Settings.
//!
//! Turning the arranged order into rows the interface can draw, and turning
//! what the user does to those rows back into a list the worker is told about.
//! The key itself never passes through here in either direction: it goes
//! straight to the worker, and comes back only as a masked tail.

use std::{cell::RefCell, rc::Rc};

use rust_protocol::{
    SpeechWorkerCommand, TranscriptionProviderEntry, TranslationProviderEntry,
    TranslationWorkerCommand,
};
use slint::{ModelRc, VecModel};

use crate::{
    AppState, ProviderRow, SettingsWindow, set_indicator_status,
    settings_document::save_settings,
    settings_projection::{chain_positions, privacy_claim, provider_row, translation_provider_row},
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

/// Why a provider's configuration was refused.
///
/// Every rule lives here rather than in the markup: a rule enforced only where
/// the form is drawn stops applying the moment anything else calls the same
/// callback, and it cannot be tested without a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SaveRefused(pub String);

/// Checks one provider's configuration before any of it is stored.
///
/// Returns the values as they should be written. Nothing is written on a
/// refusal — the previous save called two callbacks in a row, so an endpoint
/// this function rejects still had its key stored, and the form closed as
/// though it had worked.
pub(crate) fn check_provider_configuration(
    provider_id: &str,
    secret: &str,
    extra: &str,
) -> Result<(String, Option<String>), SaveRefused> {
    let secret = secret.trim();
    let extra = extra.trim();

    // A key of nothing but spaces used to pass the form's own emptiness check,
    // reach here, and delete a stored key without being asked — beside a Remove
    // key button that does ask.
    if secret.is_empty() {
        return Err(SaveRefused(
            "A key is needed. To remove the one already stored, use Remove key.".to_owned(),
        ));
    }

    let label = provider_field_label(provider_id);
    if !label.is_empty() && extra.is_empty() {
        return Err(SaveRefused(format!(
            "{label} is needed as well as the key."
        )));
    }

    // A custom endpoint carries the key and the audio in the same request.
    if provider_id == "custom" && !extra.is_empty() && !extra.starts_with("https://") {
        return Err(SaveRefused(
            "The endpoint must be HTTPS: the key and the audio both travel in this request."
                .to_owned(),
        ));
    }

    Ok((
        secret.to_owned(),
        (!extra.is_empty()).then(|| extra.to_owned()),
    ))
}

/// Checks one translation provider's configuration before any of it is stored.
///
/// The same contract as the transcription form and deliberately its own
/// function: the two lists ask for different things. Nothing here takes an
/// endpoint, so there is no address to insist is HTTPS; Azure is the only one
/// that needs a second field, and it needs it.
pub(crate) fn check_translation_provider_configuration(
    provider_id: &str,
    secret: &str,
    extra: &str,
) -> Result<(String, Option<String>), SaveRefused> {
    let secret = secret.trim();
    let extra = extra.trim();

    if secret.is_empty() {
        return Err(SaveRefused(
            "A key is needed. To remove the one already stored, use Remove key.".to_owned(),
        ));
    }
    if provider_id == "azure" && extra.is_empty() {
        return Err(SaveRefused(
            "Region is needed as well as the key.".to_owned(),
        ));
    }

    Ok((
        secret.to_owned(),
        (!extra.is_empty()).then(|| extra.to_owned()),
    ))
}

/// Which of the two lists a row belongs to.
///
/// Both lists have a row for Azure and a row for this computer, and they are
/// different services with different keys and different allowances. Every
/// callback the interface raises carries this so nothing acts on the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Speech,
    Text,
}

/// Reads back the prefix the projection put on a row id.
pub(crate) fn split_row_id(id: &str) -> Option<(Kind, &str)> {
    match id.split_once(':') {
        Some(("speech", rest)) => Some((Kind::Speech, rest)),
        Some(("text", rest)) => Some((Kind::Text, rest)),
        _ => None,
    }
}

/// Which list a position in the drawn list falls in, and where in that list.
///
/// The two are drawn as one list so they scroll together, so the index a row
/// reports is an index into the pair of them.
pub(crate) fn locate(state: &AppState, index: usize) -> Option<(Kind, usize)> {
    let speech = state.transcription_chain.len();
    if index < speech {
        return Some((Kind::Speech, index));
    }
    let text = index - speech;
    (text < state.translation_chain.len()).then_some((Kind::Text, text))
}

/// What to call a row in a message to the user.
///
/// The two lists both have Azure, and they are not the same Azure.
pub(crate) fn row_display_name(kind: Kind, id: &str) -> String {
    match kind {
        Kind::Speech => provider_display_name(id),
        Kind::Text => crate::settings_projection::translation_provider_display_name(id),
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
    let has_credential = |id: &str| {
        state
            .transcription_providers
            .iter()
            .any(|status| status.id == id && status.has_credential)
    };
    let has_text_credential = |id: &str| {
        state
            .translation_providers
            .iter()
            .any(|status| status.id == id && status.has_credential)
    };
    // Where the walk starts and where it stops, worked out once for the whole
    // list: a row cannot say "in use" while another row says the same.
    let (first_local, first_usable) = chain_positions(&state.transcription_chain, has_credential);

    let mut rows: Vec<ProviderRow> = state
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

    let (text_local, text_usable) =
        text_chain_positions(&state.translation_chain, has_text_credential);
    rows.extend(
        state
            .translation_chain
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                let status = state
                    .translation_providers
                    .iter()
                    .find(|status| status.id == entry.id);
                translation_provider_row(entry, status, position, text_local, text_usable)
            }),
    );

    let leaves = audio_can_leave_device(&state.transcription_chain, has_credential);
    let text_leaves = text_can_leave_device(&state.translation_chain, has_text_credential);
    settings.set_remote_transcription_enabled(leaves || text_leaves);
    settings.set_provider_claim(
        privacy_claim(&state.transcription_chain, leaves, has_credential).into(),
    );
    settings.set_app_version(env!("CARGO_PKG_VERSION").into());
    settings.set_provider_rows(ModelRc::new(VecModel::from(rows)));
}

/// Where the translation walk starts and where it stops.
///
/// The same question `chain_positions` answers for transcription, asked of the
/// other list. Its own function rather than a generic one: the two lists hold
/// different entry types and share nothing but the shape of the answer.
fn text_chain_positions(
    chain: &[TranslationProviderEntry],
    has_credential: impl Fn(&str) -> bool,
) -> (Option<usize>, Option<usize>) {
    let mut first_local = None;
    let mut first_usable = None;
    for (position, entry) in chain.iter().enumerate() {
        if !entry.enabled {
            continue;
        }
        if entry.id == "local" {
            first_local.get_or_insert(position);
            first_usable.get_or_insert(position);
            break;
        }
        if has_credential(&entry.id) {
            first_usable.get_or_insert(position);
        }
    }
    (first_local, first_usable)
}

/// Whether this arrangement can send text off the machine.
///
/// The same reading the worker does, so the claim on Overview and the walk the
/// worker performs cannot disagree: the first switched-on row decides, and a
/// local engine anywhere above a service means nothing below it is asked.
pub(crate) fn text_can_leave_device(
    chain: &[TranslationProviderEntry],
    has_credential: impl Fn(&str) -> bool,
) -> bool {
    chain
        .iter()
        .filter(|entry| entry.enabled)
        .find_map(|entry| {
            if entry.id == "local" {
                return Some(false);
            }
            has_credential(&entry.id).then_some(true)
        })
        .unwrap_or(false)
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

/// Sends the arranged translation list to the worker and writes it to settings.
pub(crate) fn apply_translation_chain(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
) {
    let providers = state.borrow().translation_chain.clone();
    if let Err(error) = workers
        .translation
        .send(&TranslationWorkerCommand::SetTranslationProviders { providers })
    {
        set_indicator_status(
            settings,
            &format!("The translation order could not be applied: {error}"),
        );
        return;
    }
    let _ = save_settings(&state.borrow());
    sync_providers(settings, state);
}

#[cfg(test)]
mod translation_provider_tests {
    use rust_protocol::TranslationProviderEntry;

    use super::{
        Kind, SaveRefused, check_translation_provider_configuration, split_row_id,
        text_can_leave_device, text_chain_positions,
    };

    fn entry(id: &str, enabled: bool) -> TranslationProviderEntry {
        TranslationProviderEntry {
            id: id.to_owned(),
            enabled,
            region: None,
        }
    }

    #[test]
    fn a_row_id_says_which_list_it_came_from() {
        // Both lists have an azure and a local. Without the prefix, storing a
        // DeepL-page key for Azure would reach the speech service instead.
        assert_eq!(split_row_id("speech:azure"), Some((Kind::Speech, "azure")));
        assert_eq!(split_row_id("text:azure"), Some((Kind::Text, "azure")));
        assert_eq!(split_row_id("azure"), None);
        assert_eq!(split_row_id("other:azure"), None);
    }

    #[test]
    fn text_leaves_the_machine_only_when_a_keyed_service_is_reached_first() {
        let keyed = |id: &str| id != "unkeyed";

        assert!(text_can_leave_device(
            &[entry("deepl", true), entry("local", true)],
            keyed
        ));
        assert!(
            !text_can_leave_device(&[entry("local", true), entry("deepl", true)], keyed),
            "the walk stops at the local engine, so nothing under it is asked"
        );
        assert!(
            !text_can_leave_device(&[entry("unkeyed", true), entry("local", true)], keyed),
            "a service with no key is never asked, so it sends nothing"
        );
        assert!(
            !text_can_leave_device(&[entry("deepl", false)], keyed),
            "a row that is switched off sends nothing"
        );
    }

    #[test]
    fn the_first_row_that_can_run_is_the_one_marked_in_use() {
        let keyed = |id: &str| id != "unkeyed";
        let (local, usable) = text_chain_positions(
            &[
                entry("unkeyed", true),
                entry("deepl", true),
                entry("local", true),
            ],
            keyed,
        );

        assert_eq!(
            usable,
            Some(1),
            "the keyed service above the engine is used"
        );
        assert_eq!(local, Some(2));
    }

    #[test]
    fn a_key_of_nothing_but_spaces_is_refused_rather_than_treated_as_a_deletion() {
        assert_eq!(
            check_translation_provider_configuration("deepl", "   ", ""),
            Err(SaveRefused(
                "A key is needed. To remove the one already stored, use Remove key.".to_owned()
            ))
        );
    }

    #[test]
    fn azure_needs_its_region_and_nothing_else_does() {
        assert!(check_translation_provider_configuration("azure", "key", "").is_err());
        assert_eq!(
            check_translation_provider_configuration("azure", " key ", " westeurope "),
            Ok(("key".to_owned(), Some("westeurope".to_owned())))
        );
        assert_eq!(
            check_translation_provider_configuration("deepl", "key", ""),
            Ok(("key".to_owned(), None))
        );
    }
}

#[cfg(test)]
mod transcription_provider_tests {
    use rust_protocol::TranscriptionProviderEntry;

    use super::{
        audio_can_leave_device, check_provider_configuration, provider_display_name,
        provider_field_label, provider_field_value, set_provider_field_value,
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
    fn a_key_of_nothing_but_spaces_does_not_delete_the_one_already_stored() {
        // The form checked `value == ""`, which spaces pass. Rust then trimmed
        // it to empty and sent `secret: None`, deleting a working key with no
        // question asked — next to a Remove key button that asks one.
        let refused = check_provider_configuration("groq", "   ", "")
            .expect_err("a blank key is not a deletion");

        assert!(refused.0.contains("Remove key"));
    }

    #[test]
    fn an_endpoint_that_is_not_https_stops_the_whole_save() {
        // It used to reject the endpoint and store the key anyway, leaving the
        // provider keyed and misconfigured while the form closed as though it
        // had worked.
        let refused = check_provider_configuration("custom", "sk-live-1234", "http://example.test")
            .expect_err("the key and the audio both travel in this request");

        assert!(refused.0.contains("HTTPS"));
    }

    #[test]
    fn a_provider_that_needs_a_second_field_will_not_save_without_it() {
        let refused = check_provider_configuration("azure", "sk-live-1234", "  ")
            .expect_err("Azure answers 403 without a region");

        assert!(refused.0.contains("Region"));
    }

    #[test]
    fn a_good_configuration_comes_back_trimmed_and_whole() {
        let (secret, extra) =
            check_provider_configuration("azure", "  sk-live-1234  ", "  westeurope  ")
                .expect("both fields are present");

        assert_eq!(secret, "sk-live-1234");
        assert_eq!(extra.as_deref(), Some("westeurope"));
    }

    #[test]
    fn a_provider_with_no_second_field_saves_with_the_key_alone() {
        let (secret, extra) = check_provider_configuration("groq", "gsk_live_9c07", "")
            .expect("Groq takes no region");

        assert_eq!(secret, "gsk_live_9c07");
        assert_eq!(extra, None);
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
