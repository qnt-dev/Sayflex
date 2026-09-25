use rust_model_manager::{BundleArtifact, BundleDescriptor};
use rust_protocol::{TranslationLanguage, TranslationModelSummary, TranslationTier};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const BUILT_IN_CATALOG: &str = include_str!("../models/catalog.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranslationBackendDescriptor {
    LlamaCpp {
        model_file: String,
        context_size: u32,
        max_output_tokens: u32,
    },
    Firefox {
        directions: Vec<FirefoxDirectionDescriptor>,
    },
}

impl TranslationBackendDescriptor {
    #[must_use]
    pub const fn requires_llama_runtime(&self) -> bool {
        matches!(self, Self::LlamaCpp { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirefoxDirectionDescriptor {
    pub source: TranslationLanguage,
    pub target: TranslationLanguage,
    pub model_file: String,
    pub source_vocab_file: String,
    pub target_vocab_file: String,
    pub shortlist_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationPackDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub tier: TranslationTier,
    pub license: String,
    pub recommended_locales: Vec<String>,
    pub backend: TranslationBackendDescriptor,
    pub artifacts: Vec<BundleArtifact>,
    /// Languages the multilingual model may use in either direction.
    /// Listing capabilities once avoids an error-prone N x N direction table.
    pub languages: Vec<TranslationLanguage>,
}

impl TranslationPackDescriptor {
    #[must_use]
    pub fn supports(&self, source: TranslationLanguage, target: TranslationLanguage) -> bool {
        source != target && self.languages.contains(&source) && self.languages.contains(&target)
    }

    #[must_use]
    pub fn as_bundle(&self) -> BundleDescriptor {
        BundleDescriptor {
            id: self.id.clone(),
            license: Some(self.license.clone()),
            attribution: None,
            artifacts: self.artifacts.clone(),
        }
    }

    #[must_use]
    pub fn summary(
        &self,
        installed: bool,
        selected: bool,
        locale: &str,
    ) -> TranslationModelSummary {
        TranslationModelSummary {
            catalog_id: self.id.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            tier: self.tier,
            languages: self.languages.clone(),
            size_bytes: self.as_bundle().total_bytes(),
            installed,
            selected,
            recommended: locale.split(['-', '_']).next().is_some_and(|language| {
                !language.eq_ignore_ascii_case("en")
                    && self
                        .recommended_locales
                        .iter()
                        .any(|candidate| language.eq_ignore_ascii_case(candidate))
            }),
            license: self.license.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationCatalog {
    pub schema_version: u32,
    pub packs: Vec<TranslationPackDescriptor>,
}

impl TranslationCatalog {
    pub fn load() -> Result<Self, TranslationCatalogError> {
        let catalog: Self = serde_json::from_str(BUILT_IN_CATALOG)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), TranslationCatalogError> {
        if self.schema_version != 4 {
            return Err(TranslationCatalogError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.packs.is_empty() {
            return Err(TranslationCatalogError::Empty);
        }
        for pack in &self.packs {
            if pack.id.trim().is_empty()
                || pack.display_name.trim().is_empty()
                || pack.languages.len() < 2
                || self
                    .packs
                    .iter()
                    .filter(|candidate| candidate.id == pack.id)
                    .count()
                    != 1
                || (pack.tier != TranslationTier::Compact
                    && self
                        .packs
                        .iter()
                        .filter(|candidate| candidate.tier == pack.tier)
                        .count()
                        != 1)
                || (pack.tier == TranslationTier::Compact
                    && self
                        .packs
                        .iter()
                        .filter(|candidate| {
                            candidate.tier == TranslationTier::Compact
                                && candidate.languages.len() == pack.languages.len()
                                && candidate
                                    .languages
                                    .iter()
                                    .all(|language| pack.languages.contains(language))
                        })
                        .count()
                        != 1)
            {
                return Err(TranslationCatalogError::InvalidPack(pack.id.clone()));
            }
            let backend_valid = match &pack.backend {
                TranslationBackendDescriptor::LlamaCpp {
                    model_file,
                    context_size,
                    max_output_tokens,
                } => {
                    !model_file.trim().is_empty()
                        && *context_size >= 1_024
                        && *max_output_tokens > 0
                        && pack
                            .artifacts
                            .iter()
                            .any(|artifact| artifact.relative_path == *model_file)
                }
                TranslationBackendDescriptor::Firefox { directions } => {
                    directions.len() == 2
                        && directions.iter().all(|direction| {
                            direction.source != direction.target
                                && pack.languages.contains(&direction.source)
                                && pack.languages.contains(&direction.target)
                                && [
                                    &direction.model_file,
                                    &direction.source_vocab_file,
                                    &direction.target_vocab_file,
                                    &direction.shortlist_file,
                                ]
                                .into_iter()
                                .all(|path| {
                                    !path.trim().is_empty()
                                        && pack
                                            .artifacts
                                            .iter()
                                            .any(|artifact| artifact.relative_path == *path)
                                })
                        })
                        && directions[0].source == directions[1].target
                        && directions[0].target == directions[1].source
                }
            };
            if !backend_valid {
                return Err(TranslationCatalogError::InvalidPack(pack.id.clone()));
            }
            pack.as_bundle()
                .validate()
                .map_err(|_| TranslationCatalogError::InvalidPack(pack.id.clone()))?;
            for language in &pack.languages {
                if pack
                    .languages
                    .iter()
                    .filter(|candidate| *candidate == language)
                    .count()
                    != 1
                {
                    return Err(TranslationCatalogError::InvalidPack(pack.id.clone()));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn by_tier(&self, tier: TranslationTier) -> Option<&TranslationPackDescriptor> {
        self.packs.iter().find(|pack| pack.tier == tier)
    }

    #[must_use]
    pub fn by_id(&self, catalog_id: &str) -> Option<&TranslationPackDescriptor> {
        self.packs.iter().find(|pack| pack.id == catalog_id)
    }

    #[must_use]
    pub fn compact_for_pair(
        &self,
        source: TranslationLanguage,
        target: TranslationLanguage,
    ) -> Option<&TranslationPackDescriptor> {
        self.packs
            .iter()
            .find(|pack| pack.tier == TranslationTier::Compact && pack.supports(source, target))
    }
}

#[derive(Debug, Error)]
pub enum TranslationCatalogError {
    #[error("translation catalog JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("translation catalog schema {0} is unsupported")]
    UnsupportedSchema(u32),
    #[error("translation catalog is empty")]
    Empty,
    #[error("translation pack is invalid: {0}")]
    InvalidPack(String),
}

#[cfg(test)]
mod tests {
    use rust_protocol::{TranslationLanguage, TranslationTier};

    use super::TranslationCatalog;

    #[test]
    fn built_in_catalog_has_fast_and_quality_multilingual_packs() {
        let catalog = TranslationCatalog::load().unwrap();
        assert_eq!(catalog.packs.len(), 15);
        for tier in [TranslationTier::Tiny, TranslationTier::Teacher] {
            let pack = catalog.by_tier(tier).unwrap();
            assert!(pack.supports(TranslationLanguage::Russian, TranslationLanguage::English));
            assert!(pack.supports(TranslationLanguage::English, TranslationLanguage::Russian));
            assert!(pack.supports(TranslationLanguage::Ukrainian, TranslationLanguage::English));
            assert!(pack.supports(TranslationLanguage::English, TranslationLanguage::Japanese));
            assert_eq!(pack.languages, TranslationLanguage::ALL);
            assert!(pack.as_bundle().total_bytes() > 2_000_000_000);
        }
    }

    #[test]
    fn compact_catalog_pack_is_bidirectional_russian_english_only() {
        let catalog = TranslationCatalog::load().unwrap();
        let compact = catalog.by_tier(TranslationTier::Compact).unwrap();

        assert!(compact.supports(TranslationLanguage::English, TranslationLanguage::Russian));
        assert!(compact.supports(TranslationLanguage::Russian, TranslationLanguage::English));
        assert!(!compact.supports(TranslationLanguage::German, TranslationLanguage::Russian));
        assert!(compact.as_bundle().total_bytes() > 46_000_000);
        assert!(compact.as_bundle().total_bytes() < 48_000_000);
        assert!(compact.as_bundle().installed_bytes() > 72_000_000);
        assert!(compact.as_bundle().installed_bytes() < 74_000_000);
    }

    #[test]
    fn recommendation_uses_primary_system_language_but_not_english() {
        let catalog = TranslationCatalog::load().unwrap();
        let fast = catalog.by_tier(TranslationTier::Tiny).unwrap();
        assert!(fast.summary(false, false, "ru-RU").recommended);
        assert!(!fast.summary(false, false, "en-US").recommended);
        assert!(!fast.summary(false, false, "de-DE").recommended);
    }

    #[test]
    fn compact_packs_are_identified_by_catalog_id_and_language_pair() {
        let catalog = TranslationCatalog::load().unwrap();

        assert!(catalog.validate().is_ok());
        assert_eq!(
            catalog.by_id("firefox-de-en-v3").unwrap().id,
            "firefox-de-en-v3"
        );
        assert_eq!(
            catalog
                .compact_for_pair(TranslationLanguage::German, TranslationLanguage::English)
                .unwrap()
                .id,
            "firefox-de-en-v3"
        );
        assert_eq!(
            catalog
                .compact_for_pair(TranslationLanguage::Russian, TranslationLanguage::English)
                .unwrap()
                .id,
            "firefox-ru-en-v3.1"
        );
    }

    #[test]
    fn every_sayflex_language_has_one_verified_compact_english_pair() {
        let catalog = TranslationCatalog::load().unwrap();
        let compact = catalog
            .packs
            .iter()
            .filter(|pack| pack.tier == TranslationTier::Compact)
            .collect::<Vec<_>>();
        assert_eq!(compact.len(), TranslationLanguage::ALL.len() - 1);

        for language in TranslationLanguage::ALL
            .into_iter()
            .filter(|language| *language != TranslationLanguage::English)
        {
            let matches = compact
                .iter()
                .filter(|pack| {
                    pack.supports(TranslationLanguage::English, language)
                        && pack.supports(language, TranslationLanguage::English)
                })
                .collect::<Vec<_>>();
            assert_eq!(matches.len(), 1, "compact coverage for {language:?}");
            let pack = matches[0];
            let super::TranslationBackendDescriptor::Firefox { directions } = &pack.backend else {
                panic!("compact pack {} must use Firefox", pack.id);
            };
            for direction in directions {
                for path in [
                    &direction.model_file,
                    &direction.source_vocab_file,
                    &direction.target_vocab_file,
                    &direction.shortlist_file,
                ] {
                    assert!(
                        pack.artifacts
                            .iter()
                            .any(|artifact| artifact.relative_path == *path),
                        "{} references missing {}",
                        pack.id,
                        path
                    );
                }
            }
        }
    }
}
