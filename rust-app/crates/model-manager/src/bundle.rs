use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ArtifactFetcher, DownloadProgress, InstallPhase, ModelInstallError, RemovalReceipt,
    installer::{ensure_direct_child, remove_path_accounted, validate_direct_child_name},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleArtifact {
    pub relative_path: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDescriptor {
    pub id: String,
    pub artifacts: Vec<BundleArtifact>,
}

impl BundleDescriptor {
    pub fn validate(&self) -> Result<(), ModelInstallError> {
        if validate_direct_child_name(&self.id).is_err() || self.artifacts.is_empty() {
            return Err(ModelInstallError::InvalidBundle(self.id.clone()));
        }
        let mut paths = BTreeMap::new();
        for artifact in &self.artifacts {
            let relative = Path::new(&artifact.relative_path);
            if artifact.url.trim().is_empty()
                || artifact.bytes == 0
                || artifact.sha256.len() != 64
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || paths.insert(&artifact.relative_path, ()).is_some()
            {
                return Err(ModelInstallError::InvalidBundle(self.id.clone()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.artifacts.iter().map(|artifact| artifact.bytes).sum()
    }
}

#[derive(Debug, Clone)]
pub struct BundleInstaller {
    root: PathBuf,
}

impl BundleInstaller {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn bundle_path(&self, bundle: &BundleDescriptor) -> PathBuf {
        self.root.join(&bundle.id)
    }

    pub fn is_installed(&self, bundle: &BundleDescriptor) -> Result<bool, ModelInstallError> {
        bundle.validate()?;
        let root = self.bundle_path(bundle);
        if !root.is_dir() {
            return Ok(false);
        }
        if bundle_receipt_matches(&root, bundle)? {
            return Ok(true);
        }
        for artifact in &bundle.artifacts {
            if verify_artifact(&root.join(&artifact.relative_path), artifact).is_err() {
                let _ = fs::remove_file(bundle_receipt_path(&root));
                return Ok(false);
            }
        }
        write_bundle_receipt(&root, bundle)?;
        Ok(true)
    }

    pub fn ensure_installed(
        &self,
        bundle: &BundleDescriptor,
        fetcher: &dyn ArtifactFetcher,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<PathBuf, ModelInstallError> {
        bundle.validate()?;
        if self.is_installed(bundle)? {
            progress(DownloadProgress {
                phase: InstallPhase::Ready,
                bytes_received: bundle.total_bytes(),
                total_bytes: bundle.total_bytes(),
            });
            return Ok(self.bundle_path(bundle));
        }

        fs::create_dir_all(&self.root)?;
        let final_path = self.bundle_path(bundle);
        let temporary_path = self.root.join(format!(".{}.download", bundle.id));
        remove_directory_if_present(&temporary_path)?;
        fs::create_dir_all(&temporary_path)?;

        let total_bytes = bundle.total_bytes();
        let mut completed_bytes = 0_u64;
        let result = (|| {
            for artifact in &bundle.artifacts {
                let destination = temporary_path.join(&artifact.relative_path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fetcher.fetch(
                    &artifact.url,
                    &destination,
                    artifact.bytes,
                    &mut |artifact_progress| {
                        progress(DownloadProgress {
                            phase: artifact_progress.phase,
                            bytes_received: completed_bytes
                                .saturating_add(artifact_progress.bytes_received),
                            total_bytes,
                        });
                    },
                )?;
                progress(DownloadProgress {
                    phase: InstallPhase::Verifying,
                    bytes_received: completed_bytes.saturating_add(artifact.bytes),
                    total_bytes,
                });
                verify_artifact(&destination, artifact)?;
                completed_bytes = completed_bytes.saturating_add(artifact.bytes);
            }

            progress(DownloadProgress {
                phase: InstallPhase::Installing,
                bytes_received: total_bytes,
                total_bytes,
            });
            write_bundle_receipt(&temporary_path, bundle)?;
            install_verified_directory(&temporary_path, &final_path)?;
            progress(DownloadProgress {
                phase: InstallPhase::Ready,
                bytes_received: total_bytes,
                total_bytes,
            });
            Ok(final_path)
        })();

        if result.is_err() {
            let _ = remove_directory_if_present(&temporary_path);
        }
        result
    }

    pub fn remove(&self, bundle: &BundleDescriptor) -> Result<(), ModelInstallError> {
        bundle.validate()?;
        remove_directory_if_present(&self.bundle_path(bundle))
    }

    /// Removes the installed bundle together with exact staging and backup
    /// siblings left by an interrupted atomic install.
    pub fn remove_complete(
        &self,
        bundle: &BundleDescriptor,
    ) -> Result<RemovalReceipt, ModelInstallError> {
        bundle.validate()?;
        let targets = [
            self.bundle_path(bundle),
            self.root.join(format!(".{}.download", bundle.id)),
            self.root.join(format!("{}.previous", bundle.id)),
        ];
        let mut removed = RemovalReceipt::default();
        for target in targets {
            ensure_direct_child(&self.root, &target)
                .map_err(|()| ModelInstallError::InvalidBundle(bundle.id.clone()))?;
            removed.add(remove_path_accounted(&target)?);
        }
        Ok(removed)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BundleReceipt {
    bundle_id: String,
    artifacts: Vec<ArtifactReceipt>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactReceipt {
    relative_path: String,
    bytes: u64,
    sha256: String,
    modified_unix_nanos: u128,
}

fn bundle_receipt_matches(
    root: &Path,
    bundle: &BundleDescriptor,
) -> Result<bool, ModelInstallError> {
    let receipt_path = bundle_receipt_path(root);
    let receipt: BundleReceipt = match fs::read(&receipt_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(receipt) => receipt,
        None => return Ok(false),
    };
    if receipt.bundle_id != bundle.id || receipt.artifacts.len() != bundle.artifacts.len() {
        return Ok(false);
    }
    for artifact in &bundle.artifacts {
        let Some(stored) = receipt
            .artifacts
            .iter()
            .find(|stored| stored.relative_path == artifact.relative_path)
        else {
            return Ok(false);
        };
        let path = root.join(&artifact.relative_path);
        let Ok(metadata) = fs::metadata(path) else {
            return Ok(false);
        };
        if stored.bytes != artifact.bytes
            || metadata.len() != artifact.bytes
            || !stored.sha256.eq_ignore_ascii_case(&artifact.sha256)
            || stored.modified_unix_nanos != modified_unix_nanos(&metadata)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write_bundle_receipt(root: &Path, bundle: &BundleDescriptor) -> Result<(), ModelInstallError> {
    let artifacts = bundle
        .artifacts
        .iter()
        .map(|artifact| {
            let metadata = fs::metadata(root.join(&artifact.relative_path))?;
            Ok(ArtifactReceipt {
                relative_path: artifact.relative_path.clone(),
                bytes: artifact.bytes,
                sha256: artifact.sha256.clone(),
                modified_unix_nanos: modified_unix_nanos(&metadata)?,
            })
        })
        .collect::<Result<Vec<_>, ModelInstallError>>()?;
    let receipt = BundleReceipt {
        bundle_id: bundle.id.clone(),
        artifacts,
    };
    let path = bundle_receipt_path(root);
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&receipt)?)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn bundle_receipt_path(root: &Path) -> PathBuf {
    root.join("bundle.verified.json")
}

fn verify_artifact(path: &Path, artifact: &BundleArtifact) -> Result<(), ModelInstallError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != artifact.bytes {
        return Err(ModelInstallError::SizeMismatch {
            expected: artifact.bytes,
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
    if !actual.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(ModelInstallError::ChecksumMismatch {
            expected: artifact.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn install_verified_directory(
    temporary_path: &Path,
    final_path: &Path,
) -> Result<(), ModelInstallError> {
    let mut backup_name = OsString::from(final_path.as_os_str());
    backup_name.push(".previous");
    let backup_path = PathBuf::from(backup_name);
    remove_directory_if_present(&backup_path)?;
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
    remove_directory_if_present(&backup_path)?;
    Ok(())
}

fn remove_directory_if_present(path: &Path) -> Result<(), ModelInstallError> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn modified_unix_nanos(metadata: &fs::Metadata) -> Result<u128, ModelInstallError> {
    Ok(metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| ModelInstallError::VerificationTime(error.to_string()))?
        .as_nanos())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, path::Path};

    use tempfile::tempdir;

    use super::{BundleArtifact, BundleDescriptor, BundleInstaller};
    use crate::{ArtifactFetcher, DownloadProgress, ModelInstallError};

    struct MapFetcher(HashMap<String, Vec<u8>>);

    impl ArtifactFetcher for MapFetcher {
        fn fetch(
            &self,
            url: &str,
            destination: &Path,
            expected_bytes: u64,
            progress: &mut dyn FnMut(DownloadProgress),
        ) -> Result<(), ModelInstallError> {
            let bytes = self
                .0
                .get(url)
                .ok_or_else(|| ModelInstallError::Download("missing fixture".to_owned()))?;
            fs::write(destination, bytes)?;
            progress(DownloadProgress {
                phase: crate::InstallPhase::Downloading,
                bytes_received: bytes.len() as u64,
                total_bytes: expected_bytes,
            });
            Ok(())
        }
    }

    fn bundle() -> BundleDescriptor {
        BundleDescriptor {
            id: "pair-tiny".to_owned(),
            artifacts: vec![
                BundleArtifact {
                    relative_path: "ru-en/model.bin".to_owned(),
                    url: "fixture:model".to_owned(),
                    bytes: 5,
                    sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                        .to_owned(),
                },
                BundleArtifact {
                    relative_path: "ru-en/config.json".to_owned(),
                    url: "fixture:config".to_owned(),
                    bytes: 2,
                    sha256: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
                        .to_owned(),
                },
            ],
        }
    }

    #[test]
    fn installs_and_verifies_a_multi_file_bundle_atomically() {
        let directory = tempdir().unwrap();
        let installer = BundleInstaller::new(directory.path().to_owned());
        let fetcher = MapFetcher(HashMap::from([
            ("fixture:model".to_owned(), b"hello".to_vec()),
            ("fixture:config".to_owned(), b"{}".to_vec()),
        ]));
        let path = installer
            .ensure_installed(&bundle(), &fetcher, |_| {})
            .unwrap();

        assert_eq!(fs::read(path.join("ru-en/model.bin")).unwrap(), b"hello");
        assert!(installer.is_installed(&bundle()).unwrap());
    }

    #[test]
    fn rejects_path_traversal() {
        let mut invalid = bundle();
        invalid.artifacts[0].relative_path = "../escape.bin".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn rejects_bundle_id_path_traversal() {
        let mut invalid = bundle();
        invalid.id = "../escape".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn complete_removal_deletes_final_partial_and_backup_but_not_a_sibling() {
        let directory = tempdir().unwrap();
        let installer = BundleInstaller::new(directory.path().to_owned());
        let bundle = bundle();
        let final_path = installer.bundle_path(&bundle);
        let partial = directory.path().join(".pair-tiny.download");
        let previous = directory.path().join("pair-tiny.previous");
        let unrelated = directory.path().join("keep-me");

        fs::create_dir_all(&final_path).unwrap();
        fs::write(final_path.join("model.bin"), b"hello").unwrap();
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("partial.bin"), b"abc").unwrap();
        fs::create_dir_all(&previous).unwrap();
        fs::write(previous.join("old.bin"), b"1234").unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("sentinel"), b"untouched").unwrap();

        let removed = installer.remove_complete(&bundle).unwrap();

        assert_eq!(removed.removed_files, 3);
        assert_eq!(removed.removed_bytes, 12);
        assert!(!final_path.exists());
        assert!(!partial.exists());
        assert!(!previous.exists());
        assert_eq!(fs::read(unrelated.join("sentinel")).unwrap(), b"untouched");
    }
}
