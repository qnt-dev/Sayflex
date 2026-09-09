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
}

impl TranslationBackendDescriptor {
    #[must_use]
    pub fn model_file(&self) -> &str {
        match self {
            Self::LlamaCpp { model_file, .. } => model_file,
        }
    }

    #[must_use]
    pub const fn context_size(&self) -> u32 {
        match self {
            Self::LlamaCpp { context_size, .. } => *context_size,
        }
    }

    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        match self {
            Self::LlamaCpp {
                max_output_tokens, ..
            } => *max_output_tokens,
        }
    }
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
        if self.schema_version != 3 {
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
                || pack.backend.model_file().trim().is_empty()
                || pack.backend.context_size() < 1_024
                || pack.backend.max_output_tokens() == 0
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
            if !pack
                .artifacts
                .iter()
                .any(|artifact| artifact.relative_path == pack.backend.model_file())
            {
                return Err(TranslationCatalogError::InvalidPack(pack.id.clone()));
            }
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
        assert_eq!(catalog.packs.len(), 2);
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
    fn recommendation_uses_primary_system_language_but_not_english() {
        let catalog = TranslationCatalog::load().unwrap();
        let fast = catalog.by_tier(TranslationTier::Tiny).unwrap();
        assert!(fast.summary(false, false, "ru-RU").recommended);
        assert!(!fast.summary(false, false, "en-US").recommended);
        assert!(!fast.summary(false, false, "de-DE").recommended);
    }
}
