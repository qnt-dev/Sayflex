use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::{ArtifactFetcher, CancelToken, DownloadProgress, InstallPhase, ModelInstallError};

pub const CUDA_DOWNLOAD_BYTES: u64 = 393_706_755;
pub const CUDA_LICENSE_URL: &str = "https://docs.nvidia.com/cuda/eula/index.html";
const RECEIPT: &str = ".sayflex-cuda-runtime.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeFile {
    archive_path: String,
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeManifest {
    id: String,
    url: String,
    archive_bytes: u64,
    archive_sha256: String,
    files: Vec<RuntimeFile>,
}

/// A pinned, application-owned CUDA component. This installs no driver or Toolkit.
pub struct CudaRuntimeManager {
    root: PathBuf,
    manifest: RuntimeManifest,
}

impl CudaRuntimeManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            manifest: pinned_manifest(),
        }
    }

    pub fn runtime_path(&self) -> PathBuf {
        self.root.join(&self.manifest.id)
    }

    /// The first probe verifies content. Subsequent probes in this process reuse
    /// that verification only while all file and receipt identities are unchanged.
    pub fn is_installed(&self) -> bool {
        self.installed(&CancelToken::default()).unwrap_or(false)
    }

    pub fn ensure_installed(
        &self,
        fetcher: &dyn ArtifactFetcher,
        cancellation: &CancelToken,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<PathBuf, ModelInstallError> {
        cancelled(cancellation)?;
        if self.installed(cancellation)? {
            progress(self.progress(InstallPhase::Ready));
            return Ok(self.runtime_path());
        }
        fs::create_dir_all(&self.root)?;
        regular_directory(&self.root)?;
        // Windows denies another installer access while this handle is alive;
        // the persistent lock file is harmless after a crash (no stale lock).
        let lock_path = self
            .root
            .join(format!(".{}.install.lock", self.manifest.id));
        reject_link_if_present(&lock_path)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let _lock = options.open(lock_path)?;
        if self.installed(cancellation)? {
            progress(self.progress(InstallPhase::Ready));
            return Ok(self.runtime_path());
        }
        let archive = self.root.join(format!("{}.zip.download", self.manifest.id));
        reject_link_if_present(&archive)?;
        if archive
            .metadata()
            .is_ok_and(|m| m.len() > self.manifest.archive_bytes)
        {
            fs::remove_file(&archive)?;
        }
        let mut archive_verified = false;
        if archive
            .metadata()
            .is_ok_and(|m| m.len() == self.manifest.archive_bytes)
        {
            progress(self.progress(InstallPhase::Verifying));
            match verify_file(
                &archive,
                self.manifest.archive_bytes,
                &self.manifest.archive_sha256,
                cancellation,
            ) {
                Ok(()) => archive_verified = true,
                Err(ModelInstallError::Cancelled) => return Err(ModelInstallError::Cancelled),
                Err(_) => {
                    fs::remove_file(&archive)?;
                }
            }
        }
        if !archive_verified {
            // HttpFetcher resumes this file; it must share `cancellation` with
            // its caller to interrupt a stalled response as well as local work.
            fetcher.fetch(
                &self.manifest.url,
                &archive,
                self.manifest.archive_bytes,
                &mut progress,
            )?;
            progress(self.progress(InstallPhase::Verifying));
            let verified = verify_file(
                &archive,
                self.manifest.archive_bytes,
                &self.manifest.archive_sha256,
                cancellation,
            );
            if let Err(error) = verified {
                if !matches!(error, ModelInstallError::Cancelled) {
                    let _ = fs::remove_file(&archive);
                }
                return Err(error);
            }
        }
        cancelled(cancellation)?;
        progress(self.progress(InstallPhase::Installing));
        cancelled(cancellation)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ModelInstallError::VerificationTime(e.to_string()))?
            .as_nanos();
        let stage = self.root.join(format!(
            ".{}.staging-{}-{nonce}",
            self.manifest.id,
            std::process::id()
        ));
        fs::create_dir(&stage)?;
        let staged = StagedDirectory(stage);
        self.extract_verified(&archive, &staged.0, cancellation)?;
        fs::write(staged.0.join(RECEIPT), serde_json::to_vec(&self.manifest)?)?;
        cancelled(cancellation)?;
        let destination = self.runtime_path();
        reject_link_if_present(&destination)?;
        let previous = self.root.join(format!(
            ".{}.previous-{}-{nonce}",
            self.manifest.id,
            std::process::id()
        ));
        let had_previous = destination.exists();
        if had_previous {
            fs::rename(&destination, &previous)?;
        }
        if let Err(error) = fs::rename(&staged.0, &destination) {
            if had_previous {
                let _ = fs::rename(&previous, &destination);
            }
            return Err(error.into());
        }
        if had_previous {
            let _ = fs::remove_dir_all(&previous);
        }
        // Cache only after the verified directory has been published; receipts
        // from disk alone are never authority to skip the first hash check.
        remember_verified(&destination, &self.manifest)?;
        // Keep partial/verified archives on failure or cancellation for retry,
        // but do not permanently duplicate the component's compressed payload.
        let _ = fs::remove_file(&archive);
        progress(self.progress(InstallPhase::Ready));
        Ok(destination)
    }

    fn progress(&self, phase: InstallPhase) -> DownloadProgress {
        DownloadProgress {
            phase,
            bytes_received: self.manifest.archive_bytes,
            total_bytes: self.manifest.archive_bytes,
        }
    }

    fn installed(&self, cancellation: &CancelToken) -> Result<bool, ModelInstallError> {
        cancelled(cancellation)?;
        let path = self.runtime_path();
        let Some(identity) = installation_identity(&path, &self.manifest) else {
            return Ok(false);
        };
        let receipt = fs::read(path.join(RECEIPT))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RuntimeManifest>(&bytes).ok());
        if receipt.as_ref() != Some(&self.manifest) {
            return Ok(false);
        }
        if verified_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&identity)
        {
            return Ok(true);
        }
        for file in &self.manifest.files {
            match verify_file(
                &path.join(&file.name),
                file.bytes,
                &file.sha256,
                cancellation,
            ) {
                Ok(()) => {}
                Err(ModelInstallError::Cancelled) => return Err(ModelInstallError::Cancelled),
                Err(_) => return Ok(false),
            }
        }
        // A concurrent replacement must not get the identity of the bytes we
        // just checked. Do not hold the cache lock while hashing large DLLs.
        if installation_identity(&path, &self.manifest).as_ref() != Some(&identity) {
            return Ok(false);
        }
        let mut cache = verified_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|entry| entry.path != identity.path);
        cache.push(identity);
        Ok(true)
    }

    fn extract_verified(
        &self,
        archive: &Path,
        stage: &Path,
        cancellation: &CancelToken,
    ) -> Result<(), ModelInstallError> {
        let mut zip = ZipArchive::new(File::open(archive)?).map_err(invalid_zip)?;
        let mut found = HashSet::new();
        for index in 0..zip.len() {
            cancelled(cancellation)?;
            let mut entry = zip.by_index(index).map_err(invalid_zip)?;
            validate_entry(&entry)?;
            let Some(expected) = self
                .manifest
                .files
                .iter()
                .find(|file| file.archive_path == entry.name())
            else {
                continue;
            };
            if !found.insert(expected.name.clone())
                || entry.is_dir()
                || entry.size() != expected.bytes
            {
                return Err(ModelInstallError::InvalidBundle(
                    "duplicate or invalid CUDA archive file".into(),
                ));
            }
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(stage.join(&expected.name))?;
            let mut hasher = Sha256::new();
            let mut copied = 0_u64;
            let mut buffer = vec![0_u8; 1024 * 1024];
            loop {
                cancelled(cancellation)?;
                let count = entry.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                if copied > expected.bytes {
                    return Err(ModelInstallError::SizeMismatch {
                        expected: expected.bytes,
                        actual: copied,
                    });
                }
                hasher.update(&buffer[..count]);
                output.write_all(&buffer[..count])?;
            }
            output.sync_all()?;
            check_digest(
                copied,
                format!("{:x}", hasher.finalize()),
                expected.bytes,
                &expected.sha256,
            )?;
        }
        if found.len() != self.manifest.files.len() {
            return Err(ModelInstallError::InvalidBundle(
                "CUDA archive is missing required runtime or license files".into(),
            ));
        }
        Ok(())
    }
}

fn pinned_manifest() -> RuntimeManifest {
    // NVIDIA CUDA 13.3.1 redistrib manifest; both DLL hashes also match the
    // previously bundled Toolkit build, preserving the execution binaries.
    let package = "libcublas-windows-x86_64-13.6.0.2-archive";
    RuntimeManifest {
        id: "cuda-cublas-13.6.0.2".into(),
        url: format!(
            "https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/{package}.zip"
        ),
        archive_bytes: CUDA_DOWNLOAD_BYTES,
        archive_sha256: "62e9fa30560c8f0a28e0cdcf9d6fc1fed347bcfab8847239b9ae1fdc1d86408a".into(),
        files: [
            (
                "bin/x64/cublas64_13.dll",
                "cublas64_13.dll",
                52_697_712,
                "8c6bac24474af29627ec96500025a5d410a1b7e8ffa00e1fe13b1781fc6b3ddc",
            ),
            (
                "bin/x64/cublasLt64_13.dll",
                "cublasLt64_13.dll",
                463_655_536,
                "e323abd89e0f03c1db91a09d7aeb1928de08e1f86192cc713b335fefe3e368a9",
            ),
            (
                "LICENSE",
                "LICENSE",
                68_070,
                "088381bc2d891e719a2a9398645b00bb45f3b24231473a8283ac7e3e66b8a028",
            ),
        ]
        .into_iter()
        .map(|(path, name, bytes, sha256)| RuntimeFile {
            archive_path: format!("{package}/{path}"),
            name: name.into(),
            bytes,
            sha256: sha256.into(),
        })
        .collect(),
    }
}

fn validate_entry(entry: &zip::read::ZipFile<'_, File>) -> Result<(), ModelInstallError> {
    let name = entry.name();
    let mode = entry.unix_mode().unwrap_or(0) & 0o170_000;
    if entry.enclosed_name().is_none()
        || name.contains(['\\', ':'])
        || name.split('/').any(|part| part == "..")
        || !matches!(mode, 0 | 0o100_000 | 0o040_000)
    {
        return Err(ModelInstallError::InvalidBundle(
            "CUDA archive contains an unsafe path or link".into(),
        ));
    }
    Ok(())
}

fn invalid_zip(error: zip::result::ZipError) -> ModelInstallError {
    ModelInstallError::InvalidBundle(format!("invalid CUDA ZIP: {error}"))
}

fn cancelled(cancellation: &CancelToken) -> Result<(), ModelInstallError> {
    if cancellation.is_cancelled() {
        Err(ModelInstallError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_digest(
    actual_bytes: u64,
    actual: String,
    expected_bytes: u64,
    expected: &str,
) -> Result<(), ModelInstallError> {
    if actual_bytes != expected_bytes {
        return Err(ModelInstallError::SizeMismatch {
            expected: expected_bytes,
            actual: actual_bytes,
        });
    }
    if actual != expected {
        return Err(ModelInstallError::ChecksumMismatch {
            expected: expected.into(),
            actual,
        });
    }
    Ok(())
}

fn verify_file(
    path: &Path,
    expected_bytes: u64,
    expected: &str,
    cancellation: &CancelToken,
) -> Result<(), ModelInstallError> {
    let metadata = regular_file(path)?;
    if metadata.len() != expected_bytes {
        return Err(ModelInstallError::SizeMismatch {
            expected: expected_bytes,
            actual: metadata.len(),
        });
    }
    let mut file = File::open(path)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    loop {
        cancelled(cancellation)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hasher.update(&buffer[..count]);
    }
    check_digest(
        bytes,
        format!("{:x}", hasher.finalize()),
        expected_bytes,
        expected,
    )
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn reject_link_if_present(path: &Path) -> Result<(), ModelInstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Err(ModelInstallError::InvalidBundle(
            "CUDA cache contains a link or reparse point".into(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn regular_file(path: &Path) -> Result<fs::Metadata, ModelInstallError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(ModelInstallError::InvalidBundle(
            "CUDA component must contain regular files".into(),
        ));
    }
    Ok(metadata)
}

fn regular_directory(path: &Path) -> Result<(), ModelInstallError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(ModelInstallError::InvalidBundle(
            "CUDA component must be an ordinary directory".into(),
        ));
    }
    Ok(())
}

struct StagedDirectory(PathBuf);
impl Drop for StagedDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    bytes: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
}

#[derive(PartialEq, Eq)]
struct InstallationIdentity {
    path: PathBuf,
    manifest: RuntimeManifest,
    files: Vec<FileIdentity>,
}

fn installation_identity(path: &Path, manifest: &RuntimeManifest) -> Option<InstallationIdentity> {
    regular_directory(path).ok()?;
    // This directory is placed on the worker's DLL search path. An additional
    // unverified DLL must invalidate even a warm integrity-cache entry.
    let entries = fs::read_dir(path)
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if entries.len() != manifest.files.len() + 1
        || entries.iter().any(|entry| {
            let name = entry.file_name();
            name != RECEIPT && !manifest.files.iter().any(|file| name == file.name.as_str())
        })
    {
        return None;
    }
    let files = manifest
        .files
        .iter()
        .map(|file| file.name.as_str())
        .chain(std::iter::once(RECEIPT))
        .map(|name| {
            let m = regular_file(&path.join(name)).ok()?;
            Some(FileIdentity {
                bytes: m.len(),
                modified: m.modified().ok()?,
                created: m.created().ok(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(InstallationIdentity {
        path: path.canonicalize().ok()?,
        manifest: manifest.clone(),
        files,
    })
}

fn verified_cache() -> &'static Mutex<Vec<InstallationIdentity>> {
    static CACHE: OnceLock<Mutex<Vec<InstallationIdentity>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn remember_verified(path: &Path, manifest: &RuntimeManifest) -> Result<(), ModelInstallError> {
    let identity = installation_identity(path, manifest)
        .ok_or_else(|| ModelInstallError::InvalidBundle("published CUDA files changed".into()))?;
    let mut cache = verified_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|entry| entry.path != identity.path);
    cache.push(identity);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InstallPhase;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Cursor, Write},
        path::Path,
    };
    use tempfile::tempdir;
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    #[ignore = "requires an explicitly supplied NVIDIA archive; writes only to a temporary directory"]
    fn installs_the_official_pinned_archive_without_loading_cuda() {
        struct LocalArchive(PathBuf);
        impl ArtifactFetcher for LocalArchive {
            fn fetch(
                &self,
                _: &str,
                destination: &Path,
                _: u64,
                _: &mut dyn FnMut(DownloadProgress),
            ) -> Result<(), ModelInstallError> {
                fs::copy(&self.0, destination)?;
                Ok(())
            }
        }
        let archive = std::env::var_os("SAYFLEX_CUDA_RUNTIME_ARCHIVE")
            .expect("set the local NVIDIA archive path");
        let root = tempdir().unwrap();
        let manager = CudaRuntimeManager::new(root.path().to_owned());
        let path = manager
            .ensure_installed(
                &LocalArchive(archive.into()),
                &CancelToken::default(),
                |_| {},
            )
            .unwrap();
        assert!(manager.is_installed());
        assert!(
            !manager
                .root
                .join(format!("{}.zip.download", manager.manifest.id))
                .exists()
        );
        // Force a cold integrity probe; an on-disk receipt alone cannot replace
        // checking the real published DLL contents.
        verified_cache()
            .lock()
            .unwrap()
            .retain(|entry| entry.path != path);
        assert!(manager.is_installed());
        assert_eq!(fs::read_dir(path).unwrap().count(), 4);
    }

    struct BytesFetcher(Vec<u8>);
    impl ArtifactFetcher for BytesFetcher {
        fn fetch(
            &self,
            _url: &str,
            destination: &Path,
            total: u64,
            progress: &mut dyn FnMut(DownloadProgress),
        ) -> Result<(), ModelInstallError> {
            fs::write(destination, &self.0)?;
            progress(DownloadProgress {
                phase: InstallPhase::Downloading,
                bytes_received: self.0.len() as u64,
                total_bytes: total,
            });
            Ok(())
        }
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture(root: &Path, extra: Option<&str>) -> (CudaRuntimeManager, Vec<u8>) {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files = Vec::new();
        for (name, bytes) in [
            ("cublas64_13.dll", b"test cublas".as_slice()),
            ("cublasLt64_13.dll", b"test cublasLt".as_slice()),
            ("LICENSE", b"test NVIDIA license".as_slice()),
        ] {
            let archive_path = format!("package/{name}");
            zip.start_file(&archive_path, SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
            files.push(RuntimeFile {
                archive_path,
                name: name.into(),
                bytes: bytes.len() as u64,
                sha256: digest(bytes),
            });
        }
        if let Some(name) = extra {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"untrusted").unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let manager = CudaRuntimeManager {
            root: root.to_owned(),
            manifest: RuntimeManifest {
                id: "cuda-test-version".into(),
                url: "https://example.invalid/pinned.zip".into(),
                archive_bytes: bytes.len() as u64,
                archive_sha256: digest(&bytes),
                files,
            },
        };
        (manager, bytes)
    }

    #[test]
    fn publishes_only_verified_dlls_and_license_and_detects_installed_corruption() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        let mut ready = false;
        manager
            .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |p| {
                if p.phase == InstallPhase::Ready {
                    ready = true;
                    assert!(manager.is_installed());
                } else {
                    assert!(
                        !manager.runtime_path().exists(),
                        "no activation before verification"
                    );
                }
            })
            .unwrap();
        assert!(ready);
        assert!(
            !manager
                .root
                .join(format!("{}.zip.download", manager.manifest.id))
                .exists()
        );
        assert_eq!(
            fs::read(manager.runtime_path().join("LICENSE")).unwrap(),
            b"test NVIDIA license"
        );
        assert!(manager.is_installed());
        fs::write(
            manager.runtime_path().join("cublas64_13.dll"),
            b"bad! cublas",
        )
        .unwrap();
        assert!(!manager.is_installed());
    }

    #[test]
    fn rejects_wrong_archive_hash_without_publishing_and_retry_recovers() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        assert!(matches!(
            manager.ensure_installed(&BytesFetcher(corrupt), &CancelToken::default(), |_| {}),
            Err(ModelInstallError::ChecksumMismatch { .. })
        ));
        assert!(!manager.runtime_path().exists());
        manager
            .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
            .unwrap();
        assert!(manager.is_installed());
    }

    #[test]
    fn rejects_unsafe_zip_paths_even_in_unselected_entries() {
        for name in [
            "../escaped.dll",
            "C:/escaped.dll",
            "package/../../escaped.dll",
            "package\\..\\escaped.dll",
        ] {
            let root = tempdir().unwrap();
            let (manager, bytes) = fixture(root.path(), Some(name));
            assert!(
                manager
                    .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
                    .is_err()
            );
            assert!(!manager.runtime_path().exists());
        }
    }

    #[test]
    fn cancelled_install_never_publishes_and_retry_uses_verified_download() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        let cancellation = CancelToken::default();
        let result = manager.ensure_installed(&BytesFetcher(bytes.clone()), &cancellation, |p| {
            if p.phase == InstallPhase::Installing {
                cancellation.cancel();
            }
        });
        assert!(matches!(result, Err(ModelInstallError::Cancelled)));
        assert!(!manager.runtime_path().exists());
        assert!(
            manager
                .root
                .join(format!("{}.zip.download", manager.manifest.id))
                .is_file()
        );
        cancellation.reset();
        struct Offline;
        impl ArtifactFetcher for Offline {
            fn fetch(
                &self,
                _: &str,
                _: &Path,
                _: u64,
                _: &mut dyn FnMut(DownloadProgress),
            ) -> Result<(), ModelInstallError> {
                Err(ModelInstallError::Download("offline".into()))
            }
        }
        manager
            .ensure_installed(&Offline, &cancellation, |_| {})
            .unwrap();
        assert!(manager.is_installed());
    }

    #[test]
    fn rejects_a_symlink_zip_entry_even_when_not_selected() {
        let root = tempdir().unwrap();
        let (mut manager, bytes) = fixture(root.path(), None);
        let mut zip = ZipWriter::new_append(Cursor::new(bytes)).unwrap();
        zip.add_symlink(
            "package/linked",
            "../../outside",
            SimpleFileOptions::default(),
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        manager.manifest.archive_bytes = bytes.len() as u64;
        manager.manifest.archive_sha256 = digest(&bytes);
        assert!(
            manager
                .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
                .is_err()
        );
        assert!(!manager.runtime_path().exists());
    }

    #[test]
    fn rejects_wrong_inner_dll_hash_and_never_emits_ready() {
        let root = tempdir().unwrap();
        let (mut manager, bytes) = fixture(root.path(), None);
        manager.manifest.files[0].sha256 = "00".repeat(32);
        let mut ready = false;
        let result = manager.ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |p| {
            ready |= p.phase == InstallPhase::Ready;
        });
        assert!(matches!(
            result,
            Err(ModelInstallError::ChecksumMismatch { .. })
        ));
        assert!(!ready);
        assert!(!manager.runtime_path().exists());
    }

    #[test]
    fn extracted_files_without_matching_receipt_are_not_installed() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        manager
            .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
            .unwrap();
        fs::remove_file(manager.runtime_path().join(".sayflex-cuda-runtime.json")).unwrap();
        assert!(!manager.is_installed());
    }

    #[test]
    fn a_receipt_for_another_component_version_cannot_activate_existing_files() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        manager
            .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
            .unwrap();
        let mut receipt = manager.manifest.clone();
        receipt.id = "different-runtime-version".into();
        fs::write(
            manager.runtime_path().join(RECEIPT),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        assert!(!manager.is_installed());
    }

    #[test]
    fn unverified_extra_dll_in_cached_search_directory_blocks_activation() {
        let root = tempdir().unwrap();
        let (manager, bytes) = fixture(root.path(), None);
        manager
            .ensure_installed(&BytesFetcher(bytes), &CancelToken::default(), |_| {})
            .unwrap();
        fs::write(manager.runtime_path().join("nvcuda.dll"), b"not verified").unwrap();
        assert!(!manager.is_installed());
    }
}
