use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rust_model_manager::{ArtifactFetcher, DownloadProgress, InstallPhase};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

pub const RUNTIME_DOWNLOAD_BYTES: u64 = 33_597_393;

const RUNTIME_ID: &str = "llama-b10164-vulkan";
const RUNTIME_ARCHIVE: &str = "llama-b10164-vulkan.zip";
const RUNTIME_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10164/\
     llama-b10164-bin-win-vulkan-x64.zip";
const RUNTIME_SHA256: &str = "5812a095d3b810b8c87fa7232aa2acbdc1279dfb008ee448ef7547a28c8f23e8";
const INSTALL_MARKER: &str = ".styleflex-runtime";
const REQUIRED_FILES: &[&str] = &[
    "llama-server.exe",
    "llama-server-impl.dll",
    "llama-common.dll",
    "llama.dll",
    "ggml.dll",
    "ggml-base.dll",
    "ggml-vulkan.dll",
];

pub struct LlamaRuntimeManager {
    root: PathBuf,
}

impl LlamaRuntimeManager {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn runtime_path(&self) -> PathBuf {
        self.root.join(RUNTIME_ID)
    }

    #[must_use]
    pub fn is_installed(&self) -> bool {
        let path = self.runtime_path();
        path.join(INSTALL_MARKER)
            .is_file()
            .then(|| fs::read_to_string(path.join(INSTALL_MARKER)).ok())
            .flatten()
            .is_some_and(|marker| marker.trim().eq_ignore_ascii_case(RUNTIME_SHA256))
            && REQUIRED_FILES.iter().all(|file| path.join(file).is_file())
    }

    pub fn ensure_installed(
        &self,
        fetcher: &dyn ArtifactFetcher,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<PathBuf> {
        if self.is_installed() {
            progress(DownloadProgress {
                phase: InstallPhase::Ready,
                bytes_received: RUNTIME_DOWNLOAD_BYTES,
                total_bytes: RUNTIME_DOWNLOAD_BYTES,
            });
            return Ok(self.runtime_path());
        }

        fs::create_dir_all(&self.root)?;
        let archive = self.root.join(RUNTIME_ARCHIVE);
        if !archive_matches(&archive)? {
            let temporary = append_suffix(&archive, ".download");
            if temporary
                .metadata()
                .is_ok_and(|metadata| metadata.len() > RUNTIME_DOWNLOAD_BYTES)
            {
                fs::remove_file(&temporary)?;
            }
            fetcher
                .fetch(
                    RUNTIME_URL,
                    &temporary,
                    RUNTIME_DOWNLOAD_BYTES,
                    &mut progress,
                )
                .context("could not download the llama.cpp Vulkan runtime")?;
            progress(DownloadProgress {
                phase: InstallPhase::Verifying,
                bytes_received: RUNTIME_DOWNLOAD_BYTES,
                total_bytes: RUNTIME_DOWNLOAD_BYTES,
            });
            if !archive_matches(&temporary)? {
                bail!("downloaded llama.cpp runtime failed SHA-256 verification");
            }
            replace_file(&temporary, &archive)?;
        }

        progress(DownloadProgress {
            phase: InstallPhase::Installing,
            bytes_received: RUNTIME_DOWNLOAD_BYTES,
            total_bytes: RUNTIME_DOWNLOAD_BYTES,
        });
        self.extract_atomically(&archive)?;
        progress(DownloadProgress {
            phase: InstallPhase::Ready,
            bytes_received: RUNTIME_DOWNLOAD_BYTES,
            total_bytes: RUNTIME_DOWNLOAD_BYTES,
        });
        Ok(self.runtime_path())
    }

    fn extract_atomically(&self, archive_path: &Path) -> Result<()> {
        let final_path = self.runtime_path();
        let staging = self.root.join(format!("{RUNTIME_ID}.staging"));
        let previous = self.root.join(format!("{RUNTIME_ID}.previous"));
        remove_directory_if_present(&staging)?;
        remove_directory_if_present(&previous)?;
        fs::create_dir_all(&staging)?;

        let file = File::open(archive_path)?;
        let mut archive =
            ZipArchive::new(file).context("llama.cpp runtime archive is not a valid ZIP")?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| anyhow::anyhow!("runtime archive contains an unsafe path"))?
                .to_owned();
            let destination = staging.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&destination)?;
                continue;
            }
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
            {
                bail!("runtime archive contains a symbolic link");
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = File::create(&destination)?;
            std::io::copy(&mut entry, &mut output)?;
            output.flush()?;
        }
        if !REQUIRED_FILES
            .iter()
            .all(|file| staging.join(file).is_file())
        {
            remove_directory_if_present(&staging)?;
            bail!("llama.cpp runtime archive is incomplete");
        }
        fs::write(staging.join(INSTALL_MARKER), RUNTIME_SHA256)?;

        let had_previous = final_path.exists();
        if had_previous {
            fs::rename(&final_path, &previous)?;
        }
        if let Err(error) = fs::rename(&staging, &final_path) {
            if had_previous {
                let _ = fs::rename(&previous, &final_path);
            }
            return Err(error.into());
        }
        remove_directory_if_present(&previous)?;
        Ok(())
    }
}

fn archive_matches(path: &Path) -> Result<bool> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.len() != RUNTIME_DOWNLOAD_BYTES {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(RUNTIME_SHA256))
}

fn replace_file(temporary: &Path, final_path: &Path) -> Result<()> {
    let previous = append_suffix(final_path, ".previous");
    if previous.exists() {
        fs::remove_file(&previous)?;
    }
    let had_previous = final_path.exists();
    if had_previous {
        fs::rename(final_path, &previous)?;
    }
    if let Err(error) = fs::rename(temporary, final_path) {
        if had_previous {
            let _ = fs::rename(&previous, final_path);
        }
        return Err(error.into());
    }
    if previous.exists() {
        fs::remove_file(previous)?;
    }
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_directory_if_present(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::LlamaRuntimeManager;

    #[test]
    fn incomplete_runtime_is_never_reported_as_installed() {
        let root = tempdir().unwrap();
        let manager = LlamaRuntimeManager::new(root.path().to_owned());
        std::fs::create_dir_all(manager.runtime_path()).unwrap();
        std::fs::write(
            manager.runtime_path().join(".styleflex-runtime"),
            "5812a095d3b810b8c87fa7232aa2acbdc1279dfb008ee448ef7547a28c8f23e8",
        )
        .unwrap();
        assert!(!manager.is_installed());
    }
}
