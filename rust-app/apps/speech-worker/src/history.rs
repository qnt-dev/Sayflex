use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rust_protocol::{HistoryEntry, Transcript};

const MAX_HISTORY_ENTRIES: usize = 250;

pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        let entries = if path.exists() {
            let bytes = fs::read(&path).context("could not read dictation history")?;
            serde_json::from_slice(&bytes).context("dictation history is invalid")?
        } else {
            Vec::new()
        };
        Ok(Self { path, entries })
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
        if self.path.exists() {
            fs::remove_file(&self.path).context("could not remove dictation history")?;
        }
        Ok(())
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
    use rust_protocol::{SessionId, Transcript};
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
        store.clear().unwrap();
        assert!(store.latest(10).is_empty());
        assert!(!path.exists());
    }
}
