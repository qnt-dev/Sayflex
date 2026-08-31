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
    ActionPalette, AppState, AppTray, HistoryRow, MAX_TRANSLATION_BLUR_RADIUS, ModelRow,
    PendingSelection, PendingTranslation, RecordingOverlays, SelectionChooser,
    SelectionTranslationRequest, SettingsWindow, TextActionMode, TranslationDelivery,
    TranslationOrigin, TranslatorWindow, begin_selection_translation,
    configure_standard_app_window, dispatch_or_defer_selection_replacement, flow_sphere,
    native_window_handle,
    overlay_presenter::OverlayPresenter,
    providers_view::{
        PROVIDER_ROW_HEIGHT, apply_provider_chain, provider_display_name, set_provider_field_value,
        sync_providers,
    },
    settings_document::save_settings,
    slint_window_handle,
    speech_backend::SpeechBackendPreference,
    translation_view::{TranslationPreviewRegistry, adaptive_text_panel_height},
    unix_ms,
    visualizer::{
        CircularSpectrumPalette, IndicatorAnimation, VisualizerStyle, WritingSlateSurface,
    },
    worker_supervisor::Workers,
};

pub(crate) fn sync_models(settings: &slint::Weak<SettingsWindow>, state: &Rc<RefCell<AppState>>) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = state
        .borrow()
        .models
        .iter()
        .map(|model| ModelRow {
            title: model.display_name.clone().into(),
            detail: format!(
                "{} · {:.1} GB{}",
                model.description,
                model.size_bytes as f64 / 1_000_000_000.0,
                if model.recommended {
                    " · recommended"
                } else {
                    ""
                }
            )
            .into(),
            state: if model.installed {
                "Installed".into()
            } else {
                "Download".into()
            },
            selected: model.selected,
        })
        .collect::<Vec<_>>();
    settings.set_model_rows(ModelRc::new(VecModel::from(rows)));
}

pub(crate) fn sync_translation_models(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let rows = state
        .borrow()
        .translation_models
        .iter()
        .map(|model| ModelRow {
            title: model.display_name.clone().into(),
            detail: format!(
                "{} · {:.0} MB · {}{}",
                model.description,
                model.size_bytes as f64 / 1_000_000.0,
                model.license,
                if model.recommended {
                    " · recommended for your system language"
                } else {
                    ""
                }
            )
            .into(),
            state: if model.installed {
                "Installed".into()
            } else {
                "Download".into()
            },
            selected: model.selected,
        })
        .collect::<Vec<_>>();
    settings.set_translation_model_rows(ModelRc::new(VecModel::from(rows)));
}

pub(crate) fn sync_action_settings(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    let Some(settings) = settings.upgrade() else {
        return;
    };
    let state = state.borrow();
    settings.set_speech_backend_label(
        format!("Backend: {}", state.speech_backend_preference.label()).into(),
    );
    settings.set_speech_backend_detail(
        state
            .speech_backend_active
            .map_or_else(
                || "No compatible speech backend is active".to_owned(),
                |active| {
                    format!(
                        "Preference: {} · active now: {}",
                        state.speech_backend_preference.label(),
                        active.display_name()
                    )
                },
            )
            .into(),
    );
    settings.set_translation_enabled_label(
        if state.translation_enabled {
            "Translation: enabled"
        } else {
            "Translation: disabled"
        }
        .into(),
    );
    settings.set_layout_enabled_label(
        if state.layout_enabled {
            "Layout correction: enabled"
        } else {
            "Layout correction: disabled"
        }
        .into(),
    );
    settings.set_text_action_mode_label(
        match state.text_action_mode {
            TextActionMode::Adaptive => "Ctrl+Alt+F: adaptive",
            TextActionMode::AlwaysPalette => "Ctrl+Alt+F: always show palette",
        }
        .into(),
    );
    settings.set_translation_delivery_label(
        match state.translation_delivery {
            TranslationDelivery::Preview => "Translation result: preview",
            TranslationDelivery::Replace => "Translation result: replace selection",
            TranslationDelivery::Copy => "Translation result: copy",
        }
        .into(),
    );
    settings.set_translation_warm_label(
        if state.translation_keep_warm {
            "Warm engine: on (fastest)"
        } else {
            "Warm engine: off (less memory)"
        }
        .into(),
    );
    settings.set_translation_blur_radius(state.translation_blur_radius as i32);
    settings.set_release_optimization_label(
        if state.fast_release_previews {
            "Release optimization: fastest"
        } else {
            "Release optimization: efficient"
        }
        .into(),
    );
    settings.set_selected_visualizer(state.visualizer_style.index());
    settings.set_selected_circular_palette(state.circular_spectrum_palette.index());
    settings.set_selected_flow_sphere_palette(state.flow_sphere_palette.index());
    settings.set_selected_writing_slate_surface(state.writing_slate_surface.index());
    settings.set_translation_ready(state.translation_ready);
    settings.set_history_enabled_label(
        if state.history_enabled {
            "History: enabled"
        } else {
            "History: disabled"
        }
        .into(),
    );
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

pub(crate) fn history_row(entry: &HistoryEntry) -> HistoryRow {
    // A line produced by a service says so: which dictations left the machine
    // is not something a user should have to remember.
    let source = entry
        .transcript
        .provider
        .as_deref()
        .map_or_else(String::new, |provider| {
            format!(" · via {}", provider_display_name(provider))
        });
    HistoryRow {
        text: entry.transcript.text.clone().into(),
        detail: if entry.transcript.latency.after_release_ms > 0 {
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
            .into()
        } else {
            format!(
                "{} · {:.1}s audio · {}ms processing{source}",
                entry.transcript.language,
                entry.transcript.audio_duration_ms as f64 / 1000.0,
                entry.transcript.processing_duration_ms
            )
            .into()
        },
    }
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
    let rows = state
        .borrow()
        .history
        .iter()
        .map(history_row)
        .collect::<Vec<_>>();
    history_rows_model(&settings).set_vec(rows);
}

pub(crate) fn sync_output_mode(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
) {
    if let Some(settings) = settings.upgrade() {
        settings.set_output_mode_label(
            match state.borrow().output_mode {
                SpeechOutputMode::OriginalLanguage => "Output: original language",
                SpeechOutputMode::TranslateToEnglish => "Output: translate to English",
            }
            .into(),
        );
    }
}

pub(crate) fn set_runtime_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    if let Some(settings) = settings.upgrade() {
        settings.set_runtime_status(message.into());
    }
}

pub(crate) fn set_indicator_status(settings: &slint::Weak<SettingsWindow>, message: &str) {
    if let Some(settings) = settings.upgrade() {
        settings.set_indicator_status(message.into());
    }
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
                configure_standard_app_window(translator.window(), "Translator");
                translator.window().request_redraw();
            }
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_cycle_speech_backend(move || {
            let preference = {
                let state = state.borrow();
                SpeechBackendPreference::from_index(
                    (state.speech_backend_preference.index() + 1) % 4,
                )
                .unwrap_or_default()
            };
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
                Err(error) => set_indicator_status(
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
        settings.on_select_model(move |index| {
            let Some(model) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().models.get(index).cloned())
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
                sync_output_mode(&settings_weak, &state);
            }
            let _ = save_settings(&state.borrow());
            let _ = workers.speech.send(&SpeechWorkerCommand::SelectModel {
                catalog_id: model.catalog_id,
            });
        });
    }
    {
        let workers = Rc::clone(workers);
        settings.on_refresh_history(move || {
            let _ = workers
                .speech
                .send(&SpeechWorkerCommand::GetHistory { limit: 5 });
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_history_enabled(move || {
            let enabled = {
                let mut state = state.borrow_mut();
                state.history_enabled = !state.history_enabled;
                state.history_enabled
            };
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
            {
                let mut state = state.borrow_mut();
                let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                    return;
                };
                if from >= state.transcription_chain.len() {
                    return;
                }
                let to = to.min(state.transcription_chain.len() - 1);
                let entry = state.transcription_chain.remove(from);
                state.transcription_chain.insert(to, entry);
            }
            apply_provider_chain(&settings_weak, &state, &workers);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_drag_provider(move |index, travel| {
            // The handle reports how far the pointer moved; turning that into
            // rows belongs here, where the row height is defined.
            let moved_rows = (travel / PROVIDER_ROW_HEIGHT).round() as i32;
            if moved_rows == 0 {
                return;
            }
            {
                let mut state = state.borrow_mut();
                let Ok(from) = usize::try_from(index) else {
                    return;
                };
                if from >= state.transcription_chain.len() {
                    return;
                }
                let last = state.transcription_chain.len() - 1;
                let to = (index + moved_rows).clamp(0, last as i32) as usize;
                if to == from {
                    return;
                }
                let entry = state.transcription_chain.remove(from);
                state.transcription_chain.insert(to, entry);
            }
            apply_provider_chain(&settings_weak, &state, &workers);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_provider(move |index| {
            {
                let mut state = state.borrow_mut();
                let Some(entry) = usize::try_from(index)
                    .ok()
                    .and_then(|index| state.transcription_chain.get_mut(index))
                else {
                    return;
                };
                entry.enabled = !entry.enabled;
            }
            apply_provider_chain(&settings_weak, &state, &workers);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_provider_key(move |provider_id, secret| {
            let secret = secret.trim().to_owned();
            // The key goes to the worker, which owns the encrypted store. It
            // is never written to settings and never read back for display.
            let sent = workers
                .speech
                .send(&SpeechWorkerCommand::SetProviderCredential {
                    provider_id: provider_id.to_string(),
                    secret: (!secret.is_empty()).then_some(secret.clone()),
                });
            match sent {
                // Deliberately not "stored": the write happens in the worker,
                // and the row below reports what is actually in the store once
                // it answers. Claiming success here would be a guess.
                Ok(()) => set_indicator_status(
                    &settings_weak,
                    &if secret.is_empty() {
                        format!(
                            "Removing the key for {}…",
                            provider_display_name(&provider_id)
                        )
                    } else {
                        format!(
                            "Storing the key for {}…",
                            provider_display_name(&provider_id)
                        )
                    },
                ),
                Err(error) => set_indicator_status(
                    &settings_weak,
                    &format!("The key could not be stored: {error}"),
                ),
            }
            sync_providers(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_set_provider_field(move |provider_id, value| {
            {
                let mut state = state.borrow_mut();
                let Some(entry) = state
                    .transcription_chain
                    .iter_mut()
                    .find(|entry| entry.id == provider_id.as_str())
                else {
                    return;
                };
                set_provider_field_value(entry, value.to_string());
            }
            apply_provider_chain(&settings_weak, &state, &workers);
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
        settings.on_toggle_output_mode(move || {
            let supports_translation = {
                let state = state.borrow();
                state
                    .models
                    .iter()
                    .find(|model| model.catalog_id == state.selected_model_id)
                    .is_none_or(|model| model.supports_speech_translation)
            };
            if !supports_translation {
                set_indicator_status(
                    &settings_weak,
                    "Whisper Turbo is transcription-only. Translation will use the separate selection translator.",
                );
                return;
            }
            let output_mode = {
                let mut state = state.borrow_mut();
                state.output_mode = match state.output_mode {
                    SpeechOutputMode::OriginalLanguage => SpeechOutputMode::TranslateToEnglish,
                    SpeechOutputMode::TranslateToEnglish => SpeechOutputMode::OriginalLanguage,
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
        settings.on_toggle_release_optimization(move || {
            let enabled = {
                let mut state = state.borrow_mut();
                state.fast_release_previews = !state.fast_release_previews;
                state.fast_release_previews
            };
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
        settings.on_select_translation_model(move |index| {
            let Some(model) = usize::try_from(index)
                .ok()
                .and_then(|index| state.borrow().translation_models.get(index).cloned())
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
                    &settings_weak,
                    &format!("Could not configure translation model: {error}"),
                );
            } else {
                set_indicator_status(
                    &settings_weak,
                    if model.installed {
                        "Loading the selected translation model…"
                    } else {
                        "Downloading and verifying the translation model…"
                    },
                );
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_toggle_translation_enabled(move || {
            {
                let mut state = state.borrow_mut();
                state.translation_enabled = !state.translation_enabled;
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_toggle_layout_enabled(move || {
            {
                let mut state = state.borrow_mut();
                state.layout_enabled = !state.layout_enabled;
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_cycle_text_action_mode(move || {
            {
                let mut state = state.borrow_mut();
                state.text_action_mode = match state.text_action_mode {
                    TextActionMode::Adaptive => TextActionMode::AlwaysPalette,
                    TextActionMode::AlwaysPalette => TextActionMode::Adaptive,
                };
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        settings.on_cycle_translation_delivery(move || {
            {
                let mut state = state.borrow_mut();
                state.translation_delivery = match state.translation_delivery {
                    TranslationDelivery::Preview => TranslationDelivery::Replace,
                    TranslationDelivery::Replace => TranslationDelivery::Copy,
                    TranslationDelivery::Copy => TranslationDelivery::Preview,
                };
            }
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        settings.on_toggle_translation_warm(move || {
            let selected = {
                let mut state = state.borrow_mut();
                state.translation_keep_warm = !state.translation_keep_warm;
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
        settings.on_decrease_translation_blur(move || {
            let radius = {
                let mut state = state.borrow_mut();
                state.translation_blur_radius = state.translation_blur_radius.saturating_sub(1);
                state.translation_blur_radius
            };
            translation_previews.borrow_mut().set_blur_radius(radius);
            let _ = save_settings(&state.borrow());
            sync_action_settings(&settings_weak, &state);
        });
    }
    {
        let settings_weak = settings.as_weak();
        let translation_previews = Rc::clone(translation_previews);
        let state = Rc::clone(state);
        settings.on_increase_translation_blur(move || {
            let radius = {
                let mut state = state.borrow_mut();
                state.translation_blur_radius =
                    (state.translation_blur_radius + 1).min(MAX_TRANSLATION_BLUR_RADIUS);
                state.translation_blur_radius
            };
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
                configure_standard_app_window(settings.window(), "Settings");
                settings.window().request_redraw();
            }
        });
    }
    {
        let translator = translator.as_weak();
        tray.on_open_translator(move || {
            if let Some(translator) = translator.upgrade() {
                let _ = translator.show();
                configure_standard_app_window(translator.window(), "Translator");
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
