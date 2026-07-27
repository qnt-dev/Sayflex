use std::{
    fs,
    path::{Path, PathBuf},
};

use rust_protocol::TranslationHistoryEntry;

const MAX_HISTORY: usize = 250;

pub struct TranslationHistory {
    path: PathBuf,
    entries: Vec<TranslationHistoryEntry>,
}

impl TranslationHistory {
    pub fn load(path: PathBuf) -> Self {
        let entries = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<TranslationHistoryEntry>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    pub fn push(&mut self, entry: TranslationHistoryEntry) {
        self.entries
            .retain(|candidate| candidate.result.request_id != entry.result.request_id);
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_HISTORY);
        if let Err(error) = write_atomically(&self.path, &self.entries) {
            eprintln!("translation history persistence failed: {error}");
        }
    }

    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<TranslationHistoryEntry> {
        self.entries.iter().take(limit).cloned().collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        if self.path.exists()
            && let Err(error) = fs::remove_file(&self.path)
        {
            eprintln!("translation history clear failed: {error}");
        }
    }
}

fn write_atomically(
    path: &Path,
    entries: &[TranslationHistoryEntry],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("previous");
    fs::write(&temporary, serde_json::to_vec_pretty(entries)?)?;
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
    use rust_protocol::{
        RequestId, TranslationHistoryEntry, TranslationLanguage, TranslationResult, TranslationTier,
    };
    use tempfile::tempdir;

    use super::TranslationHistory;

    #[test]
    fn clear_removes_translation_history_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("translation-history.json");
        let mut history = TranslationHistory::load(path.clone());
        history.push(TranslationHistoryEntry {
            result: TranslationResult {
                request_id: RequestId::new(),
                source_text: "hello".into(),
                translated_text: "привет".into(),
                source_language: TranslationLanguage::English,
                target_language: TranslationLanguage::Russian,
                tier: TranslationTier::Tiny,
                processing_duration_ms: 1,
            },
            created_unix_ms: 2,
        });
        assert!(path.exists());
        history.clear();
        assert!(history.recent(10).is_empty());
        assert!(!path.exists());
    }
}
