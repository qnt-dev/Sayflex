//! Static catalog-sized rows for the isolated scroll comparison preview.

use std::error::Error;

use rust_model_manager::{BuiltInCatalog, ModelTier};
use rust_protocol::{ModelSummary, TranslationLanguage, TranslationModelSummary, TranslationTier};
use serde::Deserialize;
use slint::{ModelRc, VecModel};

use crate::{
    ModelRow, SettingsWindow,
    localization::{UiLocale, UiText, text},
    settings_projection::{speech_model_row_localized, translation_model_row_localized},
};

const TRANSLATION_CATALOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/translation-engine/models/catalog.json"
));

// Read only the display fields from the embedded catalog. Importing the
// translation engine here would add its runtime dependencies to the desktop.
#[derive(Deserialize)]
struct TranslationCatalog {
    packs: Vec<TranslationPack>,
}

#[derive(Deserialize)]
struct TranslationPack {
    id: String,
    display_name: String,
    description: String,
    tier: TranslationTier,
    languages: Vec<TranslationLanguage>,
    recommended_locales: Vec<String>,
    license: String,
    artifacts: Vec<ArtifactSize>,
}

#[derive(Deserialize)]
struct ArtifactSize {
    bytes: u64,
}

pub(super) fn fill(settings: &SettingsWindow, locale: UiLocale) -> Result<(), Box<dyn Error>> {
    let rows = rows(locale)?;
    settings.set_models_summary(text(locale, UiText::OneInstalled).into());
    settings.set_model_rows(ModelRc::new(VecModel::from(rows)));
    Ok(())
}

fn rows(locale: UiLocale) -> Result<Vec<ModelRow>, Box<dyn Error>> {
    let speech = BuiltInCatalog::load()?;
    let translation: TranslationCatalog = serde_json::from_str(TRANSLATION_CATALOG)?;
    let mut rows = Vec::with_capacity(speech.models.len() + translation.packs.len());
    for model in speech.models {
        let selected = model.id == speech.default_model;
        let summary = ModelSummary {
            catalog_id: model.id,
            display_name: model.display_name,
            description: model.description,
            size_bytes: model.artifact.bytes,
            installed: selected,
            selected,
            recommended: model.tier == ModelTier::Recommended,
            supports_speech_translation: model.supports_speech_translation,
        };
        rows.push(speech_model_row_localized(
            &summary, None, selected, false, locale,
        ));
    }
    for pack in translation.packs {
        let recommended = locale
            .code()
            .split(['-', '_'])
            .next()
            .is_some_and(|language| {
                !language.eq_ignore_ascii_case("en")
                    && pack
                        .recommended_locales
                        .iter()
                        .any(|candidate| language.eq_ignore_ascii_case(candidate))
            });
        let summary = TranslationModelSummary {
            catalog_id: pack.id,
            display_name: pack.display_name,
            description: pack.description,
            tier: pack.tier,
            languages: pack.languages,
            size_bytes: pack.artifacts.iter().map(|artifact| artifact.bytes).sum(),
            installed: false,
            selected: false,
            recommended,
            license: pack.license,
        };
        rows.push(translation_model_row_localized(
            &summary, false, None, false, false, locale,
        ));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_contains_all_nineteen_models_in_worker_catalog_order() {
        let rows = rows(UiLocale::English).unwrap();
        assert_eq!(rows.len(), 19);
        let ids: Vec<&str> = rows.iter().map(|row| row.catalog_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "whisper-small",
                "whisper-medium",
                "whisper-large-v3-turbo",
                "whisper-large-v3",
                "firefox-ru-en-v3.1",
                "firefox-uk-en-v3",
                "firefox-es-en-v3",
                "firefox-pt-en-v3",
                "firefox-de-en-v3",
                "firefox-fr-en-v3",
                "firefox-it-en-v3",
                "firefox-zh-hans-en-v3",
                "firefox-zh-hant-en-v3",
                "firefox-ja-en-v3",
                "firefox-ko-en-v3",
                "firefox-hi-en-v3",
                "firefox-ar-en-v3",
                "qwen35-4b-q4",
                "qwen35-9b-q4",
            ]
        );
    }

    #[test]
    fn comparison_has_only_turbo_installed_selected_and_loaded_without_progress() {
        let rows = rows(UiLocale::English).unwrap();
        let installed: Vec<&str> = rows
            .iter()
            .filter(|row| row.installed)
            .map(|row| row.catalog_id.as_str())
            .collect();
        assert_eq!(installed, ["whisper-large-v3-turbo"]);
        assert!(rows.iter().all(|row| row.selected == row.installed));
        assert_eq!(rows[2].state.as_str(), "In use");
        assert!(rows.iter().all(|row| row.progress == 0.0));
        assert!(rows.iter().all(|row| row.action_key != "cancel"));
    }

    #[test]
    fn comparison_uses_localized_production_model_labels() {
        let rows = rows(UiLocale::Russian).unwrap();
        assert_eq!(rows[0].section.as_str(), "Речь");
        assert_eq!(rows[0].state.as_str(), "Не установлено");
        assert_eq!(rows[2].state.as_str(), "В использовании");
        assert_eq!(rows[4].title.as_str(), "Русский ↔ Английский");
        assert_eq!(rows[4].state.as_str(), "Не установлено");
    }
}
