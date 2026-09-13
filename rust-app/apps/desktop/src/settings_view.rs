//! The settings window and the other windows the user interacts with directly.
//!
//! Wiring, not deciding: every callback here turns a click into a change of
//! state and a command to a worker, and every `sync_` function turns state back
//! into what the window shows. The rules those changes obey live elsewhere.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

use rust_app_core::{
    LayoutDirection, build_layout_candidates,
    hotkeys::{format_hotkey, validate as validate_hotkeys},
};
use rust_protocol::{
    CompanionRendererCommand, DictationLanguage, HistoryEntry, HotkeyAction, HotkeyBindings,
    HotkeyConfiguration, HotkeyModifiers, HotkeyRejection, HotkeySpec, InputHostCommand, RequestId,
    SpeechOutputMode, SpeechWorkerCommand, TargetSnapshot, TranscriptionProviderEntry,
    TranslationLanguage, TranslationLanguagePolicy, TranslationProviderEntry,
    TranslationWorkerCommand,
};
use slint::{
    ComponentHandle, Model, ModelRc, PhysicalPosition, SharedString, VecModel,
    winit_030::{
        EventResult, WinitWindowAccessor,
        winit::{
            event::{ElementState, MouseScrollDelta, WindowEvent},
            keyboard::{KeyCode, ModifiersState, PhysicalKey},
        },
    },
};

use crate::{
    ActionPalette, AppState, AppTray, Fact, HISTORY_LIMIT, HistoryRow, HotkeyRow,
    MAX_TRANSLATION_BLUR_RADIUS, ModelRow, PendingCredential, PendingSelection, PendingTranslation,
    RecordingOverlays, SelectionChooser, SelectionTranslationRequest, SettingsWindow,
    TextActionMode, TranslationDelivery, TranslationOrigin, TranslatorWindow,
    begin_selection_translation, configure_standard_app_window,
    dispatch_or_defer_selection_replacement, flow_sphere,
    localization::{UiLocale, UiText, active_locale, render, text},
    native_window_handle,
    overlay_presenter::OverlayPresenter,
    providers_view::{
        Kind, SaveRefused, apply_provider_chain, apply_translation_chain,
        begin_provider_draft_save, cancel_provider_draft_save, check_provider_configuration,
        check_translation_provider_configuration, discard_provider_draft, edit_provider_draft,
        locate, row_display_name_localized, split_row_id, sync_providers, update_provider_draft,
    },
    settings_document::save_settings,
    settings_projection::{
        departure_badge_localized, departures, history_row_localized, privacy_claim_localized,
        speech_model_row_localized, tone, translation_model_row_localized,
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

#[derive(Clone, Copy)]
struct SettingsScrollMetrics {
    current: f32,
    minimum: f32,
    maximum: f32,
    set: fn(&SettingsWindow, f32),
}

fn settings_scroll_metrics(settings: &SettingsWindow) -> SettingsScrollMetrics {
    let page = settings.get_active_page();
    let (current, content, shown, set): (f32, f32, f32, fn(&SettingsWindow, f32)) = match page {
        3 => (
            settings.get_models_scroll_y(),
            settings.get_models_scroll_content_height(),
            settings.get_models_scroll_shown_height(),
            SettingsWindow::set_models_scroll_y,
        ),
        4 => (
            settings.get_providers_scroll_y(),
            settings.get_providers_scroll_content_height(),
            settings.get_providers_scroll_shown_height(),
            SettingsWindow::set_providers_scroll_y,
        ),
        6 => (
            settings.get_history_scroll_y(),
            settings.get_history_scroll_content_height(),
            settings.get_history_scroll_shown_height(),
            SettingsWindow::set_history_scroll_y,
        ),
        7 => (
            settings.get_hotkeys_scroll_y(),
            settings.get_hotkeys_scroll_content_height(),
            settings.get_hotkeys_scroll_shown_height(),
            SettingsWindow::set_hotkeys_scroll_y,
        ),
        _ => (
            settings.get_workspace_scroll_y(),
            settings.get_workspace_scroll_content_height(),
            settings.get_workspace_scroll_shown_height(),
            SettingsWindow::set_workspace_scroll_y,
        ),
    };
    SettingsScrollMetrics {
        current,
        minimum: (shown - content).min(0.0),
        maximum: 0.0,
        set,
    }
}

fn handle_mouse_wheel(settings: &SettingsWindow, delta: &MouseScrollDelta) -> bool {
    let Some(delta) = mouse_wheel_delta(delta) else {
        return false;
    };
    if delta == 0.0 {
        return false;
    }
    let metrics = settings_scroll_metrics(settings);
    if metrics.minimum >= metrics.maximum {
        return false;
    }
    let next = immediate_wheel_position(metrics.current, metrics.minimum, metrics.maximum, delta);
    (metrics.set)(settings, next);
    true
}

fn immediate_wheel_position(current: f32, minimum: f32, maximum: f32, delta: f32) -> f32 {
    (current + delta).clamp(minimum, maximum)
}

fn mouse_wheel_delta(delta: &MouseScrollDelta) -> Option<f32> {
    match delta {
        MouseScrollDelta::LineDelta(_, vertical) => Some(*vertical * 60.0),
        // Pixel deltas carry touchpad gesture information. Slint's native
        // Flickable owns those phases and inertia.
        MouseScrollDelta::PixelDelta(_) => None,
    }
}

const fn translation_language_text(language: TranslationLanguage) -> UiText {
    match language {
        TranslationLanguage::English => UiText::EnglishLanguage,
        TranslationLanguage::Russian => UiText::RussianLanguage,
        TranslationLanguage::Ukrainian => UiText::UkrainianLanguage,
        TranslationLanguage::Spanish => UiText::SpanishLanguage,
        TranslationLanguage::PortugueseBrazil => UiText::PortugueseBrazilLanguage,
        TranslationLanguage::German => UiText::GermanLanguage,
        TranslationLanguage::French => UiText::FrenchLanguage,
        TranslationLanguage::Italian => UiText::ItalianLanguage,
        TranslationLanguage::ChineseSimplified => UiText::ChineseSimplifiedLanguage,
        TranslationLanguage::ChineseTraditional => UiText::ChineseTraditionalLanguage,
        TranslationLanguage::Japanese => UiText::JapaneseLanguage,
        TranslationLanguage::Korean => UiText::KoreanLanguage,
        TranslationLanguage::Hindi => UiText::HindiLanguage,
        TranslationLanguage::Arabic => UiText::ArabicLanguage,
    }
}

const fn dictation_language_text(language: DictationLanguage) -> UiText {
    match language {
        DictationLanguage::Auto => UiText::AutoDetect,
        DictationLanguage::English => UiText::EnglishLanguage,
        DictationLanguage::Russian => UiText::RussianLanguage,
        DictationLanguage::Ukrainian => UiText::UkrainianLanguage,
        DictationLanguage::Spanish => UiText::SpanishLanguage,
        DictationLanguage::PortugueseBrazil => UiText::PortugueseBrazilLanguage,
        DictationLanguage::German => UiText::GermanLanguage,
        DictationLanguage::French => UiText::FrenchLanguage,
        DictationLanguage::Italian => UiText::ItalianLanguage,
        DictationLanguage::ChineseSimplified => UiText::ChineseSimplifiedLanguage,
        DictationLanguage::ChineseTraditional => UiText::ChineseTraditionalLanguage,
        DictationLanguage::Japanese => UiText::JapaneseLanguage,
        DictationLanguage::Korean => UiText::KoreanLanguage,
        DictationLanguage::Hindi => UiText::HindiLanguage,
        DictationLanguage::Arabic => UiText::ArabicLanguage,
    }
}

/// Both lists, in the order the page shows them: speech models, then the
/// translator. One list because the page is about models, and because a
/// `ListView` takes a single `for`.
pub(crate) fn sync_models(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let locale = active_locale(state.ui_locale);
    let mut rows: Vec<ModelRow> = state
        .models
        .iter()
        .map(|model| {
            speech_model_row_localized(
                model,
                state.downloads.get(&model.catalog_id).copied(),
                state.speech_loaded_model_id.as_deref() == Some(model.catalog_id.as_str()),
                state.removing_models.contains(&model.catalog_id),
                locale,
            )
        })
        .collect();
    rows.extend(state.translation_models.iter().map(|model| {
        translation_model_row_localized(
            model,
            state.selected_translation_tier == Some(model.tier),
            state.downloads.get(&model.catalog_id).copied(),
            state.translation_loaded_model_id.as_deref() == Some(model.catalog_id.as_str()),
            state.removing_models.contains(&model.catalog_id),
            locale,
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
    let locale = active_locale(state.ui_locale);
    let installed = state.models.iter().filter(|model| model.installed).count()
        + state
            .translation_models
            .iter()
            .filter(|model| model.installed)
            .count();
    settings.set_models_summary(
        match installed {
            0 => text(locale, UiText::NoneInstalledYet).to_owned(),
            1 => text(locale, UiText::OneInstalled).to_owned(),
            many => render(
                locale,
                UiText::ManyInstalled,
                &[("count", &many.to_string())],
            ),
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
    let locale = active_locale(state.ui_locale);

    // ── Dictation ──
    settings.set_speech_backend_index(state.speech_backend_preference.index());
    settings.set_speech_backend_verified(state.speech_ready);
    settings.set_speech_backend_detail(
        state
            .speech_backend_active
            .map_or_else(
                || text(locale, UiText::NoCompatibleSpeechBackend).to_owned(),
                |active| {
                    if state.speech_ready {
                        render(
                            locale,
                            UiText::BackendVerified,
                            &[("backend", active.display_name())],
                        )
                    } else {
                        // Chosen is not the same as proved. The warm-up is what
                        // proves it, and until it finishes the line says so.
                        render(
                            locale,
                            UiText::WarmingBackend,
                            &[("backend", active.display_name())],
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
    settings.set_release_mode_label(
        text(
            locale,
            if previews_on {
                UiText::FastResponse
            } else {
                UiText::LowerResourceUse
            },
        )
        .into(),
    );
    settings.set_release_mode_locked(!previews_possible);
    settings.set_release_mode_note(
        if previews_possible {
            ""
        } else {
            text(locale, UiText::CpuPreviewUnavailable)
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
            SpeechOutputMode::OriginalLanguage => text(locale, UiText::OriginalLanguage),
            SpeechOutputMode::TranslateToEnglish => text(locale, UiText::TranslateToEnglish),
        }
        .into(),
    );
    settings.set_dictation_language(state.dictation_language.code().unwrap_or("auto").into());
    settings.set_dictation_language_label(
        text(locale, dictation_language_text(state.dictation_language)).into(),
    );
    settings.set_dictate_hotkey(format_hotkey(state.hotkeys.dictate.spec).into());
    settings.set_output_mode_locked(!state.speech_translation_supported());
    settings.set_dictation_subtitle(
        if state.speech_path_ready() {
            match state.first_transcription_provider() {
                Some(provider) if provider.id != "local" => {
                    format!(
                        "{} · {}",
                        row_display_name_localized(Kind::Speech, &provider.id, locale),
                        text(locale, UiText::Ready)
                    )
                }
                _ => text(locale, UiText::LocalSpeechReady).to_owned(),
            }
        } else if state.speech_waits_for_local() {
            text(locale, UiText::LocalSpeechStarting).to_owned()
        } else {
            text(locale, UiText::SpeechServiceUnavailable).to_owned()
        }
        .into(),
    );
    settings.set_dictation_readout(
        if state.speech_path_ready() {
            text(locale, UiText::ReadyTitle)
        } else if state.speech_waits_for_local() {
            text(locale, UiText::StartingUp)
        } else {
            text(locale, UiText::NotReady)
        }
        .into(),
    );
    let shortcut = format_hotkey(state.hotkeys.dictate.spec);
    settings.set_dictation_caption(
        render(locale, UiText::HoldShortcut, &[("shortcut", &shortcut)]).into(),
    );

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
            TranslationDelivery::Preview => text(locale, UiText::ShowPreview),
            TranslationDelivery::Replace => text(locale, UiText::ReplaceSelection),
            TranslationDelivery::Copy => text(locale, UiText::CopyIt),
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
            TextActionMode::Adaptive => text(locale, UiText::DecideFromText),
            TextActionMode::AlwaysPalette => text(locale, UiText::AlwaysAsk),
        }
        .into(),
    );
    settings.set_translation_blur_radius(state.translation_blur_radius as i32);
    settings.set_translation_ready(state.translation_path_ready());
    settings.set_translation_primary_language(
        text(
            locale,
            translation_language_text(state.translation_primary_language),
        )
        .into(),
    );
    settings
        .set_translation_primary_language_value(state.translation_primary_language.code().into());
    settings.set_translation_secondary_language(
        text(
            locale,
            translation_language_text(state.translation_secondary_language),
        )
        .into(),
    );
    settings.set_translation_secondary_language_value(
        state.translation_secondary_language.code().into(),
    );

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
    settings.set_hotkeys_summary_signal(!state.hotkey_errors.is_empty());
    settings.set_ui_locale(state.ui_locale.code().into());
    settings.set_ui_locale_label(
        if state.ui_locale == UiLocale::System {
            text(locale, UiText::SystemLanguage)
        } else {
            state.ui_locale.display_name()
        }
        .into(),
    );
    settings.set_today(rust_platform_windows::local_date_line_for_locale(locale.code()).into());

    sync_overview(&settings, &state);
}

/// The subtitle and note beside the visualizer preview.
///
/// Transparent visualizers are drawn on a native surface of their own. That is
/// not decoration, it is why they behave differently, so the page says it.
pub(crate) fn sync_visualizer_copy(settings: &SettingsWindow, style: VisualizerStyle) {
    let locale = UiLocale::from_code(settings.get_ui_locale().as_str())
        .map(active_locale)
        .unwrap_or(UiLocale::English);
    let (name, caption) = match style {
        VisualizerStyle::VoiceOrb => (
            text(locale, UiText::VoiceOrb),
            text(locale, UiText::VisualLevelDetail),
        ),
        VisualizerStyle::SymmetricSpectrum => (
            text(locale, UiText::Spectrum),
            text(locale, UiText::VisualBandsDetail),
        ),
        VisualizerStyle::FluidRibbon => (
            text(locale, UiText::FluidRibbon),
            text(locale, UiText::VisualLineDetail),
        ),
        VisualizerStyle::BroadcastMic => (
            text(locale, UiText::BroadcastMic),
            text(locale, UiText::VisualMicrophoneDetail),
        ),
        VisualizerStyle::CircularSpectrum => (
            text(locale, UiText::Circular),
            text(locale, UiText::VisualRingDetail),
        ),
        VisualizerStyle::FlowSphere => (
            text(locale, UiText::FlowSphere),
            text(locale, UiText::VisualFlowDetail),
        ),
        VisualizerStyle::WritingSlate => (
            text(locale, UiText::WritingSlate),
            text(locale, UiText::VisualWordsDetail),
        ),
        VisualizerStyle::CompanionOrb => (
            text(locale, UiText::CompanionOrb),
            text(locale, UiText::VisualCompanionDetail),
        ),
        VisualizerStyle::Off => (
            text(locale, UiText::Off),
            text(locale, UiText::VisualNoneDetail),
        ),
    };
    let native =
        style.recording_surface() == Some(crate::visualizer::RecordingSurface::Transparent);
    settings.set_visuals_subtitle(
        render(
            locale,
            UiText::VisualShownWhileRecording,
            &[("visual", name)],
        )
        .into(),
    );
    settings.set_visuals_caption(caption.into());
    settings.set_visuals_note(
        if style == VisualizerStyle::Off {
            text(locale, UiText::VisualDisabledDetail)
        } else if style == VisualizerStyle::CompanionOrb {
            text(locale, UiText::VisualCompanionHelp)
        } else if native {
            text(locale, UiText::VisualExternalDetail)
        } else {
            text(locale, UiText::VisualInternalDetail)
        }
        .into(),
    );
}

fn hotkey_rows(state: &AppState) -> Vec<HotkeyRow> {
    let locale = active_locale(state.ui_locale);
    let pending = state.pending_hotkeys.is_some();
    let defaults = HotkeyBindings::default();
    let held = |on: bool| {
        if on {
            (tone::RESTING, text(locale, UiText::Enabled))
        } else {
            (tone::OFF, text(locale, UiText::Off))
        }
    };

    HotkeyAction::ALL
        .into_iter()
        .map(|action| {
            let binding = *state.hotkeys.binding(action);
            let rejection = state.hotkey_errors.get(&action).copied();
            let (row_tone, row_state) = if rejection.is_some() && binding.enabled {
                (tone::WORKING, text(locale, UiText::PreviousShortcutKept))
            } else if rejection.is_some() {
                (tone::REFUSED, text(locale, UiText::Unavailable))
            } else if action == HotkeyAction::PauseResume {
                if state.input_paused {
                    (tone::ACTIVE, text(locale, UiText::Disabled))
                } else {
                    (tone::RESTING, text(locale, UiText::ReadyTitle))
                }
            } else {
                held(binding.enabled)
            };
            let (name, normal_detail) = match action {
                HotkeyAction::Dictate => (
                    text(locale, UiText::Dictation),
                    text(locale, UiText::WhateverYouSpoke),
                ),
                HotkeyAction::InsertLast => (
                    text(locale, UiText::InsertLatestDictation),
                    text(locale, UiText::OnThisDevice),
                ),
                HotkeyAction::CorrectLayout => (
                    text(locale, UiText::CorrectLayout),
                    if binding.enabled {
                        text(locale, UiText::OriginalSelectionActive)
                    } else {
                        text(locale, UiText::SwitchOff)
                    },
                ),
                HotkeyAction::Translate => (
                    text(locale, UiText::TextActions),
                    if binding.enabled {
                        text(locale, UiText::OnSelectedText)
                    } else {
                        text(locale, UiText::SwitchOff)
                    },
                ),
                HotkeyAction::PauseResume => (
                    text(locale, UiText::PauseResumeAction),
                    text(locale, UiText::AlwaysAvailable),
                ),
            };
            let detail = rejection
                .map(|reason| {
                    if reason == HotkeyRejection::RegistrationFailed && binding.enabled {
                        text(locale, UiText::ShortcutAlreadyInUsePreviousKept)
                    } else {
                        hotkey_rejection_detail(locale, reason)
                    }
                })
                .unwrap_or(normal_detail);
            HotkeyRow {
                action: hotkey_action_key(action).into(),
                keys: format_hotkey(binding.spec).into(),
                name: name.into(),
                detail: detail.into(),
                state: if pending {
                    text(locale, UiText::Applying).into()
                } else {
                    row_state.into()
                },
                tone: if pending {
                    tone::WORKING.into()
                } else {
                    row_tone.into()
                },
                enabled: binding.enabled,
                toggle_enabled: !pending && action != HotkeyAction::PauseResume,
                is_default: binding == *defaults.binding(action),
                locked_reason: if pending {
                    text(locale, UiText::WaitingForWindows).into()
                } else {
                    "".into()
                },
                mode: if action == HotkeyAction::Translate {
                    "text-action".into()
                } else {
                    "".into()
                },
                mode_value: if action == HotkeyAction::Translate {
                    match state.text_action_mode {
                        TextActionMode::Adaptive => text(locale, UiText::DecideFromText).into(),
                        TextActionMode::AlwaysPalette => text(locale, UiText::AlwaysAsk).into(),
                    }
                } else {
                    "".into()
                },
                mode_key: if action == HotkeyAction::Translate {
                    match state.text_action_mode {
                        TextActionMode::Adaptive => "adaptive".into(),
                        TextActionMode::AlwaysPalette => "palette".into(),
                    }
                } else {
                    "".into()
                },
            }
        })
        .collect()
}

pub(crate) fn hotkey_rejection_detail(locale: UiLocale, reason: HotkeyRejection) -> &'static str {
    match reason {
        HotkeyRejection::RegistrationFailed => text(locale, UiText::ShortcutAlreadyInUse),
        HotkeyRejection::Busy => text(locale, UiText::FinishActiveDictation),
        HotkeyRejection::Invalid | HotkeyRejection::Duplicate => {
            text(locale, UiText::ShortcutRejected)
        }
    }
}

/// A startup registration conflict is durable row state, not a global
/// operation that should occupy the footer forever. While the capture editor
/// is open the same message is useful next to the user's current action.
pub(crate) fn hotkey_rejection_global_message(
    locale: UiLocale,
    capture_index: i32,
    reason: Option<HotkeyRejection>,
) -> Option<&'static str> {
    (capture_index >= 0)
        .then_some(reason)
        .flatten()
        .map(|reason| hotkey_rejection_detail(locale, reason))
}

fn hotkeys_summary(state: &AppState) -> String {
    let locale = active_locale(state.ui_locale);
    let enabled = HotkeyAction::ALL
        .into_iter()
        .filter(|action| state.hotkeys.binding(*action).enabled)
        .count();
    if state.input_paused {
        render(
            locale,
            UiText::PausedShortcuts,
            &[("count", &enabled.to_string())],
        )
    } else if state.pending_hotkeys.is_some() {
        text(locale, UiText::ApplyingShortcuts).to_owned()
    } else {
        render(
            locale,
            UiText::ActiveShortcuts,
            &[("count", &enabled.to_string())],
        )
    }
}

const fn hotkey_action_at(index: i32) -> Option<HotkeyAction> {
    match index {
        0 => Some(HotkeyAction::Dictate),
        1 => Some(HotkeyAction::InsertLast),
        2 => Some(HotkeyAction::CorrectLayout),
        3 => Some(HotkeyAction::Translate),
        4 => Some(HotkeyAction::PauseResume),
        _ => None,
    }
}

const fn hotkey_action_key(action: HotkeyAction) -> &'static str {
    match action {
        HotkeyAction::Dictate => "dictate",
        HotkeyAction::InsertLast => "insert_last",
        HotkeyAction::CorrectLayout => "correct_layout",
        HotkeyAction::Translate => "translate",
        HotkeyAction::PauseResume => "pause_resume",
    }
}

fn submit_hotkey_bindings(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    bindings: HotkeyBindings,
) -> bool {
    let locale = active_locale(state.borrow().ui_locale);
    if let Err(error) = validate_hotkeys(&bindings) {
        set_status_problem(settings, hotkey_rejection_detail(locale, error.reason));
        return false;
    }
    {
        let state = state.borrow();
        if !state.input_ready {
            set_status_problem(settings, text(locale, UiText::InputServiceNotReady));
            return false;
        }
        if state.pending_hotkeys.is_some() {
            set_status_problem(settings, text(locale, UiText::ShortcutChangeBusy));
            return false;
        }
    }

    let operation_id = RequestId::new();
    if let Err(error) = workers.input.send(&InputHostCommand::SetHotkeys {
        operation_id,
        configuration: HotkeyConfiguration {
            bindings: bindings.clone(),
        },
    }) {
        set_status_problem(
            settings,
            &render(
                locale,
                UiText::CouldNotApplyShortcuts,
                &[("error", &error.to_string())],
            ),
        );
        return false;
    }
    state.borrow_mut().pending_hotkeys = Some((operation_id, bindings));
    state.borrow_mut().hotkey_errors.clear();
    sync_action_settings(settings, state);
    true
}

fn captured_modifiers(state: ModifiersState) -> HotkeyModifiers {
    HotkeyModifiers {
        control: state.control_key(),
        alt: state.alt_key(),
        shift: state.shift_key(),
        windows: state.super_key(),
    }
}

fn virtual_key_for_code(code: KeyCode) -> Option<u16> {
    Some(match code {
        KeyCode::KeyA => u16::from(b'A'),
        KeyCode::KeyB => u16::from(b'B'),
        KeyCode::KeyC => u16::from(b'C'),
        KeyCode::KeyD => u16::from(b'D'),
        KeyCode::KeyE => u16::from(b'E'),
        KeyCode::KeyF => u16::from(b'F'),
        KeyCode::KeyG => u16::from(b'G'),
        KeyCode::KeyH => u16::from(b'H'),
        KeyCode::KeyI => u16::from(b'I'),
        KeyCode::KeyJ => u16::from(b'J'),
        KeyCode::KeyK => u16::from(b'K'),
        KeyCode::KeyL => u16::from(b'L'),
        KeyCode::KeyM => u16::from(b'M'),
        KeyCode::KeyN => u16::from(b'N'),
        KeyCode::KeyO => u16::from(b'O'),
        KeyCode::KeyP => u16::from(b'P'),
        KeyCode::KeyQ => u16::from(b'Q'),
        KeyCode::KeyR => u16::from(b'R'),
        KeyCode::KeyS => u16::from(b'S'),
        KeyCode::KeyT => u16::from(b'T'),
        KeyCode::KeyU => u16::from(b'U'),
        KeyCode::KeyV => u16::from(b'V'),
        KeyCode::KeyW => u16::from(b'W'),
        KeyCode::KeyX => u16::from(b'X'),
        KeyCode::KeyY => u16::from(b'Y'),
        KeyCode::KeyZ => u16::from(b'Z'),
        KeyCode::Digit0 => u16::from(b'0'),
        KeyCode::Digit1 => u16::from(b'1'),
        KeyCode::Digit2 => u16::from(b'2'),
        KeyCode::Digit3 => u16::from(b'3'),
        KeyCode::Digit4 => u16::from(b'4'),
        KeyCode::Digit5 => u16::from(b'5'),
        KeyCode::Digit6 => u16::from(b'6'),
        KeyCode::Digit7 => u16::from(b'7'),
        KeyCode::Digit8 => u16::from(b'8'),
        KeyCode::Digit9 => u16::from(b'9'),
        KeyCode::Space => 0x20,
        KeyCode::Enter => 0x0D,
        KeyCode::Tab => 0x09,
        KeyCode::ArrowLeft => 0x25,
        KeyCode::ArrowUp => 0x26,
        KeyCode::ArrowRight => 0x27,
        KeyCode::ArrowDown => 0x28,
        KeyCode::F1 => 0x70,
        KeyCode::F2 => 0x71,
        KeyCode::F3 => 0x72,
        KeyCode::F4 => 0x73,
        KeyCode::F5 => 0x74,
        KeyCode::F6 => 0x75,
        KeyCode::F7 => 0x76,
        KeyCode::F8 => 0x77,
        KeyCode::F9 => 0x78,
        KeyCode::F10 => 0x79,
        KeyCode::F11 => 0x7A,
        KeyCode::F12 => 0x7B,
        KeyCode::F13 => 0x7C,
        KeyCode::F14 => 0x7D,
        KeyCode::F15 => 0x7E,
        KeyCode::F16 => 0x7F,
        KeyCode::F17 => 0x80,
        KeyCode::F18 => 0x81,
        KeyCode::F19 => 0x82,
        KeyCode::F20 => 0x83,
        KeyCode::F21 => 0x84,
        KeyCode::F22 => 0x85,
        KeyCode::F23 => 0x86,
        KeyCode::F24 => 0x87,
        _ => return None,
    })
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
    let locale = active_locale(state.ui_locale);
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
    settings.set_departure_badge(departure_badge_localized(&leaving, locale).into());
    settings.set_provider_claim(privacy_claim_localized(&leaving, locale).into());

    let installed_speech = state.models.iter().any(|model| model.installed);
    let speech_path_ready = state.speech_path_ready();
    let speech_waits_for_local = state.speech_waits_for_local();
    let (statement, sub) = if speech_path_ready {
        (
            text(locale, UiText::ReadyTitle),
            text(locale, UiText::LocalSpeechReady),
        )
    } else if speech_waits_for_local && !installed_speech {
        (
            text(locale, UiText::NoSpeechModel),
            text(locale, UiText::NoneInstalledYet),
        )
    } else if speech_waits_for_local {
        (
            text(locale, UiText::StartingUp),
            text(locale, UiText::LocalSpeechStarting),
        )
    } else {
        (
            text(locale, UiText::NotReady),
            text(locale, UiText::SpeechServiceUnavailable),
        )
    };
    settings.set_overview_statement(statement.into());
    settings.set_overview_sub(sub.into());

    // The note carries the one thing that wants attention, and its button goes
    // where the thing can be dealt with.
    if speech_waits_for_local && !installed_speech {
        settings.set_overview_note(text(locale, UiText::NoSpeechModel).into());
        settings.set_overview_note_action(text(locale, UiText::Install).into());
        settings.set_overview_note_signal(true);
    } else if downloading_now(state) {
        settings.set_overview_note(text(locale, UiText::ModelDownloadResume).into());
        settings.set_overview_note_action(text(locale, UiText::Refresh).into());
        settings.set_overview_note_signal(true);
    } else if state.translation_enabled && !state.translation_path_ready() {
        settings.set_overview_note(text(locale, UiText::TranslationRouteNotReady).into());
        settings.set_overview_note_action(text(locale, UiText::Refresh).into());
        settings.set_overview_note_signal(false);
    } else {
        settings.set_overview_note("".into());
        settings.set_overview_note_action("".into());
        settings.set_overview_note_signal(false);
    }

    let facts = vec![
        Fact {
            label: text(locale, UiText::SpeechEngine).into(),
            value: state
                .speech_backend_active
                .map_or_else(
                    || text(locale, UiText::None).to_owned(),
                    |active| active.display_name().to_owned(),
                )
                .into(),
            detail: render(
                locale,
                UiText::Preference,
                &[("value", state.speech_backend_preference.label())],
            )
            .into(),
            signal: state.speech_backend_active.is_none(),
        },
        Fact {
            label: text(locale, UiText::SpeechModel).into(),
            value: state
                .models
                .iter()
                .find(|model| model.catalog_id == state.selected_model_id)
                .map(|model| model.display_name.clone())
                .unwrap_or_else(|| text(locale, UiText::NoneSelected).to_owned())
                .into(),
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: text(locale, UiText::Translation).into(),
            value: if !state.translation_enabled {
                text(locale, UiText::Off).into()
            } else if state.translation_path_ready() {
                text(locale, UiText::Ready).into()
            } else {
                text(locale, UiText::Cold).into()
            },
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: text(locale, UiText::History).into(),
            value: if state.history_enabled {
                render(
                    locale,
                    UiText::KeptOnDevice,
                    &[("count", &state.history.len().to_string())],
                )
                .into()
            } else {
                text(locale, UiText::NotKept).into()
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
    locale: crate::localization::UiLocale,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = history_rows_model(&settings);
    rows.insert(0, history_row_localized(entry, active_locale(locale)));
    while rows.row_count() > retained {
        rows.remove(rows.row_count() - 1);
    }
}

pub(crate) fn sync_history(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    let locale = active_locale(state.ui_locale);
    let rows = state
        .history
        .iter()
        .map(|entry| history_row_localized(entry, locale))
        .collect::<Vec<_>>();
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
            (0, _) => text(locale, UiText::NothingKeptYet).to_owned(),
            (total, 0) if total < HISTORY_LIMIT => render(
                locale,
                UiText::AllKeptOnDevice,
                &[("count", &total.to_string())],
            ),
            (total, 0) => render(
                locale,
                UiText::ShowingAllOnDevice,
                &[("count", &total.to_string())],
            ),
            (total, remote) if total < HISTORY_LIMIT => render(
                locale,
                UiText::SomeSentToService,
                &[
                    ("count", &total.to_string()),
                    ("remote", &remote.to_string()),
                ],
            ),
            (total, remote) => render(
                locale,
                UiText::ShowingSomeSent,
                &[
                    ("count", &total.to_string()),
                    ("remote", &remote.to_string()),
                ],
            ),
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
        let locale = active_locale(state.ui_locale);
        settings.set_output_mode(
            match state.output_mode {
                SpeechOutputMode::OriginalLanguage => "original",
                SpeechOutputMode::TranslateToEnglish => "english",
            }
            .into(),
        );
        settings.set_output_mode_label(
            match state.output_mode {
                SpeechOutputMode::OriginalLanguage => text(locale, UiText::OriginalLanguage),
                SpeechOutputMode::TranslateToEnglish => text(locale, UiText::TranslateToEnglish),
            }
            .into(),
        );
        // Whether the choice can be made at all belongs with the choice, so the
        // menu can grey it and say why rather than refusing after the click.
        settings.set_output_mode_locked(!state.speech_translation_supported());
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

fn ready_to_speak_status(state: &AppState) -> String {
    let shortcut = format_hotkey(state.hotkeys.dictate.spec);
    render(
        active_locale(state.ui_locale),
        UiText::ReadyHoldShortcut,
        &[("shortcut", &shortcut)],
    )
}

fn can_refresh_idle_status(state: &AppState) -> bool {
    state.registry.active_session().is_none()
        && state.pending_hotkeys.is_none()
        && state.pending_insert_last.is_none()
        && state.pending_selection.is_none()
        && state.pending_text_action.is_none()
        && state.pending_selection_captures.is_empty()
        && state.selection_release_gates.is_empty()
        && state.pending_translations.is_empty()
        && state.queued_selection_translation.is_none()
        && state.downloads.is_empty()
        && state.pending_credentials.is_empty()
}

/// Rebuild the idle footer after an interface-language change.
///
/// The footer used to retain whatever already-rendered English string happened
/// to be stored in Slint. Active operations keep their precise status; an idle
/// surface is safe to project from the typed Rust state again.
pub(crate) fn sync_idle_status(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let state = state.borrow();
    if !can_refresh_idle_status(&state) {
        return;
    }

    let locale = active_locale(state.ui_locale);
    let (runtime, severity, indicator) = if state.input_paused {
        (
            text(locale, UiText::InputReady).to_owned(),
            Severity::Progress,
            text(locale, UiText::InputPaused).to_owned(),
        )
    } else if !state.input_ready {
        (
            text(locale, UiText::WorkerUnavailable).to_owned(),
            Severity::Trouble,
            text(locale, UiText::InputWorkerNotReady).to_owned(),
        )
    } else if state.speech_path_ready() {
        (
            text(locale, UiText::ReadyTitle).to_owned(),
            Severity::Progress,
            ready_to_speak_status(&state),
        )
    } else if state.speech_waits_for_local() {
        (
            text(locale, UiText::PreparingModel).to_owned(),
            Severity::Progress,
            text(locale, UiText::SpeechModelPreparing).to_owned(),
        )
    } else {
        (
            text(locale, UiText::WorkerUnavailable).to_owned(),
            Severity::Trouble,
            text(locale, UiText::NoTranscriptionProviderReady).to_owned(),
        )
    };
    drop(state);
    set_runtime_status(settings, &runtime);
    set_status(settings, severity, &indicator);
}

pub(crate) fn sync_hotkey_capture_status(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    reason: Option<HotkeyRejection>,
) {
    let capture_index = settings
        .upgrade()
        .map_or(-1, |settings| settings.get_hotkey_capture_index());
    let locale = active_locale(state.borrow().ui_locale);
    if let Some(message) = hotkey_rejection_global_message(locale, capture_index, reason) {
        set_status_problem(settings, message);
    } else {
        sync_idle_status(settings, state);
    }
}

pub(crate) fn show_layout_chooser(
    request_id: RequestId,
    target: TargetSnapshot,
    source_text: String,
    settings: &slint::Weak<SettingsWindow>,
    chooser: &slint::Weak<SelectionChooser>,
    chooser_presenter: &OverlayPresenter,
    state: &Rc<RefCell<AppState>>,
) {
    let candidates = build_layout_candidates(&source_text);
    let locale = active_locale(state.borrow().ui_locale);
    if candidates.is_empty() {
        state
            .borrow_mut()
            .selection_release_gates
            .remove(&request_id);
        set_indicator_status(settings, text(locale, UiText::LayoutConversionUnavailable));
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
        chooser.set_source_panel_height(adaptive_text_panel_height(
            &source_text,
            82,
            2,
            5,
            15.0,
            16.0,
        ));
        chooser.set_source_text(source_text.into());
        chooser.set_candidate_labels(ModelRc::new(VecModel::from(
            candidates
                .iter()
                .map(|candidate| match candidate.direction {
                    LayoutDirection::ToCyrillic => {
                        SharedString::from(text(locale, UiText::EnglishToCyrillic))
                    }
                    LayoutDirection::ToLatin => {
                        SharedString::from(text(locale, UiText::CyrillicToLatin))
                    }
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
        if !state.speech_translation_supported()
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
            &render(
                active_locale(state.borrow().ui_locale),
                UiText::CouldNotConfigureTranslationModel,
                &[("error", &error.to_string())],
            ),
        );
    } else {
        set_indicator_status(
            settings_weak,
            if model.installed {
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::LoadingSelectedTranslationModel,
                )
            } else {
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::DownloadingTranslationModel,
                )
            },
        );
    }
    let _ = save_settings(&state.borrow());
    sync_action_settings(settings_weak, state);
}

fn speech_chain_with_local_disabled(state: &AppState) -> Vec<TranscriptionProviderEntry> {
    let mut chain = if state.transcription_chain.is_empty() {
        state
            .transcription_providers
            .iter()
            .map(|status| TranscriptionProviderEntry {
                id: status.id.clone(),
                enabled: status.enabled,
                endpoint: None,
                model: None,
                account: None,
            })
            .collect()
    } else {
        state.transcription_chain.clone()
    };
    for entry in &mut chain {
        if entry.id == "local" {
            entry.enabled = false;
        }
    }
    chain
}

fn translation_chain_with_local_disabled(state: &AppState) -> Vec<TranslationProviderEntry> {
    let mut chain = if state.translation_chain.is_empty() {
        state
            .translation_providers
            .iter()
            .map(|status| TranslationProviderEntry {
                id: status.id.clone(),
                enabled: status.enabled,
                region: None,
            })
            .collect()
    } else {
        state.translation_chain.clone()
    };
    for entry in &mut chain {
        if entry.id == "local" {
            entry.enabled = false;
        }
    }
    chain
}

/// Starts a destructive model removal only after the local route has been
/// disabled when the selected model owns that route. Commands share one
/// ordered writer per worker, so the unload-causing provider update reaches
/// the actor before the remove request.
fn remove_local_model(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
    catalog_id: &str,
) {
    if state.borrow().removing_models.contains(catalog_id) {
        return;
    }

    let speech = state
        .borrow()
        .models
        .iter()
        .find(|model| model.catalog_id == catalog_id)
        .cloned();
    if let Some(model) = speech {
        if !model.installed {
            return;
        }
        if state.borrow().selected_model_id == catalog_id && state.borrow().local_speech_enabled() {
            let chain = speech_chain_with_local_disabled(&state.borrow());
            if let Err(error) =
                workers
                    .speech
                    .send(&SpeechWorkerCommand::SetTranscriptionProviders {
                        providers: chain.clone(),
                    })
            {
                set_status_problem(
                    settings,
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::ModelRemoveFailed,
                        &[("error", &error.to_string())],
                    ),
                );
                return;
            }
            {
                let mut app = state.borrow_mut();
                app.transcription_chain = chain;
                app.speech_ready = false;
            }
        }
        if let Err(error) = workers.speech.send(&SpeechWorkerCommand::RemoveModel {
            catalog_id: catalog_id.to_owned(),
        }) {
            set_status_problem(
                settings,
                &render(
                    active_locale(state.borrow().ui_locale),
                    UiText::ModelRemoveFailed,
                    &[("error", &error.to_string())],
                ),
            );
            let _ = save_settings(&state.borrow());
            sync_providers(settings, state);
            return;
        }
    } else {
        let translation = state
            .borrow()
            .translation_models
            .iter()
            .find(|model| model.catalog_id == catalog_id)
            .cloned();
        let Some(model) = translation else { return };
        if !model.installed {
            return;
        }
        if state.borrow().selected_translation_tier == Some(model.tier)
            && state.borrow().local_translation_enabled()
        {
            let chain = translation_chain_with_local_disabled(&state.borrow());
            if let Err(error) =
                workers
                    .translation
                    .send(&TranslationWorkerCommand::SetTranslationProviders {
                        providers: chain.clone(),
                    })
            {
                set_status_problem(
                    settings,
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::ModelRemoveFailed,
                        &[("error", &error.to_string())],
                    ),
                );
                return;
            }
            {
                let mut app = state.borrow_mut();
                app.translation_chain = chain;
                app.translation_ready = false;
            }
        }
        if let Err(error) = workers
            .translation
            .send(&TranslationWorkerCommand::RemoveModel { tier: model.tier })
        {
            set_status_problem(
                settings,
                &render(
                    active_locale(state.borrow().ui_locale),
                    UiText::ModelRemoveFailed,
                    &[("error", &error.to_string())],
                ),
            );
            let _ = save_settings(&state.borrow());
            sync_providers(settings, state);
            return;
        }
    }

    state
        .borrow_mut()
        .removing_models
        .insert(catalog_id.to_owned());
    let _ = save_settings(&state.borrow());
    sync_models(settings, state);
    sync_providers(settings, state);
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
    // Capturing a chord and applying it are separate user decisions. The old
    // path submitted the first non-modifier key immediately, leaving no field
    // to confirm what was captured and no Save action to make the boundary
    // clear.
    let hotkey_draft = Rc::new(Cell::new(None::<(i32, HotkeySpec)>));
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
        settings.on_set_ui_locale(move |value| {
            let Some(locale) = crate::localization::UiLocale::from_code(&value) else {
                let current = active_locale(state.borrow().ui_locale);
                set_status_problem(
                    &settings_weak,
                    text(current, UiText::InterfaceLanguageUnavailable),
                );
                return;
            };
            let system_locale = rust_platform_windows::user_locale_name();
            if let Err(error) =
                slint::select_bundled_translation(locale.resolved_code(&system_locale))
            {
                set_status_problem(
                    &settings_weak,
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::CouldNotSwitchInterface,
                        &[("error", &error.to_string())],
                    ),
                );
                return;
            }
            state.borrow_mut().ui_locale = locale;
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
            sync_history(&settings_weak, &state);
            sync_models(&settings_weak, &state);
            sync_providers(&settings_weak, &state);
            sync_idle_status(&settings_weak, &state);
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
                        state.speech_worker_ready = false;
                        state.speech_ready = false;
                        workers.speech.send(&SpeechWorkerCommand::Initialize {
                            selected_model_id: state.selected_model_id.clone(),
                            output_mode: state.output_mode,
                            dictation_language: state.dictation_language,
                            history_enabled: state.history_enabled,
                            fast_release_previews: state.fast_release_previews,
                            prepare_local_model: state.local_speech_enabled(),
                        })
                    };
                    let _ = save_settings(&state.borrow());
                    sync_action_settings(&settings_weak, &state);
                    if let Err(error) = initialization {
                        set_indicator_status(
                            &settings_weak,
                            &render(
                                active_locale(state.borrow().ui_locale),
                                UiText::BackendInitializationFailed,
                                &[("error", &error.to_string())],
                            ),
                        );
                    } else {
                        let locale = active_locale(state.borrow().ui_locale);
                        set_runtime_status(&settings_weak, text(locale, UiText::CheckingBackend));
                        set_indicator_status(
                            &settings_weak,
                            &render(
                                locale,
                                UiText::CheckingNamedBackend,
                                &[("backend", active.display_name())],
                            ),
                        );
                    }
                }
                Err(error) => set_status_problem(
                    &settings_weak,
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::BackendChangeFailed,
                        &[("error", &error.to_string())],
                    ),
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
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_remove_model(move |catalog_id| {
            remove_local_model(&settings_weak, &state, &workers, &catalog_id);
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
                        &render(
                            active_locale(state.borrow().ui_locale),
                            UiText::CouldNotStartCompanion,
                            &[("error", &error.to_string())],
                        ),
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
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::CompanionActive,
                    )
                } else {
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::VisualizerUpdated,
                    )
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
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::BroadcastColourUpdated,
                ),
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
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::CircularPaletteUpdated,
                ),
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
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::FlowPaletteUpdated,
                ),
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
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::SlateSurfaceUpdated,
                ),
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
                text(
                    active_locale(state.borrow().ui_locale),
                    UiText::HistoryCleared,
                ),
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
        settings.on_use_provider_only(move |index| {
            let selected = (|| {
                let mut state = state.borrow_mut();
                let (kind, position) = usize::try_from(index)
                    .ok()
                    .and_then(|index| locate(&state, index))?;
                let configured = match kind {
                    Kind::Speech => state
                        .transcription_chain
                        .get(position)
                        .is_some_and(|entry| {
                            entry.id == "local"
                                || state
                                    .transcription_providers
                                    .iter()
                                    .any(|status| status.id == entry.id && status.has_credential)
                        }),
                    Kind::Text => state.translation_chain.get(position).is_some_and(|entry| {
                        entry.id == "local"
                            || state
                                .translation_providers
                                .iter()
                                .any(|status| status.id == entry.id && status.has_credential)
                    }),
                };
                if !configured {
                    return None;
                }
                match kind {
                    Kind::Speech => {
                        for (candidate, entry) in state.transcription_chain.iter_mut().enumerate() {
                            entry.enabled = candidate == position;
                        }
                    }
                    Kind::Text => {
                        for (candidate, entry) in state.translation_chain.iter_mut().enumerate() {
                            entry.enabled = candidate == position;
                        }
                    }
                }
                Some(kind)
            })();
            let Some(selected) = selected else {
                set_status_problem(
                    &settings_weak,
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::ProviderKeyNeededForOnly,
                    ),
                );
                return;
            };
            match selected {
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
                    Ok(()) => text(
                        active_locale(state.borrow().ui_locale),
                        UiText::InstallCancelled,
                    )
                    .to_owned(),
                    Err(error) => render(
                        active_locale(state.borrow().ui_locale),
                        UiText::CouldNotCancelInstall,
                        &[("error", &error.to_string())],
                    ),
                },
            );
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_edit_provider(move |row_id| {
            edit_provider_draft(&settings_weak, &state, &row_id);
        });
    }
    {
        let state = Rc::clone(state);
        settings.on_update_provider_draft(move |row_id, secret, extra| {
            update_provider_draft(&state, &row_id, &secret, &extra);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_discard_provider_draft(move |row_id| {
            discard_provider_draft(&settings_weak, &state, &row_id);
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
            update_provider_draft(&state, &row_id, &secret, &extra);
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
                Err(reason) => {
                    if let Some(settings) = settings_weak.upgrade() {
                        let locale = active_locale(state.borrow().ui_locale);
                        let message = match reason {
                            SaveRefused::KeyRequired => {
                                text(locale, UiText::ProviderKeyRequired).to_owned()
                            }
                            SaveRefused::FieldRequired(field) => {
                                let field = match field.as_str() {
                                    "Account" => text(locale, UiText::Account),
                                    "Region" => text(locale, UiText::Region),
                                    "Endpoint" => text(locale, UiText::Endpoint),
                                    _ => &field,
                                };
                                render(locale, UiText::ProviderFieldRequired, &[("field", field)])
                            }
                            SaveRefused::InsecureEndpoint => {
                                text(locale, UiText::ProviderEndpointHttps).to_owned()
                            }
                        };
                        settings.set_provider_problem(message.into());
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
            begin_provider_draft_save(&state, &row_id, operation_id);
            state.borrow_mut().pending_credentials.insert(
                operation_id,
                PendingCredential {
                    operation_id,
                    row_id: row_id.clone(),
                    extra: extra.clone(),
                    display_name: row_display_name_localized(
                        kind,
                        &provider_id,
                        active_locale(state.borrow().ui_locale),
                    ),
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
                cancel_provider_draft_save(&state, &row_id, operation_id);
                if let Some(settings) = settings_weak.upgrade() {
                    settings.set_provider_problem(
                        render(
                            active_locale(state.borrow().ui_locale),
                            UiText::ProviderKeySendFailed,
                            &[("error", &error.to_string())],
                        )
                        .into(),
                    );
                    settings.set_provider_problem_for(row_id.clone().into());
                }
                return;
            }

            // Deliberately not "stored": it is on its way. `settle_credential`
            // says what happened when the worker answers.
            set_indicator_status(
                &settings_weak,
                &render(
                    active_locale(state.borrow().ui_locale),
                    UiText::StoringProviderKey,
                    &[(
                        "provider",
                        &row_display_name_localized(
                            kind,
                            &provider_id,
                            active_locale(state.borrow().ui_locale),
                        ),
                    )],
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
                    operation_id,
                    row_id: row_id.clone(),
                    extra: None,
                    display_name: row_display_name_localized(
                        kind,
                        &provider_id,
                        active_locale(state.borrow().ui_locale),
                    ),
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
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::RemovingProviderKey,
                        &[(
                            "provider",
                            &row_display_name_localized(
                                kind,
                                &provider_id,
                                active_locale(state.borrow().ui_locale),
                            ),
                        )],
                    ),
                ),
                Err(error) => set_status_problem(
                    &settings_weak,
                    &render(
                        active_locale(state.borrow().ui_locale),
                        UiText::ProviderKeyRemoveFailed,
                        &[("error", &error.to_string())],
                    ),
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
        settings.on_set_dictation_language(move |value| {
            let Some(language) = DictationLanguage::from_code(&value) else {
                let locale = active_locale(state.borrow().ui_locale);
                set_status_problem(
                    &settings_weak,
                    text(locale, UiText::SpokenLanguageUnsupported),
                );
                return;
            };
            state.borrow_mut().dictation_language = language;
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::SetDictationLanguage { language });
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_output_mode(move |value| {
            let wants_english = value == "english";
            let supports_translation = state.borrow().speech_translation_supported();
            // The menu shows this choice greyed with its reason attached, so
            // reaching here with it means the model changed underneath. Say so
            // rather than silently doing nothing.
            if wants_english && !supports_translation {
                set_status_problem(
                    &settings_weak,
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::ModelCannotTranslate,
                    ),
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
        let workers = Rc::clone(workers);
        settings.on_set_translation_enabled(move |enabled| {
            let mut bindings = state.borrow().hotkeys.clone();
            bindings.binding_mut(HotkeyAction::Translate).enabled = enabled;
            let _ = submit_hotkey_bindings(&settings_weak, &state, &workers, bindings);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_hotkey_enabled(move |index, enabled| {
            let Some(action) = hotkey_action_at(index) else {
                return;
            };
            if action == HotkeyAction::PauseResume {
                set_status_problem(
                    &settings_weak,
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::PauseShortcutRequired,
                    ),
                );
                return;
            }
            let mut bindings = state.borrow().hotkeys.clone();
            bindings.binding_mut(action).enabled = enabled;
            let _ = submit_hotkey_bindings(&settings_weak, &state, &workers, bindings);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let hotkey_draft = Rc::clone(&hotkey_draft);
        settings.on_change_hotkey(move |index| {
            let Some(action) = hotkey_action_at(index) else {
                return;
            };
            if let Some(settings) = settings_weak.upgrade() {
                hotkey_draft.set(None);
                settings.set_hotkey_capture_index(index);
                settings.set_hotkey_capture_value("".into());
                settings.set_hotkey_capture_valid(false);
                let locale = active_locale(state.borrow().ui_locale);
                settings.set_hotkey_capture_label(
                    match action {
                        HotkeyAction::Dictate => text(locale, UiText::DictateAction),
                        HotkeyAction::InsertLast => text(locale, UiText::InsertLatestDictation),
                        HotkeyAction::CorrectLayout => text(locale, UiText::CorrectLayout),
                        HotkeyAction::Translate => text(locale, UiText::ActOnSelectionAction),
                        HotkeyAction::PauseResume => text(locale, UiText::PauseResumeAction),
                    }
                    .into(),
                );
                sync_hotkey_capture_status(&settings_weak, &state, None);
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let hotkey_draft = Rc::clone(&hotkey_draft);
        settings.on_cancel_hotkey_capture(move || {
            if let Some(settings) = settings_weak.upgrade() {
                hotkey_draft.set(None);
                settings.set_hotkey_capture_index(-1);
                settings.set_hotkey_capture_label("".into());
                settings.set_hotkey_capture_value("".into());
                settings.set_hotkey_capture_valid(false);
            }
            sync_hotkey_capture_status(&settings_weak, &state, None);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let hotkey_draft = Rc::clone(&hotkey_draft);
        settings.on_save_hotkey_capture(move || {
            let Some((index, spec)) = hotkey_draft.get() else {
                return;
            };
            let Some(settings) = settings_weak.upgrade() else {
                return;
            };
            if settings.get_hotkey_capture_index() != index {
                return;
            }
            let Some(action) = hotkey_action_at(index) else {
                return;
            };
            let mut bindings = state.borrow().hotkeys.clone();
            bindings.binding_mut(action).spec = spec;
            if submit_hotkey_bindings(&settings_weak, &state, &workers, bindings) {
                // The editor remains visible while Windows owns the decision.
                // Acceptance closes it; rejection leaves it open with the
                // rejected chord and its reason.
                settings.set_hotkey_capture_valid(false);
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_reset_hotkey(move |index| {
            let Some(action) = hotkey_action_at(index) else {
                return;
            };
            let mut bindings = state.borrow().hotkeys.clone();
            bindings.binding_mut(action).spec = HotkeyBindings::default().binding(action).spec;
            let _ = submit_hotkey_bindings(&settings_weak, &state, &workers, bindings);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let hotkey_draft = Rc::clone(&hotkey_draft);
        let modifiers = Rc::new(Cell::new(ModifiersState::default()));
        settings.window().on_winit_window_event(move |_, event| {
            if let WindowEvent::ModifiersChanged(next) = event {
                modifiers.set(next.state());
                return EventResult::Propagate;
            }
            let Some(settings) = settings_weak.upgrade() else {
                return EventResult::Propagate;
            };
            if let WindowEvent::MouseWheel { delta, .. } = event
                && handle_mouse_wheel(&settings, delta)
            {
                return EventResult::PreventDefault;
            }
            let index = settings.get_hotkey_capture_index();
            if index < 0 {
                return EventResult::Propagate;
            }
            let WindowEvent::KeyboardInput { event, .. } = event else {
                return EventResult::Propagate;
            };
            if event.state != ElementState::Pressed || event.repeat {
                return EventResult::PreventDefault;
            }
            let PhysicalKey::Code(code) = event.physical_key else {
                set_status_problem(
                    &settings_weak,
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::InvalidGlobalShortcut,
                    ),
                );
                return EventResult::PreventDefault;
            };
            if code == KeyCode::Escape {
                hotkey_draft.set(None);
                settings.set_hotkey_capture_index(-1);
                settings.set_hotkey_capture_label("".into());
                settings.set_hotkey_capture_value("".into());
                settings.set_hotkey_capture_valid(false);
                sync_hotkey_capture_status(&settings_weak, &state, None);
                return EventResult::PreventDefault;
            }
            if matches!(
                code,
                KeyCode::ControlLeft
                    | KeyCode::ControlRight
                    | KeyCode::AltLeft
                    | KeyCode::AltRight
                    | KeyCode::ShiftLeft
                    | KeyCode::ShiftRight
                    | KeyCode::SuperLeft
                    | KeyCode::SuperRight
            ) {
                return EventResult::PreventDefault;
            }
            let Some(action) = hotkey_action_at(index) else {
                hotkey_draft.set(None);
                settings.set_hotkey_capture_index(-1);
                settings.set_hotkey_capture_value("".into());
                settings.set_hotkey_capture_valid(false);
                sync_hotkey_capture_status(&settings_weak, &state, None);
                return EventResult::PreventDefault;
            };
            let Some(virtual_key) = virtual_key_for_code(code) else {
                set_status_problem(
                    &settings_weak,
                    text(
                        active_locale(state.borrow().ui_locale),
                        UiText::AllowedGlobalShortcutKeys,
                    ),
                );
                return EventResult::PreventDefault;
            };
            let spec = HotkeySpec {
                modifiers: captured_modifiers(modifiers.get()),
                virtual_key,
            };
            hotkey_draft.set(Some((index, spec)));
            settings.set_hotkey_capture_value(format_hotkey(spec).into());

            let mut bindings = state.borrow().hotkeys.clone();
            bindings.binding_mut(action).spec = spec;
            match validate_hotkeys(&bindings) {
                Ok(()) => {
                    settings.set_hotkey_capture_valid(true);
                    sync_hotkey_capture_status(&settings_weak, &state, None);
                }
                Err(error) => {
                    settings.set_hotkey_capture_valid(false);
                    sync_hotkey_capture_status(&settings_weak, &state, Some(error.reason));
                }
            }
            EventResult::PreventDefault
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
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_set_translation_primary_language(move |value| {
            let Some(language) = TranslationLanguage::from_code(value.as_str()) else {
                return;
            };
            {
                let mut state = state.borrow_mut();
                let previous = state.translation_primary_language;
                state.translation_primary_language = language;
                if state.translation_secondary_language == language {
                    state.translation_secondary_language = previous;
                }
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_set_translation_secondary_language(move |value| {
            let Some(language) = TranslationLanguage::from_code(value.as_str()) else {
                return;
            };
            {
                let mut state = state.borrow_mut();
                let previous = state.translation_secondary_language;
                state.translation_secondary_language = language;
                if state.translation_primary_language == language {
                    state.translation_primary_language = previous;
                }
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
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
                    source_language: None,
                    target_language: None,
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
            let source_text = translator.get_source_text().trim().to_owned();
            let locale = active_locale(state.borrow().ui_locale);
            if source_text.is_empty() {
                translator.set_status_text(text(locale, UiText::EnterTextToTranslate).into());
                return;
            }
            let (selected_tier, path_ready, waits_for_local) = {
                let state = state.borrow();
                (
                    state.selected_translation_tier,
                    state.translation_path_ready(),
                    state.translation_waits_for_local(),
                )
            };
            if !(path_ready || selected_tier.is_some() && waits_for_local) {
                translator.set_status_text(text(locale, UiText::NoTranslationProviderReady).into());
                return;
            }
            if !path_ready {
                translator.set_status_text(text(locale, UiText::LocalTranslationLoading).into());
                return;
            }
            let request_id = RequestId::new();
            state.borrow_mut().pending_translations.insert(
                request_id,
                PendingTranslation {
                    origin: TranslationOrigin::Translator,
                    source_text: source_text.clone(),
                    started: Instant::now(),
                    request_started_unix_ms: unix_ms(),
                    first_partial_shown: false,
                },
            );
            translator.set_processing(true);
            translator.set_result_text("".into());
            translator.set_status_text(text(locale, UiText::TranslatingNow).into());
            if let Err(error) = workers
                .translation
                .send(&TranslationWorkerCommand::Translate {
                    request_id,
                    text: source_text,
                    source_language: None,
                    target_language: None,
                    language_policy: TranslationLanguagePolicy {
                        primary: state.borrow().translation_primary_language,
                        secondary: state.borrow().translation_secondary_language,
                    },
                })
            {
                state.borrow_mut().pending_translations.remove(&request_id);
                translator.set_processing(false);
                translator.set_status_text(
                    render(
                        locale,
                        UiText::CouldNotTranslate,
                        &[("error", &error.to_string())],
                    )
                    .into(),
                );
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        translator.on_copy_result(move || {
            let Some(translator) = translator_weak.upgrade() else {
                return;
            };
            let result_text = translator.get_result_text().to_string();
            if !result_text.trim().is_empty() {
                let _ = workers.input.send(&InputHostCommand::CopyText {
                    request_id: RequestId::new(),
                    text: result_text,
                });
                let locale = active_locale(state.borrow().ui_locale);
                translator.set_status_text(text(locale, UiText::CopiedToClipboard).into());
            }
        });
    }
    {
        let translator_weak = translator.as_weak();
        let state = Rc::clone(state);
        translator.on_swap_text(move || {
            if let Some(translator) = translator_weak.upgrade() {
                let source = translator.get_source_text();
                let result = translator.get_result_text();
                translator.set_source_text(result);
                translator.set_result_text(source);
                let locale = active_locale(state.borrow().ui_locale);
                translator.set_status_text(text(locale, UiText::DirectionDetectAgain).into());
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

#[cfg(test)]
mod hotkey_capture_tests {
    use rust_protocol::{HotkeyAction, HotkeyRejection, SpeechOutputMode};
    use slint::winit_030::winit::keyboard::KeyCode;

    use crate::{
        AppState, SavedSettings, SpeechBackend, localization::UiLocale, settings_projection::tone,
    };

    use super::{hotkey_action_at, hotkey_rows, hotkeys_summary, virtual_key_for_code};

    #[test]
    fn every_hotkey_row_has_a_stable_typed_action() {
        assert_eq!(hotkey_action_at(0), Some(HotkeyAction::Dictate));
        assert_eq!(hotkey_action_at(1), Some(HotkeyAction::InsertLast));
        assert_eq!(hotkey_action_at(2), Some(HotkeyAction::CorrectLayout));
        assert_eq!(hotkey_action_at(3), Some(HotkeyAction::Translate));
        assert_eq!(hotkey_action_at(4), Some(HotkeyAction::PauseResume));
        assert_eq!(hotkey_action_at(5), None);
    }

    #[test]
    fn physical_capture_uses_windows_virtual_keys_not_localized_text() {
        assert_eq!(virtual_key_for_code(KeyCode::KeyA), Some(u16::from(b'A')));
        assert_eq!(virtual_key_for_code(KeyCode::Digit7), Some(u16::from(b'7')));
        assert_eq!(virtual_key_for_code(KeyCode::Space), Some(0x20));
        assert_eq!(virtual_key_for_code(KeyCode::F12), Some(0x7B));
        assert_eq!(virtual_key_for_code(KeyCode::AudioVolumeUp), None);
    }

    #[test]
    fn a_windows_registration_failure_is_visible_on_the_rejected_row() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        state.ui_locale = UiLocale::English;
        state.hotkey_errors.insert(
            HotkeyAction::PauseResume,
            HotkeyRejection::RegistrationFailed,
        );
        state.hotkeys.pause_resume.enabled = false;

        let rows = hotkey_rows(&state);
        let row = &rows[4];
        assert_eq!(row.tone, tone::REFUSED);
        assert_eq!(row.state, "Unavailable");
        assert_eq!(row.detail, "Shortcut already in use. Choose another.");
        assert!(row.locked_reason.is_empty());
        assert_eq!(hotkeys_summary(&state), "3 of 5 shortcuts active");
    }

    #[test]
    fn a_failed_replacement_keeps_the_previous_live_shortcut_truthful() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        state.ui_locale = UiLocale::English;
        state.hotkey_errors.insert(
            HotkeyAction::CorrectLayout,
            HotkeyRejection::RegistrationFailed,
        );

        let row = &hotkey_rows(&state)[2];
        assert_eq!(row.tone, tone::WORKING);
        assert_eq!(row.state, "Previous shortcut kept");
        assert_eq!(
            row.detail,
            "That shortcut is already in use. The previous shortcut still works."
        );
        assert!(row.enabled);
        assert_eq!(hotkeys_summary(&state), "4 of 5 shortcuts active");
    }

    #[test]
    fn pause_resume_keeps_its_action_name_in_a_localized_row() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        state.ui_locale = UiLocale::Russian;

        let row = &hotkey_rows(&state)[4];
        assert_eq!(row.name, "Пауза/возобновление");
        assert_eq!(row.detail, "Всегда доступно");
    }
}

#[cfg(test)]
mod status_refresh_tests {
    use rust_protocol::{HotkeyRejection, SpeechOutputMode};

    use crate::{
        AppState, SavedSettings, SpeechBackend, localization::UiLocale,
        settings_projection::Download,
    };

    use super::{can_refresh_idle_status, hotkey_rejection_global_message};

    #[test]
    fn startup_hotkey_conflicts_stay_on_their_row_instead_of_occupying_the_footer() {
        assert_eq!(
            hotkey_rejection_global_message(
                UiLocale::Russian,
                -1,
                Some(HotkeyRejection::RegistrationFailed),
            ),
            None,
        );
        assert_eq!(
            hotkey_rejection_global_message(
                UiLocale::Russian,
                0,
                Some(HotkeyRejection::RegistrationFailed),
            ),
            Some("Сочетание уже занято. Выберите другое."),
        );
        assert_eq!(
            hotkey_rejection_global_message(UiLocale::Russian, 0, None),
            None
        );
    }

    #[test]
    fn a_language_change_does_not_replace_an_active_download_status() {
        let mut state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &SavedSettings::default(),
        );
        assert!(can_refresh_idle_status(&state));

        state.downloads.insert(
            "whisper-large-v3-turbo".to_owned(),
            Download {
                downloaded_bytes: 1,
                total_bytes: 2,
            },
        );

        assert!(!can_refresh_idle_status(&state));
    }
}

#[cfg(test)]
mod wheel_motion_tests {
    use slint::winit_030::winit::{dpi::PhysicalPosition, event::MouseScrollDelta};

    use super::{immediate_wheel_position, mouse_wheel_delta};

    #[test]
    fn detented_wheels_use_the_fast_path_but_touchpads_stay_native() {
        assert_eq!(
            mouse_wheel_delta(&MouseScrollDelta::LineDelta(0.0, -1.0)),
            Some(-60.0)
        );
        assert_eq!(
            mouse_wheel_delta(&MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                0.0, -24.0
            ))),
            None
        );
    }

    #[test]
    fn a_fast_wheel_burst_applies_every_step_immediately() {
        let mut position = 0.0;

        // A detented wheel is discrete input. Ten detents therefore mean
        // exactly 600 logical pixels with no timer-driven tail afterwards.
        for _ in 0..10 {
            position = immediate_wheel_position(position, -13_600.0, 0.0, -60.0);
        }

        assert_eq!(position, -600.0);
    }

    #[test]
    fn the_first_detent_reaches_its_destination_without_a_follow_up_tick() {
        assert_eq!(immediate_wheel_position(0.0, -13_600.0, 0.0, -60.0), -60.0);
    }

    #[test]
    fn wheel_destinations_are_clamped_to_both_ends_of_the_page() {
        assert_eq!(immediate_wheel_position(-490.0, -500.0, 0.0, -60.0), -500.0);
        assert_eq!(immediate_wheel_position(-10.0, -500.0, 0.0, 60.0), 0.0);
    }
}
