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

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{Fact, HistoryRow, HotkeyRow, ModelRow, ProviderRow, SettingsWindow};

/// Which page to open, from `--settings-preview=4`. Looking at one page at a
/// time is how a page gets looked at.
pub(crate) fn requested_page() -> i32 {
    std::env::args()
        .find_map(|argument| {
            argument
                .strip_prefix("--settings-preview=")
                .and_then(|value| value.parse::<i32>().ok())
        })
        .unwrap_or(0)
        .clamp(0, 7)
}

pub(crate) fn run_settings_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.SettingsPreview")?;
    let settings = SettingsWindow::new()?;
    settings.set_app_version("0.5.27".into());
    settings.set_today(rust_platform_windows::local_date_line().into());
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
    settings.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
    settings.show()?;
    // The same native dressing the real window gets, so the preview shows the
    // whole thing including its title bar rather than the page inside a piece
    // of chrome that belongs to a different design.
    crate::configure_standard_app_window(&settings, "Settings preview");
    slint::run_event_loop()?;
    Ok(())
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
                settings.set_selected_visualizer(index);
                settings.set_visuals_native_surface(index == 4 || index == 5);
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
            secondary: "".into(),
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
            secondary: "".into(),
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
            secondary: "".into(),
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
            secondary: "".into(),
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
            secondary: "".into(),
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
    settings.set_remote_transcription_enabled(true);
    settings.set_provider_rows(ModelRc::new(VecModel::from(vec![
        ProviderRow {
            id: "cloudflare".into(),
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
            id: "groq".into(),
            title: "Groq".into(),
            detail:
                "Key \u{2022}\u{2022}\u{2022}\u{2022}9c07 \u{b7} Free tier: 2 000 requests a day"
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
            id: "local".into(),
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
            id: "deepgram".into(),
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
            id: "custom".into(),
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
    ])));
}

fn fill_visuals(settings: &SettingsWindow) {
    settings.set_visuals_subtitle("Flow sphere \u{b7} shown while recording".into());
    settings.set_visuals_caption("A point cloud that accelerates on voice onset".into());
    settings.set_visuals_note(
        "Drawn on its own native surface, so it can be transparent per pixel.".into(),
    );
    settings.set_visuals_native_surface(true);
    settings.set_selected_visualizer(5);
}

fn fill_history(settings: &SettingsWindow) {
    settings.set_history_scope("12 kept \u{b7} 3 went to a service".into());
    settings.set_history_rows(ModelRc::new(VecModel::from(vec![
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
    ])));
}

fn fill_hotkeys(settings: &SettingsWindow) {
    const FIXED: &str = "Fixed in this version";
    settings.set_hotkeys_summary("Four of five held \u{b7} pause is not implemented".into());
    settings.set_hotkey_rows(ModelRc::new(VecModel::from(vec![
        HotkeyRow {
            keys: "Ctrl + Alt + Space".into(),
            name: "Dictate".into(),
            detail: "Records while held \u{b7} transcribes and inserts on release".into(),
            state: "Held".into(),
            tone: "active".into(),
            enabled: true,
            locked_reason: FIXED.into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            keys: "Ctrl + Alt + V".into(),
            name: "Insert last".into(),
            detail: "Places the newest transcript where the caret is now".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            locked_reason: FIXED.into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            keys: "Ctrl + Alt + K".into(),
            name: "Correct layout".into(),
            detail: "Offers RU/EN candidates for the selection \u{b7} never automatic".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            locked_reason: FIXED.into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
        HotkeyRow {
            keys: "Ctrl + Alt + F".into(),
            name: "Act on selection".into(),
            detail: "Captures the selection once and acts on it".into(),
            state: "Held".into(),
            tone: "resting".into(),
            enabled: true,
            locked_reason: FIXED.into(),
            mode: "text-action".into(),
            mode_value: "Decide from the text".into(),
            mode_key: "adaptive".into(),
        },
        HotkeyRow {
            keys: "Ctrl + Alt + P".into(),
            name: "Pause input features".into(),
            detail: "Not implemented in this version".into(),
            state: "Not held".into(),
            tone: "off".into(),
            enabled: false,
            locked_reason: "Not implemented in this version".into(),
            mode: "".into(),
            mode_value: "".into(),
            mode_key: "".into(),
        },
    ])));
}
