//! The settings window and the other windows the user interacts with directly.
//!
//! Wiring, not deciding: every callback here turns a click into a change of
//! state and a command to a worker, and every `sync_` function turns state back
//! into what the window shows. The rules those changes obey live elsewhere.

use std::{cell::RefCell, rc::Rc, time::Instant};

use rust_app_core::{LayoutDirection, build_layout_candidates};
use rust_protocol::{
    CompanionRendererCommand, HistoryEntry, InputHostCommand, RequestId, SpeechOutputMode,
    SpeechWorkerCommand, TargetSnapshot, TranslationWorkerCommand,
};
use slint::{
    ComponentHandle, Model, ModelRc, PhysicalPosition, SharedString, VecModel,
    winit_030::WinitWindowAccessor,
};

use crate::{
    ActionPalette, AppState, AppTray, Fact, HISTORY_LIMIT, HistoryRow, HotkeyRow,
    MAX_TRANSLATION_BLUR_RADIUS, ModelRow, PendingCredential, PendingSelection, PendingTranslation,
    RecordingOverlays, SelectionChooser, SelectionTranslationRequest, SettingsWindow,
    TextActionMode, TranslationDelivery, TranslationOrigin, TranslatorWindow,
    begin_selection_translation, configure_standard_app_window,
    dispatch_or_defer_selection_replacement, flow_sphere, native_window_handle,
    overlay_presenter::OverlayPresenter,
    providers_view::{
        Kind, SaveRefused, apply_provider_chain, apply_translation_chain,
        check_provider_configuration, check_translation_provider_configuration, locate,
        row_display_name, split_row_id, sync_providers,
    },
    settings_document::save_settings,
    settings_projection::{
        departure_badge, departures, history_row, privacy_claim, speech_model_row, tone,
        translation_model_row,
    },
    slint_window_handle,
    speech_backend::SpeechBackendPreference,
    translation_view::{TranslationPreviewRegistry, adaptive_text_panel_height},
    unix_ms,
    visualizer::{
        BroadcastMicPalette, CircularSpectrumPalette, IndicatorAnimation, VisualizerStyle,
        WritingSlateSurface,
    },
    worker_supervisor::Workers,
};

/// Both lists, in the order the page shows them: speech models, then the
/// translator. One list because the page is about models, and because a
/// `ListView` takes a single `for`.
pub(crate) fn sync_models(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let mut rows: Vec<ModelRow> = state
        .models
        .iter()
        .map(|model| speech_model_row(model, state.downloads.get(&model.catalog_id).copied()))
        .collect();
    rows.extend(state.translation_models.iter().map(|model| {
        translation_model_row(
            model,
            state.selected_translation_tier == Some(model.tier),
            state.downloads.get(&model.catalog_id).copied(),
        )
    }));
    settings.set_model_rows(ModelRc::new(VecModel::from(rows)));
    // The list and the claim above it come from the same facts, so they are
    // refreshed together: a note saying a model is downloading, next to a list
    // showing it installed, is the window disagreeing with itself.
    sync_overview(&settings, &state);
    settings.set_translation_model_name(
        state
            .translation_models
            .iter()
            .find(|model| state.selected_translation_tier == Some(model.tier) && model.installed)
            .map(|model| model.display_name.clone())
            .unwrap_or_default()
            .into(),
    );
    set_models_summary(&settings, &state);
}

/// The translation list changes for the same reasons the speech one does, and
/// they are drawn as one list, so they are rebuilt together.
pub(crate) fn sync_translation_models(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    sync_models(settings, state);
}

/// How many models are on disk, for the line beside the heading. A count is a
/// caption, not a figure worth the large-number treatment: it does not change
/// while you are looking at it.
fn set_models_summary(settings: &SettingsWindow, state: &AppState) {
    let installed = state.models.iter().filter(|model| model.installed).count()
        + state
            .translation_models
            .iter()
            .filter(|model| model.installed)
            .count();
    settings.set_models_summary(
        match installed {
            0 => "None installed yet".to_owned(),
            1 => "1 installed".to_owned(),
            many => format!("{many} installed"),
        }
        .into(),
    );
}

pub(crate) fn sync_action_settings(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();

    // ── Dictation ──
    settings.set_speech_backend_index(state.speech_backend_preference.index());
    settings.set_speech_backend_verified(state.speech_ready);
    settings.set_speech_backend_detail(
        state
            .speech_backend_active
            .map_or_else(
                || "No compatible speech backend is active".to_owned(),
                |active| {
                    if state.speech_ready {
                        format!(
                            "{} verified by warm-up. The preference above is where the search starts.",
                            active.display_name()
                        )
                    } else {
                        // Chosen is not the same as proved. The warm-up is what
                        // proves it, and until it finishes the line says so.
                        format!(
                            "Warming up on {}. If it fails, the search moves to the next one down.",
                            active.display_name()
                        )
                    }
                },
            )
            .into(),
    );
    // Fastest costs nothing worth having on a GPU and costs the final result
    // its own runtime on the CPU, where there is one inference actor and every
    // preview is time the transcript waits. The worker refuses it there, so the
    // page must not go on offering it: a control that reports a setting the
    // program ignores is a control that lies about the product.
    let previews_possible =
        state.speech_backend_active != Some(crate::speech_backend::SpeechBackend::Cpu);
    let previews_on = state.fast_release_previews && previews_possible;
    settings.set_release_mode(if previews_on { "fastest" } else { "efficient" }.into());
    settings.set_release_mode_label(if previews_on { "Fastest" } else { "Efficient" }.into());
    settings.set_release_mode_locked(!previews_possible);
    settings.set_release_mode_note(
        if previews_possible {
            ""
        } else {
            "Not available on the CPU engine: one pass at a time, and a preview is time the transcript waits."
        }
        .into(),
    );
    settings.set_history_enabled(state.history_enabled);
    settings.set_output_mode(
        match state.output_mode {
            SpeechOutputMode::OriginalLanguage => "original",
            SpeechOutputMode::TranslateToEnglish => "english",
        }
        .into(),
    );
    settings.set_output_mode_label(
        match state.output_mode {
            SpeechOutputMode::OriginalLanguage => "Original language",
            SpeechOutputMode::TranslateToEnglish => "Translate to English",
        }
        .into(),
    );
    settings.set_output_mode_locked(
        state
            .models
            .iter()
            .find(|model| model.catalog_id == state.selected_model_id)
            .is_some_and(|model| !model.supports_speech_translation),
    );
    settings.set_dictation_subtitle(
        if state.speech_ready {
            "Local speech · ready"
        } else {
            "Local speech · starting"
        }
        .into(),
    );
    settings.set_dictation_readout(
        if state.speech_ready {
            "Ready"
        } else {
            "Warming up"
        }
        .into(),
    );
    settings.set_dictation_caption("hold Ctrl + Alt + Space".into());

    // ── Translation ──
    settings.set_translation_enabled(state.translation_enabled);
    settings.set_translation_warm(state.translation_keep_warm);
    settings.set_translation_delivery(
        match state.translation_delivery {
            TranslationDelivery::Preview => "preview",
            TranslationDelivery::Replace => "replace",
            TranslationDelivery::Copy => "copy",
        }
        .into(),
    );
    settings.set_translation_delivery_label(
        match state.translation_delivery {
            TranslationDelivery::Preview => "Show a preview",
            TranslationDelivery::Replace => "Replace the selection",
            TranslationDelivery::Copy => "Copy it",
        }
        .into(),
    );
    settings.set_text_action_mode(
        match state.text_action_mode {
            TextActionMode::Adaptive => "adaptive",
            TextActionMode::AlwaysPalette => "palette",
        }
        .into(),
    );
    settings.set_text_action_mode_label(
        match state.text_action_mode {
            TextActionMode::Adaptive => "Decide from the text",
            TextActionMode::AlwaysPalette => "Always ask",
        }
        .into(),
    );
    settings.set_translation_blur_radius(state.translation_blur_radius as i32);
    settings.set_translation_ready(state.translation_ready);

    // ── Visuals ──
    settings.set_selected_visualizer(state.visualizer_style.index());
    settings.set_selected_circular_palette(state.circular_spectrum_palette.index());
    settings.set_selected_broadcast_mic_palette(state.broadcast_mic_palette.index());
    settings.set_selected_flow_sphere_palette(state.flow_sphere_palette.index());
    settings.set_selected_writing_slate_surface(state.writing_slate_surface.index());
    sync_visualizer_copy(&settings, state.visualizer_style);

    // ── Hotkeys ──
    settings.set_hotkey_rows(ModelRc::new(VecModel::from(hotkey_rows(&state))));
    settings.set_hotkeys_summary(hotkeys_summary(&state).into());

    sync_overview(&settings, &state);
}

/// The subtitle and note beside the visualizer preview.
///
/// Transparent visualizers are drawn on a native surface of their own. That is
/// not decoration, it is why they behave differently, so the page says it.
pub(crate) fn sync_visualizer_copy(settings: &SettingsWindow, style: VisualizerStyle) {
    let (name, caption) = match style {
        VisualizerStyle::VoiceOrb => ("Voice orb", "Follows the level of your voice"),
        VisualizerStyle::SymmetricSpectrum => {
            ("Spectrum", "Thirty-two bands, thirty-one times a second")
        }
        VisualizerStyle::FluidRibbon => ("Fluid ribbon", "One line that thickens with the voice"),
        VisualizerStyle::BroadcastMic => ("Broadcast mic", "Level bars beside a studio microphone"),
        VisualizerStyle::CircularSpectrum => ("Circular", "The spectrum wrapped into a ring"),
        VisualizerStyle::FlowSphere => (
            "Flow sphere",
            "A point cloud that accelerates on voice onset",
        ),
        VisualizerStyle::WritingSlate => ("Writing slate", "The words as they are recognised"),
        VisualizerStyle::CompanionOrb => (
            "Companion orb",
            "Stays where you put it, between dictations",
        ),
        VisualizerStyle::Off => ("Off", "No overlay is drawn while recording"),
    };
    let native =
        style.recording_surface() == Some(crate::visualizer::RecordingSurface::Transparent);
    settings.set_visuals_subtitle(format!("{name} · shown while recording").into());
    settings.set_visuals_caption(caption.into());
    settings.set_visuals_note(
        if style == VisualizerStyle::Off {
            "The microphone and transcription keep working; only the overlay is disabled."
        } else if native {
            "Drawn on its own native surface, so it can be transparent per pixel."
        } else {
            "Drawn by the application itself, over whatever you are typing into."
        }
        .into(),
    );
}

/// The five commands SayFlex registers.
///
/// The combinations are fixed in this build — `input-host` registers K, V and F
/// at start-up and the dictate gesture is recognised by the low-level hook — so
/// the row says so on its Change button rather than the page quietly not
/// offering one. Switching a command off is a different thing from changing its
/// combination, and only the first is possible here.
fn hotkey_rows(state: &AppState) -> Vec<HotkeyRow> {
    const FIXED: &str = "Fixed in this version";
    let held = |on: bool| {
        if on {
            (tone::RESTING, "Held")
        } else {
            (tone::OFF, "Off")
        }
    };

    let mut rows = Vec::with_capacity(5);
    rows.push(HotkeyRow {
        keys: "Ctrl + Alt + Space".into(),
        name: "Dictate".into(),
        detail: "Records while held · transcribes and inserts on release".into(),
        state: "Held".into(),
        tone: tone::ACTIVE.into(),
        enabled: true,
        locked_reason: FIXED.into(),
        mode: "".into(),
        mode_value: "".into(),
        mode_key: "".into(),
    });
    rows.push(HotkeyRow {
        keys: "Ctrl + Alt + V".into(),
        name: "Insert last".into(),
        detail: "Places the newest transcript where the caret is now".into(),
        state: "Held".into(),
        tone: tone::RESTING.into(),
        enabled: true,
        locked_reason: FIXED.into(),
        mode: "".into(),
        mode_value: "".into(),
        mode_key: "".into(),
    });

    let (layout_tone, layout_state) = held(state.layout_enabled);
    rows.push(HotkeyRow {
        keys: "Ctrl + Alt + K".into(),
        name: "Correct layout".into(),
        detail: if state.layout_enabled {
            "Offers RU/EN candidates for the selection · never automatic".into()
        } else {
            // Not "left to other applications": input-host registers this
            // combination at start-up and never releases it, so SayFlex still
            // receives the keys and simply does nothing with them.
            "Switched off · SayFlex ignores it but still holds the combination".into()
        },
        state: layout_state.into(),
        tone: layout_tone.into(),
        enabled: state.layout_enabled,
        locked_reason: FIXED.into(),
        mode: "".into(),
        mode_value: "".into(),
        mode_key: "".into(),
    });

    let (action_tone, action_state) = held(state.translation_enabled);
    rows.push(HotkeyRow {
        keys: "Ctrl + Alt + F".into(),
        name: "Act on selection".into(),
        detail: if state.translation_enabled {
            "Captures the selection once and acts on it".into()
        } else {
            "Switched off · SayFlex ignores it but still holds the combination".into()
        },
        state: action_state.into(),
        tone: action_tone.into(),
        enabled: state.translation_enabled,
        locked_reason: FIXED.into(),
        mode: "text-action".into(),
        mode_value: match state.text_action_mode {
            TextActionMode::Adaptive => "Decide from the text".into(),
            TextActionMode::AlwaysPalette => "Always ask".into(),
        },
        mode_key: match state.text_action_mode {
            TextActionMode::Adaptive => "adaptive".into(),
            TextActionMode::AlwaysPalette => "palette".into(),
        },
    });

    rows.push(HotkeyRow {
        keys: "Ctrl + Alt + P".into(),
        name: "Pause input features".into(),
        detail: "Not implemented in this version".into(),
        state: "Not held".into(),
        tone: tone::OFF.into(),
        enabled: false,
        locked_reason: "Not implemented in this version".into(),
        mode: "".into(),
        mode_value: "".into(),
        mode_key: "".into(),
    });
    rows
}

fn hotkeys_summary(_state: &AppState) -> String {
    // Four combinations are registered at start-up and never released, whatever
    // the switches say: `input-host` calls RegisterHotKey for K, V and F once,
    // and the dictate gesture is recognised by the low-level hook. Switching a
    // command off stops SayFlex acting on its keys; it does not give them back
    // to Windows.
    //
    // The line used to count the switches and report "Two of five held" above
    // rows that each said SayFlex still holds the combination. Until the
    // registration is actually dynamic, the true number is four.
    "Four of five held · pause is not implemented".to_owned()
}

/// Whether anything is actually coming down the wire.
///
/// Not "is the map non-empty": an entry for a model that is already on disk is
/// a leftover from the last progress report, and Overview announcing a download
/// that finished before the window opened is exactly the kind of claim this
/// page must not make.
fn downloading_now(state: &AppState) -> bool {
    state.downloads.keys().any(|catalog_id| {
        let installed = |installed: bool| !installed;
        state
            .models
            .iter()
            .find(|model| &model.catalog_id == catalog_id)
            .map(|model| installed(model.installed))
            .or_else(|| {
                state
                    .translation_models
                    .iter()
                    .find(|model| &model.catalog_id == catalog_id)
                    .map(|model| installed(model.installed))
            })
            // An id the catalogue does not know is one the worker is still
            // fetching; it cannot be installed if it is not listed.
            .unwrap_or(true)
    })
}

/// The claim, the statement and the facts on Overview.
fn sync_overview(settings: &SettingsWindow, state: &AppState) {
    let keyed = |id: &str| {
        state
            .transcription_providers
            .iter()
            .any(|status| status.id == id && status.has_credential)
    };
    let text_keyed = |id: &str| {
        state
            .translation_providers
            .iter()
            .any(|status| status.id == id && status.has_credential)
    };
    let leaving = departures(
        &state.transcription_chain,
        keyed,
        &state.translation_chain,
        text_keyed,
    );
    settings.set_audio_leaves_device(leaving.audio_leaves());
    settings.set_text_leaves_device(leaving.text_leaves());
    settings.set_departure_badge(departure_badge(&leaving).into());
    settings.set_provider_claim(privacy_claim(&leaving).into());

    let installed_speech = state.models.iter().any(|model| model.installed);
    let (statement, sub) = if !installed_speech {
        (
            "No speech model yet",
            "Install one on the Models page and dictation starts working.",
        )
    } else if !state.speech_ready {
        ("Warming up", "The engine is loading the model you chose.")
    } else {
        (
            "Ready to dictate",
            "Hold Ctrl + Alt + Space anywhere in Windows.",
        )
    };
    settings.set_overview_statement(statement.into());
    settings.set_overview_sub(sub.into());

    // The note carries the one thing that wants attention, and its button goes
    // where the thing can be dealt with.
    if !installed_speech {
        settings.set_overview_note(
            "No speech model is installed, so nothing can be dictated yet.".into(),
        );
        settings.set_overview_note_action("Install one".into());
        settings.set_overview_note_signal(true);
    } else if downloading_now(state) {
        settings
            .set_overview_note("A model is downloading. It resumes if it is interrupted.".into());
        settings.set_overview_note_action("Details".into());
        settings.set_overview_note_signal(true);
    } else if state.translation_enabled && !state.translation_ready {
        settings.set_overview_note("Translation is on, but its engine is not warm yet.".into());
        settings.set_overview_note_action("Details".into());
        settings.set_overview_note_signal(false);
    } else {
        settings.set_overview_note("".into());
        settings.set_overview_note_action("".into());
        settings.set_overview_note_signal(false);
    }

    let facts = vec![
        Fact {
            label: "SPEECH ENGINE".into(),
            value: state
                .speech_backend_active
                .map_or_else(
                    || "none".to_owned(),
                    |active| active.display_name().to_owned(),
                )
                .into(),
            detail: format!("preference: {}", state.speech_backend_preference.label()).into(),
            signal: state.speech_backend_active.is_none(),
        },
        Fact {
            label: "SPEECH MODEL".into(),
            value: state
                .models
                .iter()
                .find(|model| model.catalog_id == state.selected_model_id)
                .map(|model| model.display_name.clone())
                .unwrap_or_else(|| "none selected".to_owned())
                .into(),
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: "TRANSLATION".into(),
            value: if !state.translation_enabled {
                "off".into()
            } else if state.translation_ready {
                "warm".into()
            } else {
                "cold".into()
            },
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: "HISTORY".into(),
            value: if state.history_enabled {
                format!("{} kept on this device", state.history.len()).into()
            } else {
                "not kept".into()
            },
            detail: "".into(),
            signal: false,
        },
    ];
    settings.set_overview_facts(ModelRc::new(VecModel::from(facts)));
}

thread_local! {
    /// The history list is owned once and mutated in place. Replacing the model
    /// on every completion would re-allocate every row and reset the list's
    /// scroll position.
    static HISTORY_ROWS: RefCell<Option<Rc<VecModel<HistoryRow>>>> = const { RefCell::new(None) };
}

pub(crate) fn history_rows_model(settings: &SettingsWindow) -> Rc<VecModel<HistoryRow>> {
    HISTORY_ROWS.with(|model| {
        let mut model = model.borrow_mut();
        if let Some(existing) = model.as_ref() {
            return Rc::clone(existing);
        }
        let created = Rc::new(VecModel::<HistoryRow>::default());
        settings.set_history_rows(ModelRc::from(Rc::clone(&created)));
        *model = Some(Rc::clone(&created));
        created
    })
}

/// Adds one completed transcript to the top of the visible history without
/// rebuilding the other rows. Called only after the insertion command has
/// already been dispatched.
pub(crate) fn prepend_history_row(
    settings: &slint::Weak<SettingsWindow>,
    entry: &HistoryEntry,
    retained: usize,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = history_rows_model(&settings);
    rows.insert(0, history_row(entry));
    while rows.row_count() > retained {
        rows.remove(rows.row_count() - 1);
    }
}

pub(crate) fn sync_history(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let rows = state.history.iter().map(history_row).collect::<Vec<_>>();
    // How many entries there are is a caption under a heading, not a figure
    // worth the large-number treatment: it does not change while you look at
    // it. How many of them left the machine, on the other hand, is worth
    // saying out loud.
    let remote = state
        .history
        .iter()
        .filter(|entry| {
            entry
                .transcript
                .provider
                .as_deref()
                .is_some_and(|provider| provider != "local")
        })
        .count();
    // The worker keeps 250 and hands over as many as were asked for, so a page
    // showing two hundred of them must not report two hundred as the whole of
    // it. Both counts are of what is on this page, and the line says so.
    let shown = rows.len();
    settings.set_history_scope(
        match (shown, remote) {
            (0, _) => "Nothing kept yet".to_owned(),
            (total, 0) if total < HISTORY_LIMIT => {
                format!("{total} kept · all on this device")
            }
            (total, 0) => format!("Showing {total} · all on this device"),
            (total, remote) if total < HISTORY_LIMIT => {
                format!("{total} kept · {remote} went to a service")
            }
            (total, remote) => {
                format!("Showing {total} · {remote} of them went to a service")
            }
        }
        .into(),
    );
    history_rows_model(&settings).set_vec(rows);
}

pub(crate) fn sync_output_mode(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    if let Some(settings) = settings.upgrade() {
        let state = state.borrow();
        settings.set_output_mode(
            match state.output_mode {
                SpeechOutputMode::OriginalLanguage => "original",
                SpeechOutputMode::TranslateToEnglish => "english",
            }
            .into(),
        );
        settings.set_output_mode_label(
            match state.output_mode {
                SpeechOutputMode::OriginalLanguage => "Original language",
                SpeechOutputMode::TranslateToEnglish => "Translate to English",
            }
            .into(),
        );
        // Whether the choice can be made at all belongs with the choice, so the
        // menu can grey it and say why rather than refusing after the click.
        settings.set_output_mode_locked(
            state
                .models
                .iter()
                .find(|model| model.catalog_id == state.selected_model_id)
                .is_some_and(|model| !model.supports_speech_translation),
        );
    }
}

pub(crate) fn set_runtime_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    if let Some(settings) = settings.upgrade() {
        settings.set_runtime_status(message.into());
    }
}

/// How much attention a status line deserves.
///
/// Declared by whoever writes the message. The previous version read English
/// substrings — "could not", "failed" — which missed "Input worker is not
/// ready", missed a protocol mismatch, changed meaning whenever a sentence was
/// reworded, and would classify nothing at all once these strings are
/// translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    /// Something is happening, or has finished happily.
    Progress,
    /// Something went wrong that the user has to act on.
    Trouble,
}

/// Says what the application is doing, and whether it is in trouble.
///
/// The one place either of those properties is written. Setting the message
/// without the severity left the accent lit after a recovery, or dark through
/// a failure, depending on which of the two was written last.
pub(crate) fn set_status(
    settings: &slint::Weak<SettingsWindow>,
    severity: Severity,
    message: &str,
) {
    if let Some(settings) = settings.upgrade() {
        settings.set_indicator_status(message.into());
        settings.set_status_is_a_problem(severity == Severity::Trouble);
    }
}

/// The common case: reporting progress.
pub(crate) fn set_indicator_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    set_status(settings, Severity::Progress, message);
}

/// Reporting something the user has to do something about.
pub(crate) fn set_status_problem(settings: &slint::Weak<SettingsWindow>, message: &str) {
    set_status(settings, Severity::Trouble, message);
}

pub(crate) fn show_layout_chooser(
    request_id: RequestId,
    target: TargetSnapshot,
    text: String,
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    chooser_presenter: &OverlayPresenter,
    state: &Rc<RefCell<AppState>>,
) {
    let candidates = build_layout_candidates(&text);
    if candidates.is_empty() {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(settings, "No RU/EN layout conversion is available.");
        return;
    }
    if let Some(chooser) = chooser.upgrade() {
        chooser.set_prepared(false);
        let ticket = match chooser_presenter.begin_prepare() {
            Ok(ticket) => ticket,
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "selection_chooser_overlay transition=failed stage=begin_prepare error={error}"
                ));
                return;
            }
        };
        chooser.set_source_panel_height(adaptive_text_panel_height(&text, 82, 2, 5, 15.0, 16.0));
        chooser.set_source_text(text.into());
        chooser.set_candidate_labels(ModelRc::new(VecModel::from(
            candidates
                .iter()
                .map(|candidate| match candidate.direction {
                    LayoutDirection::ToCyrillic => {
                        SharedString::from("English keys → Cyrillic layout")
                    }
                    LayoutDirection::ToLatin => SharedString::from("Cyrillic keys → Latin layout"),
                })
                .collect::<Vec<_>>(),
        )));
        chooser.set_candidate_texts(ModelRc::new(VecModel::from(
            candidates
                .iter()
                .map(|candidate| SharedString::from(&candidate.text))
                .collect::<Vec<_>>(),
        )));
        state.borrow_mut().pending_selection = Some(PendingSelection {
            request_id,
            target,
            candidates,
        });
        let configured = slint_window_handle(chooser.window())
            .ok_or_else(|| "native selection-chooser HWND is unavailable".to_owned())
            .and_then(|handle| {
                rust_platform_windows::configure_action_window_for_target(
                    handle,
                    target.window_handle,
                )
            });
        if let Err(error) = configured {
            chooser_presenter.abort_prepare(chooser.window(), ticket, "configure", &error);
            return;
        }
        chooser.set_prepared(true);
        if let Err(error) = chooser_presenter.await_frame(chooser.window(), ticket) {
            chooser.set_prepared(false);
            chooser_presenter.abort_prepare(chooser.window(), ticket, "await_frame", &error);
        }
    }
}

/// Selects a speech model, installing it first if it is not on disk.
fn choose_speech_model(
    settings_weak: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    catalog_id: &str,
) {
    let Some(model) = state
        .borrow()
        .models
        .iter()
        .find(|model| model.catalog_id == catalog_id)
        .cloned()
    else {
        return;
    };

    let reset_output_mode = {
        let mut state = state.borrow_mut();
        state.selected_model_id.clone_from(&model.catalog_id);
        state.speech_ready = false;
        if !model.supports_speech_translation
            && state.output_mode == SpeechOutputMode::TranslateToEnglish
        {
            state.output_mode = SpeechOutputMode::OriginalLanguage;
            true
        } else {
            false
        }
    };
    if reset_output_mode {
        let _ = workers.speech.send(&SpeechWorkerCommand::SetOutputMode {
            output_mode: SpeechOutputMode::OriginalLanguage,
        });
        sync_output_mode(settings_weak, state);
    }
    let _ = save_settings(&state.borrow());
    let _ = workers.speech.send(&SpeechWorkerCommand::SelectModel {
        catalog_id: model.catalog_id,
    });
}

/// Selects a translation model, downloading it first if it is not on disk.
fn choose_translation_model(
    settings_weak: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    catalog_id: &str,
) {
    let Some(model) = state
        .borrow()
        .translation_models
        .iter()
        .find(|model| model.catalog_id == catalog_id)
        .cloned()
    else {
        return;
    };

    {
        let mut state = state.borrow_mut();
        state.translation_enabled = true;
        state.translation_ready = false;
    }
    let command = if model.installed {
        TranslationWorkerCommand::SelectTier {
            tier: model.tier,
            keep_warm: state.borrow().translation_keep_warm,
        }
    } else {
        TranslationWorkerCommand::EnsureModel { tier: model.tier }
    };
    if let Err(error) = workers.translation.send(&command) {
        set_indicator_status(
            settings_weak,
            &format!("Could not configure translation model: {error}"),
        );
    } else {
        set_indicator_status(
            settings_weak,
            if model.installed {
                "Loading the selected translation model…"
            } else {
                "Downloading and verifying the translation model…"
            },
        );
    }
    let _ = save_settings(&state.borrow());
    sync_action_settings(settings_weak, state);
}

pub(crate) fn wire_settings(
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    recording_overlays: &RecordingOverlays,
    indicator_animation: &Rc<RefCell<IndicatorAnimation>>,
) {
    {
        let translator = translator.as_weak();
        settings.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                configure_standard_app_window(&translator, "Translator");
                translator.window().request_redraw();
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        // Four backends shown at once rather than cycled through: the one in
        // force is the answer to "why is this slow", and an answer you have to
        // click three times to read is not one.
        settings.on_select_speech_backend(move |index| {
            let preference = SpeechBackendPreference::from_index(index).unwrap_or_default();
            if preference == state.borrow().speech_backend_preference {
                return;
            }
            match workers.switch_speech_backend(preference) {
                Ok(active) => {
                    let initialization = {
                        let mut state = state.borrow_mut();
                        state.speech_backend_preference = preference;
                        state.speech_backend_active = Some(active);
                        state.speech_ready = false;
                        workers.speech.send(&SpeechWorkerCommand::Initialize {
                            selected_model_id: state.selected_model_id.clone(),
                            output_mode: state.output_mode,
                            history_enabled: state.history_enabled,
                            fast_release_previews: state.fast_release_previews,
                        })
                    };
                    let _ = save_settings(&state.borrow());
                    sync_action_settings(&settings_weak, &state);
                    if let Err(error) = initialization {
                        set_indicator_status(
                            &settings_weak,
                            &format!("Speech backend initialization failed: {error}"),
                        );
                    } else {
                        set_runtime_status(&settings_weak, "Checking backend");
                        set_indicator_status(
                            &settings_weak,
                            &format!(
                                "Checking {} with the selected Whisper model…",
                                active.display_name()
                            ),
                        );
                    }
                }
                Err(error) => set_status_problem(
                    &settings_weak,
                    &format!("Speech backend could not be changed: {error}"),
                ),
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        // One callback, one registration. Slint keeps the last handler given to
        // a callback, so registering this twice — once for speech models, once
        // for the translator — silently threw the first away and left every
        // speech row's Install and Use doing nothing.
        //
        // The row is identified by catalog id, so the handler asks which list it
        // belongs to and does the matching thing.
        settings.on_choose_model(move |catalog_id| {
            let is_speech = state
                .borrow()
                .models
                .iter()
                .any(|model| model.catalog_id == catalog_id.as_str());
            if is_speech {
                choose_speech_model(&settings_weak, &state, &workers, &catalog_id);
            } else {
                choose_translation_model(&settings_weak, &state, &workers, &catalog_id);
            }
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_refresh_history(move || {
            let _ = workers.speech.send(&SpeechWorkerCommand::GetHistory {
                limit: HISTORY_LIMIT,
            });
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_history_enabled(move |enabled| {
            state.borrow_mut().history_enabled = enabled;
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetHistoryEnabled { enabled });
            let _ = workers
                .translation
                .send(&TranslationWorkerCommand::SetHistoryEnabled { enabled });
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let recording_overlays = recording_overlays.clone();
        let indicator_animation = Rc::clone(indicator_animation);
        let workers = Rc::clone(workers);
        settings.on_select_visualizer(move |index| {
            let Some(style) = VisualizerStyle::from_index(index) else {
                return;
            };
            if style.uses_external_renderer() {
                if let Err(error) = workers.ensure_companion().and_then(|()| {
                    workers.send_companion(&CompanionRendererCommand::SetEnabled { enabled: true })
                }) {
                    set_indicator_status(
                        &settings_weak,
                        &format!("Could not start Companion Orb: {error}"),
                    );
                    return;
                }
            } else {
                let _ = workers.send_companion_if_running(&CompanionRendererCommand::SetEnabled {
                    enabled: false,
                });
            }
            let session_active = state.borrow().indicator_session.is_some();
            state.borrow_mut().visualizer_style = style;
            if !session_active {
                indicator_animation.borrow_mut().set_style(style);
                recording_overlays.set_style(style);
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                if style.uses_external_renderer() {
                    "Companion Orb is active on the desktop."
                } else {
                    "Visualizer updated. The next dictation uses the selected style."
                },
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let recording_overlays = recording_overlays.clone();
        let state = Rc::clone(state);
        settings.on_select_broadcast_mic_palette(move |index| {
            let Some(palette) = BroadcastMicPalette::from_index(index) else {
                return;
            };
            state.borrow_mut().broadcast_mic_palette = palette;
            recording_overlays.set_broadcast_mic_palette(palette);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Broadcast microphone colour updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let recording_overlays = recording_overlays.clone();
        let state = Rc::clone(state);
        settings.on_select_circular_palette(move |index| {
            let Some(palette) = CircularSpectrumPalette::from_index(index) else {
                return;
            };
            state.borrow_mut().circular_spectrum_palette = palette;
            recording_overlays.set_circular_palette(palette);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Circular spectrum palette updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let indicator_animation = Rc::clone(indicator_animation);
        settings.on_select_flow_sphere_palette(move |index| {
            let Some(palette) = flow_sphere::FlowSpherePalette::from_index(index) else {
                return;
            };
            state.borrow_mut().flow_sphere_palette = palette;
            indicator_animation
                .borrow_mut()
                .set_flow_sphere_palette(palette);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Flow Sphere palette updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let recording_overlays = recording_overlays.clone();
        let state = Rc::clone(state);
        settings.on_select_writing_slate_surface(move |index| {
            let Some(surface) = WritingSlateSurface::from_index(index) else {
                return;
            };
            state.borrow_mut().writing_slate_surface = surface;
            recording_overlays.set_writing_slate_surface(surface);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Writing Slate surface updated for the next dictation.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_clear_history(move || {
            let _ = workers.speech.send(&SpeechWorkerCommand::ClearHistory);
            let _ = workers
                .translation
                .send(&TranslationWorkerCommand::ClearHistory);
            {
                let mut state = state.borrow_mut();
                state.history.clear();
                state.translation_history.clear();
                state.transcripts.clear();
                state.latest_history_session = None;
                state.history_loaded = true;
            }
            sync_history(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                "Local voice and translation history cleared.",
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_move_provider(move |from, to| {
            // The two lists are drawn as one, so both ends of a move have to
            // land in the same list: dragging a speech provider into the
            // translation order would be arranging a list it is not in.
            let moved = {
                let mut state = state.borrow_mut();
                let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                    return;
                };
                let (Some((kind, from)), Some((to_kind, to))) =
                    (locate(&state, from), locate(&state, to))
                else {
                    return;
                };
                if kind != to_kind {
                    return;
                }
                match kind {
                    Kind::Speech => {
                        let entry = state.transcription_chain.remove(from);
                        state.transcription_chain.insert(to, entry);
                    }
                    Kind::Text => {
                        let entry = state.translation_chain.remove(from);
                        state.translation_chain.insert(to, entry);
                    }
                }
                kind
            };
            match moved {
                Kind::Speech => apply_provider_chain(&settings_weak, &state, &workers),
                Kind::Text => apply_translation_chain(&settings_weak, &state, &workers),
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_provider(move |index| {
            let toggled = {
                let mut state = state.borrow_mut();
                let Some((kind, position)) = usize::try_from(index)
                    .ok()
                    .and_then(|index| locate(&state, index))
                else {
                    return;
                };
                // Switching a service off keeps its place and its key. Only the
                // order and this switch decide what is used.
                let enabled = match kind {
                    Kind::Speech => state
                        .transcription_chain
                        .get_mut(position)
                        .map(|entry| &mut entry.enabled),
                    Kind::Text => state
                        .translation_chain
                        .get_mut(position)
                        .map(|entry| &mut entry.enabled),
                };
                let Some(enabled) = enabled else {
                    return;
                };
                *enabled = !*enabled;
                kind
            };
            match toggled {
                Kind::Speech => apply_provider_chain(&settings_weak, &state, &workers),
                Kind::Text => apply_translation_chain(&settings_weak, &state, &workers),
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        // The verb that appears in a row only while something is running. The
        // partial file is kept, so a later attempt resumes rather than starting
        // over — which is why the row says so while it downloads.
        // Cancel belongs to the row it was pressed in. Sending it to both
        // workers and clearing the whole map stopped a download the user had
        // not asked to stop.
        settings.on_cancel_model_install(move |catalog_id| {
            let catalog_id = catalog_id.to_string();
            let is_speech = state
                .borrow()
                .models
                .iter()
                .any(|model| model.catalog_id == catalog_id);
            let sent = if is_speech {
                workers
                    .speech
                    .send(&SpeechWorkerCommand::CancelModelInstall)
            } else {
                workers
                    .translation
                    .send(&TranslationWorkerCommand::CancelModelInstall)
            };
            state.borrow_mut().downloads.remove(&catalog_id);
            sync_models(&settings_weak, &state);
            set_indicator_status(
                &settings_weak,
                &match sent {
                    Ok(()) => "Install cancelled. The partial file is kept.".to_owned(),
                    Err(error) => format!("Could not cancel the install: {error}"),
                },
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        // One owner for the whole of a provider's configuration. Every rule is
        // decided before anything is written, and on a refusal nothing is: the
        // previous version called two callbacks in a row, so an endpoint that
        // was rejected still had its key stored and the form closed as though
        // it had worked.
        settings.on_save_provider(move |row_id, secret, extra| {
            let row_id = row_id.to_string();
            // Which list this row is in, and which service inside it. The two
            // lists both have an `azure` and a `local`, so a bare id would let
            // a translation key be stored against the speech service.
            let Some((kind, provider_id)) = split_row_id(&row_id) else {
                return;
            };
            let provider_id = provider_id.to_owned();
            let checked = match kind {
                Kind::Speech => check_provider_configuration(&provider_id, &secret, &extra),
                Kind::Text => {
                    check_translation_provider_configuration(&provider_id, &secret, &extra)
                }
            };
            let (secret, extra) = match checked {
                Ok(values) => values,
                Err(SaveRefused(reason)) => {
                    if let Some(settings) = settings_weak.upgrade() {
                        settings.set_provider_problem(reason.into());
                        settings.set_provider_problem_for(row_id.clone().into());
                    }
                    return;
                }
            };

            // Nothing is written and nothing closes until the worker says the
            // key is in the store. A successful `send` only means the pipe took
            // the bytes; the store is DPAPI, on another process, and it can
            // refuse — a profile copied from another Windows account cannot be
            // decrypted. This used to treat the pipe as the store: it saved the
            // second field, closed the form, and discarded what the user had
            // typed, and if the write then failed the row was left configured
            // for a service it had no key for.
            let operation_id = RequestId::new();
            state.borrow_mut().pending_credentials.insert(
                operation_id,
                PendingCredential {
                    row_id: row_id.clone(),
                    extra: extra.clone(),
                    display_name: row_display_name(kind, &provider_id),
                    forget: false,
                    sent: Instant::now(),
                },
            );

            let sent = match kind {
                Kind::Speech => workers
                    .speech
                    .send(&SpeechWorkerCommand::SetProviderCredential {
                        operation_id,
                        provider_id: provider_id.clone(),
                        secret: Some(secret),
                    }),
                Kind::Text => workers.translation.send(
                    &TranslationWorkerCommand::SetTranslationProviderCredential {
                        operation_id,
                        provider_id: provider_id.clone(),
                        secret: Some(secret),
                    },
                ),
            };
            if let Err(error) = sent {
                state.borrow_mut().pending_credentials.remove(&operation_id);
                if let Some(settings) = settings_weak.upgrade() {
                    settings.set_provider_problem(
                        format!("The key could not be sent to the worker: {error}").into(),
                    );
                    settings.set_provider_problem_for(row_id.clone().into());
                }
                return;
            }

            // Deliberately not "stored": it is on its way. `settle_credential`
            // says what happened when the worker answers.
            set_indicator_status(
                &settings_weak,
                &format!(
                    "Storing the key for {}…",
                    row_display_name(kind, &provider_id)
                ),
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        // Deleting a key stays its own command, asked for in place. It is the
        // only path that clears one, which is why an empty Save is refused
        // rather than quietly treated as a deletion.
        settings.on_forget_provider_key(move |row_id| {
            let row_id = row_id.to_string();
            let Some((kind, provider_id)) = split_row_id(&row_id) else {
                return;
            };
            let provider_id = provider_id.to_owned();
            // A removal is a store operation too, and answered the same way,
            // so the row's key hint only disappears once it has actually gone.
            let operation_id = RequestId::new();
            state.borrow_mut().pending_credentials.insert(
                operation_id,
                PendingCredential {
                    row_id: row_id.clone(),
                    extra: None,
                    display_name: row_display_name(kind, &provider_id),
                    forget: true,
                    sent: Instant::now(),
                },
            );
            let sent = match kind {
                Kind::Speech => workers
                    .speech
                    .send(&SpeechWorkerCommand::SetProviderCredential {
                        operation_id,
                        provider_id: provider_id.clone(),
                        secret: None,
                    }),
                Kind::Text => workers.translation.send(
                    &TranslationWorkerCommand::SetTranslationProviderCredential {
                        operation_id,
                        provider_id: provider_id.clone(),
                        secret: None,
                    },
                ),
            };
            if sent.is_err() {
                state.borrow_mut().pending_credentials.remove(&operation_id);
            }
            match sent {
                Ok(()) => set_indicator_status(
                    &settings_weak,
                    &format!(
                        "Removing the key for {}…",
                        row_display_name(kind, &provider_id)
                    ),
                ),
                Err(error) => set_status_problem(
                    &settings_weak,
                    &format!("The key could not be removed: {error}"),
                ),
            }
            sync_providers(&settings_weak, &state);
        });
    }
    {
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_copy_history(move |index| {
            let Some(transcript) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().history.get(index).cloned())
                .map(|entry| entry.transcript)
            else {
                return;
            };
            let _ = workers
                .input
                .send(&InputHostCommand::CopyTranscript { transcript });
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_output_mode(move |value| {
            let wants_english = value == "english";
            let supports_translation = {
                let state = state.borrow();
                state
                    .models
                    .iter()
                    .find(|model| model.catalog_id == state.selected_model_id)
                    .is_none_or(|model| model.supports_speech_translation)
            };
            // The menu shows this choice greyed with its reason attached, so
            // reaching here with it means the model changed underneath. Say so
            // rather than silently doing nothing.
            if wants_english && !supports_translation {
                set_status_problem(
                    &settings_weak,
                    "This model was not trained to translate. Use the selection translator instead.",
                );
                return;
            }
            let output_mode = {
                let mut state = state.borrow_mut();
                state.output_mode = if wants_english {
                    SpeechOutputMode::TranslateToEnglish
                } else {
                    SpeechOutputMode::OriginalLanguage
                };
                state.output_mode
            };
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetOutputMode { output_mode });
            let _ = save_settings(&state.borrow());
            sync_output_mode(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_release_mode(move |value| {
            let enabled = value == "fastest";
            state.borrow_mut().fast_release_previews = enabled;
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetFastReleasePreviews { enabled });
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_set_translation_enabled(move |enabled| {
            state.borrow_mut().translation_enabled = enabled;
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        // The old Text tools page was these switches with the combinations
        // taken away from them, so each one now lives on the command it
        // belongs to. The indexes are the rows built by `hotkey_rows`.
        settings.on_set_hotkey_enabled(move |index, enabled| {
            {
                let mut state = state.borrow_mut();
                match index {
                    2 => state.layout_enabled = enabled,
                    3 => state.translation_enabled = enabled,
                    // Dictate, insert-last and pause are registered
                    // unconditionally in this build; the row says so.
                    _ => return,
                }
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_set_text_action_mode(move |value| {
            state.borrow_mut().text_action_mode = if value == "palette" {
                TextActionMode::AlwaysPalette
            } else {
                TextActionMode::Adaptive
            };
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_set_translation_delivery(move |value| {
            state.borrow_mut().translation_delivery = match value.as_str() {
                "replace" => TranslationDelivery::Replace,
                "copy" => TranslationDelivery::Copy,
                _ => TranslationDelivery::Preview,
            };
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_translation_warm(move |warm| {
            let selected = {
                let mut state = state.borrow_mut();
                state.translation_keep_warm = warm;
                state.selected_translation_tier
            };
            if let Some(tier) = selected {
                state.borrow_mut().translation_ready = false;
                let _ = workers
                    .translation
                    .send(&TranslationWorkerCommand::SelectTier {
                        tier,
                        keep_warm: state.borrow().translation_keep_warm,
                    });
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        // The stepper clamps before it asks, so a value out of range never
        // reaches here in the first place; the clamp is repeated because a
        // callback that trusts its caller is a callback waiting to be called
        // from somewhere else.
        settings.on_set_translation_blur(move |value| {
            let radius = value.clamp(0, MAX_TRANSLATION_BLUR_RADIUS as i32) as u32;
            state.borrow_mut().translation_blur_radius = radius;
            translation_previews.borrow_mut().set_blur_radius(radius);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_quit_requested(move || {
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=exit_requested reason=settings_quit",
            );
            workers.shutdown();
            let _ = slint::quit_event_loop();
        });
    }
}

pub(crate) fn wire_tray(
    tray: &AppTray,
    settings: &SettingsWindow,
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let settings = settings.as_weak();
        tray.on_open_settings(move || {
            if let Some(settings) = settings.upgrade() {
                let _ = settings.show();
                configure_standard_app_window(&settings, "Settings");
                settings.window().request_redraw();
            }
        });
    }
    {
        let translator = translator.as_weak();
        tray.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                configure_standard_app_window(&translator, "Translator");
                translator.window().request_redraw();
            }
        });
    }
    {
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        tray.on_copy_last(move || {
            if let Some(transcript) = state.borrow().latest_transcript().cloned() {
                let _ = workers
                    .input
                    .send(&InputHostCommand::CopyTranscript { transcript });
            }
        });
    }
    {
        let workers = Rc::clone(workers);
        tray.on_quit_requested(move || {
            rust_app_core::diagnostics::log_line(
                "desktop lifecycle=exit_requested reason=tray_quit",
            );
            workers.shutdown();
            let _ = slint::quit_event_loop();
        });
    }
}

pub(crate) fn wire_chooser(
    chooser: &SelectionChooser,
    presenter: &OverlayPresenter,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let presenter = presenter.clone();
        chooser.on_choose_candidate(move |index| {
            let pending = state.borrow_mut().pending_selection.take();
            let Some(pending) = pending else {
                return;
            };
            let Some(candidate) = usize::try_from(index)
                .ok()
                .and_then(|index| pending.candidates.get(index))
            else {
                return;
            };
            if let Some(chooser) = chooser_weak.upgrade() {
                chooser.set_prepared(false);
                presenter.retire(chooser.window(), "candidate_selected");
            }
            let _ = dispatch_or_defer_selection_replacement(
                &state,
                &workers,
                pending.request_id,
                pending.target,
                candidate.text.clone(),
            );
        });
    }
    {
        let chooser_weak = chooser.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let presenter = presenter.clone();
        chooser.on_cancel(move || {
            let pending = state.borrow_mut().pending_selection.take();
            if let Some(pending) = pending {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&pending.request_id);
                let _ = workers.input.send(&InputHostCommand::CancelSelection {
                    request_id: pending.request_id,
                });
            }
            if let Some(chooser) = chooser_weak.upgrade() {
                chooser.set_prepared(false);
                presenter.retire(chooser.window(), "cancelled");
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_action_palette(
    palette: &ActionPalette,
    palette_presenter: &OverlayPresenter,
    translation_previews: &Rc<RefCell<TranslationPreviewRegistry>>,
    chooser: &SelectionChooser,
    chooser_presenter: &OverlayPresenter,
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let palette_weak = palette.as_weak();
        let settings = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let palette_presenter = palette_presenter.clone();
        palette.on_translate(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                palette.set_prepared(false);
                palette_presenter.retire(palette.window(), "translate_selected");
            }
            let Some(pending) = pending else {
                return;
            };
            let delivery = state.borrow().translation_delivery;
            begin_selection_translation(
                SelectionTranslationRequest {
                    request_id: pending.request_id,
                    target: pending.target,
                    text: pending.text,
                    delivery,
                    request_started_unix_ms: pending.request_started_unix_ms,
                },
                &settings,
                &translation_previews,
                &state,
                &workers,
            );
        });
    }
    {
        let palette_weak = palette.as_weak();
        let chooser = chooser.as_weak();
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let palette_presenter = palette_presenter.clone();
        let chooser_presenter = chooser_presenter.clone();
        palette.on_correct_layout(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(palette) = palette_weak.upgrade() {
                palette.set_prepared(false);
                palette_presenter.retire(palette.window(), "layout_selected");
            }
            let Some(pending) = pending else {
                return;
            };
            show_layout_chooser(
                pending.request_id,
                pending.target,
                pending.text,
                &settings,
                &chooser,
                &chooser_presenter,
                &state,
            );
        });
    }
    {
        let palette_weak = palette.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let presenter = palette_presenter.clone();
        palette.on_cancel(move || {
            let pending = state.borrow_mut().pending_text_action.take();
            if let Some(pending) = pending {
                state
                    .borrow_mut()
                    .selection_release_gates
                    .remove(&pending.request_id);
                let _ = workers.input.send(&InputHostCommand::CancelSelection {
                    request_id: pending.request_id,
                });
            }
            if let Some(palette) = palette_weak.upgrade() {
                palette.set_prepared(false);
                presenter.retire(palette.window(), "cancelled");
            }
        });
    }
}

pub(crate) fn wire_translator(
    translator: &TranslatorWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    {
        let translator_weak = translator.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        translator.on_translate(move || {
            let Some(translator) = translator_weak.upgrade() else {
                return;
            };
            let text = translator.get_source_text().trim().to_owned();
            if text.is_empty() {
                translator.set_status_text("Enter text to translate.".into());
                return;
            }
            let (selected_tier, ready) = {
                let state = state.borrow();
                (state.selected_translation_tier, state.translation_ready)
            };
            if selected_tier.is_none() {
                translator.set_status_text(
                    "No model installed. Choose Fast or Quality in Settings.".into(),
                );
                return;
            }
            if !ready {
                translator.set_status_text("Translation model is still loading.".into());
                return;
            }
            let request_id = RequestId::new();
            state.borrow_mut().pending_translations.insert(
                request_id,
                PendingTranslation {
                    origin: TranslationOrigin::Translator,
                    source_text: text.clone(),
                    started: Instant::now(),
                    request_started_unix_ms: unix_ms(),
                    first_partial_shown: false,
                },
            );
            translator.set_processing(true);
            translator.set_result_text("".into());
            translator.set_status_text("Translating locally…".into());
            if let Err(error) = workers
                .translation
                .send(&TranslationWorkerCommand::Translate {
                    request_id,
                    text,
                    source_language: None,
                    target_language: None,
                })
            {
                state.borrow_mut().pending_translations.remove(&request_id);
                translator.set_processing(false);
                translator.set_status_text(format!("Could not translate: {error}").into());
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        let workers = Rc::clone(workers);
        translator.on_copy_result(move || {
            let Some(translator) = translator_weak.upgrade() else {
                return;
            };
            let text = translator.get_result_text().to_string();
            if !text.trim().is_empty() {
                let _ = workers.input.send(&InputHostCommand::CopyText {
                    request_id: RequestId::new(),
                    text,
                });
                translator.set_status_text("Copied to clipboard.".into());
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        translator.on_swap_text(move || {
            if let Some(translator) = translator_weak.upgrade() {
                let source = translator.get_source_text();
                let result = translator.get_result_text();
                translator.set_source_text(result);
                translator.set_result_text(source);
                translator
                    .set_status_text("Direction will be detected again when you translate.".into());
            }
        });
    }
}

pub(crate) fn precreate_action_windows(
    chooser: &SelectionChooser,
    chooser_presenter: &OverlayPresenter,
    palette: &ActionPalette,
    palette_presenter: &OverlayPresenter,
) {
    chooser.set_prepared(false);
    chooser
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if chooser.show().is_ok() {
        let chooser = chooser.as_weak();
        let presenter = chooser_presenter.clone();
        let _ = slint::spawn_local(async move {
            if let Some(chooser) = chooser.upgrade() {
                let attached = chooser
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::prepare_action_window(handle)
                            .and_then(|()| presenter.attach(handle))
                            .is_ok()
                    });
                if !attached {
                    rust_app_core::diagnostics::log_line(
                        "selection_chooser_overlay transition=failed stage=attach",
                    );
                }
            }
        });
    }

    palette.set_prepared(false);
    palette
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if palette.show().is_ok() {
        let palette = palette.as_weak();
        let presenter = palette_presenter.clone();
        let _ = slint::spawn_local(async move {
            if let Some(palette) = palette.upgrade() {
                let attached = palette
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .is_some_and(|handle| {
                        rust_platform_windows::prepare_action_window(handle)
                            .and_then(|()| presenter.attach(handle))
                            .is_ok()
                    });
                if !attached {
                    rust_app_core::diagnostics::log_line(
                        "action_palette_overlay transition=failed stage=attach",
                    );
                }
            }
        });
    }
}
