//! Reading and writing the settings document.
//!
//! One typed shape, a schema version, and a reader that takes the document
//! field by field so a single unusable value cannot reset every other
//! preference. Kept apart from the interface it configures: this is a file
//! format, and it changes for different reasons than a window does.

use std::{
    error::Error,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rust_protocol::{
    DictationLanguage, HotkeyBindings, SpeechOutputMode, TranscriptionProviderEntry,
    TranslationLanguage, TranslationProviderEntry, TranslationTier,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    AppState, BroadcastMicPalette, CircularSpectrumPalette, DEFAULT_TRANSLATION_BLUR_RADIUS,
    MAX_TRANSLATION_BLUR_RADIUS, TextActionMode, TranslationDelivery, VisualizerStyle,
    WritingSlateSurface, flow_sphere, localization::UiLocale, pulse_matrix,
    speech_backend::SpeechBackendPreference,
};

pub(crate) fn data_root() -> PathBuf {
    rust_app_core::product::data_root()
}

fn settings_path() -> PathBuf {
    data_root().join("settings.json")
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsStorage {
    primary: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedSettings {
    pub(crate) settings: SavedSettings,
    pub(crate) needs_first_run_choice: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FirstRunChoice {
    pub(crate) download_recommended_speech_model: bool,
}

impl SettingsStorage {
    pub(crate) fn new(primary: PathBuf) -> Self {
        Self { primary }
    }

    fn temporary(&self) -> PathBuf {
        self.primary.with_extension("tmp")
    }

    fn previous(&self) -> PathBuf {
        self.primary.with_extension("previous")
    }

    /// Reads the newest valid candidate left by either a completed or an
    /// interrupted replacement. Invalid candidates are retained under a
    /// diagnostic name instead of being silently overwritten.
    #[cfg(test)]
    pub(crate) fn load_recovering(&self) -> SavedSettings {
        self.load_recovering_with_origin().settings
    }

    #[cfg(test)]
    pub(crate) fn load_recovering_with_origin(&self) -> LoadedSettings {
        self.load_recovering_with_installer_language(None)
    }

    #[cfg(test)]
    pub(crate) fn load_recovering_with_installer_locale(
        &self,
        installer_locale: Option<UiLocale>,
    ) -> LoadedSettings {
        let language = installer_locale.map(crate::installer_locale::InstallerLanguage::Legacy);
        self.load_recovering_with_installer_language(language.as_ref())
    }

    pub(crate) fn load_recovering_with_installer_language(
        &self,
        language: Option<&crate::installer_locale::InstallerLanguage>,
    ) -> LoadedSettings {
        let previous = self.previous();
        let temporary = self.temporary();
        let mut candidates = [&self.primary, &previous, &temporary]
            .into_iter()
            .filter_map(|path| {
                let bytes = fs::read(path).ok()?;
                let modified = fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(UNIX_EPOCH);
                Some((path.to_path_buf(), bytes, modified))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, _, modified)| std::cmp::Reverse(*modified));
        let mut invalid = Vec::new();
        let mut valid = Vec::new();
        for (path, bytes, modified) in candidates {
            match parse_settings_checked(&bytes) {
                Ok(settings) => valid.push((path, bytes, modified, settings)),
                Err(error) => invalid.push((path, error)),
            }
        }
        for (path, error) in invalid {
            self.quarantine(&path, &error);
        }

        let Some((source, bytes, _, mut settings)) = valid.into_iter().next() else {
            let mut settings = SavedSettings::default();
            if let Some(language) = language {
                settings.ui_locale = language.locale();
                if let crate::installer_locale::InstallerLanguage::Request {
                    installation_id, ..
                } = language
                {
                    settings.applied_installer_language_id = Some(installation_id.clone());
                }
            }
            // No profile is written until the user confirms the first-run
            // choice. A cancelled prompt sees this durable request again.
            return LoadedSettings {
                settings,
                needs_first_run_choice: true,
            };
        };
        if source != self.primary
            && let Err(error) = self.replace_bytes_atomically(&bytes)
        {
            rust_app_core::diagnostics::log_line(&format!(
                "settings recovery=loaded source={} reinstall=failed error={error}",
                source.display()
            ));
        }
        if let Some(crate::installer_locale::InstallerLanguage::Request {
            locale,
            installation_id,
        }) = language
            && settings.applied_installer_language_id.as_deref() != Some(installation_id.as_str())
        {
            settings.ui_locale = *locale;
            let mut requested = settings.clone();
            requested.applied_installer_language_id = Some(installation_id.clone());
            match self.replace_installer_language_atomically(&requested) {
                Ok(()) => {
                    settings = requested;
                    rust_app_core::diagnostics::log_line(&format!(
                        "installer_language result=applied locale={} installation_id={installation_id}",
                        locale.code()
                    ));
                }
                Err(error) => rust_app_core::diagnostics::log_line(&format!(
                    "installer_language result=save_failed locale={} installation_id={installation_id} retry=pending error={error}",
                    locale.code()
                )),
            }
        }
        LoadedSettings {
            settings,
            needs_first_run_choice: false,
        }
    }

    pub(crate) fn replace_atomically(&self, settings: &SavedSettings) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
        self.replace_bytes_atomically(&bytes)
    }

    fn replace_bytes_atomically(&self, bytes: &[u8]) -> io::Result<()> {
        self.replace_bytes_using_temporary(bytes, &self.temporary())
    }

    fn replace_installer_language_atomically(&self, settings: &SavedSettings) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
        // A failed request must not look consumed when ordinary startup
        // recovery inspects .tmp. Retain its separate temporary for diagnosis;
        // only the atomic primary replacement consumes locale and ID together.
        self.replace_bytes_using_temporary(
            &bytes,
            &self.primary.with_extension("installer-language.tmp"),
        )
    }

    fn replace_bytes_using_temporary(&self, bytes: &[u8], temporary: &Path) -> io::Result<()> {
        if let Some(parent) = self.primary.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);

        let previous = self.previous();
        // A failed ordinary .tmp remains available for startup recovery; a
        // failed installer request uses its separate, ignored temporary.
        // ReplaceFileW replaces an existing backup as part of the same
        // operation; deleting it first would create a window
        // in which a failed replacement had also destroyed the last known-good
        // document.
        rust_platform_windows::replace_file_atomically(&self.primary, temporary, &previous)?;
        Ok(())
    }

    fn quarantine(&self, path: &Path, error: &serde_json::Error) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings");
        let diagnostic = path.with_file_name(format!("{name}.corrupt-{stamp}"));
        match fs::rename(path, &diagnostic) {
            Ok(()) => rust_app_core::diagnostics::log_line(&format!(
                "settings result=unreadable source={} retained={} error={error}",
                path.display(),
                diagnostic.display()
            )),
            Err(rename_error) => rust_app_core::diagnostics::log_line(&format!(
                "settings result=unreadable source={} retain=failed error={error}; rename_error={rename_error}",
                path.display()
            )),
        }
    }
}

/// Version of the on-disk settings document.
///
/// Bump this when a field changes meaning, not when one is added: unknown
/// fields are ignored on read and missing fields fall back to their default,
/// so pure additions stay compatible in both directions.
pub(crate) const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub(crate) const DEFAULT_SCREENSHOT_RETENTION_HOURS: u32 = 24;
pub(crate) const DEFAULT_SCREENSHOT_RETENTION_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SavedSettings {
    pub(crate) schema_version: u32,
    pub(crate) ui_locale: UiLocale,
    pub(crate) applied_installer_language_id: Option<String>,
    pub(crate) selected_model_id: Option<String>,
    pub(crate) speech_backend_preference: SpeechBackendPreference,
    pub(crate) output_mode: SpeechOutputMode,
    pub(crate) dictation_language: DictationLanguage,
    pub(crate) fast_release_previews: bool,
    pub(crate) visualizer_style: VisualizerStyle,
    pub(crate) circular_spectrum_palette: CircularSpectrumPalette,
    pub(crate) broadcast_mic_palette: BroadcastMicPalette,
    pub(crate) flow_sphere_palette: flow_sphere::FlowSpherePalette,
    pub(crate) pulse_matrix_palette: pulse_matrix::PulseMatrixPalette,
    pub(crate) writing_slate_surface: WritingSlateSurface,
    pub(crate) companion_home_position: Option<(i32, i32)>,
    pub(crate) selected_translation_tier: Option<TranslationTier>,
    pub(crate) selected_translation_model_id: Option<String>,
    pub(crate) translation_enabled: bool,
    pub(crate) layout_enabled: bool,
    pub(crate) translation_keep_warm: bool,
    pub(crate) translation_blur_radius: u32,
    pub(crate) translation_primary_language: TranslationLanguage,
    pub(crate) translation_secondary_language: TranslationLanguage,
    pub(crate) text_action_mode: TextActionMode,
    pub(crate) translation_delivery: TranslationDelivery,
    pub(crate) history_enabled: bool,
    pub(crate) screenshot_retention_enabled: bool,
    pub(crate) screenshot_retention_hours: u32,
    pub(crate) screenshot_retention_max_bytes: u64,
    pub(crate) hotkeys: HotkeyBindings,
    /// The user's ordered transcription list. Empty means "never configured",
    /// and the worker keeps its own default of the local engine first.
    pub(crate) transcription_providers: Vec<TranscriptionProviderEntry>,
    pub(crate) translation_providers: Vec<TranslationProviderEntry>,
}

impl Default for SavedSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            ui_locale: UiLocale::System,
            applied_installer_language_id: None,
            selected_model_id: None,
            speech_backend_preference: SpeechBackendPreference::default(),
            output_mode: SpeechOutputMode::default(),
            dictation_language: DictationLanguage::default(),
            fast_release_previews: true,
            visualizer_style: VisualizerStyle::FlowSphere,
            circular_spectrum_palette: CircularSpectrumPalette::default(),
            broadcast_mic_palette: BroadcastMicPalette::default(),
            flow_sphere_palette: flow_sphere::FlowSpherePalette::BlackYellow,
            pulse_matrix_palette: pulse_matrix::PulseMatrixPalette::default(),
            writing_slate_surface: WritingSlateSurface::default(),
            companion_home_position: None,
            selected_translation_tier: None,
            selected_translation_model_id: None,
            translation_enabled: false,
            layout_enabled: true,
            translation_keep_warm: true,
            translation_blur_radius: DEFAULT_TRANSLATION_BLUR_RADIUS,
            translation_primary_language: TranslationLanguage::English,
            translation_secondary_language: TranslationLanguage::Russian,
            text_action_mode: TextActionMode::default(),
            translation_delivery: TranslationDelivery::default(),
            history_enabled: true,
            screenshot_retention_enabled: true,
            screenshot_retention_hours: DEFAULT_SCREENSHOT_RETENTION_HOURS,
            screenshot_retention_max_bytes: DEFAULT_SCREENSHOT_RETENTION_BYTES,
            hotkeys: HotkeyBindings::default(),
            transcription_providers: Vec::new(),
            translation_providers: Vec::new(),
        }
    }
}

/// Reads settings field by field so one unusable value costs only that field.
///
/// A derived `Deserialize` would reject the whole document over a single
/// renamed enum variant or hand-edited value, silently resetting every other
/// preference. Unknown fields are ignored, which is what makes a document
/// written by a newer build readable by an older one.
impl<'de> Deserialize<'de> for SavedSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn read<T: DeserializeOwned>(
            document: &serde_json::Map<String, serde_json::Value>,
            key: &str,
            slot: &mut T,
        ) {
            if let Some(value) = document.get(key)
                && let Ok(parsed) = T::deserialize(value.clone())
            {
                *slot = parsed;
            }
        }

        let document = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let mut settings = Self::default();
        read(&document, "schema_version", &mut settings.schema_version);
        read(&document, "ui_locale", &mut settings.ui_locale);
        read(
            &document,
            "applied_installer_language_id",
            &mut settings.applied_installer_language_id,
        );
        read(
            &document,
            "selected_model_id",
            &mut settings.selected_model_id,
        );
        read(
            &document,
            "speech_backend_preference",
            &mut settings.speech_backend_preference,
        );
        read(&document, "output_mode", &mut settings.output_mode);
        read(
            &document,
            "dictation_language",
            &mut settings.dictation_language,
        );
        read(
            &document,
            "fast_release_previews",
            &mut settings.fast_release_previews,
        );
        read(
            &document,
            "visualizer_style",
            &mut settings.visualizer_style,
        );
        settings.visualizer_style = settings.visualizer_style.released_or_default();
        read(
            &document,
            "circular_spectrum_palette",
            &mut settings.circular_spectrum_palette,
        );
        read(
            &document,
            "broadcast_mic_palette",
            &mut settings.broadcast_mic_palette,
        );
        read(
            &document,
            "flow_sphere_palette",
            &mut settings.flow_sphere_palette,
        );
        read(
            &document,
            "pulse_matrix_palette",
            &mut settings.pulse_matrix_palette,
        );
        read(
            &document,
            "writing_slate_surface",
            &mut settings.writing_slate_surface,
        );
        read(
            &document,
            "companion_home_position",
            &mut settings.companion_home_position,
        );
        read(
            &document,
            "selected_translation_tier",
            &mut settings.selected_translation_tier,
        );
        read(
            &document,
            "selected_translation_model_id",
            &mut settings.selected_translation_model_id,
        );
        read(
            &document,
            "translation_enabled",
            &mut settings.translation_enabled,
        );
        read(&document, "layout_enabled", &mut settings.layout_enabled);
        read(
            &document,
            "translation_keep_warm",
            &mut settings.translation_keep_warm,
        );
        read(
            &document,
            "translation_blur_radius",
            &mut settings.translation_blur_radius,
        );
        read(
            &document,
            "translation_primary_language",
            &mut settings.translation_primary_language,
        );
        read(
            &document,
            "translation_secondary_language",
            &mut settings.translation_secondary_language,
        );
        read(
            &document,
            "text_action_mode",
            &mut settings.text_action_mode,
        );
        read(
            &document,
            "translation_delivery",
            &mut settings.translation_delivery,
        );
        read(&document, "history_enabled", &mut settings.history_enabled);
        read(
            &document,
            "screenshot_retention_enabled",
            &mut settings.screenshot_retention_enabled,
        );
        read(
            &document,
            "screenshot_retention_hours",
            &mut settings.screenshot_retention_hours,
        );
        read(
            &document,
            "screenshot_retention_max_bytes",
            &mut settings.screenshot_retention_max_bytes,
        );
        let has_hotkeys = document.contains_key("hotkeys");
        read(&document, "hotkeys", &mut settings.hotkeys);
        if !has_hotkeys {
            settings.hotkeys.correct_layout.enabled = settings.layout_enabled;
            settings.hotkeys.translate.enabled = settings.translation_enabled;
        }
        read(
            &document,
            "transcription_providers",
            &mut settings.transcription_providers,
        );
        read(
            &document,
            "translation_providers",
            &mut settings.translation_providers,
        );
        Ok(settings)
    }
}

impl SavedSettings {
    pub(crate) fn from_state(state: &AppState) -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            ui_locale: state.ui_locale,
            applied_installer_language_id: state.applied_installer_language_id.clone(),
            selected_model_id: Some(state.selected_model_id.clone()),
            speech_backend_preference: state.speech_backend_preference,
            output_mode: state.output_mode,
            dictation_language: state.dictation_language,
            fast_release_previews: state.fast_release_previews,
            visualizer_style: state.visualizer_style,
            circular_spectrum_palette: state.circular_spectrum_palette,
            broadcast_mic_palette: state.broadcast_mic_palette,
            flow_sphere_palette: state.flow_sphere_palette,
            pulse_matrix_palette: state.pulse_matrix_palette,
            writing_slate_surface: state.writing_slate_surface,
            companion_home_position: state.companion_home_position,
            selected_translation_tier: state.selected_translation_tier,
            selected_translation_model_id: state.selected_translation_model_id.clone(),
            translation_enabled: state.translation_enabled,
            layout_enabled: state.layout_enabled,
            translation_keep_warm: state.translation_keep_warm,
            translation_blur_radius: state.translation_blur_radius,
            translation_primary_language: state.translation_primary_language,
            translation_secondary_language: state.translation_secondary_language,
            text_action_mode: state.text_action_mode,
            translation_delivery: state.translation_delivery,
            history_enabled: state.history_enabled,
            screenshot_retention_enabled: state.screenshot_retention_enabled,
            screenshot_retention_hours: state.screenshot_retention_hours,
            screenshot_retention_max_bytes: state.screenshot_retention_max_bytes,
            hotkeys: state.hotkeys.clone(),
            transcription_providers: state.transcription_chain.clone(),
            translation_providers: state.translation_chain.clone(),
        }
    }

    /// Brings a document read from disk into the range the application can
    /// apply. A value outside it comes from a hand edit or an older build, and
    /// is not a reason to refuse to start.
    fn sanitized(mut self) -> Self {
        self.applied_installer_language_id = self
            .applied_installer_language_id
            .filter(|id| crate::installer_locale::valid_installation_id(id))
            .map(|id| id.to_ascii_lowercase());
        self.translation_blur_radius = self
            .translation_blur_radius
            .min(MAX_TRANSLATION_BLUR_RADIUS);
        self.screenshot_retention_hours = self.screenshot_retention_hours.max(1);
        self.screenshot_retention_max_bytes = self.screenshot_retention_max_bytes.max(1);
        self.selected_model_id = self
            .selected_model_id
            .filter(|identifier| !identifier.trim().is_empty());
        if self.translation_primary_language == self.translation_secondary_language {
            self.translation_secondary_language =
                if self.translation_primary_language == TranslationLanguage::Russian {
                    TranslationLanguage::English
                } else {
                    TranslationLanguage::Russian
                };
        }
        // Repair only what the validator rejects. Builds up to 0.8.9 saved the
        // set Windows accepted, so a refused Ctrl+Alt+P was stored as a
        // disabled pause; resetting the whole table for that discarded every
        // other custom chord on each start.
        self.hotkeys = rust_app_core::hotkeys::repair(&self.hotkeys);
        self.schema_version = SETTINGS_SCHEMA_VERSION;
        self
    }
}

/// Reads a settings document. Unknown fields are ignored, missing fields take
/// their default, and an unreadable document falls back to defaults rather
/// than blocking startup.
#[cfg(test)]
pub(crate) fn parse_settings(bytes: &[u8]) -> SavedSettings {
    match parse_settings_checked(bytes) {
        Ok(settings) => settings,
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "settings result=unreadable error={error} recovery=defaults"
            ));
            SavedSettings::default()
        }
    }
}

fn parse_settings_checked(bytes: &[u8]) -> Result<SavedSettings, serde_json::Error> {
    serde_json::from_slice::<SavedSettings>(bytes).map(SavedSettings::sanitized)
}

pub(crate) fn load_settings() -> LoadedSettings {
    SettingsStorage::new(settings_path())
        .load_recovering_with_installer_language(crate::installer_locale::load().as_ref())
}

const TRANSCRIPTION_PROVIDER_IDS: [&str; 6] =
    ["local", "groq", "cloudflare", "azure", "deepgram", "custom"];

pub(crate) fn apply_first_run_choice(
    mut settings: SavedSettings,
    choice: FirstRunChoice,
) -> SavedSettings {
    settings.selected_model_id = Some("whisper-large-v3-turbo".to_owned());
    settings.transcription_providers = TRANSCRIPTION_PROVIDER_IDS
        .into_iter()
        .map(|id| TranscriptionProviderEntry {
            id: id.to_owned(),
            enabled: id == "local" && choice.download_recommended_speech_model,
            endpoint: None,
            model: None,
            account: None,
        })
        .collect();
    settings
}

pub(crate) fn save_first_run_settings(settings: &SavedSettings) -> Result<(), Box<dyn Error>> {
    SettingsStorage::new(settings_path()).replace_atomically(settings)?;
    Ok(())
}

pub(crate) fn save_settings(state: &AppState) -> Result<(), Box<dyn Error>> {
    SettingsStorage::new(settings_path()).replace_atomically(&SavedSettings::from_state(state))?;
    Ok(())
}

#[cfg(test)]
mod settings_document_tests {
    use std::{fs, thread, time::Duration};

    use rust_protocol::{
        DictationLanguage, HotkeyAction, HotkeyBindings, HotkeySpec, SpeechOutputMode,
        TranslationLanguage, TranslationTier,
    };

    use crate::{pulse_matrix::PulseMatrixPalette, speech_backend::SpeechBackend};
    use tempfile::tempdir;

    use super::{
        AppState, BroadcastMicPalette, DEFAULT_SCREENSHOT_RETENTION_BYTES,
        DEFAULT_SCREENSHOT_RETENTION_HOURS, FirstRunChoice, MAX_TRANSLATION_BLUR_RADIUS,
        SETTINGS_SCHEMA_VERSION, SavedSettings, SettingsStorage, TextActionMode, VisualizerStyle,
        apply_first_run_choice, parse_settings,
    };

    /// Exactly what the 0.5.27 build wrote, before the document carried a
    /// schema version.
    const PREVIOUS_DOCUMENT: &[u8] = br#"{
      "selected_model_id": "whisper-large-v3-turbo",
      "speech_backend_preference": "auto",
      "output_mode": "original_language",
      "fast_release_previews": true,
      "visualizer_style": "voice_orb",
      "companion_home_position": [120, 240],
      "selected_translation_tier": "tiny",
      "translation_enabled": true,
      "history_enabled": false
    }"#;

    #[test]
    fn a_document_from_the_previous_build_keeps_its_values() {
        let settings = parse_settings(PREVIOUS_DOCUMENT);

        assert_eq!(
            settings.selected_model_id.as_deref(),
            Some("whisper-large-v3-turbo")
        );
        assert_eq!(settings.companion_home_position, Some((120, 240)));
        assert_eq!(
            settings.selected_translation_tier,
            Some(TranslationTier::Tiny)
        );
        assert_eq!(settings.selected_translation_model_id, None);
        assert!(settings.translation_enabled);
        assert!(!settings.history_enabled);
    }

    #[test]
    fn a_translation_catalog_id_round_trips_without_replacing_the_legacy_tier() {
        let mut settings = SavedSettings {
            selected_translation_model_id: Some("firefox-de-en-v3".to_owned()),
            selected_translation_tier: Some(TranslationTier::Compact),
            ..SavedSettings::default()
        };
        let encoded = serde_json::to_vec(&settings).unwrap();
        settings = parse_settings(&encoded);

        assert_eq!(
            settings.selected_translation_model_id.as_deref(),
            Some("firefox-de-en-v3")
        );
        assert_eq!(
            settings.selected_translation_tier,
            Some(TranslationTier::Compact)
        );
    }

    #[test]
    fn fields_absent_from_an_older_document_take_their_default() {
        let settings = parse_settings(PREVIOUS_DOCUMENT);
        let defaults = SavedSettings::default();

        // Written by neither the previous build nor this document.
        assert_eq!(settings.layout_enabled, defaults.layout_enabled);
        assert_eq!(
            settings.translation_keep_warm,
            defaults.translation_keep_warm
        );
        assert_eq!(
            settings.translation_blur_radius,
            defaults.translation_blur_radius
        );
        assert_eq!(settings.text_action_mode, defaults.text_action_mode);
        assert_eq!(
            settings.broadcast_mic_palette,
            defaults.broadcast_mic_palette
        );
        assert_eq!(settings.pulse_matrix_palette, defaults.pulse_matrix_palette);
        assert_eq!(
            settings.screenshot_retention_enabled,
            defaults.screenshot_retention_enabled
        );
        assert_eq!(
            settings.screenshot_retention_hours,
            DEFAULT_SCREENSHOT_RETENTION_HOURS
        );
        assert_eq!(
            settings.screenshot_retention_max_bytes,
            DEFAULT_SCREENSHOT_RETENTION_BYTES
        );
        assert_eq!(
            settings.writing_slate_surface,
            defaults.writing_slate_surface
        );
        let mut expected_hotkeys = HotkeyBindings::default();
        expected_hotkeys.correct_layout.enabled = settings.layout_enabled;
        expected_hotkeys.translate.enabled = settings.translation_enabled;
        assert_eq!(settings.hotkeys, expected_hotkeys);
    }

    #[test]
    fn retired_visualizers_migrate_to_the_first_supported_style() {
        assert_eq!(
            SavedSettings::default().visualizer_style,
            VisualizerStyle::FlowSphere
        );
        for retired in ["voice_orb", "symmetric_spectrum", "fluid_ribbon"] {
            let document = format!(r#"{{"visualizer_style":"{retired}"}}"#);
            assert_eq!(
                parse_settings(document.as_bytes()).visualizer_style,
                VisualizerStyle::BroadcastMic,
                "retired style {retired}"
            );
        }
    }

    #[test]
    fn customized_hotkeys_survive_a_settings_round_trip() {
        let mut saved = SavedSettings::default();
        saved.hotkeys.binding_mut(HotkeyAction::Translate).spec =
            HotkeySpec::control_alt(b'G' as u16);
        saved.hotkeys.binding_mut(HotkeyAction::InsertLast).enabled = false;

        let state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &saved,
        );
        let document = serde_json::to_vec_pretty(&SavedSettings::from_state(&state)).unwrap();

        assert_eq!(parse_settings(&document).hotkeys, saved.hotkeys);
    }

    #[test]
    fn a_document_from_a_newer_build_is_read_without_its_unknown_fields() {
        let settings = parse_settings(
            br#"{"history_enabled": false, "a_setting_from_the_future": {"nested": [1, 2]}}"#,
        );

        assert!(!settings.history_enabled);
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn one_unusable_value_does_not_reset_the_other_settings() {
        // "orb" was never a variant name; a renamed enum or a hand edit looks
        // exactly like this.
        let settings = parse_settings(
            br#"{"visualizer_style": "orb", "history_enabled": false, "layout_enabled": false}"#,
        );

        assert_eq!(
            settings.visualizer_style,
            SavedSettings::default().visualizer_style
        );
        assert!(!settings.history_enabled);
        assert!(!settings.layout_enabled);
    }

    #[test]
    fn an_unreadable_document_falls_back_to_defaults_instead_of_failing() {
        assert_eq!(
            parse_settings(b"{ this is not json"),
            SavedSettings::default()
        );
        assert_eq!(parse_settings(b""), SavedSettings::default());
    }

    #[test]
    fn values_outside_the_supported_range_are_brought_back_into_it() {
        let settings = parse_settings(
            br#"{"translation_blur_radius": 4294967295, "selected_model_id": "   "}"#,
        );

        assert_eq!(
            settings.translation_blur_radius,
            MAX_TRANSLATION_BLUR_RADIUS
        );
        assert_eq!(settings.selected_model_id, None);
    }

    #[test]
    fn a_saved_document_reads_back_as_the_same_settings() {
        let saved = SavedSettings {
            translation_enabled: true,
            dictation_language: DictationLanguage::Russian,
            translation_secondary_language: TranslationLanguage::Ukrainian,
            companion_home_position: Some((-640, 320)),
            visualizer_style: VisualizerStyle::default(),
            broadcast_mic_palette: BroadcastMicPalette::Red,
            pulse_matrix_palette: PulseMatrixPalette::RedAmber,
            text_action_mode: TextActionMode::default(),
            screenshot_retention_enabled: false,
            screenshot_retention_hours: 48,
            screenshot_retention_max_bytes: 128 * 1024 * 1024,
            ..SavedSettings::default()
        };

        let state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &saved,
        );
        let document = serde_json::to_vec_pretty(&SavedSettings::from_state(&state)).unwrap();

        assert_eq!(parse_settings(&document), SavedSettings::from_state(&state));
    }

    #[test]
    fn equal_translation_languages_are_repaired_without_resetting_the_document() {
        let settings = parse_settings(
            br#"{"translation_primary_language":"ukrainian","translation_secondary_language":"ukrainian","history_enabled":false}"#,
        );

        assert_eq!(
            settings.translation_primary_language,
            TranslationLanguage::Ukrainian
        );
        assert_eq!(
            settings.translation_secondary_language,
            TranslationLanguage::Russian
        );
        assert!(!settings.history_enabled);
    }

    #[test]
    fn invalid_persisted_hotkeys_are_repaired_as_one_atomic_set() {
        let settings = parse_settings(
            br#"{
                "translation_enabled": true,
                "hotkeys": {
                    "dictate": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":32},"enabled":true},
                    "insert_last": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":86},"enabled":true},
                    "correct_layout": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":75},"enabled":true},
                    "translate": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":86},"enabled":true},
                    "pause_resume": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":80},"enabled":true}
                }
            }"#,
        );

        assert_eq!(
            settings.hotkeys.translate.spec,
            HotkeySpec::control_alt(b'F' as u16)
        );
        assert!(settings.hotkeys.translate.enabled);
    }

    #[test]
    fn a_pause_saved_as_disabled_does_not_reset_the_other_custom_chords() {
        // The shape 0.8.9 wrote after Windows refused Ctrl+Alt+P: pause
        // disabled, everything else the user's own. Loading it used to replace
        // the whole table with defaults on every start.
        let settings = parse_settings(
            br#"{
                "translation_enabled": true,
                "hotkeys": {
                    "dictate": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":68},"enabled":true},
                    "insert_last": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":73},"enabled":true},
                    "correct_layout": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":75},"enabled":false},
                    "translate": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":84},"enabled":true},
                    "screen_translate": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":71},"enabled":true},
                    "pause_resume": {"spec":{"modifiers":{"control":true,"alt":true,"shift":false,"windows":false},"virtual_key":80},"enabled":false}
                }
            }"#,
        );

        assert!(settings.hotkeys.pause_resume.enabled);
        assert_eq!(
            settings.hotkeys.pause_resume.spec,
            HotkeySpec::control_alt(b'P' as u16)
        );
        assert_eq!(
            settings.hotkeys.dictate.spec,
            HotkeySpec::control_alt(b'D' as u16)
        );
        assert_eq!(
            settings.hotkeys.insert_last.spec,
            HotkeySpec::control_alt(b'I' as u16)
        );
        assert!(!settings.hotkeys.correct_layout.enabled);
        assert_eq!(
            settings.hotkeys.translate.spec,
            HotkeySpec::control_alt(b'T' as u16)
        );
    }

    fn history_setting(enabled: bool) -> SavedSettings {
        SavedSettings {
            history_enabled: enabled,
            ..SavedSettings::default()
        }
    }

    #[test]
    fn a_missing_primary_recovers_the_newest_valid_temporary_candidate() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        fs::write(
            primary.with_extension("previous"),
            serde_json::to_vec(&history_setting(true)).unwrap(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(
            primary.with_extension("tmp"),
            serde_json::to_vec(&history_setting(false)).unwrap(),
        )
        .unwrap();

        let loaded = storage.load_recovering();

        assert!(!loaded.history_enabled);
        assert_eq!(parse_settings(&fs::read(primary).unwrap()), loaded);
    }

    #[test]
    fn a_corrupt_primary_is_retained_and_a_valid_previous_is_restored() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        fs::write(&primary, b"{broken").unwrap();
        fs::write(
            primary.with_extension("previous"),
            serde_json::to_vec(&history_setting(false)).unwrap(),
        )
        .unwrap();

        let loaded = storage.load_recovering();

        assert!(!loaded.history_enabled);
        assert!(primary.exists());
        assert!(fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("settings.json.corrupt-")
        }));
    }

    #[test]
    fn an_atomic_replace_keeps_the_previous_complete_document() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        storage.replace_atomically(&history_setting(true)).unwrap();
        storage.replace_atomically(&history_setting(false)).unwrap();

        assert!(!parse_settings(&fs::read(&primary).unwrap()).history_enabled);
        assert!(
            parse_settings(&fs::read(primary.with_extension("previous")).unwrap()).history_enabled
        );
        assert!(!primary.with_extension("tmp").exists());
    }

    #[test]
    fn a_missing_profile_requires_an_explicit_first_run_choice() {
        let directory = tempdir().unwrap();
        let storage = SettingsStorage::new(directory.path().join("settings.json"));

        let loaded = storage.load_recovering_with_origin();

        assert!(loaded.needs_first_run_choice);
        assert_eq!(loaded.settings, SavedSettings::default());
    }

    #[test]
    fn installer_locale_survives_deferred_launch_and_cancelled_first_run() {
        let directory = tempdir().unwrap();
        let installation = directory.path().join("installation");
        fs::create_dir(&installation).unwrap();
        fs::write(
            installation.join(crate::installer_locale::SEED_FILE_NAME),
            "russian",
        )
        .unwrap();
        let primary = directory.path().join("profile/settings.json");
        let storage = SettingsStorage::new(primary.clone());

        // The installer did not launch the app, and the user can cancel the
        // first-run prompt. Neither condition may discard the language seed.
        for _ in 0..2 {
            let loaded = storage.load_recovering_with_installer_locale(
                crate::installer_locale::load_from_directory(&installation)
                    .map(|seed| seed.locale()),
            );
            assert!(loaded.needs_first_run_choice);
            assert_eq!(
                loaded.settings.ui_locale,
                crate::localization::UiLocale::Russian
            );
            assert!(!primary.exists());
        }

        let loaded = storage
            .load_recovering_with_installer_locale(Some(crate::localization::UiLocale::Russian));
        storage.replace_atomically(&loaded.settings).unwrap();
        fs::write(
            installation.join(crate::installer_locale::SEED_FILE_NAME),
            "english",
        )
        .unwrap();
        let restarted = storage.load_recovering_with_installer_locale(
            crate::installer_locale::load_from_directory(&installation).map(|seed| seed.locale()),
        );
        assert!(!restarted.needs_first_run_choice);
        assert_eq!(
            restarted.settings.ui_locale,
            crate::localization::UiLocale::Russian
        );
    }

    #[test]
    fn installer_locale_never_overwrites_a_stored_preference_including_system() {
        for locale in [
            crate::localization::UiLocale::French,
            crate::localization::UiLocale::System,
        ] {
            let directory = tempdir().unwrap();
            let primary = directory.path().join("settings.json");
            let storage = SettingsStorage::new(primary.clone());
            let settings = SavedSettings {
                ui_locale: locale,
                history_enabled: false,
                ..SavedSettings::default()
            };
            storage.replace_atomically(&settings).unwrap();
            let before = fs::read(&primary).unwrap();

            let loaded = storage.load_recovering_with_installer_locale(Some(
                crate::localization::UiLocale::Russian,
            ));

            assert_eq!(loaded.settings.ui_locale, locale);
            assert!(!loaded.settings.history_enabled);
            assert_eq!(fs::read(&primary).unwrap(), before);
        }
    }

    #[test]
    fn legacy_installer_locale_does_not_change_an_existing_profile_even_without_a_locale() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        fs::write(&primary, br#"{"history_enabled":false}"#).unwrap();
        let storage = SettingsStorage::new(primary);

        let loaded = storage
            .load_recovering_with_installer_locale(Some(crate::localization::UiLocale::Russian));

        assert!(!loaded.needs_first_run_choice);
        assert!(!loaded.settings.history_enabled);
        assert_eq!(
            loaded.settings.ui_locale,
            crate::localization::UiLocale::System
        );
    }

    const INSTALLATION_ONE: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const INSTALLATION_TWO: &str = "11234567-89ab-cdef-0123-456789abcdef";

    fn russian_installation(id: &str) -> crate::installer_locale::InstallerLanguage {
        crate::installer_locale::InstallerLanguage::Request {
            locale: crate::localization::UiLocale::Russian,
            installation_id: id.to_owned(),
        }
    }

    fn applied_installation(settings: &SavedSettings) -> Option<String> {
        serde_json::to_value(settings).unwrap()["applied_installer_language_id"]
            .as_str()
            .map(str::to_owned)
    }

    #[test]
    fn installer_request_overrides_system_once_and_a_new_installation_applies_again() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        let saved = SavedSettings {
            history_enabled: false,
            selected_model_id: Some("existing-model".into()),
            ..SavedSettings::default()
        };
        storage.replace_atomically(&saved).unwrap();
        let request = russian_installation(INSTALLATION_ONE);
        let loaded = storage.load_recovering_with_installer_language(Some(&request));
        assert_eq!(
            loaded.settings.ui_locale,
            crate::localization::UiLocale::Russian
        );
        assert!(
            !loaded.needs_first_run_choice,
            "upgrade must not repeat the model choice"
        );
        assert!(!loaded.settings.history_enabled);
        assert_eq!(
            loaded.settings.selected_model_id.as_deref(),
            Some("existing-model")
        );
        let committed = parse_settings(&fs::read(&primary).unwrap());
        assert_eq!(committed.ui_locale, crate::localization::UiLocale::Russian);
        assert_eq!(
            applied_installation(&committed).as_deref(),
            Some(INSTALLATION_ONE)
        );

        let mut state = AppState::new(
            "existing-model".into(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &loaded.settings,
        );
        state.ui_locale = crate::localization::UiLocale::French;
        storage
            .replace_atomically(&SavedSettings::from_state(&state))
            .unwrap();
        let restarted = storage.load_recovering_with_installer_language(Some(&request));
        assert_eq!(
            restarted.settings.ui_locale,
            crate::localization::UiLocale::French,
            "an in-app language change must survive the unchanged installer file"
        );
        let reinstall = storage
            .load_recovering_with_installer_language(Some(&russian_installation(INSTALLATION_TWO)));
        assert_eq!(
            reinstall.settings.ui_locale,
            crate::localization::UiLocale::Russian
        );
        assert_eq!(
            applied_installation(&reinstall.settings).as_deref(),
            Some(INSTALLATION_TWO)
        );
        assert!(!reinstall.needs_first_run_choice);
    }

    #[test]
    fn installer_request_fresh_cancel_keeps_onboarding_pending_until_choice_is_saved() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        let request = russian_installation(INSTALLATION_ONE);
        for _ in 0..2 {
            let loaded = storage.load_recovering_with_installer_language(Some(&request));
            assert!(loaded.needs_first_run_choice);
            assert_eq!(
                loaded.settings.ui_locale,
                crate::localization::UiLocale::Russian
            );
            assert!(
                !primary.exists(),
                "opening/cancelling onboarding must not create a complete profile"
            );
            assert!(!primary.with_extension("tmp").exists());
        }
        let loaded = storage.load_recovering_with_installer_language(Some(&request));
        let accepted = apply_first_run_choice(
            loaded.settings,
            FirstRunChoice {
                download_recommended_speech_model: false,
            },
        );
        storage.replace_atomically(&accepted).unwrap();
        assert_eq!(
            applied_installation(&parse_settings(&fs::read(&primary).unwrap())).as_deref(),
            Some(INSTALLATION_ONE)
        );
        let restarted = storage.load_recovering_with_installer_language(Some(&request));
        assert!(!restarted.needs_first_run_choice);
        assert!(
            restarted
                .settings
                .transcription_providers
                .iter()
                .all(|provider| !provider.enabled)
        );
    }

    #[test]
    fn installer_request_marker_survives_recovery_and_app_state_saves() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let original = serde_json::json!({"ui_locale":"french", "history_enabled":false,
            "applied_installer_language_id":INSTALLATION_ONE});
        fs::write(primary.with_extension("previous"), original.to_string()).unwrap();
        fs::write(&primary, "broken").unwrap();
        let storage = SettingsStorage::new(primary.clone());
        let loaded = storage
            .load_recovering_with_installer_language(Some(&russian_installation(INSTALLATION_ONE)));
        assert_eq!(
            loaded.settings.ui_locale,
            crate::localization::UiLocale::French
        );
        assert!(!loaded.needs_first_run_choice);
        assert!(!loaded.settings.history_enabled);
        let state = AppState::new(
            "existing-model".into(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &loaded.settings,
        );
        storage
            .replace_atomically(&SavedSettings::from_state(&state))
            .unwrap();
        assert_eq!(
            applied_installation(&parse_settings(&fs::read(primary).unwrap())).as_deref(),
            Some(INSTALLATION_ONE)
        );
    }

    #[test]
    fn installer_request_failed_persistence_keeps_old_file_and_retries_without_consuming() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let previous = primary.with_extension("previous");
        let backup = br#"{"ui_locale":"english","history_enabled":true}"#;
        fs::write(&previous, backup).unwrap();
        thread::sleep(Duration::from_millis(20));
        let original = serde_json::json!({"ui_locale":"system", "history_enabled":false,
            "applied_installer_language_id":INSTALLATION_ONE})
        .to_string();
        fs::write(&primary, &original).unwrap();
        let blocked_temporary = primary.with_extension("installer-language.tmp");
        fs::create_dir(&blocked_temporary).unwrap();
        let storage = SettingsStorage::new(primary.clone());
        let request = russian_installation(INSTALLATION_TWO);
        let failed = storage.load_recovering_with_installer_language(Some(&request));
        assert_eq!(
            failed.settings.ui_locale,
            crate::localization::UiLocale::Russian
        );
        assert_eq!(
            applied_installation(&failed.settings).as_deref(),
            Some(INSTALLATION_ONE)
        );
        assert!(!failed.needs_first_run_choice);
        assert_eq!(fs::read_to_string(&primary).unwrap(), original);
        assert!(
            !primary.with_extension("tmp").exists(),
            "failed request must not be a consumed recovery candidate"
        );
        fs::remove_dir(blocked_temporary).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            // Real ReplaceFileW failure after the complete request temporary
            // has been flushed. Read sharing allows startup to read primary;
            // withholding write/delete sharing prevents its replacement.
            let held = fs::OpenOptions::new()
                .read(true)
                .share_mode(0x0000_0001) // FILE_SHARE_READ
                .open(&primary)
                .unwrap();
            let failed = storage.load_recovering_with_installer_language(Some(&request));
            assert_eq!(
                failed.settings.ui_locale,
                crate::localization::UiLocale::Russian
            );
            assert_eq!(
                applied_installation(&failed.settings).as_deref(),
                Some(INSTALLATION_ONE)
            );
            let failed_request = primary.with_extension("installer-language.tmp");
            assert_eq!(
                applied_installation(&parse_settings(&fs::read(failed_request).unwrap()))
                    .as_deref(),
                Some(INSTALLATION_TWO),
                "the complete request existed before ReplaceFileW failed"
            );
            let recovered = storage.load_recovering_with_origin();
            assert_eq!(
                recovered.settings.ui_locale,
                crate::localization::UiLocale::System
            );
            assert_eq!(
                applied_installation(&recovered.settings).as_deref(),
                Some(INSTALLATION_ONE),
                "a failed request temporary must never count as consumed on restart"
            );
            assert!(!recovered.needs_first_run_choice);
            assert_eq!(fs::read_to_string(&primary).unwrap(), original);
            assert_eq!(fs::read(&previous).unwrap(), backup);
            drop(held);
        }
        let retried = storage.load_recovering_with_installer_language(Some(&request));
        assert_eq!(
            retried.settings.ui_locale,
            crate::localization::UiLocale::Russian
        );
        assert_eq!(
            applied_installation(&retried.settings).as_deref(),
            Some(INSTALLATION_TWO)
        );
        assert!(!retried.settings.history_enabled);
    }

    #[test]
    fn installer_locale_preserves_the_explicit_locale_of_the_recovered_candidate() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        fs::write(
            primary.with_extension("previous"),
            br#"{"ui_locale":"system","history_enabled":false}"#,
        )
        .unwrap();
        fs::write(&primary, b"broken").unwrap();
        let storage = SettingsStorage::new(primary.clone());

        let loaded = storage
            .load_recovering_with_installer_locale(Some(crate::localization::UiLocale::Russian));

        assert!(!loaded.needs_first_run_choice);
        assert_eq!(
            loaded.settings.ui_locale,
            crate::localization::UiLocale::System
        );
        assert!(!loaded.settings.history_enabled);
        assert_eq!(
            parse_settings(&fs::read(primary).unwrap()).ui_locale,
            crate::localization::UiLocale::System
        );
    }

    #[test]
    fn an_existing_valid_profile_never_reopens_first_run() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary);
        storage.replace_atomically(&history_setting(false)).unwrap();

        let loaded = storage.load_recovering_with_origin();

        assert!(!loaded.needs_first_run_choice);
        assert!(!loaded.settings.history_enabled);
    }

    #[test]
    fn an_unrecoverable_profile_is_quarantined_and_requires_a_choice() {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("settings.json");
        let storage = SettingsStorage::new(primary.clone());
        fs::write(&primary, b"{broken").unwrap();

        let loaded = storage.load_recovering_with_origin();

        assert!(loaded.needs_first_run_choice);
        assert!(fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("settings.json.corrupt-")
        }));
    }

    #[test]
    fn declining_the_recommended_download_persists_cloud_ready_defaults() {
        let settings = apply_first_run_choice(
            SavedSettings::default(),
            FirstRunChoice {
                download_recommended_speech_model: false,
            },
        );
        let state = AppState::new(
            "whisper-large-v3-turbo".to_owned(),
            SpeechOutputMode::OriginalLanguage,
            SpeechBackend::Cpu,
            &settings,
        );

        assert!(!state.local_speech_enabled());
        assert_eq!(settings.transcription_providers.len(), 6);
        assert!(
            settings
                .transcription_providers
                .iter()
                .all(|entry| !entry.enabled)
        );
    }

    #[test]
    fn accepting_the_recommended_download_enables_only_the_local_default() {
        let settings = apply_first_run_choice(
            SavedSettings::default(),
            FirstRunChoice {
                download_recommended_speech_model: true,
            },
        );
        let enabled = settings
            .transcription_providers
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(enabled, vec!["local"]);
        assert_eq!(
            settings.selected_model_id.as_deref(),
            Some("whisper-large-v3-turbo")
        );
    }
}
