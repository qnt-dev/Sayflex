use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rust_protocol::TranslationHistoryEntry;

const MAX_HISTORY: usize = 250;

pub struct TranslationHistory {
    path: PathBuf,
    entries: Vec<TranslationHistoryEntry>,
}

impl TranslationHistory {
    pub fn load(path: PathBuf) -> Self {
        let entries = match load_with_recovery(&path) {
            Ok((entries, warning)) => {
                if let Some(warning) = warning {
                    eprintln!("translation history recovered: {warning}");
                }
                entries
            }
            Err(error) => {
                eprintln!("translation history could not be loaded: {error}");
                Vec::new()
            }
        };
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
        if let Err(error) = remove_history_artifacts(&self.path) {
            eprintln!("translation history clear failed: {error}");
        }
    }
}

fn load_with_recovery(
    path: &Path,
) -> Result<(Vec<TranslationHistoryEntry>, Option<String>), Box<dyn std::error::Error>> {
    if path.exists() {
        let bytes = fs::read(path)?;
        match serde_json::from_slice(&bytes) {
            Ok(entries) => return Ok((entries, None)),
            Err(error) => {
                let quarantined = quarantine(path)?;
                let entries = match recover_candidate(path)? {
                    Some(entries) => {
                        write_atomically(path, &entries)?;
                        entries
                    }
                    None => Vec::new(),
                };
                return Ok((
                    entries,
                    Some(format!(
                        "invalid data moved to {}: {error}",
                        quarantined.display()
                    )),
                ));
            }
        }
    }
    if let Some(entries) = recover_candidate(path)? {
        write_atomically(path, &entries)?;
        return Ok((
            entries,
            Some("recovered after an interrupted write".to_owned()),
        ));
    }
    Ok((Vec::new(), None))
}

fn recover_candidate(
    path: &Path,
) -> Result<Option<Vec<TranslationHistoryEntry>>, Box<dyn std::error::Error>> {
    for candidate in [path.with_extension("tmp"), path.with_extension("previous")] {
        if !candidate.exists() {
            continue;
        }
        let bytes = fs::read(&candidate)?;
        match serde_json::from_slice(&bytes) {
            Ok(entries) => return Ok(Some(entries)),
            Err(_) => {
                let _ = quarantine(&candidate)?;
            }
        }
    }
    Ok(None)
}

fn quarantine(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("history path has no parent")?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("translation-history");
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
    Err("could not allocate a quarantine filename for translation history".into())
}

fn remove_history_artifacts(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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
        .unwrap_or("translation-history");
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

fn remove_if_exists(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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
    use std::fs;

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
                tier: Some(TranslationTier::Tiny),
                provider: None,
                processing_duration_ms: 1,
                first_token_duration_ms: 1,
                backend_request_count: 1,
                tokenize_duration_ms: 0,
                chunk_count: 1,
            },
            created_unix_ms: 2,
        });
        assert!(path.exists());
        let temporary = path.with_extension("tmp");
        let previous = path.with_extension("previous");
        let corrupt = directory.path().join("translation-history.corrupt-1.json");
        fs::write(&temporary, b"temporary").unwrap();
        fs::write(&previous, b"previous").unwrap();
        fs::write(&corrupt, b"corrupt").unwrap();
        history.clear();
        assert!(history.recent(10).is_empty());
        assert!(!path.exists());
        assert!(!temporary.exists());
        assert!(!previous.exists());
        assert!(!corrupt.exists());
    }

    #[test]
    fn corrupt_primary_uses_valid_previous_file() {
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
                tier: Some(TranslationTier::Tiny),
                provider: None,
                processing_duration_ms: 1,
                first_token_duration_ms: 1,
                backend_request_count: 1,
                tokenize_duration_ms: 0,
                chunk_count: 1,
            },
            created_unix_ms: 2,
        });
        fs::copy(&path, path.with_extension("previous")).unwrap();
        fs::write(&path, b"not json").unwrap();

        let recovered = TranslationHistory::load(path);
        assert_eq!(recovered.recent(1)[0].result.translated_text, "привет");
    }
}
