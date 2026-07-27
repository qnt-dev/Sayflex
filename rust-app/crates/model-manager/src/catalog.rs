use rust_speech_api::{ComputeBackend, SpeechEngineKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const BUILT_IN_CATALOG: &str = include_str!("../models/catalog.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Lightweight,
    Balanced,
    Recommended,
    MaximumQuality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub file_name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub engine: SpeechEngineKind,
    pub tier: ModelTier,
    pub artifact: ModelArtifact,
    pub compute_backends: Vec<ComputeBackend>,
    pub supports_speech_translation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub schema_version: u32,
    pub default_model: String,
    pub models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    pub fn parse(json: &str) -> Result<Self, CatalogError> {
        let catalog: Self = serde_json::from_str(json)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != 1 {
            return Err(CatalogError::UnsupportedSchema(self.schema_version));
        }
        if self.models.is_empty() {
            return Err(CatalogError::Empty);
        }
        if self.find(&self.default_model).is_none() {
            return Err(CatalogError::MissingDefault(self.default_model.clone()));
        }

        for model in &self.models {
            if model.id.trim().is_empty()
                || model.artifact.file_name.trim().is_empty()
                || model.artifact.url.trim().is_empty()
                || model.artifact.bytes == 0
                || model.artifact.sha256.len() != 64
            {
                return Err(CatalogError::InvalidModel(model.id.clone()));
            }
            if self
                .models
                .iter()
                .filter(|candidate| candidate.id == model.id)
                .count()
                != 1
            {
                return Err(CatalogError::DuplicateModel(model.id.clone()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<&ModelDescriptor> {
        self.models.iter().find(|model| model.id == id)
    }

    #[must_use]
    pub fn recommended(&self) -> &ModelDescriptor {
        self.find(&self.default_model)
            .expect("validated catalog always contains its default model")
    }
}

pub struct BuiltInCatalog;

impl BuiltInCatalog {
    pub fn load() -> Result<ModelCatalog, CatalogError> {
        ModelCatalog::parse(BUILT_IN_CATALOG)
    }
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("model catalog JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("model catalog schema {0} is unsupported")]
    UnsupportedSchema(u32),
    #[error("model catalog is empty")]
    Empty,
    #[error("default model is missing: {0}")]
    MissingDefault(String),
    #[error("model descriptor is invalid: {0}")]
    InvalidModel(String),
    #[error("model ID appears more than once: {0}")]
    DuplicateModel(String),
}

#[cfg(test)]
mod tests {
    use super::{BuiltInCatalog, ModelTier};

    #[test]
    fn built_in_catalog_keeps_current_model_as_default() {
        let catalog = BuiltInCatalog::load().unwrap();

        assert_eq!(catalog.default_model, "whisper-large-v3-turbo");
        assert_eq!(catalog.recommended().tier, ModelTier::Recommended);
        assert!(!catalog.recommended().supports_speech_translation);
        assert!(
            catalog
                .find("whisper-large-v3")
                .unwrap()
                .supports_speech_translation
        );
        assert_eq!(catalog.models.len(), 4);
    }
}
