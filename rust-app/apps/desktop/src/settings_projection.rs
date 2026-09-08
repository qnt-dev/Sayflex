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

use crate::{HistoryRow, ModelRow, ProviderRow, providers_view::provider_display_name};

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
pub(crate) fn speech_model_row(model: &ModelSummary, download: Option<Download>) -> ModelRow {
    let (state, tone, action, progress) = model_progress(model.installed, model.selected, download);
    ModelRow {
        catalog_id: model.catalog_id.clone().into(),
        section: "Speech".into(),
        title: model.display_name.clone().into(),
        detail: format!(
            "{}{}",
            model.description,
            if model.recommended {
                " · recommended"
            } else {
                ""
            }
        )
        .into(),
        size: human_size(model.size_bytes).into(),
        state: state.into(),
        tone: tone.into(),
        action: action.into(),
        secondary: "".into(),
        selected: model.selected,
        installed: model.installed,
        progress,
    }
}

/// One row of the translation-model list.
pub(crate) fn translation_model_row(
    model: &TranslationModelSummary,
    selected: bool,
    download: Option<Download>,
) -> ModelRow {
    let (state, tone, action, progress) = model_progress(model.installed, selected, download);
    ModelRow {
        catalog_id: model.catalog_id.clone().into(),
        section: "Translation".into(),
        title: model.display_name.clone().into(),
        detail: format!(
            "{} · {}{}",
            model.description,
            model.license,
            if model.recommended {
                " · recommended for your system language"
            } else {
                ""
            }
        )
        .into(),
        size: human_size(model.size_bytes).into(),
        state: state.into(),
        tone: tone.into(),
        action: action.into(),
        secondary: "".into(),
        selected,
        installed: model.installed,
        progress,
    }
}

fn model_progress(
    installed: bool,
    selected: bool,
    download: Option<Download>,
) -> (String, &'static str, &'static str, f32) {
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
            "Cancel",
            download.fraction(),
        );
    }
    if selected {
        return ("In use".to_owned(), tone::ACTIVE, "", 0.0);
    }
    if installed {
        return ("Installed".to_owned(), tone::RESTING, "Use", 0.0);
    }
    ("Not installed".to_owned(), tone::OFF, "Install", 0.0)
}

/// One row of the transcription order.
///
/// `status` is what the worker last reported about this entry, including its
/// own words for why it cannot run. `unreachable` is this window's own
/// conclusion: the router hands the recording to the local engine as soon as it
/// reaches an enabled one and stops there, so anything below that row is never
/// asked. Leaving such a row looking ready would be the interface promising
/// something the router does not do.
pub(crate) fn provider_row(
    entry: &TranscriptionProviderEntry,
    status: Option<&TranscriptionProviderStatus>,
    position: usize,
    first_local: Option<usize>,
    first_usable: Option<usize>,
) -> ProviderRow {
    let unreachable = first_local.is_some_and(|local| position > local);
    let has_credential = status.is_some_and(|status| status.has_credential);
    let detail = status
        .map(|status| status.detail.clone())
        .unwrap_or_default();
    let unavailable_reason = !detail.is_empty();

    let (state, tone) = row_state(
        entry.enabled,
        &entry.id,
        has_credential,
        &detail,
        unreachable,
        first_usable == Some(position),
    );
    let _ = unavailable_reason;

    ProviderRow {
        id: qualified_row_id("speech", &entry.id).into(),
        section: "SPEECH".into(),
        title: provider_display_name(&entry.id).into(),
        // The worker's sentence when it says more than the state column
        // already does. "Switched off" in both places is a row saying one
        // thing twice and losing the line that told you what the service is.
        detail: if detail.is_empty() || detail == state {
            provider_detail(entry, status)
        } else {
            detail
        }
        .into(),
        enabled: entry.enabled,
        available: status.is_some_and(|status| status.available),
        needs_key: entry.id != "local",
        key_hint: status
            .and_then(|status| status.credential_hint.clone())
            .unwrap_or_default()
            .into(),
        field_label: crate::providers_view::provider_field_label(&entry.id).into(),
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
/// worker's own sentence goes on the detail line, where there is room for it
/// and where the eye is already reading.
///
/// Shared by both lists, because a row is a row: whether it is a service that
/// transcribes or one that translates changes nothing about what "cooling down"
/// looks like.
fn row_state(
    enabled: bool,
    id: &str,
    has_credential: bool,
    detail: &str,
    unreachable: bool,
    in_use: bool,
) -> (&'static str, &'static str) {
    if !enabled {
        ("Switched off", tone::OFF)
    } else if id != "local" && !has_credential {
        ("No key stored", tone::OFF)
    } else if !detail.is_empty() {
        if detail.starts_with("Key refused") {
            ("Key refused", tone::REFUSED)
        } else if detail.contains("available again") || detail.starts_with("Waiting") {
            ("Cooling down", tone::WORKING)
        } else {
            ("Unavailable", tone::WORKING)
        }
    } else if unreachable {
        ("Never reached", tone::OFF)
    } else if in_use {
        ("In use", tone::ACTIVE)
    } else {
        ("Standing by", tone::RESTING)
    }
}

/// Both lists have a row called `azure` and a row called `local`, and the two
/// mean different services with different keys. A callback that carried the
/// bare id would act on whichever of them it found first, which is how a key
/// for one service ends up stored against the other.
pub(crate) fn qualified_row_id(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

/// One row of the translation order.
pub(crate) fn translation_provider_row(
    entry: &TranslationProviderEntry,
    status: Option<&TranslationProviderStatus>,
    position: usize,
    first_local: Option<usize>,
    first_usable: Option<usize>,
) -> ProviderRow {
    let unreachable = first_local.is_some_and(|local| position > local);
    let has_credential = status.is_some_and(|status| status.has_credential);
    let detail = status
        .map(|status| status.detail.clone())
        .unwrap_or_default();
    let (state, tone) = row_state(
        entry.enabled,
        &entry.id,
        has_credential,
        &detail,
        unreachable,
        first_usable == Some(position),
    );

    ProviderRow {
        id: qualified_row_id("text", &entry.id).into(),
        section: "TEXT".into(),
        title: translation_provider_display_name(&entry.id).into(),
        detail: if detail.is_empty() || detail == state {
            translation_provider_detail(entry, status)
        } else {
            detail
        }
        .into(),
        enabled: entry.enabled,
        available: status.is_some_and(|status| status.available),
        needs_key: entry.id != "local",
        key_hint: status
            .and_then(|status| status.credential_hint.clone())
            .unwrap_or_default()
            .into(),
        field_label: if entry.id == "azure" { "Region" } else { "" }.into(),
        field_value: entry.region.clone().unwrap_or_default().into(),
        state: state.into(),
        tone: tone.into(),
        unreachable,
    }
}

pub(crate) fn translation_provider_display_name(id: &str) -> String {
    match id {
        "local" => "This computer".to_owned(),
        "deepl" => "DeepL".to_owned(),
        "azure" => "Microsoft Azure Translator".to_owned(),
        "google" => "Google Cloud Translation".to_owned(),
        other => other.to_owned(),
    }
}

/// What the service is, plus the masked tail of the key if one is stored.
///
/// The free allowance each one publishes, because that is the thing a person
/// choosing between them actually wants to know. No counter of what is left:
/// nothing here is told, and a number this window invents is a number it then
/// has to defend.
fn translation_provider_detail(
    entry: &TranslationProviderEntry,
    status: Option<&TranslationProviderStatus>,
) -> String {
    let about = match entry.id.as_str() {
        "local" => "Runs on this computer · nothing leaves it",
        "deepl" => "Free tier: 500 000 characters a month",
        "azure" => "Free tier F0: 2 million characters a month · needs a region",
        "google" => "Free tier: 500 000 characters a month",
        _ => "This build does not know this service",
    };
    match status.and_then(|status| status.credential_hint.as_deref()) {
        Some(hint) if !hint.is_empty() => {
            let extra = entry
                .region
                .as_deref()
                .filter(|region| !region.is_empty())
                .map_or_else(String::new, |region| format!(" · {region}"));
            format!("Key {hint}{extra} · {about}")
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
) -> String {
    let about = provider_about(&entry.id, entry);
    match status.and_then(|status| status.credential_hint.as_deref()) {
        Some(hint) if !hint.is_empty() => {
            let extra = entry
                .account
                .as_deref()
                .filter(|account| !account.is_empty())
                .map_or_else(String::new, |account| format!(" · {account}"));
            format!("Key {hint}{extra} · {about}")
        }
        _ => about,
    }
}

fn provider_about(id: &str, entry: &TranscriptionProviderEntry) -> String {
    match id {
        "local" => "Runs on this computer · nothing leaves it".to_owned(),
        "groq" => "Free tier: 2 000 requests a day".to_owned(),
        "cloudflare" => "Free tier: 10 000 neurons a day, shared across models".to_owned(),
        "azure" => "Free tier F0: 5 hours a month · needs a region".to_owned(),
        "deepgram" => "$200 of credit, no expiry".to_owned(),
        "custom" => entry
            .endpoint
            .clone()
            .unwrap_or_else(|| "Any OpenAI-compatible address · HTTPS only".to_owned()),
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
pub(crate) fn privacy_claim(leaving: &Departures) -> String {
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
        (false, _, None) => "Nothing can transcribe right now".to_owned(),
        (false, _, Some(text)) => {
            format!("Nothing can transcribe · text leaves this device for {text}")
        }
        (true, None, None) => "Audio and text stay on this device".to_owned(),
        (true, Some(audio), None) => format!("Audio leaves this device for {audio}"),
        (true, None, Some(text)) => format!("Text leaves this device for {text}"),
        (true, Some(audio), Some(text)) if audio == text => {
            format!("Audio and text leave this device for {audio}")
        }
        (true, Some(audio), Some(text)) => format!("Audio leaves for {audio}, text for {text}"),
    }
}

/// The short line along the bottom of the window.
///
/// Three words at most, and never a claim the list does not support.
pub(crate) fn departure_badge(leaving: &Departures) -> &'static str {
    match (leaving.audio_leaves(), leaving.text_leaves()) {
        (false, false) => "EVERYTHING STAYS HERE",
        (true, false) => "AUDIO LEAVES DEVICE",
        (false, true) => "TEXT LEAVES DEVICE",
        (true, true) => "AUDIO AND TEXT LEAVE",
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
pub(crate) fn translation_origin(provider: Option<&str>) -> String {
    provider.map_or_else(|| "local".to_owned(), translation_provider_display_name)
}

/// The sentence the indicator shows when a translation finishes.
///
/// Read from the result rather than from a separate announcement: the
/// announcement arrived first and this sentence overwrote it, so every
/// translation reported itself as local however far it had actually travelled.
pub(crate) fn translation_completed_sentence(provider: Option<&str>) -> String {
    provider.map_or_else(
        || "Translation completed locally.".to_owned(),
        |provider| {
            format!(
                "Translated by {}.",
                translation_provider_display_name(provider)
            )
        },
    )
}

/// One dictation in the history list.
pub(crate) fn history_row(entry: &HistoryEntry) -> HistoryRow {
    let provider = entry.transcript.provider.as_deref();
    let source = provider.map_or_else(String::new, |provider| {
        format!(" · via {}", provider_display_name(provider))
    });
    let detail = if entry.transcript.latency.after_release_ms > 0 {
        format!(
            "{} · {:.1}s audio · {}ms release{}{source}",
            entry.transcript.language,
            entry.transcript.audio_duration_ms as f64 / 1000.0,
            entry.transcript.latency.after_release_ms,
            if entry.transcript.latency.speculative_reuse {
                " · prepared"
            } else {
                ""
            }
        )
    } else {
        format!(
            "{} · {:.1}s audio · {}ms processing{source}",
            entry.transcript.language,
            entry.transcript.audio_duration_ms as f64 / 1000.0,
            entry.transcript.processing_duration_ms
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
        TranscriptionProviderEntry, TranscriptionProviderStatus, TranslationProviderEntry,
    };

    use super::{
        Download, chain_positions, departure_badge, departures, model_progress, privacy_claim,
        provider_row, tone,
    };

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
        assert!(row.detail.contains("2 000 requests a day"));
    }

    #[test]
    fn a_reason_the_state_column_abbreviates_is_kept() {
        let chain = [entry("groq", true)];
        let mut status = keyed("groq", 0);
        status.detail = "Key refused: the service rejected this token".to_owned();
        let (local, usable) = chain_positions(&chain, |_| true);
        let row = provider_row(&chain[0], Some(&status), 0, local, usable);

        assert_eq!(row.state, "Key refused");
        assert_eq!(row.detail, "Key refused: the service rejected this token");
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
            (state.as_str(), tone, action),
            ("Not installed", "off", "Install")
        );
        assert!((progress - 0.0).abs() < f32::EPSILON);

        let (state, tone, action, _) = model_progress(true, false, None);
        assert_eq!(
            (state.as_str(), tone, action),
            ("Installed", "resting", "Use")
        );

        let (state, tone, action, _) = model_progress(true, true, None);
        assert_eq!((state.as_str(), tone, action), ("In use", "active", ""));

        let running = Download {
            downloaded_bytes: 43,
            total_bytes: 100,
        };
        let (state, tone, action, progress) = model_progress(false, false, Some(running));
        assert_eq!(
            (state.as_str(), tone, action),
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
            (state.as_str(), tone, action),
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
