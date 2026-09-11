use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ModelDescriptor;

/// Files removed from one catalog-owned model installation.
///
/// `removed_bytes` is the sum of the logical lengths of regular files that no
/// longer exist at their SayFlex-owned paths. It is intentionally not named
/// `freed_bytes`: hard links, compression and filesystem allocation mean only
/// the filesystem can know the physical space reclaimed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemovalReceipt {
    pub removed_files: u64,
    pub removed_bytes: u64,
}

impl RemovalReceipt {
    pub(crate) fn add(&mut self, other: Self) {
        self.removed_files = self.removed_files.saturating_add(other.removed_files);
        self.removed_bytes = self.removed_bytes.saturating_add(other.removed_bytes);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Downloading,
    Verifying,
    Installing,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub phase: InstallPhase,
    pub bytes_received: u64,
    pub total_bytes: u64,
}

pub trait ArtifactFetcher: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        expected_bytes: u64,
        progress: &mut dyn FnMut(DownloadProgress),
    ) -> Result<(), ModelInstallError>;
}

#[derive(Debug, Clone)]
pub struct ModelInstaller {
    root: PathBuf,
}

impl ModelInstaller {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn model_path(&self, model: &ModelDescriptor) -> PathBuf {
        self.root.join(&model.artifact.file_name)
    }

    pub fn is_installed(&self, model: &ModelDescriptor) -> Result<bool, ModelInstallError> {
        let path = self.model_path(model);
        if !path.exists() {
            return Ok(false);
        }
        if verification_receipt_matches(&path, model)? {
            return Ok(true);
        }
        verify_file(&path, model)
            .and_then(|()| write_verification_receipt(&path, model))
            .map(|()| true)
            .or_else(|error| {
                if matches!(
                    error,
                    ModelInstallError::SizeMismatch { .. }
                        | ModelInstallError::ChecksumMismatch { .. }
                ) {
                    let _ = fs::remove_file(verification_receipt_path(&path));
                    Ok(false)
                } else {
                    Err(error)
                }
            })
    }

    pub fn ensure_installed(
        &self,
        model: &ModelDescriptor,
        fetcher: &dyn ArtifactFetcher,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<PathBuf, ModelInstallError> {
        if self.is_installed(model)? {
            progress(DownloadProgress {
                phase: InstallPhase::Ready,
                bytes_received: model.artifact.bytes,
                total_bytes: model.artifact.bytes,
            });
            return Ok(self.model_path(model));
        }

        fs::create_dir_all(&self.root)?;
        let final_path = self.model_path(model);
        let temporary_path = final_path.with_extension("download");
        if temporary_path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > model.artifact.bytes)
        {
            fs::remove_file(&temporary_path)?;
        }

        let result = (|| {
            fetcher.fetch(
                &model.artifact.url,
                &temporary_path,
                model.artifact.bytes,
                &mut progress,
            )?;
            progress(DownloadProgress {
                phase: InstallPhase::Verifying,
                bytes_received: model.artifact.bytes,
                total_bytes: model.artifact.bytes,
            });
            verify_file(&temporary_path, model)?;
            progress(DownloadProgress {
                phase: InstallPhase::Installing,
                bytes_received: model.artifact.bytes,
                total_bytes: model.artifact.bytes,
            });
            install_verified_file(&temporary_path, &final_path)?;
            write_verification_receipt(&final_path, model)?;
            progress(DownloadProgress {
                phase: InstallPhase::Ready,
                bytes_received: model.artifact.bytes,
                total_bytes: model.artifact.bytes,
            });
            Ok(final_path)
        })();

        // A cancelled or interrupted download keeps its partial file so the
        // next attempt resumes; a verification failure does not, because those
        // bytes are known to be wrong.
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error,
                ModelInstallError::Download(_)
                    | ModelInstallError::Io(_)
                    | ModelInstallError::Cancelled
            )
        }) && temporary_path.exists()
        {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    /// Removes every on-disk artifact owned by this model descriptor.
    ///
    /// Callers must unload the engine before invoking this method. Only names
    /// derived from the validated catalog descriptor are accepted; no path is
    /// received from IPC or followed outside `root`.
    pub fn remove(&self, model: &ModelDescriptor) -> Result<RemovalReceipt, ModelInstallError> {
        validate_direct_child_name(&model.artifact.file_name)
            .map_err(|()| ModelInstallError::InvalidModel(model.id.clone()))?;
        let final_path = self.model_path(model);
        let receipt = verification_receipt_path(&final_path);
        let targets = [
            final_path.clone(),
            receipt.clone(),
            receipt.with_extension("tmp"),
            final_path.with_extension("download"),
            final_path.with_extension("previous"),
        ];
        let mut removed = RemovalReceipt::default();
        for target in targets {
            ensure_direct_child(&self.root, &target)
                .map_err(|()| ModelInstallError::InvalidModel(model.id.clone()))?;
            removed.add(remove_path_accounted(&target)?);
        }
        Ok(removed)
    }
}

pub(crate) fn validate_direct_child_name(name: &str) -> Result<(), ()> {
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
    {
        Ok(())
    } else {
        Err(())
    }
}

pub(crate) fn ensure_direct_child(root: &Path, target: &Path) -> Result<(), ()> {
    (target.parent() == Some(root) && target.file_name().is_some())
        .then_some(())
        .ok_or(())
}

pub(crate) fn remove_path_accounted(path: &Path) -> Result<RemovalReceipt, ModelInstallError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RemovalReceipt::default());
        }
        Err(error) => return Err(error.into()),
    };
    let removed = measure_path(path, &metadata)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(removed)
}

fn measure_path(path: &Path, metadata: &fs::Metadata) -> Result<RemovalReceipt, ModelInstallError> {
    if metadata.file_type().is_symlink() {
        return Ok(RemovalReceipt {
            removed_files: 1,
            removed_bytes: 0,
        });
    }
    if metadata.is_file() {
        return Ok(RemovalReceipt {
            removed_files: 1,
            removed_bytes: metadata.len(),
        });
    }
    if !metadata.is_dir() {
        return Ok(RemovalReceipt::default());
    }
    let mut receipt = RemovalReceipt::default();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child_metadata = fs::symlink_metadata(entry.path())?;
        receipt.add(measure_path(&entry.path(), &child_metadata)?);
    }
    Ok(receipt)
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationReceipt {
    bytes: u64,
    sha256: String,
    modified_unix_nanos: u128,
}

fn verification_receipt_matches(
    path: &Path,
    model: &ModelDescriptor,
) -> Result<bool, ModelInstallError> {
    let receipt_path = verification_receipt_path(path);
    if !receipt_path.exists() {
        return Ok(false);
    }
    let receipt: VerificationReceipt = match fs::read(&receipt_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(receipt) => receipt,
        None => return Ok(false),
    };
    let metadata = fs::metadata(path)?;
    Ok(metadata.len() == model.artifact.bytes
        && receipt.bytes == model.artifact.bytes
        && receipt.sha256.eq_ignore_ascii_case(&model.artifact.sha256)
        && receipt.modified_unix_nanos == modified_unix_nanos(&metadata)?)
}

fn write_verification_receipt(
    path: &Path,
    model: &ModelDescriptor,
) -> Result<(), ModelInstallError> {
    let receipt = VerificationReceipt {
        bytes: model.artifact.bytes,
        sha256: model.artifact.sha256.clone(),
        modified_unix_nanos: modified_unix_nanos(&fs::metadata(path)?)?,
    };
    let receipt_path = verification_receipt_path(path);
    let temporary = receipt_path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&receipt)?)?;
    if receipt_path.exists() {
        fs::remove_file(&receipt_path)?;
    }
    fs::rename(temporary, receipt_path)?;
    Ok(())
}

fn verification_receipt_path(path: &Path) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(".verified.json");
    PathBuf::from(value)
}

fn modified_unix_nanos(metadata: &fs::Metadata) -> Result<u128, ModelInstallError> {
    Ok(metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| ModelInstallError::VerificationTime(error.to_string()))?
        .as_nanos())
}

fn install_verified_file(
    temporary_path: &Path,
    final_path: &Path,
) -> Result<(), ModelInstallError> {
    let backup_path = final_path.with_extension("previous");
    if backup_path.exists() {
        fs::remove_file(&backup_path)?;
    }

    let had_previous = final_path.exists();
    if had_previous {
        fs::rename(final_path, &backup_path)?;
    }

    if let Err(error) = fs::rename(temporary_path, final_path) {
        if had_previous {
            let _ = fs::rename(&backup_path, final_path);
        }
        return Err(error.into());
    }

    if backup_path.exists() {
        fs::remove_file(backup_path)?;
    }
    Ok(())
}

fn verify_file(path: &Path, model: &ModelDescriptor) -> Result<(), ModelInstallError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != model.artifact.bytes {
        return Err(ModelInstallError::SizeMismatch {
            expected: model.artifact.bytes,
            actual: metadata.len(),
        });
    }

    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(&model.artifact.sha256) {
        return Err(ModelInstallError::ChecksumMismatch {
            expected: model.artifact.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ModelInstallError {
    #[error("model I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("downloaded model size differs: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("downloaded model checksum differs")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("model download failed: {0}")]
    Download(String),
    #[error("model installation was cancelled")]
    Cancelled,
    #[error("model verification timestamp is invalid: {0}")]
    VerificationTime(String),
    #[error("model verification receipt is invalid: {0}")]
    VerificationReceipt(#[from] serde_json::Error),
    #[error("model bundle descriptor is invalid: {0}")]
    InvalidBundle(String),
    #[error("model descriptor is invalid: {0}")]
    InvalidModel(String),
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::Path};

    use rust_speech_api::{ComputeBackend, SpeechEngineKind};
    use tempfile::tempdir;

    use super::{ArtifactFetcher, DownloadProgress, ModelInstallError, ModelInstaller};
    use crate::{ModelArtifact, ModelDescriptor, ModelTier};

    struct BytesFetcher(Vec<u8>);
    struct InterruptedFetcher;

    impl ArtifactFetcher for BytesFetcher {
        fn fetch(
            &self,
            _url: &str,
            destination: &Path,
            expected_bytes: u64,
            progress: &mut dyn FnMut(DownloadProgress),
        ) -> Result<(), ModelInstallError> {
            let mut file = fs::File::create(destination)?;
            file.write_all(&self.0)?;
            progress(DownloadProgress {
                phase: super::InstallPhase::Downloading,
                bytes_received: self.0.len() as u64,
                total_bytes: expected_bytes,
            });
            Ok(())
        }
    }

    impl ArtifactFetcher for InterruptedFetcher {
        fn fetch(
            &self,
            _url: &str,
            destination: &Path,
            _expected_bytes: u64,
            _progress: &mut dyn FnMut(DownloadProgress),
        ) -> Result<(), ModelInstallError> {
            fs::write(destination, b"hel")?;
            Err(ModelInstallError::Download(
                "connection interrupted".to_owned(),
            ))
        }
    }

    fn model() -> ModelDescriptor {
        ModelDescriptor {
            id: "test-model".to_owned(),
            display_name: "Test".to_owned(),
            description: "Fixture".to_owned(),
            engine: SpeechEngineKind::WhisperCpp,
            tier: ModelTier::Lightweight,
            artifact: ModelArtifact {
                file_name: "model.bin".to_owned(),
                url: "https://example.invalid/model.bin".to_owned(),
                bytes: 5,
                sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                    .to_owned(),
            },
            compute_backends: vec![ComputeBackend::Cpu],
            supports_speech_translation: true,
        }
    }

    #[test]
    fn installation_is_verified_before_atomic_move() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().to_owned());
        let model = model();

        let path = installer
            .ensure_installed(&model, &BytesFetcher(b"hello".to_vec()), |_| {})
            .unwrap();

        assert_eq!(fs::read(path).unwrap(), b"hello");
        assert!(installer.is_installed(&model).unwrap());
        assert!(super::verification_receipt_path(&installer.model_path(&model)).exists());
    }

    #[test]
    fn invalid_download_never_replaces_the_model() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().to_owned());
        let model = model();
        fs::write(installer.model_path(&model), b"older").unwrap();

        let result = installer.ensure_installed(&model, &BytesFetcher(b"wrong".to_vec()), |_| {});

        assert!(matches!(
            result,
            Err(ModelInstallError::ChecksumMismatch { .. })
        ));
        assert_eq!(fs::read(installer.model_path(&model)).unwrap(), b"older");
        assert!(
            !installer
                .model_path(&model)
                .with_extension("download")
                .exists()
        );
    }

    #[test]
    fn valid_download_replaces_an_invalid_previous_file() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().to_owned());
        let model = model();
        fs::write(installer.model_path(&model), b"older").unwrap();

        installer
            .ensure_installed(&model, &BytesFetcher(b"hello".to_vec()), |_| {})
            .unwrap();

        assert_eq!(fs::read(installer.model_path(&model)).unwrap(), b"hello");
        assert!(
            !installer
                .model_path(&model)
                .with_extension("previous")
                .exists()
        );
    }

    #[test]
    fn interrupted_download_is_preserved_for_http_range_resume() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().to_owned());
        let model = model();

        assert!(matches!(
            installer.ensure_installed(&model, &InterruptedFetcher, |_| {}),
            Err(ModelInstallError::Download(_))
        ));
        assert_eq!(
            fs::read(installer.model_path(&model).with_extension("download")).unwrap(),
            b"hel"
        );
    }

    #[test]
    fn remove_deletes_only_files_owned_by_the_model_descriptor() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().to_owned());
        let model = model();
        let final_path = installer.model_path(&model);
        let receipt = super::verification_receipt_path(&final_path);
        let receipt_temporary = receipt.with_extension("tmp");
        let partial = final_path.with_extension("download");
        let previous = final_path.with_extension("previous");
        let unrelated = directory.path().join("keep-me.bin");

        fs::write(&final_path, b"hello").unwrap();
        fs::write(&receipt, b"four").unwrap();
        fs::write(&receipt_temporary, b"tri").unwrap();
        fs::write(&partial, b"tw").unwrap();
        fs::write(&previous, b"1").unwrap();
        fs::write(&unrelated, b"untouched").unwrap();

        let removed = installer.remove(&model).unwrap();

        assert_eq!(removed.removed_files, 5);
        assert_eq!(removed.removed_bytes, 15);
        assert!(!final_path.exists());
        assert!(!receipt.exists());
        assert!(!receipt_temporary.exists());
        assert!(!partial.exists());
        assert!(!previous.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"untouched");
    }

    #[test]
    fn remove_rejects_a_model_file_name_that_escapes_the_root() {
        let directory = tempdir().unwrap();
        let installer = ModelInstaller::new(directory.path().join("models"));
        let mut model = model();
        model.artifact.file_name = "../keep-me.bin".to_owned();
        let sentinel = directory.path().join("keep-me.bin");
        fs::write(&sentinel, b"untouched").unwrap();

        assert!(matches!(
            installer.remove(&model),
            Err(ModelInstallError::InvalidModel(_))
        ));
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
    }
}
