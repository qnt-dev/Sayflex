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

pub const CUDA_DOWNLOAD_BYTES: u64 = 393_706_755 + 2_589_792;
pub const CUDA_LICENSE_URL: &str = "https://docs.nvidia.com/cuda/eula/index.html";
const RECEIPT: &str = ".sayflex-cuda-runtime.json";
/// Earlier component versions. Their verified DLLs are reused instead of being
/// downloaded again, and their directories are removed once the new one is live.
const SUPERSEDED: &[&str] = &["cuda-cublas-13.6.0.2"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeFile {
    archive_path: String,
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeArchive {
    url: String,
    bytes: u64,
    sha256: String,
    files: Vec<RuntimeFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeManifest {
    id: String,
    archives: Vec<RuntimeArchive>,
}

impl RuntimeManifest {
    fn files(&self) -> impl Iterator<Item = &RuntimeFile> {
        self.archives
            .iter()
            .flat_map(|archive| archive.files.iter())
    }

    fn download_bytes(&self) -> u64 {
        self.archives.iter().map(|archive| archive.bytes).sum()
    }
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
        let total = self.manifest.download_bytes();
        let mut offset = 0;
        let mut downloaded = Vec::new();
        for (index, archive) in self.manifest.archives.iter().enumerate() {
            if !self.reuse_superseded(archive, &staged.0, cancellation)? {
                let path = self.archive_path(index);
                // Report one continuous download across all archives.
                download_verified(fetcher, archive, &path, cancellation, &mut |step| {
                    progress(DownloadProgress {
                        phase: step.phase,
                        bytes_received: offset + step.bytes_received.min(archive.bytes),
                        total_bytes: total,
                    })
                })?;
                downloaded.push((archive, path));
            }
            offset += archive.bytes;
        }
        cancelled(cancellation)?;
        progress(self.progress(InstallPhase::Installing));
        for (archive, path) in &downloaded {
            extract_verified(archive, path, &staged.0, cancellation)?;
        }
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
        for (_, path) in &downloaded {
            let _ = fs::remove_file(path);
        }
        // A DLL still loaded by a running worker cannot be deleted; that
        // leftover is harmless because nothing searches the old directory.
        for id in SUPERSEDED {
            let directory = self.root.join(id);
            if reject_link_if_present(&directory).is_ok() {
                let _ = fs::remove_dir_all(directory);
            }
        }
        progress(self.progress(InstallPhase::Ready));
        Ok(destination)
    }

    fn archive_path(&self, index: usize) -> PathBuf {
        self.root
            .join(format!("{}.{index}.zip.download", self.manifest.id))
    }

    /// Links (or copies) byte-identical files of an earlier component version
    /// into the stage. Every staged file is hashed again before it counts.
    fn reuse_superseded(
        &self,
        archive: &RuntimeArchive,
        stage: &Path,
        cancellation: &CancelToken,
    ) -> Result<bool, ModelInstallError> {
        for id in SUPERSEDED {
            let directory = self.root.join(id);
            if regular_directory(&directory).is_err() {
                continue;
            }
            let mut staged = Vec::new();
            let mut complete = true;
            for file in &archive.files {
                let source = directory.join(&file.name);
                let target = stage.join(&file.name);
                if regular_file(&source).is_err()
                    || (fs::hard_link(&source, &target).is_err()
                        && fs::copy(&source, &target).is_err())
                {
                    complete = false;
                    break;
                }
                staged.push(target.clone());
                match verify_file(&target, file.bytes, &file.sha256, cancellation) {
                    Ok(()) => {}
                    Err(ModelInstallError::Cancelled) => return Err(ModelInstallError::Cancelled),
                    Err(_) => {
                        complete = false;
                        break;
                    }
                }
            }
            if complete {
                return Ok(true);
            }
            for target in staged {
                let _ = fs::remove_file(target);
            }
        }
        Ok(false)
    }

    fn progress(&self, phase: InstallPhase) -> DownloadProgress {
        let total = self.manifest.download_bytes();
        DownloadProgress {
            phase,
            bytes_received: total,
            total_bytes: total,
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
        for file in self.manifest.files() {
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
}

fn download_verified(
    fetcher: &dyn ArtifactFetcher,
    archive: &RuntimeArchive,
    path: &Path,
    cancellation: &CancelToken,
    progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), ModelInstallError> {
    let verifying = DownloadProgress {
        phase: InstallPhase::Verifying,
        bytes_received: archive.bytes,
        total_bytes: archive.bytes,
    };
    reject_link_if_present(path)?;
    if path.metadata().is_ok_and(|m| m.len() > archive.bytes) {
        fs::remove_file(path)?;
    }
    if path.metadata().is_ok_and(|m| m.len() == archive.bytes) {
        progress(verifying);
        match verify_file(path, archive.bytes, &archive.sha256, cancellation) {
            Ok(()) => return Ok(()),
            Err(ModelInstallError::Cancelled) => return Err(ModelInstallError::Cancelled),
            Err(_) => fs::remove_file(path)?,
        }
    }
    // HttpFetcher resumes this file; it must share `cancellation` with its
    // caller to interrupt a stalled response as well as local work.
    fetcher.fetch(&archive.url, path, archive.bytes, progress)?;
    progress(verifying);
    let verified = verify_file(path, archive.bytes, &archive.sha256, cancellation);
    if let Err(error) = verified {
        if !matches!(error, ModelInstallError::Cancelled) {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(())
}

fn extract_verified(
    archive: &RuntimeArchive,
    path: &Path,
    stage: &Path,
    cancellation: &CancelToken,
) -> Result<(), ModelInstallError> {
    let mut zip = ZipArchive::new(File::open(path)?).map_err(invalid_zip)?;
    let mut found = HashSet::new();
    for index in 0..zip.len() {
        cancelled(cancellation)?;
        let mut entry = zip.by_index(index).map_err(invalid_zip)?;
        validate_entry(&entry)?;
        let name = entry.name().map_err(invalid_zip)?.into_owned();
        let Some(expected) = archive.files.iter().find(|file| file.archive_path == name) else {
            continue;
        };
        if !found.insert(expected.name.clone()) || entry.is_dir() || entry.size() != expected.bytes
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
    if found.len() != archive.files.len() {
        return Err(ModelInstallError::InvalidBundle(
            "CUDA archive is missing required runtime or license files".into(),
        ));
    }
    Ok(())
}

fn pinned_manifest() -> RuntimeManifest {
    // NVIDIA CUDA 13.3.1 redistrib manifest. The DLL hashes match the CUDA
    // 13.3 Toolkit the worker is built with; the worker links cudart
    // dynamically so that no NVIDIA runtime code ships in the installer.
    // Both archives carry the same LICENSE, which is published once.
    let redist = "https://developer.download.nvidia.com/compute/cuda/redist";
    let archive = |url: String,
                   bytes: u64,
                   sha256: &str,
                   package: &str,
                   files: &[(&str, &str, u64, &str)]| RuntimeArchive {
        url,
        bytes,
        sha256: sha256.into(),
        files: files
            .iter()
            .map(|(path, name, bytes, sha256)| RuntimeFile {
                archive_path: format!("{package}/{path}"),
                name: (*name).into(),
                bytes: *bytes,
                sha256: (*sha256).into(),
            })
            .collect(),
    };
    let cublas = "libcublas-windows-x86_64-13.6.0.2-archive";
    let cudart = "cuda_cudart-windows-x86_64-13.3.29-archive";
    RuntimeManifest {
        id: "cuda-runtime-13.3.1".into(),
        archives: vec![
            archive(
                format!("{redist}/libcublas/windows-x86_64/{cublas}.zip"),
                393_706_755,
                "62e9fa30560c8f0a28e0cdcf9d6fc1fed347bcfab8847239b9ae1fdc1d86408a",
                cublas,
                &[
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
                ],
            ),
            archive(
                format!("{redist}/cuda_cudart/windows-x86_64/{cudart}.zip"),
                2_589_792,
                "1feb7dd266813ffe8dbc24e115183a5ac35a4795c8d34aca0df85ab616b64d9c",
                cudart,
                &[(
                    "bin/x64/cudart64_13.dll",
                    "cudart64_13.dll",
                    551_024,
                    "b00ca6f53699120da815bf3e06e2e4285fae2f201235b883dcbb50eec51e2a2a",
                )],
            ),
        ],
    }
}

fn validate_entry(entry: &zip::read::ZipFile<'_, File>) -> Result<(), ModelInstallError> {
    let name = entry.name().map_err(invalid_zip)?;
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
    if entries.len() != manifest.files().count() + 1
        || entries.iter().any(|entry| {
            let name = entry.file_name();
            name != RECEIPT && !manifest.files().any(|file| name == file.name.as_str())
        })
    {
        return None;
    }
    let files = manifest
        .files()
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
    #[ignore = "requires explicitly supplied NVIDIA archives; writes only to a temporary directory"]
    fn installs_the_official_pinned_archives_without_loading_cuda() {
        struct LocalArchives(PathBuf);
        impl ArtifactFetcher for LocalArchives {
            fn fetch(
                &self,
                url: &str,
                destination: &Path,
                _: u64,
                _: &mut dyn FnMut(DownloadProgress),
            ) -> Result<(), ModelInstallError> {
                let name = url.rsplit('/').next().unwrap();
                fs::copy(self.0.join(name), destination)?;
                Ok(())
            }
        }
        let archives = std::env::var_os("SAYFLEX_CUDA_RUNTIME_ARCHIVE_DIR")
            .expect("set the directory that holds both NVIDIA archives");
        let root = tempdir().unwrap();
        let manager = CudaRuntimeManager::new(root.path().to_owned());
        let path = manager
            .ensure_installed(
                &LocalArchives(archives.into()),
                &CancelToken::default(),
                |_| {},
            )
            .unwrap();
        assert!(manager.is_installed());
        assert!(!manager.archive_path(0).exists());
        assert!(!manager.archive_path(1).exists());
        // Force a cold integrity probe; an on-disk receipt alone cannot replace
        // checking the real published DLL contents.
        verified_cache()
            .lock()
            .unwrap()
            .retain(|entry| entry.path != path);
        assert!(manager.is_installed());
        assert_eq!(fs::read_dir(path).unwrap().count(), 5);
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

    /// Serves each archive by URL and records which URLs were requested.
    struct UrlFetcher {
        archives: Vec<(String, Vec<u8>)>,
        requested: Mutex<Vec<String>>,
    }
    impl ArtifactFetcher for UrlFetcher {
        fn fetch(
            &self,
            url: &str,
            destination: &Path,
            total: u64,
            progress: &mut dyn FnMut(DownloadProgress),
        ) -> Result<(), ModelInstallError> {
            self.requested.lock().unwrap().push(url.to_owned());
            let (_, bytes) = self
                .archives
                .iter()
                .find(|(candidate, _)| candidate == url)
                .ok_or_else(|| ModelInstallError::Download("unknown URL".into()))?;
            fs::write(destination, bytes)?;
            progress(DownloadProgress {
                phase: InstallPhase::Downloading,
                bytes_received: bytes.len() as u64 / 2,
                total_bytes: total,
            });
            progress(DownloadProgress {
                phase: InstallPhase::Downloading,
                bytes_received: bytes.len() as u64,
                total_bytes: total,
            });
            Ok(())
        }
    }

    fn archive_fixture(
        url: &str,
        entries: &[(&str, &[u8])],
        extra: Option<&str>,
    ) -> (RuntimeArchive, Vec<u8>) {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files = Vec::new();
        for (name, bytes) in entries {
            let archive_path = format!("package/{name}");
            zip.start_file(&archive_path, SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
            files.push(RuntimeFile {
                archive_path,
                name: (*name).into(),
                bytes: bytes.len() as u64,
                sha256: digest(bytes),
            });
        }
        if let Some(name) = extra {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"untrusted").unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        (
            RuntimeArchive {
                url: url.into(),
                bytes: bytes.len() as u64,
                sha256: digest(&bytes),
                files,
            },
            bytes,
        )
    }

    const CUBLAS: &[(&str, &[u8])] = &[
        ("cublas64_13.dll", b"test cublas"),
        ("cublasLt64_13.dll", b"test cublasLt"),
        ("LICENSE", b"test NVIDIA license"),
    ];
    const CUDART: &[(&str, &[u8])] = &[("cudart64_13.dll", b"test cudart")];

    fn two_archive_fixture(root: &Path) -> (CudaRuntimeManager, UrlFetcher) {
        let (cublas, cublas_bytes) =
            archive_fixture("https://example.invalid/cublas.zip", CUBLAS, None);
        let (cudart, cudart_bytes) =
            archive_fixture("https://example.invalid/cudart.zip", CUDART, None);
        let fetcher = UrlFetcher {
            archives: vec![
                (cublas.url.clone(), cublas_bytes),
                (cudart.url.clone(), cudart_bytes),
            ],
            requested: Mutex::new(Vec::new()),
        };
        let manager = CudaRuntimeManager {
            root: root.to_owned(),
            manifest: RuntimeManifest {
                id: "cuda-test-version".into(),
                archives: vec![cublas, cudart],
            },
        };
        (manager, fetcher)
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture(root: &Path, extra: Option<&str>) -> (CudaRuntimeManager, Vec<u8>) {
        let (archive, bytes) = archive_fixture("https://example.invalid/pinned.zip", CUBLAS, extra);
        let manager = CudaRuntimeManager {
            root: root.to_owned(),
            manifest: RuntimeManifest {
                id: "cuda-test-version".into(),
                archives: vec![archive],
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
        assert!(!manager.archive_path(0).exists());
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
        assert!(manager.archive_path(0).is_file());
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
        manager.manifest.archives[0].bytes = bytes.len() as u64;
        manager.manifest.archives[0].sha256 = digest(&bytes);
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
        manager.manifest.archives[0].files[0].sha256 = "00".repeat(32);
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
    #[test]
    fn two_archives_report_one_continuous_download_and_publish_every_file() {
        let root = tempdir().unwrap();
        let (manager, fetcher) = two_archive_fixture(root.path());
        let total = manager.manifest.download_bytes();
        let mut received = Vec::new();
        manager
            .ensure_installed(&fetcher, &CancelToken::default(), |p| {
                assert_eq!(p.total_bytes, total);
                if p.phase == InstallPhase::Downloading {
                    received.push(p.bytes_received);
                }
            })
            .unwrap();
        assert!(received.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(received.last(), Some(&total));
        assert!(manager.is_installed());
        for name in [
            "cublas64_13.dll",
            "cublasLt64_13.dll",
            "LICENSE",
            "cudart64_13.dll",
        ] {
            assert!(manager.runtime_path().join(name).is_file(), "{name}");
        }
        assert!(!manager.archive_path(0).exists());
        assert!(!manager.archive_path(1).exists());
    }

    #[test]
    fn reuses_verified_files_of_a_superseded_version_and_removes_it() {
        let root = tempdir().unwrap();
        let (manager, fetcher) = two_archive_fixture(root.path());
        let legacy = root.path().join(SUPERSEDED[0]);
        fs::create_dir(&legacy).unwrap();
        for (name, bytes) in CUBLAS {
            fs::write(legacy.join(name), bytes).unwrap();
        }
        manager
            .ensure_installed(&fetcher, &CancelToken::default(), |_| {})
            .unwrap();
        assert_eq!(
            *fetcher.requested.lock().unwrap(),
            vec!["https://example.invalid/cudart.zip".to_owned()]
        );
        assert!(manager.is_installed());
        assert!(!legacy.exists());
    }

    #[test]
    fn a_corrupt_superseded_file_is_downloaded_again() {
        let root = tempdir().unwrap();
        let (manager, fetcher) = two_archive_fixture(root.path());
        let legacy = root.path().join(SUPERSEDED[0]);
        fs::create_dir(&legacy).unwrap();
        for (name, bytes) in CUBLAS {
            fs::write(legacy.join(name), bytes).unwrap();
        }
        fs::write(legacy.join("cublas64_13.dll"), b"bad! cublas").unwrap();
        manager
            .ensure_installed(&fetcher, &CancelToken::default(), |_| {})
            .unwrap();
        assert_eq!(fetcher.requested.lock().unwrap().len(), 2);
        assert_eq!(
            fs::read(manager.runtime_path().join("cublas64_13.dll")).unwrap(),
            b"test cublas"
        );
        assert!(manager.is_installed());
    }

    #[test]
    fn the_pinned_manifest_ships_cudart_beside_cublas() {
        let manifest = pinned_manifest();
        let names = manifest
            .files()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "cublas64_13.dll",
                "cublasLt64_13.dll",
                "LICENSE",
                "cudart64_13.dll"
            ]
        );
        assert_eq!(manifest.download_bytes(), CUDA_DOWNLOAD_BYTES);
        assert!(manifest.archives.iter().all(|archive| {
            archive
                .url
                .starts_with("https://developer.download.nvidia.com/compute/cuda/redist/")
        }));
    }
}
