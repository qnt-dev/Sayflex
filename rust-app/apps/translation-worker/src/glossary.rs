use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rust_protocol::{
    GlossaryEntry, GlossaryEntryDraft, GlossaryEntryId, GlossaryOrigin, MAX_GLOSSARY_EXAMPLES,
    MAX_GLOSSARY_FIELD_BYTES, MAX_SOURCE_CONTEXT_FIELD_BYTES, MAX_SOURCE_CONTEXT_NEARBY_TEXT_BYTES,
    RequestId, SourceContext, TranslationLanguage,
};
use thiserror::Error;

const MAX_GLOSSARY_ENTRIES: usize = 10_000;
const MAX_GLOSSARY_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub(crate) enum GlossaryError {
    #[error("personal glossary storage is unavailable")]
    Io(#[from] io::Error),
    #[error("personal glossary data is invalid")]
    Json(#[from] serde_json::Error),
    #[error("the glossary entry does not exist")]
    NotFound,
    #[error("the glossary entry is invalid: {0}")]
    Invalid(&'static str),
    #[error("the personal glossary reached its safety limit")]
    Capacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MergeKey {
    source: String,
    source_language: TranslationLanguage,
    target_language: TranslationLanguage,
    scope: String,
}

pub(crate) struct GlossaryStore {
    path: PathBuf,
    entries: Vec<GlossaryEntry>,
}

impl GlossaryStore {
    pub(crate) fn load(path: PathBuf) -> Result<Self, GlossaryError> {
        let entries = load_with_recovery(&path)?;
        Ok(Self { path, entries })
    }

    pub(crate) fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
        }
    }

    pub(crate) fn save(
        &mut self,
        draft: GlossaryEntryDraft,
        now_unix_ms: u64,
    ) -> Result<GlossaryEntry, GlossaryError> {
        let draft = normalized_draft(draft)?;
        let key = merge_key_for_draft(&draft);
        let mut next = self.entries.clone();
        let saved = if let Some(existing) = next
            .iter_mut()
            .find(|candidate| merge_key(candidate) == key)
        {
            existing.approved_translation = draft.approved_translation;
            push_example(&mut existing.examples, draft.example);
            existing.context = merge_context(&existing.context, draft.context);
            existing.origin = stronger_origin(existing.origin, draft.origin);
            existing.updated_unix_ms = now_unix_ms;
            existing.clone()
        } else {
            if next.len() >= MAX_GLOSSARY_ENTRIES {
                return Err(GlossaryError::Capacity);
            }
            let entry = GlossaryEntry {
                entry_id: unique_entry_id(&next),
                source_phrase: draft.source_phrase,
                approved_translation: draft.approved_translation,
                source_language: draft.source_language,
                target_language: draft.target_language,
                examples: vec![draft.example],
                context: draft.context,
                origin: draft.origin,
                created_unix_ms: now_unix_ms,
                updated_unix_ms: now_unix_ms,
            };
            next.push(entry.clone());
            entry
        };
        write_atomically(&self.path, &next)?;
        self.entries = next;
        Ok(saved)
    }

    pub(crate) fn update(
        &mut self,
        entry_id: &GlossaryEntryId,
        draft: GlossaryEntryDraft,
        now_unix_ms: u64,
    ) -> Result<GlossaryEntry, GlossaryError> {
        let draft = normalized_draft(draft)?;
        let Some(current) = self
            .entries
            .iter()
            .find(|entry| &entry.entry_id == entry_id)
            .cloned()
        else {
            return Err(GlossaryError::NotFound);
        };
        let desired_key = merge_key_for_draft(&draft);
        let mut examples = current.examples;
        push_example(&mut examples, draft.example);
        let mut next = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            if &entry.entry_id == entry_id {
                continue;
            }
            if merge_key(entry) == desired_key {
                for example in &entry.examples {
                    push_example(&mut examples, example.clone());
                }
                continue;
            }
            next.push(entry.clone());
        }
        let updated = GlossaryEntry {
            entry_id: entry_id.clone(),
            source_phrase: draft.source_phrase,
            approved_translation: draft.approved_translation,
            source_language: draft.source_language,
            target_language: draft.target_language,
            examples,
            context: merge_context(&current.context, draft.context),
            origin: stronger_origin(current.origin, draft.origin),
            created_unix_ms: current.created_unix_ms,
            updated_unix_ms: now_unix_ms,
        };
        next.push(updated.clone());
        write_atomically(&self.path, &next)?;
        self.entries = next;
        Ok(updated)
    }

    pub(crate) fn delete(&mut self, entry_id: &GlossaryEntryId) -> Result<(), GlossaryError> {
        if !self.entries.iter().any(|entry| &entry.entry_id == entry_id) {
            return Err(GlossaryError::NotFound);
        }
        let next = self
            .entries
            .iter()
            .filter(|entry| &entry.entry_id != entry_id)
            .cloned()
            .collect::<Vec<_>>();
        write_atomically(&self.path, &next)?;
        self.entries = next;
        Ok(())
    }

    pub(crate) fn list(&self, query: Option<&str>, limit: usize) -> Vec<GlossaryEntry> {
        let query = query.map(normalize).filter(|query| !query.is_empty());
        let mut entries = self
            .entries
            .iter()
            .filter(|entry| {
                query.as_ref().is_none_or(|query| {
                    normalize(&entry.source_phrase).contains(query)
                        || normalize(&entry.approved_translation).contains(query)
                        || normalize(&scope(&entry.context)).contains(query)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_unix_ms));
        entries.truncate(limit);
        entries
    }

    pub(crate) fn lookup(
        &self,
        source_phrase: &str,
        source_language: TranslationLanguage,
        target_language: TranslationLanguage,
        context: &SourceContext,
        limit: usize,
    ) -> Vec<GlossaryEntry> {
        let normalized_source = normalize(source_phrase);
        let mut ranked = self
            .entries
            .iter()
            .filter(|entry| {
                entry.source_language == source_language
                    && entry.target_language == target_language
                    && normalize(&entry.source_phrase) == normalized_source
            })
            .filter_map(|entry| context_rank(&entry.context, context).map(|rank| (rank, entry)))
            .collect::<Vec<_>>();
        ranked.sort_by_key(|(rank, entry)| (*rank, std::cmp::Reverse(entry.updated_unix_ms)));
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry.clone())
            .collect()
    }
}

fn normalized_draft(mut draft: GlossaryEntryDraft) -> Result<GlossaryEntryDraft, GlossaryError> {
    draft.source_phrase = collapse_whitespace(&draft.source_phrase);
    draft.approved_translation = collapse_whitespace(&draft.approved_translation);
    draft.example = collapse_whitespace(&draft.example);
    for value in [
        draft.source_phrase.as_str(),
        draft.approved_translation.as_str(),
        draft.example.as_str(),
    ] {
        if value.is_empty() {
            return Err(GlossaryError::Invalid("a required field is empty"));
        }
        if value.len() > MAX_GLOSSARY_FIELD_BYTES {
            return Err(GlossaryError::Invalid("a field is too long"));
        }
    }
    validate_context(&draft.context)?;
    Ok(draft)
}

fn validate_context(context: &SourceContext) -> Result<(), GlossaryError> {
    let ordinary = [
        &context.application_id,
        &context.executable_name,
        &context.window_title,
        &context.site_url,
        &context.document_name,
        &context.domain,
    ];
    if ordinary
        .into_iter()
        .flatten()
        .any(|value| value.len() > MAX_SOURCE_CONTEXT_FIELD_BYTES)
        || context
            .nearby_text
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SOURCE_CONTEXT_NEARBY_TEXT_BYTES)
    {
        return Err(GlossaryError::Invalid("context is too long"));
    }
    Ok(())
}

fn validate_entries(entries: &[GlossaryEntry]) -> Result<(), GlossaryError> {
    if entries.len() > MAX_GLOSSARY_ENTRIES {
        return Err(GlossaryError::Capacity);
    }
    let mut ids = HashSet::with_capacity(entries.len());
    let mut keys = HashSet::with_capacity(entries.len());
    for entry in entries {
        if entry.entry_id.as_str().trim().is_empty()
            || entry.entry_id.as_str().len() > MAX_GLOSSARY_FIELD_BYTES
            || entry.examples.is_empty()
            || entry.examples.len() > MAX_GLOSSARY_EXAMPLES
        {
            return Err(GlossaryError::Invalid(
                "entry identity or examples are invalid",
            ));
        }
        normalized_draft(GlossaryEntryDraft {
            source_phrase: entry.source_phrase.clone(),
            approved_translation: entry.approved_translation.clone(),
            source_language: entry.source_language,
            target_language: entry.target_language,
            example: entry.examples[0].clone(),
            context: entry.context.clone(),
            origin: entry.origin,
        })?;
        if entry
            .examples
            .iter()
            .any(|example| example.trim().is_empty() || example.len() > MAX_GLOSSARY_FIELD_BYTES)
            || !ids.insert(entry.entry_id.clone())
            || !keys.insert(merge_key(entry))
        {
            return Err(GlossaryError::Invalid(
                "entries are duplicated or malformed",
            ));
        }
    }
    Ok(())
}

fn unique_entry_id(entries: &[GlossaryEntry]) -> GlossaryEntryId {
    loop {
        let candidate = GlossaryEntryId::from(RequestId::new().to_string());
        if entries.iter().all(|entry| entry.entry_id != candidate) {
            return candidate;
        }
    }
}

fn merge_key_for_draft(draft: &GlossaryEntryDraft) -> MergeKey {
    MergeKey {
        source: normalize(&draft.source_phrase),
        source_language: draft.source_language,
        target_language: draft.target_language,
        scope: scope(&draft.context),
    }
}

fn merge_key(entry: &GlossaryEntry) -> MergeKey {
    MergeKey {
        source: normalize(&entry.source_phrase),
        source_language: entry.source_language,
        target_language: entry.target_language,
        scope: scope(&entry.context),
    }
}

fn scope(context: &SourceContext) -> String {
    if let Some(domain) = context
        .domain
        .as_deref()
        .map(normalize)
        .filter(|v| !v.is_empty())
    {
        return format!("domain:{domain}");
    }
    if let Some(application) = context
        .application_id
        .as_deref()
        .map(normalize)
        .filter(|value| !value.is_empty())
    {
        return format!("application:{application}");
    }
    if let Some(executable) = context
        .executable_name
        .as_deref()
        .map(normalize)
        .filter(|value| !value.is_empty())
    {
        return format!("executable:{executable}");
    }
    "general".to_owned()
}

fn context_rank(entry: &SourceContext, requested: &SourceContext) -> Option<u8> {
    match scope(entry).split_once(':') {
        None if scope(entry) == "general" => Some(3),
        Some(("domain", value))
            if requested
                .domain
                .as_deref()
                .is_some_and(|domain| normalize(domain) == value) =>
        {
            Some(0)
        }
        Some(("application", value))
            if requested
                .application_id
                .as_deref()
                .is_some_and(|application| normalize(application) == value) =>
        {
            Some(1)
        }
        Some(("executable", value))
            if requested
                .executable_name
                .as_deref()
                .is_some_and(|executable| normalize(executable) == value) =>
        {
            Some(2)
        }
        _ => None,
    }
}

fn merge_context(existing: &SourceContext, incoming: SourceContext) -> SourceContext {
    SourceContext {
        application_id: incoming
            .application_id
            .or_else(|| existing.application_id.clone()),
        executable_name: incoming
            .executable_name
            .or_else(|| existing.executable_name.clone()),
        window_title: incoming
            .window_title
            .or_else(|| existing.window_title.clone()),
        site_url: incoming.site_url.or_else(|| existing.site_url.clone()),
        document_name: incoming
            .document_name
            .or_else(|| existing.document_name.clone()),
        nearby_text: incoming
            .nearby_text
            .or_else(|| existing.nearby_text.clone()),
        domain: incoming.domain.or_else(|| existing.domain.clone()),
        partial: incoming.partial && existing.partial,
    }
}

fn stronger_origin(existing: GlossaryOrigin, incoming: GlossaryOrigin) -> GlossaryOrigin {
    if existing == GlossaryOrigin::Corrected || incoming == GlossaryOrigin::Corrected {
        GlossaryOrigin::Corrected
    } else {
        GlossaryOrigin::Confirmed
    }
}

fn push_example(examples: &mut Vec<String>, example: String) {
    let normalized = normalize(&example);
    if examples
        .iter()
        .any(|current| normalize(current) == normalized)
    {
        return;
    }
    if examples.len() == MAX_GLOSSARY_EXAMPLES {
        examples.remove(0);
    }
    examples.push(example);
}

fn normalize(value: &str) -> String {
    collapse_whitespace(value).to_lowercase()
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn load_with_recovery(path: &Path) -> Result<Vec<GlossaryEntry>, GlossaryError> {
    let mut candidates = [
        path.to_path_buf(),
        path.with_extension("tmp"),
        path.with_extension("previous"),
    ]
    .into_iter()
    .filter_map(|candidate| {
        let metadata = fs::metadata(&candidate).ok()?;
        Some((candidate, metadata.modified().unwrap_or(UNIX_EPOCH)))
    })
    .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    for (candidate, _) in candidates {
        match read_candidate(&candidate) {
            Ok(entries) => {
                if candidate != path {
                    write_atomically(path, &entries)?;
                }
                return Ok(entries);
            }
            Err(_) => {
                quarantine(&candidate)?;
            }
        }
    }
    Ok(Vec::new())
}

fn read_candidate(path: &Path) -> Result<Vec<GlossaryEntry>, GlossaryError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_GLOSSARY_FILE_BYTES {
        return Err(GlossaryError::Capacity);
    }
    let entries = serde_json::from_slice::<Vec<GlossaryEntry>>(&fs::read(path)?)?;
    validate_entries(&entries)?;
    Ok(entries)
}

fn quarantine(path: &Path) -> Result<(), GlossaryError> {
    let parent = path
        .parent()
        .ok_or(GlossaryError::Invalid("storage path has no parent"))?;
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
        let target = parent.join(format!(
            "translation-glossary.corrupt-{timestamp}{collision}.json"
        ));
        if !target.exists() {
            fs::rename(path, target)?;
            return Ok(());
        }
    }
    Err(GlossaryError::Capacity)
}

fn write_atomically(path: &Path, entries: &[GlossaryEntry]) -> Result<(), GlossaryError> {
    validate_entries(entries)?;
    let parent = path
        .parent()
        .ok_or(GlossaryError::Invalid("storage path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    let previous = path.with_extension("previous");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, entries)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    rust_platform_windows::replace_file_atomically(path, &temporary, &previous)?;
    let _ = fs::remove_file(previous);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rust_protocol::{GlossaryEntryDraft, GlossaryOrigin, SourceContext, TranslationLanguage};
    use tempfile::tempdir;

    use super::GlossaryStore;

    fn draft(domain: Option<&str>, translation: &str, example: &str) -> GlossaryEntryDraft {
        GlossaryEntryDraft {
            source_phrase: " cooldown ".to_owned(),
            approved_translation: translation.to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            example: example.to_owned(),
            context: SourceContext {
                application_id: domain.map(|value| format!("game.{value}")),
                domain: domain.map(str::to_owned),
                ..SourceContext::default()
            },
            origin: GlossaryOrigin::Confirmed,
        }
    }

    #[test]
    fn domain_specific_meanings_coexist_and_general_matches_rank_last() {
        let directory = tempdir().unwrap();
        let mut store = GlossaryStore::load(directory.path().join("glossary.json")).unwrap();
        store
            .save(draft(None, "перерыв", "General cooldown"), 1)
            .unwrap();
        store
            .save(draft(Some("dota2"), "перезарядка", "Dota cooldown"), 2)
            .unwrap();
        store
            .save(
                draft(Some("counter-strike-2"), "задержка", "CS cooldown"),
                3,
            )
            .unwrap();

        let matches = store.lookup(
            "Cooldown",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            &draft(Some("dota2"), "", "").context,
            10,
        );

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].approved_translation, "перезарядка");
        assert_eq!(matches[1].approved_translation, "перерыв");
    }

    #[test]
    fn reconfirmation_merges_examples_without_duplicate_entries() {
        let directory = tempdir().unwrap();
        let mut store = GlossaryStore::load(directory.path().join("glossary.json")).unwrap();
        let first = store
            .save(draft(Some("dota2"), "перезарядка", "First example"), 10)
            .unwrap();
        let second = store
            .save(
                draft(Some("DOTA2"), "время перезарядки", "Second example"),
                20,
            )
            .unwrap();
        store
            .save(
                draft(Some("dota2"), "время перезарядки", "Second example"),
                30,
            )
            .unwrap();

        assert_eq!(first.entry_id, second.entry_id);
        assert_eq!(store.list(None, 10).len(), 1);
        assert_eq!(store.list(None, 10)[0].examples.len(), 2);
        assert_eq!(store.list(None, 10)[0].created_unix_ms, 10);
        assert_eq!(store.list(None, 10)[0].updated_unix_ms, 30);
    }

    #[test]
    fn update_delete_and_unrelated_history_cleanup_survive_restart() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("translation-glossary.json");
        let mut store = GlossaryStore::load(path.clone()).unwrap();
        let dota = store
            .save(draft(Some("dota2"), "перезарядка", "Example"), 1)
            .unwrap();
        let cs = store
            .save(draft(Some("counter-strike-2"), "задержка", "Example"), 2)
            .unwrap();
        store
            .update(
                &dota.entry_id,
                draft(Some("dota2"), "время восстановления", "Updated"),
                3,
            )
            .unwrap();
        store.delete(&cs.entry_id).unwrap();
        fs::write(directory.path().join("translation-history.json"), b"[]").unwrap();
        fs::write(directory.path().join("screen-shot.png"), b"pixels").unwrap();
        fs::remove_file(directory.path().join("translation-history.json")).unwrap();
        fs::remove_file(directory.path().join("screen-shot.png")).unwrap();

        let reopened = GlossaryStore::load(path).unwrap();
        let entries = reopened.list(None, 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].approved_translation, "время восстановления");
    }

    #[test]
    fn corrupt_primary_is_quarantined_and_valid_previous_is_recovered() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("translation-glossary.json");
        let mut store = GlossaryStore::load(path.clone()).unwrap();
        store.save(draft(None, "перерыв", "Example"), 1).unwrap();
        fs::copy(&path, path.with_extension("previous")).unwrap();
        fs::write(&path, b"not json").unwrap();

        let reopened = GlossaryStore::load(path).unwrap();

        assert_eq!(reopened.list(None, 10).len(), 1);
        assert!(fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("corrupt-")
        }));
    }
}
