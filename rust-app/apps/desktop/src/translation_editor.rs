use rust_protocol::{RequestId, ScreenBlockId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorDraftKey {
    pub(crate) request_id: RequestId,
    pub(crate) block_id: ScreenBlockId,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorPhase {
    Editing,
    Saving,
    SaveFailed,
    Saved,
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct TranslationEditorState {
    key: EditorDraftKey,
    source: String,
    baseline: String,
    translation: String,
    phase: EditorPhase,
    pending_operation: Option<RequestId>,
    error: Option<String>,
    retryable: bool,
}

impl TranslationEditorState {
    pub(crate) fn begin(key: EditorDraftKey, source: String, baseline: String) -> Self {
        Self {
            key,
            source,
            translation: baseline.clone(),
            baseline,
            phase: EditorPhase::Editing,
            pending_operation: None,
            error: None,
            retryable: false,
        }
    }

    pub(crate) fn key(&self) -> &EditorDraftKey {
        &self.key
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn translation(&self) -> &str {
        &self.translation
    }

    pub(crate) fn set_translation(&mut self, translation: String) {
        if matches!(self.phase, EditorPhase::Editing | EditorPhase::SaveFailed) {
            self.translation = translation;
            self.error = None;
            self.retryable = false;
            self.phase = EditorPhase::Editing;
        }
    }

    pub(crate) fn phase(&self) -> EditorPhase {
        self.phase
    }

    pub(crate) fn changed(&self) -> bool {
        self.translation != self.baseline
    }

    pub(crate) fn begin_commit(&mut self) -> Option<RequestId> {
        if !matches!(self.phase, EditorPhase::Editing | EditorPhase::SaveFailed) {
            return None;
        }
        let operation_id = RequestId::new();
        self.phase = EditorPhase::Saving;
        self.pending_operation = Some(operation_id);
        self.error = None;
        Some(operation_id)
    }

    pub(crate) fn begin_retry(&mut self) -> Option<RequestId> {
        (self.phase == EditorPhase::SaveFailed && self.retryable)
            .then(|| self.begin_commit())
            .flatten()
    }

    pub(crate) fn fail(&mut self, operation_id: RequestId, error: String, retryable: bool) -> bool {
        if self.pending_operation != Some(operation_id) || self.phase != EditorPhase::Saving {
            return false;
        }
        self.pending_operation = None;
        self.error = Some(error);
        self.retryable = retryable;
        self.phase = EditorPhase::SaveFailed;
        true
    }

    pub(crate) fn succeed(&mut self, operation_id: RequestId) -> bool {
        if self.pending_operation != Some(operation_id) || self.phase != EditorPhase::Saving {
            return false;
        }
        self.pending_operation = None;
        self.error = None;
        self.retryable = false;
        self.baseline = self.translation.clone();
        self.phase = EditorPhase::Saved;
        true
    }

    pub(crate) fn cancel(&mut self) {
        self.pending_operation = None;
        self.phase = EditorPhase::Cancelled;
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{RequestId, ScreenBlockId};

    use super::{EditorDraftKey, EditorPhase, TranslationEditorState};

    #[test]
    fn failed_save_preserves_the_draft_and_is_retryable() {
        let key = EditorDraftKey {
            request_id: RequestId::new(),
            block_id: ScreenBlockId::from("block"),
            revision: 3,
        };
        let mut editor =
            TranslationEditorState::begin(key.clone(), "source".to_owned(), "machine".to_owned());
        editor.set_translation("corrected".to_owned());
        assert_eq!(editor.phase(), EditorPhase::Editing);
        assert_eq!(editor.translation(), "corrected");

        let first = editor.begin_commit().expect("first operation");
        editor.fail(first, "disk full".to_owned(), true);
        assert_eq!(editor.phase(), EditorPhase::SaveFailed);
        assert_eq!(editor.translation(), "corrected");
        let retry = editor.begin_retry().expect("retry operation");
        assert_ne!(first, retry);
        assert_eq!(editor.key(), &key);
    }

    #[test]
    fn stale_completion_cannot_close_a_newer_draft() {
        let mut editor = TranslationEditorState::begin(
            EditorDraftKey {
                request_id: RequestId::new(),
                block_id: ScreenBlockId::from("one"),
                revision: 0,
            },
            "one".to_owned(),
            "один".to_owned(),
        );
        let stale = editor.begin_commit().unwrap();
        editor.cancel();
        editor = TranslationEditorState::begin(
            EditorDraftKey {
                request_id: RequestId::new(),
                block_id: ScreenBlockId::from("two"),
                revision: 7,
            },
            "two".to_owned(),
            "два".to_owned(),
        );
        assert!(!editor.succeed(stale));
        assert_eq!(editor.phase(), EditorPhase::Editing);
    }
}
