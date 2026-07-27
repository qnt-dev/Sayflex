use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rust_protocol::{HistoryEntry, Transcript};

const MAX_HISTORY_ENTRIES: usize = 250;

pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
    recovery_message: Option<String>,
}

impl HistoryStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        let (entries, recovery_message) = load_with_recovery(&path)?;
        Ok(Self {
            path,
            entries,
            recovery_message,
        })
    }

    pub fn take_recovery_message(&mut self) -> Option<String> {
        self.recovery_message.take()
    }

    pub fn append(&mut self, transcript: Transcript, created_unix_ms: u64) -> Result<()> {
        self.entries.insert(
            0,
            HistoryEntry {
                transcript,
                created_unix_ms,
            },
        );
        self.entries.truncate(MAX_HISTORY_ENTRIES);
        write_atomic(&self.path, &serde_json::to_vec_pretty(&self.entries)?)?;
        Ok(())
    }

    #[must_use]
    pub fn latest(&self, limit: usize) -> Vec<HistoryEntry> {
        self.entries.iter().take(limit).cloned().collect()
    }

    pub fn clear(&mut self) -> Result<()> {
        self.entries.clear();
        remove_history_artifacts(&self.path)?;
        Ok(())
    }
}

fn load_with_recovery(path: &Path) -> Result<(Vec<HistoryEntry>, Option<String>)> {
    if path.exists() {
        let bytes = fs::read(path).context("could not read dictation history")?;
        match serde_json::from_slice(&bytes) {
            Ok(entries) => return Ok((entries, None)),
            Err(error) => {
                let quarantined = quarantine(path)?;
                let (entries, recovered) = match recover_candidate(path)? {
                    Some(entries) => (entries, true),
                    None => (Vec::new(), false),
                };
                if recovered {
                    write_atomic(path, &serde_json::to_vec_pretty(&entries)?)?;
                }
                return Ok((
                    entries,
                    Some(format!(
                        "Invalid dictation history was moved to {}: {error}",
                        quarantined.display()
                    )),
                ));
            }
        }
    }

    if let Some(entries) = recover_candidate(path)? {
        write_atomic(path, &serde_json::to_vec_pretty(&entries)?)?;
        return Ok((
            entries,
            Some("Dictation history was recovered after an interrupted write.".to_owned()),
        ));
    }
    Ok((Vec::new(), None))
}

fn recover_candidate(path: &Path) -> Result<Option<Vec<HistoryEntry>>> {
    for candidate in [path.with_extension("tmp"), path.with_extension("previous")] {
        if !candidate.exists() {
            continue;
        }
        let bytes = fs::read(&candidate).with_context(|| {
            format!(
                "could not read dictation history recovery file {}",
                candidate.display()
            )
        })?;
        match serde_json::from_slice(&bytes) {
            Ok(entries) => return Ok(Some(entries)),
            Err(_) => {
                let _ = quarantine(&candidate)?;
            }
        }
    }
    Ok(None)
}

fn quarantine(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("history path has no parent")?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("history");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for suffix in 0..1000_u16 {
        let collision = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let candidate = parent.join(format!("{stem}.corrupt-{timestamp}{collision}.json"));
        if !candidate.exists() {
            fs::rename(path, &candidate)?;
            return Ok(candidate);
        }
    }
    anyhow::bail!("could not allocate a quarantine filename for history")
}

fn remove_history_artifacts(path: &Path) -> Result<()> {
    for candidate in [
        path.to_path_buf(),
        path.with_extension("tmp"),
        path.with_extension("previous"),
    ] {
        remove_if_exists(&candidate)?;
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("history");
    let prefix = format!("{stem}.corrupt-");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
        {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("history path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("previous");
    fs::write(&temporary, bytes)?;
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    let had_previous = path.exists();
    if had_previous {
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if had_previous {
            let _ = fs::rename(&backup, path);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rust_protocol::{HistoryEntry, SessionId, Transcript};
    use tempfile::tempdir;

    use super::HistoryStore;

    #[test]
    fn newest_completed_session_is_first_and_survives_restart() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let first = Transcript {
            session_id: SessionId::new(),
            text: "first".into(),
            language: "en".into(),
            audio_duration_ms: 1,
            processing_duration_ms: 2,
            latency: Default::default(),
        };
        let second = Transcript {
            session_id: SessionId::new(),
            text: "second".into(),
            language: "en".into(),
            audio_duration_ms: 3,
            processing_duration_ms: 4,
            latency: Default::default(),
        };

        let mut store = HistoryStore::open(path.clone()).unwrap();
        store.append(first, 10).unwrap();
        store.append(second.clone(), 20).unwrap();

        let reopened = HistoryStore::open(path).unwrap();
        assert_eq!(reopened.latest(1)[0].transcript, second);
    }

    #[test]
    fn clear_removes_persisted_and_in_memory_history() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let mut store = HistoryStore::open(path.clone()).unwrap();
        store
            .append(
                Transcript {
                    session_id: SessionId::new(),
                    text: "private".into(),
                    language: "en".into(),
                    audio_duration_ms: 1,
                    processing_duration_ms: 2,
                    latency: Default::default(),
                },
                10,
            )
            .unwrap();
        assert!(path.exists());
        let temporary = path.with_extension("tmp");
        let previous = path.with_extension("previous");
        let corrupt = directory.path().join("history.corrupt-1.json");
        fs::write(&temporary, b"temporary").unwrap();
        fs::write(&previous, b"previous").unwrap();
        fs::write(&corrupt, b"corrupt").unwrap();
        store.clear().unwrap();
        assert!(store.latest(10).is_empty());
        assert!(!path.exists());
        assert!(!temporary.exists());
        assert!(!previous.exists());
        assert!(!corrupt.exists());
    }

    #[test]
    fn corrupt_primary_is_quarantined_and_previous_history_is_recovered() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.json");
        let mut store = HistoryStore::open(path.clone()).unwrap();
        store
            .append(
                Transcript {
                    session_id: SessionId::new(),
                    text: "recover me".into(),
                    language: "en".into(),
                    audio_duration_ms: 1,
                    processing_duration_ms: 2,
                    latency: Default::default(),
                },
                10,
            )
            .unwrap();
        fs::copy(&path, path.with_extension("previous")).unwrap();
        fs::write(&path, b"{ definitely not json").unwrap();

        let mut recovered = HistoryStore::open(path.clone()).unwrap();
        assert_eq!(recovered.latest(1)[0].transcript.text, "recover me");
        assert!(recovered.take_recovery_message().is_some());
        assert!(
            fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("history.corrupt-"))
        );
        assert!(serde_json::from_slice::<Vec<HistoryEntry>>(&fs::read(path).unwrap()).is_ok());
    }
}
