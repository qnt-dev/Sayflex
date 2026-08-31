mod bundle;
mod catalog;
mod http;
mod installer;

pub use bundle::{BundleArtifact, BundleDescriptor, BundleInstaller};
pub use catalog::{BuiltInCatalog, ModelArtifact, ModelCatalog, ModelDescriptor, ModelTier};
pub use http::HttpFetcher;
pub use installer::{
    ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError, ModelInstaller,
};
