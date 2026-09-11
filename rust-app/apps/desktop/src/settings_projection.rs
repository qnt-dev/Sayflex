//! What the settings window says, worked out apart from the drawing of it.
//!
//! Everything here is a pure function from application state to the words and
//! marks on a row. That is deliberate: the claims this window makes — whether
//! audio leaves the machine, whether a provider will ever be reached, what a
//! model is doing right now — are the kind that have to be right, and a claim
//! computed inside a Slint callback is a claim that cannot be tested.
//!
//! The vocabulary is not invented here either. Provider availability comes back
//! from the worker already worded (`describe` in the speech worker), and the
//! protocol carries no remaining-quota figure, so none is drawn.

use rust_protocol::{
    HistoryEntry, ModelSummary, TranscriptionProviderEntry, TranscriptionProviderStatus,
    TranslationModelSummary, TranslationProviderEntry, TranslationProviderStatus,
};

use crate::{
    HistoryRow, ModelRow, ProviderRow,
    localization::{UiLocale, UiText, render, text},
    providers_view::provider_display_name,
};

const SPEECH_PROVIDER_SECTION: &str = "SPEECH";
const TEXT_PROVIDER_SECTION: &str = "TEXT";

/// How far along an install is, when one is running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Download {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

impl Download {
    fn fraction(self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.downloaded_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
    }

    fn percent(self) -> u64 {
        if self.total_bytes == 0 {
            return 0;
        }
        self.downloaded_bytes
            .saturating_mul(100)
            .saturating_div(self.total_bytes)
    }
}

/// The five tones a row can carry. They are the same five everywhere, so a
/// mark means one thing across the whole window.
pub(crate) mod tone {
    pub(crate) const ACTIVE: &str = "active";
    pub(crate) const RESTING: &str = "resting";
    pub(crate) const WORKING: &str = "working";
    pub(crate) const REFUSED: &str = "refused";
    pub(crate) const OFF: &str = "off";
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    }
}

/// One row of the speech-model list.
///
/// The verb belongs to the row it acts on, and there is one at rest: Install
/// for something not here yet, Use for something installed but not chosen,
/// Cancel while a download runs. There is no Remove, because no command in the
/// protocol removes a model and a button that cannot do its job is worse than
/// no button.
pub(crate) fn speech_model_row_localized(
    model: &ModelSummary,
    download: Option<Download>,
    locale: UiLocale,
) -> ModelRow {
    let (state, tone, action, progress) =
        model_progress_localized(model.installed, model.selected, download, locale);
    ModelRow {
        catalog_id: model.catalog_id.clone().into(),
        section: text(locale, UiText::SpeechSection).into(),
        title: model.display_name.clone().into(),
        detail: format!(
            "{}{}",
            model_description_localized(&model.catalog_id, &model.description, locale),
            if model.recommended {
                format!(" · {}", text(locale, UiText::Recommended))
            } else {
                String::new()
            }
        )
        .into(),
        size: human_size(model.size_bytes).into(),
        state: state.into(),
        tone: tone.into(),
        action: action.into(),
        action_key: model_action_key(model.installed, model.selected, download).into(),
        selected: model.selected,
        installed: model.installed,
        progress,
    }
}

/// One row of the translation-model list.
pub(crate) fn translation_model_row_localized(
    model: &TranslationModelSummary,
    selected: bool,
    download: Option<Download>,
    locale: UiLocale,
) -> ModelRow {
    let (state, tone, action, progress) =
        model_progress_localized(model.installed, selected, download, locale);
    ModelRow {
        catalog_id: model.catalog_id.clone().into(),
        section: text(locale, UiText::TranslationSection).into(),
        title: model.display_name.clone().into(),
        detail: format!(
            "{} · {}{}",
            model_description_localized(&model.catalog_id, &model.description, locale),
            model.license,
            if model.recommended {
                format!(" · {}", text(locale, UiText::RecommendedForSystemLanguage))
            } else {
                String::new()
            }
        )
        .into(),
        size: human_size(model.size_bytes).into(),
        state: state.into(),
        tone: tone.into(),
        action: action.into(),
        action_key: model_action_key(model.installed, selected, download).into(),
        selected,
        installed: model.installed,
        progress,
    }
}

fn model_description_localized<'a>(
    catalog_id: &str,
    fallback: &'a str,
    locale: UiLocale,
) -> std::borrow::Cow<'a, str> {
    let message = match catalog_id {
        "whisper-small" => UiText::WhisperSmallDescription,
        "whisper-medium" => UiText::WhisperMediumDescription,
        "whisper-large-v3-turbo" => UiText::WhisperTurboDescription,
        "whisper-large-v3" => UiText::WhisperLargeDescription,
        "qwen35-4b-q4" => UiText::QwenFastDescription,
        "qwen35-9b-q4" => UiText::QwenQualityDescription,
        _ => return std::borrow::Cow::Borrowed(fallback),
    };
    std::borrow::Cow::Borrowed(text(locale, message))
}

fn model_action_key(installed: bool, selected: bool, download: Option<Download>) -> &'static str {
    if download.is_some() && !installed {
        "cancel"
    } else if selected {
        ""
    } else if installed {
        "use"
    } else {
        "install"
    }
}

#[cfg(test)]
fn model_progress(
    installed: bool,
    selected: bool,
    download: Option<Download>,
) -> (String, &'static str, String, f32) {
    model_progress_localized(installed, selected, download, UiLocale::English)
}

fn model_progress_localized(
    installed: bool,
    selected: bool,
    download: Option<Download>,
    locale: UiLocale,
) -> (String, &'static str, String, f32) {
    // A model on disk is not coming down the wire, whatever the last progress
    // report said. The lifecycle already clears the entry on every terminal
    // event; this is the rule stated where the row is drawn, so that forgetting
    // one of those events costs a stale entry rather than a row stuck at
    // "100 %" with a Cancel button that cancels nothing.
    let download = download.filter(|_| !installed);
    if let Some(download) = download {
        return (
            format!("{} %", download.percent()),
            tone::WORKING,
            text(locale, UiText::Cancel).to_owned(),
            download.fraction(),
        );
    }
    if selected {
        return (
            text(locale, UiText::InUse).to_owned(),
            tone::ACTIVE,
            String::new(),
            0.0,
        );
    }
    if installed {
        return (
            text(locale, UiText::Installed).to_owned(),
            tone::RESTING,
            text(locale, UiText::Use).to_owned(),
            0.0,
        );
    }
    (
        text(locale, UiText::NotInstalled).to_owned(),
        tone::OFF,
        text(locale, UiText::Install).to_owned(),
        0.0,
    )
}

/// One row of the transcription order.
///
/// `status` is what the worker last reported about this entry. Its typed state
/// is localized for the state column; free-form diagnostic prose stays in the
/// log instead of leaking English into another locale. `unreachable` is this window's own
/// conclusion: the router hands the recording to the local engine as soon as it
/// reaches an enabled one and stops there, so anything below that row is never
/// asked. Leaving such a row looking ready would be the interface promising
/// something the router does not do.
#[cfg(test)]
pub(crate) fn provider_row(
    entry: &TranscriptionProviderEntry,
    status: Option<&TranscriptionProviderStatus>,
    position: usize,
    first_local: Option<usize>,
    first_usable: Option<usize>,
) -> ProviderRow {
    provider_row_localized(
        entry,
        status,
        position,
        first_local,
        first_usable,
        UiLocale::English,
    )
}

pub(crate) fn provider_row_localized(
    entry: &TranscriptionProviderEntry,
    status: Option<&TranscriptionProviderStatus>,
    position: usize,
    first_local: Option<usize>,
    first_usable: Option<usize>,
    locale: UiLocale,
) -> ProviderRow {
    let unreachable = first_local.is_some_and(|local| position > local);
    let has_credential = status.is_some_and(|status| status.has_credential);
    let detail = status
        .map(|status| status.detail.clone())
        .unwrap_or_default();
    let unavailable_reason = !detail.is_empty();

    let (state, tone) = row_state_localized(
        entry.enabled,
        &entry.id,
        has_credential,
        &detail,
        unreachable,
        first_usable == Some(position),
        locale,
    );
    let _ = unavailable_reason;

    ProviderRow {
        id: qualified_row_id("speech", &entry.id).into(),
        section: SPEECH_PROVIDER_SECTION.into(),
        title: provider_display_name_localized(&entry.id, locale).into(),
        // Stable localized capability text belongs here. The state column
        // carries the localized failure class; arbitrary worker diagnostics
        // remain in the log.
        detail: provider_detail(entry, status, locale).into(),
        enabled: entry.enabled,
        available: status.is_some_and(|status| status.available),
        needs_key: entry.id != "local",
        key_hint: status
            .and_then(|status| status.credential_hint.clone())
            .unwrap_or_default()
            .into(),
        field_label: match entry.id.as_str() {
            "cloudflare" => text(locale, UiText::Account),
            "azure" => text(locale, UiText::Region),
            "custom" => text(locale, UiText::Endpoint),
            _ => "",
        }
        .into(),
        field_value: crate::providers_view::provider_field_value(entry).into(),
        state: state.into(),
        tone: tone.into(),
        unreachable,
    }
}

/// The two short words a row shows in its state column, and their tone.
///
/// The column is short words because it is a column: a sentence there either
/// elides to nothing useful or pushes the row's controls off the edge. The
/// free-form worker sentence remains diagnostic-only; the detail line keeps
/// stable localized capability text.
///
/// Shared by both lists, because a row is a row: whether it is a service that
/// transcribes or one that translates changes nothing about what "cooling down"
/// looks like.
fn row_state_localized(
    enabled: bool,
    id: &str,
    has_credential: bool,
    detail: &str,
    unreachable: bool,
    in_use: bool,
    locale: UiLocale,
) -> (String, &'static str) {
    if !enabled {
        (text(locale, UiText::SwitchedOff).to_owned(), tone::OFF)
    } else if id != "local" && !has_credential {
        (text(locale, UiText::NoKeyStored).to_owned(), tone::OFF)
    } else if !detail.is_empty() {
        if detail.starts_with("Key refused") {
            (text(locale, UiText::KeyRefused).to_owned(), tone::REFUSED)
        } else if detail.contains("available again") || detail.starts_with("Waiting") {
            (text(locale, UiText::CoolingDown).to_owned(), tone::WORKING)
        } else {
            (text(locale, UiText::Unavailable).to_owned(), tone::WORKING)
        }
    } else if unreachable {
        (text(locale, UiText::NeverReached).to_owned(), tone::OFF)
    } else if in_use {
        (text(locale, UiText::InUse).to_owned(), tone::ACTIVE)
    } else {
        (text(locale, UiText::StandingBy).to_owned(), tone::RESTING)
    }
}

/// Both lists have a row called `azure` and a row called `local`, and the two
/// mean different services with different keys. A callback that carried the
/// bare id would act on whichever of them it found first, which is how a key
/// for one service ends up stored against the other.
pub(crate) fn qualified_row_id(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

pub(crate) fn provider_display_name_localized(id: &str, locale: UiLocale) -> String {
    match id {
        "local" => text(locale, UiText::ThisComputer).to_owned(),
        "custom" => text(locale, UiText::CustomProvider).to_owned(),
        _ => provider_display_name(id),
    }
}

/// One row of the translation order.
pub(crate) fn translation_provider_row_localized(
    entry: &TranslationProviderEntry,
    status: Option<&TranslationProviderStatus>,
    position: usize,
    first_local: Option<usize>,
    first_usable: Option<usize>,
    locale: UiLocale,
) -> ProviderRow {
    let unreachable = first_local.is_some_and(|local| position > local);
    let has_credential = status.is_some_and(|status| status.has_credential);
    let detail = status
        .map(|status| status.detail.clone())
        .unwrap_or_default();
    let (state, tone) = row_state_localized(
        entry.enabled,
        &entry.id,
        has_credential,
        &detail,
        unreachable,
        first_usable == Some(position),
        locale,
    );

    ProviderRow {
        id: qualified_row_id("text", &entry.id).into(),
        section: TEXT_PROVIDER_SECTION.into(),
        title: translation_provider_display_name_localized(&entry.id, locale).into(),
        detail: translation_provider_detail(entry, status, locale).into(),
        enabled: entry.enabled,
        available: status.is_some_and(|status| status.available),
        needs_key: entry.id != "local",
        key_hint: status
            .and_then(|status| status.credential_hint.clone())
            .unwrap_or_default()
            .into(),
        field_label: if entry.id == "azure" {
            text(locale, UiText::Region)
        } else {
            ""
        }
        .into(),
        field_value: entry.region.clone().unwrap_or_default().into(),
        state: state.into(),
        tone: tone.into(),
        unreachable,
    }
}

pub(crate) fn translation_provider_display_name(id: &str) -> String {
    translation_provider_display_name_localized(id, UiLocale::English)
}

pub(crate) fn translation_provider_display_name_localized(id: &str, locale: UiLocale) -> String {
    match id {
        "local" => text(locale, UiText::ThisComputer).to_owned(),
        "deepl" => "DeepL".to_owned(),
        "azure" => "Microsoft Azure Translator".to_owned(),
        // Older settings may retain this unknown row. It is preserved but is
        // not offered or routed by this build.
        "google" => text(locale, UiText::GoogleUnsupported).to_owned(),
        other => other.to_owned(),
    }
}

/// What the service is, plus the masked tail of the key if one is stored.
///
/// Availability is deliberately worded without a numeric quota: commercial
/// allowances vary by account, region and date, and the worker has no live
/// remaining-quota value to report.
fn translation_provider_detail(
    entry: &TranslationProviderEntry,
    status: Option<&TranslationProviderStatus>,
    locale: UiLocale,
) -> String {
    let about = match entry.id.as_str() {
        "local" => text(locale, UiText::RunsOnComputer),
        "deepl" => text(locale, UiText::DeepLAvailability),
        "azure" => text(locale, UiText::AzureRegionOptional),
        "google" => text(locale, UiText::LegacyProviderUnsupported),
        _ => text(locale, UiText::UnknownService),
    };
    match status.and_then(|status| status.credential_hint.as_deref()) {
        Some(hint) if !hint.is_empty() => {
            let extra = entry
                .region
                .as_deref()
                .filter(|region| !region.is_empty())
                .map_or_else(String::new, |region| format!(" · {region}"));
            render(
                locale,
                UiText::KeyDetail,
                &[("hint", hint), ("extra", &extra), ("about", about)],
            )
        }
        _ => about.to_owned(),
    }
}

/// What the service is, plus the masked tail of the key if one is stored.
///
/// No counter: `TranscriptionProviderStatus` carries no remaining quota, and a
/// number this window makes up is a number it then has to defend.
fn provider_detail(
    entry: &TranscriptionProviderEntry,
    status: Option<&TranscriptionProviderStatus>,
    locale: UiLocale,
) -> String {
    let about = provider_about(&entry.id, entry, locale);
    match status.and_then(|status| status.credential_hint.as_deref()) {
        Some(hint) if !hint.is_empty() => {
            let extra = entry
                .account
                .as_deref()
                .filter(|account| !account.is_empty())
                .map_or_else(String::new, |account| format!(" · {account}"));
            render(
                locale,
                UiText::KeyDetail,
                &[("hint", hint), ("extra", &extra), ("about", &about)],
            )
        }
        _ => about,
    }
}

fn provider_about(id: &str, entry: &TranscriptionProviderEntry, locale: UiLocale) -> String {
    match id {
        "local" => text(locale, UiText::RunsOnComputer).to_owned(),
        "groq" => text(locale, UiText::GroqAvailability).to_owned(),
        "cloudflare" => text(locale, UiText::CloudflareAvailability).to_owned(),
        "azure" => text(locale, UiText::AzureSpeechAvailability).to_owned(),
        "deepgram" => text(locale, UiText::DeepgramAvailability).to_owned(),
        "custom" => entry
            .endpoint
            .clone()
            .unwrap_or_else(|| text(locale, UiText::CustomEndpoint).to_owned()),
        other => other.to_owned(),
    }
}

/// Where the walk actually starts and where it stops.
pub(crate) fn chain_positions(
    chain: &[TranscriptionProviderEntry],
    has_credential: impl Fn(&str) -> bool,
) -> (Option<usize>, Option<usize>) {
    let usable = |entry: &TranscriptionProviderEntry| {
        entry.enabled && (entry.id == "local" || has_credential(&entry.id))
    };
    let first_local = chain
        .iter()
        .position(|entry| entry.id == "local" && entry.enabled);
    let first_usable = chain.iter().position(usable);
    (first_local, first_usable)
}

/// What would leave this machine, and where it would go.
///
/// Two lists now, and two answers. One boolean and one sentence about audio
/// could not tell the truth about both: an arrangement that sent every
/// selection to DeepL and kept the recordings here said "Audio stays on this
/// device", which was true and was not the whole of it, while the line along
/// the bottom said AUDIO LEAVES DEVICE when only text did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Departures {
    /// The service a recording would reach. `None` means it stays here.
    pub audio_to: Option<String>,
    /// The service a selection would reach. `None` means it stays here.
    pub text_to: Option<String>,
    /// Whether anything at all can transcribe. Nothing can when every row is
    /// switched off or keyless, and that is worth saying before anything else.
    pub can_transcribe: bool,
}

impl Departures {
    pub(crate) const fn audio_leaves(&self) -> bool {
        self.audio_to.is_some()
    }

    pub(crate) const fn text_leaves(&self) -> bool {
        self.text_to.is_some()
    }
}

/// The line in the corner of Overview and Providers.
///
/// Both pages read it from here so the window cannot say two different things
/// about the same arrangement.
#[cfg(test)]
pub(crate) fn privacy_claim(leaving: &Departures) -> String {
    privacy_claim_localized(leaving, UiLocale::English)
}

pub(crate) fn privacy_claim_localized(leaving: &Departures, locale: UiLocale) -> String {
    // Composed from two independent facts rather than short-circuited on the
    // first. It used to return "Nothing can transcribe right now" and stop, so
    // an arrangement with no usable speech provider and DeepL switched on put
    // that line above a badge reading TEXT LEAVES DEVICE — the two elements
    // this function exists to keep in step, out of step.
    match (
        leaving.can_transcribe,
        leaving.audio_to.as_deref(),
        leaving.text_to.as_deref(),
    ) {
        (false, _, None) => text(locale, UiText::NothingCanTranscribe).to_owned(),
        (false, _, Some(service)) => render(
            locale,
            UiText::NothingCanTranscribeTextLeaves,
            &[("text", service)],
        ),
        (true, None, None) => text(locale, UiText::EverythingStays).to_owned(),
        (true, Some(service), None) => {
            render(locale, UiText::AudioLeavesFor, &[("audio", service)])
        }
        (true, None, Some(service)) => render(locale, UiText::TextLeavesFor, &[("text", service)]),
        (true, Some(audio), Some(text_service)) if audio == text_service => {
            render(locale, UiText::EverythingLeavesFor, &[("service", audio)])
        }
        (true, Some(audio), Some(text_service)) => render(
            locale,
            UiText::DifferentServices,
            &[("audio", audio), ("text", text_service)],
        ),
    }
}

/// The short line along the bottom of the window.
///
/// Three words at most, and never a claim the list does not support.
#[cfg(test)]
pub(crate) fn departure_badge(leaving: &Departures) -> &'static str {
    departure_badge_localized(leaving, UiLocale::English)
}

pub(crate) fn departure_badge_localized(leaving: &Departures, locale: UiLocale) -> &'static str {
    match (leaving.audio_leaves(), leaving.text_leaves()) {
        (false, false) => text(locale, UiText::EverythingStaysBadge),
        (true, false) => text(locale, UiText::AudioLeavesBadge),
        (false, true) => text(locale, UiText::TextLeavesBadge),
        (true, true) => text(locale, UiText::EverythingLeavesBadge),
    }
}

/// Works out both halves of the claim from the two lists themselves.
///
/// From the lists rather than from a setting, because the lists are what the
/// two routers actually walk.
pub(crate) fn departures(
    speech: &[TranscriptionProviderEntry],
    speech_keyed: impl Fn(&str) -> bool,
    text: &[TranslationProviderEntry],
    text_keyed: impl Fn(&str) -> bool,
) -> Departures {
    let (_, speech_usable) = chain_positions(speech, &speech_keyed);
    let audio_to = speech_usable
        .and_then(|index| speech.get(index))
        .filter(|entry| entry.id != "local")
        .map(|entry| provider_display_name(&entry.id));
    let text_to = text
        .iter()
        .filter(|entry| entry.enabled)
        .find(|entry| entry.id == "local" || text_keyed(&entry.id))
        .filter(|entry| entry.id != "local")
        .map(|entry| translation_provider_display_name(&entry.id));
    Departures {
        audio_to,
        text_to,
        can_transcribe: speech_usable.is_some(),
    }
}

/// The short word for where a translation came from, for the status line.
pub(crate) fn translation_origin_localized(provider: Option<&str>, locale: UiLocale) -> String {
    provider.map_or_else(
        || text(locale, UiText::Local).to_owned(),
        |provider| translation_provider_display_name_localized(provider, locale),
    )
}

/// The sentence the indicator shows when a translation finishes.
///
/// Read from the result rather than from a separate announcement: the
/// announcement arrived first and this sentence overwrote it, so every
/// translation reported itself as local however far it had actually travelled.
pub(crate) fn translation_completed_sentence_localized(
    provider: Option<&str>,
    locale: UiLocale,
) -> String {
    provider.map_or_else(
        || text(locale, UiText::TranslationCompletedLocal).to_owned(),
        |provider| {
            render(
                locale,
                UiText::TranslationCompletedProvider,
                &[(
                    "provider",
                    &translation_provider_display_name_localized(provider, locale),
                )],
            )
        },
    )
}

/// One dictation in the history list.
pub(crate) fn history_row_localized(entry: &HistoryEntry, locale: UiLocale) -> HistoryRow {
    let provider = entry.transcript.provider.as_deref();
    let source = provider.map_or_else(String::new, |provider| {
        format!(
            " · {}",
            render(
                locale,
                UiText::ViaProvider,
                &[(
                    "provider",
                    &provider_display_name_localized(provider, locale)
                )]
            )
        )
    });
    let detail = if entry.transcript.latency.after_release_ms > 0 {
        let language = entry.transcript.language.to_string();
        let seconds = format!("{:.1}", entry.transcript.audio_duration_ms as f64 / 1000.0);
        let milliseconds = entry.transcript.latency.after_release_ms.to_string();
        let prepared = if entry.transcript.latency.speculative_reuse {
            format!(" · {}", text(locale, UiText::Prepared))
        } else {
            String::new()
        };
        render(
            locale,
            UiText::AudioDuration,
            &[
                ("language", &language),
                ("seconds", &seconds),
                ("milliseconds", &milliseconds),
                ("prepared", &prepared),
                ("source", &source),
            ],
        )
    } else {
        let language = entry.transcript.language.to_string();
        let seconds = format!("{:.1}", entry.transcript.audio_duration_ms as f64 / 1000.0);
        let milliseconds = entry.transcript.processing_duration_ms.to_string();
        render(
            locale,
            UiText::ProcessingDuration,
            &[
                ("language", &language),
                ("seconds", &seconds),
                ("milliseconds", &milliseconds),
                ("source", &source),
            ],
        )
    };
    HistoryRow {
        text: entry.transcript.text.clone().into(),
        detail: detail.into(),
        // Which entries left the machine is not something a user should have to
        // remember, so the row carries it rather than the detail line burying
        // it at the end.
        origin: if provider.is_some_and(|provider| provider != "local") {
            "remote".into()
        } else {
            "local".into()
        },
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{
        ModelSummary, TranscriptionProviderEntry, TranscriptionProviderStatus,
        TranslationModelSummary, TranslationProviderEntry, TranslationTier,
    };

    use super::{
        Download, chain_positions, departure_badge, departures, model_progress, privacy_claim,
        provider_display_name_localized, provider_row, provider_row_localized,
        speech_model_row_localized, tone, translation_model_row_localized,
        translation_provider_row_localized,
    };

    #[test]
    fn a_generic_provider_name_is_localized_but_service_brands_are_not() {
        assert_eq!(
            provider_display_name_localized("custom", crate::localization::UiLocale::Russian),
            "Свой сервер"
        );
        assert_eq!(
            provider_display_name_localized("groq", crate::localization::UiLocale::Russian),
            "Groq"
        );
    }

    #[test]
    fn provider_section_identity_does_not_change_with_the_interface_language() {
        let speech = provider_row_localized(
            &entry("local", true),
            None,
            0,
            Some(0),
            Some(0),
            crate::localization::UiLocale::Russian,
        );
        assert_eq!(speech.section.as_str(), "SPEECH");

        let text = translation_provider_row_localized(
            &text_entry("local", true),
            None,
            0,
            Some(0),
            Some(0),
            crate::localization::UiLocale::Russian,
        );
        assert_eq!(text.section.as_str(), "TEXT");
    }

    #[test]
    fn catalog_model_descriptions_follow_the_interface_language() {
        let speech = ModelSummary {
            catalog_id: "whisper-small".to_owned(),
            display_name: "Whisper Small".to_owned(),
            description: "Lower download size and resource use for less powerful computers."
                .to_owned(),
            size_bytes: 1,
            installed: false,
            selected: false,
            recommended: false,
            supports_speech_translation: true,
        };
        let translation = TranslationModelSummary {
            catalog_id: "qwen35-4b-q4".to_owned(),
            display_name: "Qwen3.5 4B · Fast".to_owned(),
            description: "Modern multilingual 4B model accelerated locally through Vulkan."
                .to_owned(),
            tier: TranslationTier::Tiny,
            languages: Vec::new(),
            size_bytes: 1,
            installed: false,
            selected: false,
            recommended: false,
            license: "Apache-2.0".to_owned(),
        };

        let speech_row =
            speech_model_row_localized(&speech, None, crate::localization::UiLocale::Russian);
        let translation_row = translation_model_row_localized(
            &translation,
            false,
            None,
            crate::localization::UiLocale::Russian,
        );

        assert_eq!(
            speech_row.detail,
            "Компактная модель с меньшей нагрузкой для слабых компьютеров."
        );
        assert_eq!(
            translation_row.detail,
            "Современная многоязычная модель 4B с локальным ускорением Vulkan. · Apache-2.0"
        );
    }

    fn entry(id: &str, enabled: bool) -> TranscriptionProviderEntry {
        TranscriptionProviderEntry {
            id: id.to_owned(),
            enabled,
            endpoint: None,
            model: None,
            account: None,
        }
    }

    /// A service with a key stored and nothing wrong with it. Without one the
    /// row stops at "No key stored", which is a truer reason and outranks
    /// everything below it.
    fn keyed(id: &str, position: usize) -> TranscriptionProviderStatus {
        TranscriptionProviderStatus {
            id: id.to_owned(),
            position,
            enabled: true,
            available: true,
            detail: String::new(),
            has_credential: true,
            credential_hint: Some("\u{2022}\u{2022}\u{2022}\u{2022}9c07".to_owned()),
        }
    }

    #[test]
    fn a_row_below_an_enabled_local_engine_says_it_is_never_reached() {
        let chain = [entry("local", true), entry("groq", true)];
        let (local, usable) = chain_positions(&chain, |id| id == "groq");
        let row = provider_row(&chain[1], Some(&keyed("groq", 1)), 1, local, usable);

        assert!(row.unreachable);
        assert_eq!(row.state, "Never reached");
        assert_eq!(row.tone, tone::OFF);
    }

    #[test]
    fn the_same_row_above_the_local_engine_is_the_one_in_use() {
        let chain = [entry("groq", true), entry("local", true)];
        let (local, usable) = chain_positions(&chain, |id| id == "groq");
        let row = provider_row(&chain[0], Some(&keyed("groq", 0)), 0, local, usable);

        assert!(!row.unreachable);
        assert_eq!(row.state, "In use");
        assert_eq!(row.tone, tone::ACTIVE);
    }

    #[test]
    fn a_switched_off_local_engine_does_not_end_the_walk() {
        let chain = [entry("local", false), entry("groq", true)];
        let (local, usable) = chain_positions(&chain, |id| id == "groq");

        assert_eq!(local, None);
        assert_eq!(usable, Some(1));
        assert!(!provider_row(&chain[1], Some(&keyed("groq", 1)), 1, local, usable).unreachable);
    }

    #[test]
    fn a_row_does_not_say_the_same_thing_twice() {
        // The worker words a disabled row as "Switched off", which is exactly
        // what the state column says. The detail line is then wasted on a
        // repetition instead of telling you what the service is.
        let chain = [entry("groq", false)];
        let mut status = keyed("groq", 0);
        status.enabled = false;
        status.available = false;
        status.detail = "Switched off".to_owned();
        let (local, usable) = chain_positions(&chain, |_| true);
        let row = provider_row(&chain[0], Some(&status), 0, local, usable);

        assert_eq!(row.state, "Switched off");
        assert_ne!(row.detail, "Switched off");
        assert!(
            row.detail
                .contains("availability follows your Groq account")
        );
    }

    #[test]
    fn a_worker_reason_is_classified_without_leaking_free_form_english_into_the_detail() {
        let chain = [entry("groq", true)];
        let mut status = keyed("groq", 0);
        status.detail = "Key refused: the service rejected this token".to_owned();
        let (local, usable) = chain_positions(&chain, |_| true);
        let row = provider_row(&chain[0], Some(&status), 0, local, usable);

        assert_eq!(row.state, "Key refused");
        assert!(
            row.detail
                .contains("availability follows your Groq account")
        );
        assert!(!row.detail.contains("service rejected this token"));
    }

    #[test]
    fn a_row_with_no_key_says_so_rather_than_that_it_is_unreachable() {
        // Both are true of this row. "No key stored" is the one the user can
        // act on, and it is the reason that would still hold if the row moved.
        let chain = [entry("local", true), entry("groq", true)];
        let (local, usable) = chain_positions(&chain, |_| false);
        let row = provider_row(&chain[1], None, 1, local, usable);

        assert!(row.unreachable);
        assert_eq!(row.state, "No key stored");
    }

    fn text_entry(id: &str, enabled: bool) -> TranslationProviderEntry {
        TranslationProviderEntry {
            id: id.to_owned(),
            enabled,
            region: None,
        }
    }

    #[test]
    fn the_claim_names_the_service_the_recording_actually_goes_to() {
        let chain = [entry("groq", true), entry("local", true)];
        let local_text = [text_entry("local", true)];
        assert_eq!(
            privacy_claim(&departures(
                &chain,
                |id| id == "groq",
                &local_text,
                |_| false
            )),
            "Audio leaves this device for Groq"
        );
    }

    #[test]
    fn the_claim_says_nothing_can_transcribe_when_nothing_can() {
        let chain = [entry("groq", true), entry("local", false)];
        assert_eq!(
            privacy_claim(&departures(&chain, |_| false, &[], |_| false)),
            "Nothing can transcribe right now"
        );
    }

    #[test]
    fn text_leaving_is_said_out_loud_even_when_audio_stays() {
        // The failure this replaces: every selection went to DeepL, the corner
        // of the window said "Audio stays on this device", and the line along
        // the bottom said AUDIO LEAVES DEVICE. Neither was about text, and one
        // of them was about the wrong thing entirely.
        let local_speech = [entry("local", true)];
        let deepl = [text_entry("deepl", true), text_entry("local", true)];
        let leaving = departures(&local_speech, |_| false, &deepl, |_| true);

        assert_eq!(privacy_claim(&leaving), "Text leaves this device for DeepL");
        assert_eq!(departure_badge(&leaving), "TEXT LEAVES DEVICE");
        assert!(!leaving.audio_leaves());
        assert!(leaving.text_leaves());
    }

    #[test]
    fn a_speech_list_that_cannot_run_does_not_hide_text_that_can() {
        // The failure this replaces: the claim answered "Nothing can transcribe
        // right now" and stopped, while the badge underneath it — reading the
        // same two lists — said TEXT LEAVES DEVICE.
        let leaving = departures(
            &[entry("groq", true)],
            |_| false,
            &[text_entry("deepl", true), text_entry("local", true)],
            |_| true,
        );

        assert!(!leaving.can_transcribe);
        assert_eq!(
            privacy_claim(&leaving),
            "Nothing can transcribe · text leaves this device for DeepL"
        );
        assert_eq!(departure_badge(&leaving), "TEXT LEAVES DEVICE");
    }

    #[test]
    fn nothing_leaving_says_so_rather_than_saying_nothing() {
        let leaving = departures(
            &[entry("local", true)],
            |_| false,
            &[text_entry("local", true)],
            |_| true,
        );

        assert_eq!(
            privacy_claim(&leaving),
            "Audio and text stay on this device"
        );
        assert_eq!(departure_badge(&leaving), "EVERYTHING STAYS HERE");
    }

    #[test]
    fn two_services_are_both_named() {
        let groq = [entry("groq", true), entry("local", true)];
        let deepl = [text_entry("deepl", true), text_entry("local", true)];
        let leaving = departures(&groq, |_| true, &deepl, |_| true);

        assert_eq!(
            privacy_claim(&leaving),
            "Audio leaves for Groq, text for DeepL"
        );
        assert_eq!(departure_badge(&leaving), "AUDIO AND TEXT LEAVE");
    }

    #[test]
    fn a_model_offers_one_verb_at_rest_and_cancel_while_it_runs() {
        let (state, tone, action, progress) = model_progress(false, false, None);
        assert_eq!(
            (state.as_str(), tone, action.as_str()),
            ("Not installed", "off", "Install")
        );
        assert!((progress - 0.0).abs() < f32::EPSILON);

        let (state, tone, action, _) = model_progress(true, false, None);
        assert_eq!(
            (state.as_str(), tone, action.as_str()),
            ("Installed", "resting", "Use")
        );

        let (state, tone, action, _) = model_progress(true, true, None);
        assert_eq!(
            (state.as_str(), tone, action.as_str()),
            ("In use", "active", "")
        );

        let running = Download {
            downloaded_bytes: 43,
            total_bytes: 100,
        };
        let (state, tone, action, progress) = model_progress(false, false, Some(running));
        assert_eq!(
            (state.as_str(), tone, action.as_str()),
            ("43 %", "working", "Cancel")
        );
        assert!((progress - 0.43).abs() < 0.001);
    }

    #[test]
    fn a_model_on_disk_is_never_shown_as_downloading() {
        // The failure this replaces: a translation model that was already
        // installed got reported to 100% and then handed over as ready rather
        // than installed, so its row offered Cancel for the rest of the
        // session and Overview announced a download that was over before the
        // window opened.
        let finished = Download {
            downloaded_bytes: 100,
            total_bytes: 100,
        };
        let (state, tone, action, progress) = model_progress(true, false, Some(finished));

        assert_eq!(
            (state.as_str(), tone, action.as_str()),
            ("Installed", "resting", "Use")
        );
        assert!((progress - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_download_with_no_known_total_does_not_divide_by_zero() {
        let unknown = Download {
            downloaded_bytes: 900,
            total_bytes: 0,
        };
        let (state, _, _, progress) = model_progress(false, false, Some(unknown));
        assert_eq!(state, "0 %");
        assert!((progress - 0.0).abs() < f32::EPSILON);
    }
}
