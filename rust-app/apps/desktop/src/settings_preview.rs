//! The settings window, filled with representative rows and no workers behind
//! it.
//!
//! Every page has states the running application only reaches occasionally — a
//! download halfway down, a key the service refused, a service below the local
//! engine that the router will never ask. Waiting for those to occur by
//! accident is not a way to design them, and a screen that has only ever been
//! seen in its resting state has not been seen.
//!
//! This is the same argument the control sheet in `design/concept-states`
//! makes, applied to the whole window.

use std::error::Error;

use std::{cell::RefCell, rc::Rc};

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    Fact, HistoryRow, HotkeyRow, ModelRow, ProviderRow, SettingsWindow, TranslatorWindow,
    visualizer::{
        BroadcastMicPalette, CircularSpectrumPalette, INDICATOR_FRAME_INTERVAL, VisualizerStyle,
        WritingSlateSurface, new_settings_preview,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranslatorPreviewState {
    Empty,
    Processing,
    Result,
    Unavailable,
    Error,
}

pub(crate) fn requested_translator_state() -> Option<TranslatorPreviewState> {
    std::env::args().find_map(|argument| {
        let value = argument.strip_prefix("--translator-preview=")?;
        match value {
            "empty" => Some(TranslatorPreviewState::Empty),
            "processing" => Some(TranslatorPreviewState::Processing),
            "result" => Some(TranslatorPreviewState::Result),
            "unavailable" => Some(TranslatorPreviewState::Unavailable),
            "error" => Some(TranslatorPreviewState::Error),
            _ => None,
        }
    })
}

pub(crate) fn run_translator_preview() -> Result<(), Box<dyn Error>> {
    let state = requested_translator_state().unwrap_or(TranslatorPreviewState::Empty);
    let requested_locale = std::env::args()
        .find_map(|argument| argument.strip_prefix("--ui-locale=").map(str::to_owned))
        .and_then(|code| crate::localization::UiLocale::from_code(&code))
        .unwrap_or(crate::localization::UiLocale::English);
    let system_locale = rust_platform_windows::user_locale_name();
    let locale = crate::localization::active_locale(requested_locale);
    // Constructing the first component registers Slint's embedded catalogs;
    // selecting a bundle before that point returns NoTranslationsBundled.
    let catalog_seed = TranslatorWindow::new()?;
    slint::select_bundled_translation(requested_locale.resolved_code(&system_locale))?;
    let translator = TranslatorWindow::new()?;
    drop(catalog_seed);

    let source = "SayFlex keeps the source and result visible while you review a translation.";
    match state {
        TranslatorPreviewState::Empty => {
            translator.set_status_text(
                crate::localization::text(
                    locale,
                    crate::localization::UiText::ChooseTranslationModel,
                )
                .into(),
            );
        }
        TranslatorPreviewState::Processing => {
            translator.set_source_text(source.into());
            translator.set_processing(true);
            translator.set_status_text(
                crate::localization::text(locale, crate::localization::UiText::TranslatingNow)
                    .into(),
            );
        }
        TranslatorPreviewState::Result => {
            translator.set_source_text(source.into());
            translator.set_result_text(
                "SayFlex показывает исходный текст и перевод, пока вы проверяете результат.".into(),
            );
            translator.set_status_text(
                crate::localization::render(
                    locale,
                    crate::localization::UiText::TranslationCompletedProvider,
                    &[("provider", "DeepL")],
                )
                .into(),
            );
        }
        TranslatorPreviewState::Unavailable => {
            translator.set_source_text(source.into());
            translator.set_status_text(
                crate::localization::text(
                    locale,
                    crate::localization::UiText::NoTranslationProviderReady,
                )
                .into(),
            );
        }
        TranslatorPreviewState::Error => {
            translator.set_source_text(source.into());
            translator.set_status_text(
                crate::localization::render(
                    locale,
                    crate::localization::UiText::TechnicalFailure,
                    &[("code", "service"), ("message", "request refused")],
                )
                .into(),
            );
        }
    }
    translator.on_translate(|| {});
    translator.on_copy_result(|| {});
    let weak = translator.as_weak();
    translator.on_swap_text(move || {
        if let Some(translator) = weak.upgrade() {
            let source = translator.get_source_text();
            translator.set_source_text(translator.get_result_text());
            translator.set_result_text(source);
        }
    });
    translator.show()?;
    crate::configure_standard_app_window(&translator, "Translator preview");
    slint::run_event_loop()?;
    Ok(())
}

/// Which page to open, from `--settings-preview=4`. Looking at one page at a
/// time is how a page gets looked at.
pub(crate) fn requested_page() -> i32 {
    let asked = std::env::args()
        .find_map(|argument| {
            argument
                .strip_prefix("--settings-preview=")
                .and_then(|value| value.parse::<i32>().ok())
        })
        .unwrap_or(0)
        // 9 is History again, with nothing in it — the state most people see
        // first, and the one easiest to leave undrawn.
        .clamp(0, 9);
    if asked == 9 { HISTORY_PAGE } else { asked }
}

/// Where History sits in the navigation.
const HISTORY_PAGE: i32 = 6;

pub(crate) fn run_settings_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.SettingsPreview")?;
    let settings = SettingsWindow::new()?;
    if let Some(code) = std::env::args()
        .find_map(|argument| argument.strip_prefix("--ui-locale=").map(str::to_owned))
        && let Some(locale) = crate::localization::UiLocale::from_code(&code)
    {
        let system_locale = rust_platform_windows::user_locale_name();
        slint::select_bundled_translation(locale.resolved_code(&system_locale))?;
        settings.set_ui_locale(locale.code().into());
        settings.set_ui_locale_label(
            if locale == crate::localization::UiLocale::System {
                crate::localization::text(
                    crate::localization::active_locale(locale),
                    crate::localization::UiText::SystemLanguage,
                )
            } else {
                locale.display_name()
            }
            .into(),
        );
    }
    settings.set_app_version(env!("CARGO_PKG_VERSION").into());
    let preview_locale = crate::localization::UiLocale::from_code(&settings.get_ui_locale())
        .map(crate::localization::active_locale)
        .unwrap_or(crate::localization::UiLocale::English);
    settings
        .set_today(rust_platform_windows::local_date_line_for_locale(preview_locale.code()).into());
    settings.set_runtime_status("SYSTEM READY".into());
    settings.set_active_page(requested_page());

    fill_overview(&settings);
    fill_dictation(&settings);
    fill_translation(&settings);
    fill_models(&settings);
    fill_providers(&settings);
    fill_visuals(&settings);
    fill_history(&settings);
    fill_hotkeys(&settings);

    wire(&settings);
    let animation = new_settings_preview(&settings);
    let animation_timer = start_visual_preview_animation(settings.as_weak(), animation);
    settings.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
    settings.show()?;
    // The same native dressing the real window gets, so the preview shows the
    // whole thing including its title bar rather than the page inside a piece
    // of chrome that belongs to a different design.
    crate::configure_standard_app_window(&settings, "Settings preview");
    slint::run_event_loop()?;
    animation_timer.stop();
    Ok(())
}

fn start_visual_preview_animation(
    settings: slint::Weak<SettingsWindow>,
    animation: Rc<RefCell<crate::visualizer::IndicatorAnimation>>,
) -> Timer {
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, INDICATOR_FRAME_INTERVAL, move || {
        if let Some(settings) = settings.upgrade() {
            animation.borrow_mut().tick_settings_preview(&settings);
        }
    });
    timer
}

/// The preview answers its own controls.
///
/// Not to imitate the application — nothing here talks to a worker — but
/// because a control that renders and does nothing is exactly the failure this
/// preview exists to catch, and a still picture cannot catch it. The rules are
/// the same ones the real handlers follow, kept deliberately short.
fn wire(settings: &SettingsWindow) {
    {
        let weak = settings.as_weak();
        settings.on_move_provider(move |from, to| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            let rows = settings.get_provider_rows();
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            let mut moved: Vec<ProviderRow> = rows.iter().collect();
            if from >= moved.len() || from == to {
                return;
            }
            let to = to.min(moved.len() - 1);
            let row = moved.remove(from);
            moved.insert(to, row);
            settings.set_provider_rows(ModelRc::new(VecModel::from(moved)));
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_toggle_provider(move |index| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            let rows = settings.get_provider_rows();
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut toggled: Vec<ProviderRow> = rows.iter().collect();
            let Some(row) = toggled.get_mut(index) else {
                return;
            };
            row.enabled = !row.enabled;
            row.state = if row.enabled {
                "Standing by"
            } else {
                "Switched off"
            }
            .into();
            row.tone = if row.enabled { "resting" } else { "off" }.into();
            settings.set_provider_rows(ModelRc::new(VecModel::from(toggled)));
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_use_provider_only(move |index| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            let rows = settings.get_provider_rows();
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut selected: Vec<ProviderRow> = rows.iter().collect();
            let Some(section) = selected.get(index).map(|row| row.section.clone()) else {
                return;
            };
            for (candidate, row) in selected.iter_mut().enumerate() {
                if row.section == section {
                    row.enabled = candidate == index;
                    row.state = if row.enabled {
                        "Standing by"
                    } else {
                        "Switched off"
                    }
                    .into();
                    row.tone = if row.enabled { "resting" } else { "off" }.into();
                }
            }
            settings.set_provider_rows(ModelRc::new(VecModel::from(selected)));
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_translation_delivery(move |value| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            settings.set_translation_delivery(value.clone());
            settings.set_translation_delivery_label(
                match value.as_str() {
                    "replace" => "Replace the selection",
                    "copy" => "Copy it",
                    _ => "Show a preview",
                }
                .into(),
            );
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_text_action_mode(move |value| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            settings.set_text_action_mode(value.clone());
            settings.set_text_action_mode_label(
                if value == "palette" {
                    "Always ask"
                } else {
                    "Decide from the text"
                }
                .into(),
            );
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_dictation_language(move |value| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            settings.set_dictation_language(value.clone());
            settings.set_dictation_language_label(
                rust_protocol::DictationLanguage::from_code(&value)
                    .unwrap_or_default()
                    .display_name()
                    .into(),
            );
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_output_mode(move |value| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            settings.set_output_mode(value.clone());
            settings.set_output_mode_label(
                if value == "english" {
                    "Translate to English"
                } else {
                    "Original language"
                }
                .into(),
            );
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_release_mode(move |value| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            settings.set_release_mode(value.clone());
            settings.set_release_mode_label(
                if value == "efficient" {
                    "Efficient"
                } else {
                    "Fastest"
                }
                .into(),
            );
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_speech_backend(move |index| {
            if let Some(settings) = weak.upgrade() {
                settings.set_speech_backend_index(index);
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_visualizer(move |index| {
            if let Some(settings) = weak.upgrade() {
                let Some(style) = VisualizerStyle::from_index(index) else {
                    return;
                };
                settings.set_selected_visualizer(style.index());
                crate::settings_view::sync_visualizer_copy(&settings, style);
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_broadcast_mic_palette(move |index| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            if let Some(palette) = BroadcastMicPalette::from_index(index) {
                settings.set_selected_broadcast_mic_palette(palette.index());
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_circular_palette(move |index| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            if let Some(palette) = CircularSpectrumPalette::from_index(index) {
                settings.set_selected_circular_palette(palette.index());
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_flow_sphere_palette(move |index| {
            if let Some(settings) = weak.upgrade() {
                settings.set_selected_flow_sphere_palette(index.clamp(0, 1));
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_select_writing_slate_surface(move |index| {
            let Some(settings) = weak.upgrade() else {
                return;
            };
            if let Some(surface) = WritingSlateSurface::from_index(index) {
                settings.set_selected_writing_slate_surface(surface.index());
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_translation_blur(move |value| {
            if let Some(settings) = weak.upgrade() {
                settings.set_translation_blur_radius(value.clamp(0, 8));
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_history_enabled(move |on| {
            if let Some(settings) = weak.upgrade() {
                settings.set_history_enabled(on);
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_translation_enabled(move |on| {
            if let Some(settings) = weak.upgrade() {
                settings.set_translation_enabled(on);
            }
        });
    }
    {
        let weak = settings.as_weak();
        settings.on_set_translation_warm(move |on| {
            if let Some(settings) = weak.upgrade() {
                settings.set_translation_warm(on);
            }
        });
    }
}

fn fill_overview(settings: &SettingsWindow) {
    settings.set_overview_statement("Ready to dictate".into());
    settings.set_overview_sub("Hold Ctrl + Alt + Space anywhere in Windows.".into());
    settings.set_overview_facts(ModelRc::new(VecModel::from(vec![
        Fact {
            label: "SPEECH ENGINE".into(),
            value: "CUDA".into(),
            detail: "preference: Auto".into(),
            signal: false,
        },
        Fact {
            label: "SPEECH MODEL".into(),
            value: "Whisper Large v3 Turbo".into(),
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: "TRANSLATION".into(),
            value: "warm".into(),
            detail: "".into(),
            signal: false,
        },
        Fact {
            label: "HISTORY".into(),
            value: "12 kept on this device".into(),
            detail: "".into(),
            signal: false,
        },
    ])));
    settings.set_overview_note("A model is downloading. It resumes if it is interrupted.".into());
    settings.set_overview_note_action("Details".into());
    settings.set_overview_note_signal(true);
}

fn fill_dictation(settings: &SettingsWindow) {
    settings.set_dictation_subtitle("Local speech \u{b7} ready".into());
    settings.set_dictation_readout("Ready".into());
    settings.set_dictation_caption("hold Ctrl + Alt + Space".into());
    settings.set_speech_backend_detail(
        "CUDA verified by warm-up. The preference above is where the search starts.".into(),
    );
    settings.set_speech_backend_verified(true);
    settings.set_speech_backend_index(1);
    settings.set_output_mode_label("Original language".into());
    settings.set_release_mode_label("Fastest".into());
}

fn fill_translation(settings: &SettingsWindow) {
    settings.set_translation_model_name("Gemma 3 4B".into());
    settings.set_translation_ready(true);
    settings.set_translation_enabled(true);
    settings.set_translation_warm(true);
    settings.set_translation_delivery_label("Show a preview".into());
    settings.set_text_action_mode_label("Decide from the text".into());
}

/// One of each thing a model row can be doing, including the two that only
/// appear while something is running.
fn fill_models(settings: &SettingsWindow) {
    settings.set_models_summary("2 installed".into());
    settings.set_model_rows(ModelRc::new(VecModel::from(vec![
        ModelRow {
            catalog_id: "whisper-large-v3-turbo".into(),
            section: "Speech".into(),
            title: "Whisper Large v3 Turbo".into(),
            detail: "Fastest large model \u{b7} recommended".into(),
            size: "1.6 GB".into(),
            state: "In use".into(),
            tone: "active".into(),
            action: "".into(),
            action_key: "".into(),
            removable: true,
            remove_label: "Remove".into(),
            remove_question: "Remove this model?".into(),
            removing: false,
            selected: true,
            installed: true,
            progress: 0.0,
        },
        ModelRow {
            catalog_id: "whisper-large-v3".into(),
            section: "Speech".into(),
            title: "Whisper Large v3".into(),
            detail: "Most accurate, slower".into(),
            size: "3.1 GB".into(),
            state: "43 %".into(),
            tone: "working".into(),
            action: "Cancel".into(),
            action_key: "cancel".into(),
            removable: false,
            remove_label: "Remove".into(),
            remove_question: "Remove this model?".into(),
            removing: false,
            selected: false,
            installed: false,
            progress: 0.43,
        },
        ModelRow {
            catalog_id: "whisper-medium".into(),
            section: "Speech".into(),
            title: "Whisper Medium".into(),
            detail: "Half the memory".into(),
            size: "1.5 GB".into(),
            state: "Installed".into(),
            tone: "resting".into(),
            action: "Use".into(),
            action_key: "use".into(),
            removable: true,
            remove_label: "Remove".into(),
            remove_question: "Remove this model?".into(),
            removing: false,
            selected: false,
            installed: true,
            progress: 0.0,
        },
        ModelRow {
            catalog_id: "whisper-small".into(),
            section: "Speech".into(),
            title: "Whisper Small".into(),
            detail: "For older machines".into(),
            size: "480 MB".into(),
            state: "Not installed".into(),
            tone: "off".into(),
            action: "Install".into(),
            action_key: "install".into(),
            removable: false,
            remove_label: "Remove".into(),
            remove_question: "Remove this model?".into(),
            removing: false,
            selected: false,
            installed: false,
            progress: 0.0,
        },
        // The translator is in the same list, under its own label.
        ModelRow {
            catalog_id: "gemma-3-4b".into(),
            section: "Translation".into(),
            title: "Gemma 3 4B".into(),
            detail: "Balanced \u{b7} Gemma licence \u{b7} recommended for your system language"
                .into(),
            size: "2.5 GB".into(),
            state: "In use".into(),
            tone: "active".into(),
            action: "".into(),
            action_key: "".into(),
            removable: true,
            remove_label: "Remove".into(),
            remove_question: "Remove this model?".into(),
            removing: false,
            selected: true,
            installed: true,
            progress: 0.0,
        },
    ])));
}

/// A refused key, a service in use, and one below the local engine that the
/// router will never reach.
fn fill_providers(settings: &SettingsWindow) {
    settings.set_provider_claim("Audio leaves this device for Groq".into());
    settings.set_audio_leaves_device(true);
    settings.set_text_leaves_device(false);
    settings.set_departure_badge("AUDIO LEAVES DEVICE".into());
    settings.set_provider_rows(ModelRc::new(VecModel::from(vec![
        ProviderRow {
            section: "SPEECH".into(),
            id: "speech:cloudflare".into(),
            title: "Cloudflare Workers AI".into(),
            detail: "Key refused: the service rejected this token".into(),
            enabled: true,
            available: false,
            needs_key: true,
            key_hint: "\u{2022}\u{2022}\u{2022}\u{2022}4f21".into(),
            field_label: "Account identifier".into(),
            field_value: "a1b2c3d4".into(),
            state: "Key refused".into(),
            tone: "refused".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "SPEECH".into(),
            id: "speech:groq".into(),
            title: "Groq".into(),
            detail: "Key \u{2022}\u{2022}\u{2022}\u{2022}9c07 \u{b7} Cloud speech \u{b7} availability follows your Groq account"
                .into(),
            enabled: true,
            available: true,
            needs_key: true,
            key_hint: "\u{2022}\u{2022}\u{2022}\u{2022}9c07".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "In use".into(),
            tone: "active".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "SPEECH".into(),
            id: "speech:local".into(),
            title: "This computer".into(),
            detail: "Runs on this computer \u{b7} nothing leaves it".into(),
            enabled: true,
            available: true,
            needs_key: false,
            key_hint: "".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "Standing by".into(),
            tone: "resting".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "SPEECH".into(),
            id: "speech:deepgram".into(),
            title: "Deepgram".into(),
            detail: "Key \u{2022}\u{2022}\u{2022}\u{2022}1f30 \u{b7} $200 of credit, no expiry"
                .into(),
            enabled: true,
            available: true,
            needs_key: true,
            key_hint: "\u{2022}\u{2022}\u{2022}\u{2022}1f30".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "Never reached".into(),
            tone: "off".into(),
            unreachable: true,
        },
        ProviderRow {
            section: "SPEECH".into(),
            id: "speech:custom".into(),
            title: "Custom endpoint".into(),
            detail: "Any OpenAI-compatible address \u{b7} HTTPS only".into(),
            enabled: false,
            available: false,
            needs_key: true,
            key_hint: "".into(),
            field_label: "Endpoint".into(),
            field_value: "".into(),
            state: "Switched off".into(),
            tone: "off".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "TEXT".into(),
            id: "text:local".into(),
            title: "This computer".into(),
            detail: "Runs on this computer \u{b7} nothing leaves it".into(),
            enabled: true,
            available: true,
            needs_key: false,
            key_hint: "".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "In use".into(),
            tone: "active".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "TEXT".into(),
            id: "text:deepl".into(),
            title: "DeepL".into(),
            detail: "Cloud translation \u{b7} availability follows your DeepL account".into(),
            enabled: false,
            available: false,
            needs_key: true,
            key_hint: "".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "Switched off".into(),
            tone: "off".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "TEXT".into(),
            id: "text:azure".into(),
            title: "Microsoft Azure Translator".into(),
            detail: "Cloud translation \u{b7} region is optional for global resources".into(),
            enabled: false,
            available: false,
            needs_key: true,
            key_hint: "".into(),
            field_label: "Region".into(),
            field_value: "".into(),
            state: "Switched off".into(),
            tone: "off".into(),
            unreachable: false,
        },
        ProviderRow {
            section: "TEXT".into(),
            id: "text:google".into(),
            title: "Google Cloud Translation (unsupported)".into(),
            detail: "Kept from older settings \u{b7} not supported by this version".into(),
            enabled: false,
            available: false,
            needs_key: true,
            key_hint: "".into(),
            field_label: "".into(),
            field_value: "".into(),
            state: "Switched off".into(),
            tone: "off".into(),
            unreachable: false,
        },
    ])));
}

fn fill_visuals(settings: &SettingsWindow) {
    settings.set_selected_visualizer(5);
    crate::settings_view::sync_visualizer_copy(settings, VisualizerStyle::FlowSphere);
}

fn fill_history(settings: &SettingsWindow) {
    // `--settings-preview=9` shows the page with nothing on it: the state most
    // people see first, and the one easiest to leave undrawn.
    if std::env::args().any(|argument| argument == "--settings-preview=9") {
        settings.set_history_scope("Nothing kept yet".into());
        settings.set_history_rows(ModelRc::new(VecModel::from(Vec::new())));
        return;
    }
    settings.set_history_scope("12 kept \u{b7} 3 went to a service".into());
    let mut rows = vec![
        HistoryRow {
            text: "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{43a}\u{430} \u{441}\u{432}\u{44f}\u{437}\u{438}, \u{44d}\u{442}\u{43e} \u{442}\u{435}\u{441}\u{442}\u{43e}\u{432}\u{430}\u{44f} \u{434}\u{438}\u{43a}\u{442}\u{43e}\u{432}\u{43a}\u{430}".into(),
            detail: "ru \u{b7} 3.2s audio \u{b7} 210ms release \u{b7} prepared".into(),
            origin: "local".into(),
        },
        HistoryRow {
            text: "The quick brown fox jumps over the lazy dog".into(),
            detail: "en \u{b7} 2.8s audio \u{b7} 340ms release \u{b7} via Groq".into(),
            origin: "remote".into(),
        },
        HistoryRow {
            text: "Recovered after the recording was interrupted".into(),
            detail: "en \u{b7} 1.4s audio \u{b7} 180ms processing".into(),
            origin: "local".into(),
        },
    ];
    for index in 4..=12 {
        rows.push(HistoryRow {
            text: format!("Representative retained dictation {index}").into(),
            detail: format!(
                "en \u{b7} {}.{}s audio \u{b7} on this device",
                index / 2,
                index % 2
            )
            .into(),
            origin: if index == 7 || index == 11 {
                "remote"
            } else {
                "local"
            }
            .into(),
        });
    }
    settings.set_history_rows(ModelRc::new(VecModel::from(rows)));
}

fn fill_hotkeys(settings: &SettingsWindow) {
    settings.set_hotkeys_summary("Five of five shortcuts active".into());
    settings.set_hotkey_rows(ModelRc::new(VecModel::from(vec![
        HotkeyRow {
            action: "dictate".into(),
            keys: "Ctrl + Alt + Space".into(),
            name: "Dictate".into(),
            detail: "Records while held \u{b7} transcribes and inserts on release".into(),
            state: "Held".into(),
            tone: "active".into(),
            enabled: true,
            toggle_enabled: true,
            is_default: true,
            locked_reason: "".into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            action: "insert_last".into(),
            keys: "Ctrl + Alt + V".into(),
            name: "Insert latest dictation".into(),
            detail: "Places the newest transcript where the caret is now".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            toggle_enabled: true,
            is_default: true,
            locked_reason: "".into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            action: "correct_layout".into(),
            keys: "Ctrl + Alt + K".into(),
            name: "Correct layout".into(),
            detail: "Offers RU/EN candidates for the selection \u{b7} never automatic".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            toggle_enabled: true,
            is_default: true,
            locked_reason: "".into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            action: "translate".into(),
            keys: "Ctrl + Alt + F".into(),
            name: "Act on selection".into(),
            detail: "Captures the selection once and acts on it".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            toggle_enabled: true,
            is_default: true,
            locked_reason: "".into(),
            mode: "text-action".into(),
            mode_value: "Decide from the text".into(),
            mode_key: "adaptive".into(),
        },
        HotkeyRow {
            action: "pause_resume".into(),
            keys: "Ctrl + Alt + P".into(),
            name: "Pause input features".into(),
            detail: "Always available as the recovery shortcut; press again to resume".into(),
            state: "Ready".into(),
            tone: "resting".into(),
            enabled: true,
            toggle_enabled: false,
            is_default: true,
            locked_reason: "".into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
    ])));
}
