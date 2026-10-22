mod bundle;
mod catalog;
mod cuda_runtime;
mod http;
mod installer;
mod ocr_catalog;

pub use bundle::{BundleArtifact, BundleCompression, BundleDescriptor, BundleInstaller};
pub use catalog::{BuiltInCatalog, ModelArtifact, ModelCatalog, ModelDescriptor, ModelTier};
pub use cuda_runtime::{CUDA_DOWNLOAD_BYTES, CUDA_LICENSE_URL, CudaRuntimeManager};
pub use http::{CancelToken, HttpFetcher};
pub use installer::{
    ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError, ModelInstaller,
    RemovalReceipt,
};
pub use ocr_catalog::screen_ocr_bundle;
