use std::path::Path;

use rust_model_manager::{BundleArtifact, BundleDescriptor};
use rust_protocol::{TranslationLanguage, TranslationModelSummary, TranslationTier};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const BUILT_IN_CATALOG: &str = include_str!("../models/catalog.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationDirectionDescriptor {
    pub source_language: TranslationLanguage,
    pub target_language: TranslationLanguage,
    pub directory: String,
    pub weight_file: String,
    pub artifacts: Vec<BundleArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationPackDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub tier: TranslationTier,
    pub license: String,
    pub recommended_locales: Vec<String>,
    pub directions: Vec<TranslationDirectionDescriptor>,
}

impl TranslationPackDescriptor {
    #[must_use]
    pub fn direction(
        &self,
        source: TranslationLanguage,
        target: TranslationLanguage,
    ) -> Option<&TranslationDirectionDescriptor> {
        self.directions.iter().find(|direction| {
            direction.source_language == source && direction.target_language == target
        })
    }

    #[must_use]
    pub fn as_bundle(&self) -> BundleDescriptor {
        BundleDescriptor {
            id: self.id.clone(),
            artifacts: self
                .directions
                .iter()
                .flat_map(|direction| {
                    direction.artifacts.iter().map(|artifact| BundleArtifact {
                        relative_path: Path::new(&direction.directory)
                            .join(&artifact.relative_path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        url: artifact.url.clone(),
                        bytes: artifact.bytes,
                        sha256: artifact.sha256.clone(),
                    })
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn summary(
        &self,
        installed: bool,
        selected: bool,
        locale: &str,
    ) -> TranslationModelSummary {
        let mut languages = Vec::new();
        for direction in &self.directions {
            if !languages.contains(&direction.source_language) {
                languages.push(direction.source_language);
            }
            if !languages.contains(&direction.target_language) {
                languages.push(direction.target_language);
            }
        }
        TranslationModelSummary {
            catalog_id: self.id.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            tier: self.tier,
            languages,
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
        if self.schema_version != 1 {
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
                || pack.directions.len() != 2
                || self
                    .packs
                    .iter()
                    .filter(|candidate| candidate.id == pack.id)
                    .count()
                    != 1
                || self
                    .packs
                    .iter()
                    .filter(|candidate| candidate.tier == pack.tier)
                    .count()
                    != 1
            {
                return Err(TranslationCatalogError::InvalidPack(pack.id.clone()));
            }
            pack.as_bundle()
                .validate()
                .map_err(|_| TranslationCatalogError::InvalidPack(pack.id.clone()))?;
            for direction in &pack.directions {
                if direction.source_language == direction.target_language
                    || direction.directory.trim().is_empty()
                    || direction.weight_file.trim().is_empty()
                    || !direction
                        .artifacts
                        .iter()
                        .any(|artifact| artifact.relative_path == direction.weight_file)
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
    fn built_in_catalog_has_fast_and_quality_bidirectional_packs() {
        let catalog = TranslationCatalog::load().unwrap();
        assert_eq!(catalog.packs.len(), 2);
        for tier in [TranslationTier::Tiny, TranslationTier::Teacher] {
            let pack = catalog.by_tier(tier).unwrap();
            assert!(
                pack.direction(TranslationLanguage::Russian, TranslationLanguage::English)
                    .is_some()
            );
            assert!(
                pack.direction(TranslationLanguage::English, TranslationLanguage::Russian)
                    .is_some()
            );
            assert!(pack.as_bundle().total_bytes() > 100_000_000);
        }
    }

    #[test]
    fn recommendation_uses_primary_system_language_but_not_english() {
        let catalog = TranslationCatalog::load().unwrap();
        let fast = catalog.by_tier(TranslationTier::Tiny).unwrap();
        assert!(fast.summary(false, false, "ru-RU").recommended);
        assert!(!fast.summary(false, false, "en-US").recommended);
        assert!(!fast.summary(false, false, "de-DE").recommended);
    }
}
