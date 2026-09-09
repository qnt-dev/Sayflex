//! Reading and writing the settings document.
//!
//! One typed shape, a schema version, and a reader that takes the document
//! field by field so a single unusable value cannot reset every other
//! preference. Kept apart from the interface it configures: this is a file
//! format, and it changes for different reasons than a window does.

use std::{error::Error, fs, path::PathBuf};

use rust_protocol::{
    SpeechOutputMode, TranscriptionProviderEntry, TranslationProviderEntry, TranslationTier,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    AppState, BroadcastMicPalette, CircularSpectrumPalette, DEFAULT_TRANSLATION_BLUR_RADIUS,
    MAX_TRANSLATION_BLUR_RADIUS, TextActionMode, TranslationDelivery, VisualizerStyle,
    WritingSlateSurface, flow_sphere, speech_backend::SpeechBackendPreference,
};

pub(crate) fn data_root() -> PathBuf {
    rust_app_core::product::data_root()
}

fn settings_path() -> PathBuf {
    data_root().join("settings.json")
}

/// Version of the on-disk settings document.
///
/// Bump this when a field changes meaning, not when one is added: unknown
/// fields are ignored on read and missing fields fall back to their default,
/// so pure additions stay compatible in both directions.
pub(crate) const SETTINGS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SavedSettings {
    pub(crate) schema_version: u32,
    pub(crate) selected_model_id: Option<String>,
    pub(crate) speech_backend_preference: SpeechBackendPreference,
    pub(crate) output_mode: SpeechOutputMode,
    pub(crate) fast_release_previews: bool,
    pub(crate) visualizer_style: VisualizerStyle,
    pub(crate) circular_spectrum_palette: CircularSpectrumPalette,
    pub(crate) broadcast_mic_palette: BroadcastMicPalette,
    pub(crate) flow_sphere_palette: flow_sphere::FlowSpherePalette,
    pub(crate) writing_slate_surface: WritingSlateSurface,
    pub(crate) companion_home_position: Option<(i32, i32)>,
    pub(crate) selected_translation_tier: Option<TranslationTier>,
    pub(crate) translation_enabled: bool,
    pub(crate) layout_enabled: bool,
    pub(crate) translation_keep_warm: bool,
    pub(crate) translation_blur_radius: u32,
    pub(crate) text_action_mode: TextActionMode,
    pub(crate) translation_delivery: TranslationDelivery,
    pub(crate) history_enabled: bool,
    /// The user's ordered transcription list. Empty means "never configured",
    /// and the worker keeps its own default of the local engine first.
    pub(crate) transcription_providers: Vec<TranscriptionProviderEntry>,
    pub(crate) translation_providers: Vec<TranslationProviderEntry>,
}

impl Default for SavedSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            selected_model_id: None,
            speech_backend_preference: SpeechBackendPreference::default(),
            output_mode: SpeechOutputMode::default(),
            fast_release_previews: true,
            visualizer_style: VisualizerStyle::default(),
            circular_spectrum_palette: CircularSpectrumPalette::default(),
            broadcast_mic_palette: BroadcastMicPalette::default(),
            flow_sphere_palette: flow_sphere::FlowSpherePalette::default(),
            writing_slate_surface: WritingSlateSurface::default(),
            companion_home_position: None,
            selected_translation_tier: None,
            translation_enabled: false,
            layout_enabled: true,
            translation_keep_warm: true,
            translation_blur_radius: DEFAULT_TRANSLATION_BLUR_RADIUS,
            text_action_mode: TextActionMode::default(),
            translation_delivery: TranslationDelivery::default(),
            history_enabled: true,
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
            "fast_release_previews",
            &mut settings.fast_release_previews,
        );
        read(
            &document,
            "visualizer_style",
            &mut settings.visualizer_style,
        );
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
            selected_model_id: Some(state.selected_model_id.clone()),
            speech_backend_preference: state.speech_backend_preference,
            output_mode: state.output_mode,
            fast_release_previews: state.fast_release_previews,
            visualizer_style: state.visualizer_style,
            circular_spectrum_palette: state.circular_spectrum_palette,
            broadcast_mic_palette: state.broadcast_mic_palette,
            flow_sphere_palette: state.flow_sphere_palette,
            writing_slate_surface: state.writing_slate_surface,
            companion_home_position: state.companion_home_position,
            selected_translation_tier: state.selected_translation_tier,
            translation_enabled: state.translation_enabled,
            layout_enabled: state.layout_enabled,
            translation_keep_warm: state.translation_keep_warm,
            translation_blur_radius: state.translation_blur_radius,
            text_action_mode: state.text_action_mode,
            translation_delivery: state.translation_delivery,
            history_enabled: state.history_enabled,
            transcription_providers: state.transcription_chain.clone(),
            translation_providers: state.translation_chain.clone(),
        }
    }

    /// Brings a document read from disk into the range the application can
    /// apply. A value outside it comes from a hand edit or an older build, and
    /// is not a reason to refuse to start.
    fn sanitized(mut self) -> Self {
        self.translation_blur_radius = self
            .translation_blur_radius
            .min(MAX_TRANSLATION_BLUR_RADIUS);
        self.selected_model_id = self
            .selected_model_id
            .filter(|identifier| !identifier.trim().is_empty());
        self.schema_version = SETTINGS_SCHEMA_VERSION;
        self
    }
}

/// Reads a settings document. Unknown fields are ignored, missing fields take
/// their default, and an unreadable document falls back to defaults rather
/// than blocking startup.
pub(crate) fn parse_settings(bytes: &[u8]) -> SavedSettings {
    match serde_json::from_slice::<SavedSettings>(bytes) {
        Ok(settings) => settings.sanitized(),
        Err(error) => {
            rust_app_core::diagnostics::log_line(&format!(
                "settings result=unreadable error={error} recovery=defaults"
            ));
            SavedSettings::default()
        }
    }
}

pub(crate) fn load_settings() -> SavedSettings {
    match fs::read(settings_path()) {
        Ok(bytes) => parse_settings(&bytes),
        Err(_) => SavedSettings::default(),
    }
}

pub(crate) fn save_settings(state: &AppState) -> Result<(), Box<dyn Error>> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&SavedSettings::from_state(state))?;
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("previous");
    fs::write(&temporary, bytes)?;
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    let had_previous = path.exists();
    if had_previous {
        fs::rename(&path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        if had_previous {
            let _ = fs::rename(&backup, &path);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod settings_document_tests {
    use rust_protocol::{SpeechOutputMode, TranslationTier};

    use crate::speech_backend::SpeechBackend;

    use super::{
        AppState, BroadcastMicPalette, MAX_TRANSLATION_BLUR_RADIUS, SETTINGS_SCHEMA_VERSION,
        SavedSettings, TextActionMode, VisualizerStyle, parse_settings,
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
        assert!(settings.translation_enabled);
        assert!(!settings.history_enabled);
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
        assert_eq!(
            settings.writing_slate_surface,
            defaults.writing_slate_surface
        );
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

        assert_eq!(settings.visualizer_style, VisualizerStyle::default());
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
            companion_home_position: Some((-640, 320)),
            visualizer_style: VisualizerStyle::default(),
            broadcast_mic_palette: BroadcastMicPalette::Red,
            text_action_mode: TextActionMode::default(),
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
}
