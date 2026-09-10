//! The transcription list in Settings.
//!
//! Turning the arranged order into rows the interface can draw, and turning
//! what the user does to those rows back into a list the worker is told about.
//! The key itself never passes through here in either direction: it goes
//! straight to the worker, and comes back only as a masked tail.

use std::{cell::RefCell, rc::Rc, time::Duration};

use rust_app_core::provider_drafts::{ProviderDraftKey, ProviderKind as DraftProviderKind};
use rust_protocol::{
    RequestId, SpeechWorkerCommand, TranscriptionProviderEntry, TranslationProviderEntry,
    TranslationWorkerCommand,
};
use slint::{Model, ModelRc, VecModel};

use crate::{
    AppState, PendingCredential, ProviderRow, SettingsWindow,
    localization::{UiText, active_locale, render, text},
    set_indicator_status,
    settings_document::save_settings,
    settings_projection::{
        chain_positions, departure_badge_localized, departures, privacy_claim_localized,
        provider_row_localized, translation_provider_row_localized,
    },
    settings_view::set_status_problem,
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

/// Why a provider's configuration was refused.
///
/// Every rule lives here rather than in the markup: a rule enforced only where
/// the form is drawn stops applying the moment anything else calls the same
/// callback, and it cannot be tested without a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveRefused {
    KeyRequired,
    FieldRequired(String),
    InsecureEndpoint,
}

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
        return Err(SaveRefused::KeyRequired);
    }

    let label = provider_field_label(provider_id);
    if !label.is_empty() && extra.is_empty() {
        return Err(SaveRefused::FieldRequired(label.to_owned()));
    }

    // A custom endpoint carries the key and the audio in the same request.
    if provider_id == "custom" && !extra.is_empty() && !extra.starts_with("https://") {
        return Err(SaveRefused::InsecureEndpoint);
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
/// endpoint, so there is no address to insist is HTTPS.
///
/// Azure's region is offered and not demanded. A global Translator resource
/// does not have one and works without it; a regional or multi-service resource
/// does and fails without it. Which of the two a key belongs to is not
/// something this window can see, so refusing an empty region locked out every
/// person holding a global key.
pub(crate) fn check_translation_provider_configuration(
    _provider_id: &str,
    secret: &str,
    extra: &str,
) -> Result<(String, Option<String>), SaveRefused> {
    let secret = secret.trim();
    let extra = extra.trim();

    if secret.is_empty() {
        return Err(SaveRefused::KeyRequired);
    }

    Ok((
        secret.to_owned(),
        (!extra.is_empty()).then(|| extra.to_owned()),
    ))
}

/// How long a key save waits for the worker to say what happened to it.
///
/// The write is DPAPI, in another process, and normally answers in
/// milliseconds. A worker that dies mid-write answers nothing at all, and a
/// form left waiting on that is a form holding a key that will never be
/// stored, with no way for the user to know.
const CREDENTIAL_ACK_DEADLINE: Duration = Duration::from_secs(10);

/// Ends key saves that will never be answered.
///
/// Called when a worker is seen to have died or restarted, and on a timer, so
/// that a form waiting on a dead worker is released with a reason rather than
/// left holding what the user typed.
pub(crate) fn expire_pending_credentials(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
    reason: &str,
) {
    let expired: Vec<PendingCredential> = {
        let mut app = state.borrow_mut();
        let overdue: Vec<RequestId> = app
            .pending_credentials
            .iter()
            .filter(|(_, pending)| pending.sent.elapsed() >= CREDENTIAL_ACK_DEADLINE)
            .map(|(id, _)| *id)
            .collect();
        overdue
            .into_iter()
            .filter_map(|id| app.pending_credentials.remove(&id))
            .collect()
    };
    if expired.is_empty() {
        // This function is called by a two-second watchdog. Polling both
        // workers on every idle tick made each reply replace the Slint row
        // model, which reset the list's scroll position and destroyed an
        // unsaved password field. Reconciliation is needed only after a real
        // timeout left the stored state unknown.
        return;
    }
    for pending in expired {
        if let Some((kind, provider_id)) = split_row_id(&pending.row_id) {
            state
                .borrow_mut()
                .provider_drafts
                .cancel_save(&draft_key(kind, provider_id), pending.operation_id);
        }
        rust_app_core::diagnostics::log_line(&format!(
            "credential_store operation=unknown provider={} reason={reason}",
            pending.display_name
        ));
        if let Some(settings) = settings.upgrade() {
            // Deliberately not "the key was not stored". Silence does not say
            // that: the worker may have written it and then died, or simply be
            // late. The row reports what is actually in the store as soon as the
            // worker is heard from, and that is the thing to point at.
            settings.set_provider_problem(
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::CredentialOutcomeUnknown,
                )
                .into(),
            );
            settings.set_provider_problem_for(pending.row_id.clone().into());
            // The form stays open holding what was typed, so it can be sent
            // again without retyping the key.
            settings.set_provider_editing(pending.row_id.into());
        }
        set_status_problem(
            settings,
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::ProviderDidNotConfirm,
                &[("provider", &pending.display_name)],
            ),
        );
    }
    // Ask both stores what they actually hold, so the row settles the question
    // the silence left open.
    let _ = workers
        .speech
        .send(&SpeechWorkerCommand::GetTranscriptionProviders);
    let _ = workers
        .translation
        .send(&TranslationWorkerCommand::GetTranslationProviders);
}

/// Ends the key saves one worker owed an answer to.
///
/// A restarted worker has forgotten the operation, so its answer is never
/// coming.
pub(crate) fn abandon_pending_credentials(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    kind: Kind,
    reason: &str,
) {
    // Only the operations this worker owed. Draining the whole map meant a
    // speech worker falling over cancelled a translation key the user was in
    // the middle of storing, and the other way round.
    let abandoned: Vec<PendingCredential> = {
        let mut app = state.borrow_mut();
        let theirs: Vec<RequestId> = app
            .pending_credentials
            .iter()
            .filter(|(_, pending)| {
                split_row_id(&pending.row_id).is_some_and(|(owner, _)| owner == kind)
            })
            .map(|(id, _)| *id)
            .collect();
        theirs
            .into_iter()
            .filter_map(|id| app.pending_credentials.remove(&id))
            .collect()
    };
    for pending in abandoned {
        if let Some((kind, provider_id)) = split_row_id(&pending.row_id) {
            state
                .borrow_mut()
                .provider_drafts
                .cancel_save(&draft_key(kind, provider_id), pending.operation_id);
        }
        rust_app_core::diagnostics::log_line(&format!(
            "credential_store operation=abandoned provider={} reason={reason}",
            pending.display_name
        ));
        if let Some(settings) = settings.upgrade() {
            // Not "the key was not stored": the worker may have written it and
            // then died. The row reports what is really in the store once it is
            // back, and that is what to point at.
            settings.set_provider_problem(
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::CredentialWorkerStopped,
                )
                .into(),
            );
            settings.set_provider_problem_for(pending.row_id.clone().into());
            settings.set_provider_editing(pending.row_id.into());
        }
    }
}

/// Acts on a worker's answer to one key save.
///
/// This is where the form finally closes, the second field is written, and the
/// user is told. All three used to happen the moment the command reached the
/// pipe — so a store that then refused the key left the row configured for a
/// service it had no key for, the form shut over the value the user had typed,
/// and a generic failure on the status line that named no operation.
pub(crate) fn settle_credential(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
    operation_id: RequestId,
    provider_id: &str,
    error: &str,
) {
    let Some(pending) = state.borrow_mut().pending_credentials.remove(&operation_id) else {
        // Not ours, or already settled. Saying nothing is right: another window
        // or an earlier attempt owns it.
        return;
    };
    let Some((kind, _)) = split_row_id(&pending.row_id) else {
        return;
    };
    let key = draft_key(kind, provider_id);

    if !error.is_empty() {
        state
            .borrow_mut()
            .provider_drafts
            .acknowledge(&key, operation_id, false);
        // The form stays open, holding what was typed, with the store's own
        // words under it.
        if let Some(settings) = settings.upgrade() {
            let locale = active_locale(state.borrow().ui_locale);
            settings.set_provider_problem(
                render(locale, UiText::KeyNotStored, &[("error", error)]).into(),
            );
            settings.set_provider_problem_for(pending.row_id.clone().into());
            settings.set_provider_editing(pending.row_id.into());
        }
        set_status_problem(
            settings,
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::ProviderDidNotAccept,
                &[("provider", &pending.display_name)],
            ),
        );
        return;
    }

    state
        .borrow_mut()
        .provider_drafts
        .acknowledge(&key, operation_id, true);

    // Stored. Only now is the second field written, and only now does the form
    // close.
    //
    // A removal carries no second field and must not clear the one on the row:
    // an Azure region or a custom endpoint is a setting the user typed and did
    // not ask to lose. `Forget key` forgets the key.
    if pending.forget {
        if let Some(settings) = settings.upgrade() {
            settings.set_provider_draft_secret("".into());
            settings.set_provider_draft_extra("".into());
            settings.set_provider_problem("".into());
            settings.set_provider_problem_for("".into());
            settings.set_provider_editing("".into());
        }
        set_indicator_status(
            settings,
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::ProviderKeyRemoved,
                &[("provider", &pending.display_name)],
            ),
        );
        match kind {
            Kind::Speech => apply_provider_chain(settings, state, workers),
            Kind::Text => apply_translation_chain(settings, state, workers),
        }
        return;
    }
    {
        let mut app = state.borrow_mut();
        match kind {
            Kind::Speech => {
                if let Some(entry) = app
                    .transcription_chain
                    .iter_mut()
                    .find(|entry| entry.id == provider_id)
                {
                    set_provider_field_value(entry, pending.extra.clone().unwrap_or_default());
                }
            }
            Kind::Text => {
                if let Some(entry) = app
                    .translation_chain
                    .iter_mut()
                    .find(|entry| entry.id == provider_id)
                {
                    entry.region = pending.extra.clone().filter(|value| !value.is_empty());
                }
            }
        }
    }
    let _ = save_settings(&state.borrow());

    if let Some(settings) = settings.upgrade() {
        settings.set_provider_draft_secret("".into());
        settings.set_provider_draft_extra("".into());
        settings.set_provider_problem("".into());
        settings.set_provider_problem_for("".into());
        settings.set_provider_editing("".into());
    }
    set_indicator_status(
        settings,
        &render(
            active_locale(state.borrow().ui_locale),
            UiText::ProviderKeyStored,
            &[("provider", &pending.display_name)],
        ),
    );
    match kind {
        Kind::Speech => apply_provider_chain(settings, state, workers),
        Kind::Text => apply_translation_chain(settings, state, workers),
    }
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

fn draft_key(kind: Kind, provider_id: &str) -> ProviderDraftKey {
    ProviderDraftKey::new(
        match kind {
            Kind::Speech => DraftProviderKind::Speech,
            Kind::Text => DraftProviderKind::Translation,
        },
        provider_id,
    )
}

pub(crate) fn edit_provider_draft(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    row_id: &str,
) {
    let Some((kind, provider_id)) = split_row_id(row_id) else {
        return;
    };
    let key = draft_key(kind, provider_id);
    let extra = {
        let app = state.borrow();
        match kind {
            Kind::Speech => app
                .transcription_chain
                .iter()
                .find(|entry| entry.id == provider_id)
                .map(provider_field_value)
                .unwrap_or_default(),
            Kind::Text => app
                .translation_chain
                .iter()
                .find(|entry| entry.id == provider_id)
                .and_then(|entry| entry.region.clone())
                .unwrap_or_default(),
        }
    };
    let (secret, extra) = {
        let mut app = state.borrow_mut();
        app.provider_drafts.ensure(key.clone(), extra);
        let draft = app
            .provider_drafts
            .get(&key)
            .expect("the draft was just ensured");
        (draft.secret.clone(), draft.extra.clone())
    };
    if let Some(settings) = settings.upgrade() {
        settings.set_provider_draft_secret(secret.into());
        settings.set_provider_draft_extra(extra.into());
        settings.set_provider_problem("".into());
        settings.set_provider_problem_for("".into());
        settings.set_provider_editing(row_id.into());
    }
}

pub(crate) fn update_provider_draft(
    state: &Rc<RefCell<AppState>>,
    row_id: &str,
    secret: &str,
    extra: &str,
) {
    let Some((kind, provider_id)) = split_row_id(row_id) else {
        return;
    };
    state.borrow_mut().provider_drafts.edit(
        draft_key(kind, provider_id),
        secret.to_owned(),
        extra.to_owned(),
    );
}

pub(crate) fn begin_provider_draft_save(
    state: &Rc<RefCell<AppState>>,
    row_id: &str,
    operation_id: RequestId,
) {
    let Some((kind, provider_id)) = split_row_id(row_id) else {
        return;
    };
    state
        .borrow_mut()
        .provider_drafts
        .begin_save(draft_key(kind, provider_id), operation_id);
}

pub(crate) fn cancel_provider_draft_save(
    state: &Rc<RefCell<AppState>>,
    row_id: &str,
    operation_id: RequestId,
) {
    let Some((kind, provider_id)) = split_row_id(row_id) else {
        return;
    };
    state
        .borrow_mut()
        .provider_drafts
        .cancel_save(&draft_key(kind, provider_id), operation_id);
}

pub(crate) fn discard_provider_draft(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    row_id: &str,
) {
    let Some((kind, provider_id)) = split_row_id(row_id) else {
        return;
    };
    state
        .borrow_mut()
        .provider_drafts
        .discard(&draft_key(kind, provider_id));
    if let Some(settings) = settings.upgrade() {
        settings.set_provider_draft_secret("".into());
        settings.set_provider_draft_extra("".into());
        settings.set_provider_editing("".into());
    }
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
pub(crate) fn row_display_name_localized(
    kind: Kind,
    id: &str,
    locale: crate::localization::UiLocale,
) -> String {
    match kind {
        Kind::Speech => crate::settings_projection::provider_display_name_localized(id, locale),
        Kind::Text => {
            crate::settings_projection::translation_provider_display_name_localized(id, locale)
        }
    }
}

thread_local! {
    /// One model for the lifetime of the settings window.
    ///
    /// Provider status replies may arrive while a person is typing a key. A
    /// fresh `ModelRc<VecModel<_>>` makes Slint rebuild every repeated row,
    /// which destroys the live password field (and resets the list position).
    /// Keeping the model identity stable lets unchanged rows keep their local
    /// control state while their status text is updated.
    static PROVIDER_ROWS: RefCell<Option<Rc<VecModel<ProviderRow>>>> = const { RefCell::new(None) };
}

fn provider_rows_model(settings: &SettingsWindow) -> Rc<VecModel<ProviderRow>> {
    PROVIDER_ROWS.with(|model| {
        let mut model = model.borrow_mut();
        if let Some(existing) = model.as_ref() {
            return Rc::clone(existing);
        }
        let created = Rc::new(VecModel::<ProviderRow>::default());
        settings.set_provider_rows(ModelRc::from(Rc::clone(&created)));
        *model = Some(Rc::clone(&created));
        created
    })
}

fn update_provider_rows(settings: &SettingsWindow, rows: Vec<ProviderRow>) {
    let model = provider_rows_model(settings);
    let same_order = model.row_count() == rows.len()
        && rows.iter().enumerate().all(|(index, row)| {
            model
                .row_data(index)
                .is_some_and(|current| current.id == row.id && current.section == row.section)
        });
    if same_order {
        // `row_changed` updates bindings in the existing repeated component;
        // it does not tear down the Field that owns the unsaved key.
        for (index, row) in rows.into_iter().enumerate() {
            model.set_row_data(index, row);
        }
    } else {
        // A real reorder or a schema change must reset the repeater. Those are
        // explicit user/configuration changes, not background status traffic.
        model.set_vec(rows);
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
    let locale = active_locale(state.ui_locale);
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
            provider_row_localized(entry, status, position, first_local, first_usable, locale)
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
                translation_provider_row_localized(
                    entry,
                    status,
                    position,
                    text_local,
                    text_usable,
                    locale,
                )
            }),
    );

    let leaving = departures(
        &state.transcription_chain,
        has_credential,
        &state.translation_chain,
        has_text_credential,
    );
    settings.set_audio_leaves_device(leaving.audio_leaves());
    settings.set_text_leaves_device(leaving.text_leaves());
    settings.set_departure_badge(departure_badge_localized(&leaving, locale).into());
    settings.set_provider_claim(privacy_claim_localized(&leaving, locale).into());
    settings.set_app_version(env!("CARGO_PKG_VERSION").into());
    update_provider_rows(&settings, rows);
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

/// Sends the arranged list to the worker and writes it to settings.
pub(crate) fn apply_provider_chain(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
) {
    let providers = state.borrow().transcription_chain.clone();
    let local_enabled = providers
        .iter()
        .any(|provider| provider.id == "local" && provider.enabled);
    if !local_enabled {
        // The worker releases the model for this same configuration change;
        // do not retain a stale "local ready" fact in the desktop.
        state.borrow_mut().speech_ready = false;
    }
    let reset_output_mode = {
        let mut state = state.borrow_mut();
        if state.output_mode == rust_protocol::SpeechOutputMode::TranslateToEnglish
            && !state.speech_translation_supported()
        {
            state.output_mode = rust_protocol::SpeechOutputMode::OriginalLanguage;
            true
        } else {
            false
        }
    };
    if let Err(error) = workers
        .speech
        .send(&SpeechWorkerCommand::SetTranscriptionProviders { providers })
    {
        set_indicator_status(
            settings,
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::TranscriptionOrderFailed,
                &[("error", &error.to_string())],
            ),
        );
        return;
    }
    if reset_output_mode {
        let _ = workers.speech.send(&SpeechWorkerCommand::SetOutputMode {
            output_mode: rust_protocol::SpeechOutputMode::OriginalLanguage,
        });
        crate::settings_view::sync_output_mode(settings, state);
        set_indicator_status(
            settings,
            text(
                active_locale(state.borrow().ui_locale),
                UiText::SpeechTranslationDisabled,
            ),
        );
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
    let local_enabled = providers
        .iter()
        .any(|provider| provider.id == "local" && provider.enabled);
    if !local_enabled {
        state.borrow_mut().translation_ready = false;
    }
    if let Err(error) = workers
        .translation
        .send(&TranslationWorkerCommand::SetTranslationProviders { providers })
    {
        set_indicator_status(
            settings,
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::TranslationOrderFailed,
                &[("error", &error.to_string())],
            ),
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
        text_chain_positions,
    };
    use crate::settings_projection::departures;

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

        let leaves =
            |chain: &[TranslationProviderEntry]| departures(&[], |_| false, chain, keyed).text_to;

        assert_eq!(
            leaves(&[entry("deepl", true), entry("local", true)]),
            Some("DeepL".to_owned())
        );
        assert_eq!(
            leaves(&[entry("local", true), entry("deepl", true)]),
            None,
            "the walk stops at the local engine, so nothing under it is asked"
        );
        assert_eq!(
            leaves(&[entry("unkeyed", true), entry("local", true)]),
            None,
            "a service with no key is never asked, so it sends nothing"
        );
        assert_eq!(
            leaves(&[entry("deepl", false)]),
            None,
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
            Err(SaveRefused::KeyRequired)
        );
    }

    #[test]
    fn azures_region_is_offered_and_not_demanded() {
        // A global Translator resource has no region and works without one.
        // Demanding it locked out everyone holding a global key.
        assert_eq!(
            check_translation_provider_configuration("azure", "key", ""),
            Ok(("key".to_owned(), None))
        );
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

    use crate::settings_projection::departures;

    /// Which service a recording would reach, if any. The one function that
    /// answers this now also names it, so the tests read the name.
    fn audio_departure(
        chain: &[TranscriptionProviderEntry],
        keyed: impl Fn(&str) -> bool,
    ) -> Option<String> {
        departures(chain, keyed, &[], |_| false).audio_to
    }

    use super::{
        SaveRefused, check_provider_configuration, provider_display_name, provider_field_label,
        provider_field_value, set_provider_field_value,
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

        assert_eq!(refused, SaveRefused::KeyRequired);
    }

    #[test]
    fn an_endpoint_that_is_not_https_stops_the_whole_save() {
        // It used to reject the endpoint and store the key anyway, leaving the
        // provider keyed and misconfigured while the form closed as though it
        // had worked.
        let refused = check_provider_configuration("custom", "sk-live-1234", "http://example.test")
            .expect_err("the key and the audio both travel in this request");

        assert_eq!(refused, SaveRefused::InsecureEndpoint);
    }

    #[test]
    fn a_provider_that_needs_a_second_field_will_not_save_without_it() {
        let refused = check_provider_configuration("azure", "sk-live-1234", "  ")
            .expect_err("Azure answers 403 without a region");

        assert_eq!(refused, SaveRefused::FieldRequired("Region".to_owned()));
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
        assert!(audio_departure(&rows, |id| id == "groq").is_none());
    }

    #[test]
    fn a_keyed_service_above_the_local_engine_does_send_audio_out() {
        let rows = chain(&[("groq", true), ("local", true)]);
        assert!(audio_departure(&rows, |id| id == "groq").is_some());
    }

    #[test]
    fn a_service_without_a_key_is_not_a_service() {
        let rows = chain(&[("groq", true), ("local", true)]);
        assert!(audio_departure(&rows, |_| false).is_none());
    }

    #[test]
    fn a_switched_off_local_engine_does_not_end_the_walk() {
        // Switching the local engine off is how a user says "always use a
        // service", and the row below it is then genuinely reached.
        let rows = chain(&[("local", false), ("groq", true)]);
        assert!(audio_departure(&rows, |id| id == "groq").is_some());
    }

    #[test]
    fn an_empty_or_silent_chain_claims_nothing() {
        assert!(audio_departure(&[], |_| true).is_none());
        let off = chain(&[("groq", false), ("local", false)]);
        assert!(audio_departure(&off, |_| true).is_none());
    }
}
