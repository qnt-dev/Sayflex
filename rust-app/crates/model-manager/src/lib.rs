mod bundle;
mod catalog;
mod http;
mod installer;
mod ocr_catalog;

pub use bundle::{BundleArtifact, BundleDescriptor, BundleInstaller};
pub use catalog::{BuiltInCatalog, ModelArtifact, ModelCatalog, ModelDescriptor, ModelTier};
pub use http::{CancelToken, HttpFetcher};
pub use installer::{
    ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError, ModelInstaller,
    RemovalReceipt,
};
pub use ocr_catalog::screen_ocr_bundle;
