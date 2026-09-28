use rust_protocol::{GlossaryEntry, GlossaryEntryDraft, GlossaryEntryId, RequestId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlossaryAction {
    Update {
        entry_id: GlossaryEntryId,
        draft: GlossaryEntryDraft,
    },
    Delete(GlossaryEntryId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingGlossaryAction {
    pub(crate) operation_id: RequestId,
    pub(crate) action: GlossaryAction,
}

#[derive(Debug, Default)]
pub(crate) struct GlossaryViewState {
    entries: Vec<GlossaryEntry>,
    pending_list: Option<RequestId>,
    pending_action: Option<PendingGlossaryAction>,
    failed_action: Option<GlossaryAction>,
    error: Option<String>,
    retryable: bool,
}

impl GlossaryViewState {
    pub(crate) fn entries(&self) -> &[GlossaryEntry] {
        &self.entries
    }

    pub(crate) fn merge_entry(&mut self, entry: GlossaryEntry) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|candidate| candidate.entry_id == entry.entry_id)
        {
            *existing = entry;
        } else {
            self.entries.insert(0, entry);
        }
    }

    pub(crate) fn begin_list(&mut self, _query: Option<String>) -> RequestId {
        let operation_id = RequestId::new();
        self.pending_list = Some(operation_id);
        operation_id
    }

    pub(crate) fn apply_list(
        &mut self,
        operation_id: RequestId,
        entries: Vec<GlossaryEntry>,
    ) -> bool {
        if self.pending_list != Some(operation_id) {
            return false;
        }
        self.pending_list = None;
        self.entries = entries;
        true
    }

    pub(crate) fn begin_action(&mut self, action: GlossaryAction) -> RequestId {
        let operation_id = RequestId::new();
        self.pending_action = Some(PendingGlossaryAction {
            operation_id,
            action,
        });
        self.failed_action = None;
        self.error = None;
        self.retryable = false;
        operation_id
    }

    pub(crate) fn apply_saved(&mut self, operation_id: RequestId, entry: GlossaryEntry) -> bool {
        if self
            .pending_action
            .as_ref()
            .is_none_or(|pending| pending.operation_id != operation_id)
        {
            return false;
        }
        self.pending_action = None;
        self.merge_entry(entry);
        true
    }

    pub(crate) fn apply_deleted(
        &mut self,
        operation_id: RequestId,
        entry_id: &GlossaryEntryId,
    ) -> bool {
        if self
            .pending_action
            .as_ref()
            .is_none_or(|pending| pending.operation_id != operation_id)
        {
            return false;
        }
        self.pending_action = None;
        self.entries.retain(|entry| &entry.entry_id != entry_id);
        true
    }

    pub(crate) fn apply_failure(
        &mut self,
        operation_id: RequestId,
        error: String,
        retryable: bool,
    ) -> bool {
        let Some(pending) = self.pending_action.take() else {
            return false;
        };
        if pending.operation_id != operation_id {
            self.pending_action = Some(pending);
            return false;
        }
        self.failed_action = Some(pending.action);
        self.error = Some(error);
        self.retryable = retryable;
        true
    }

    pub(crate) fn can_retry(&self) -> bool {
        self.retryable && self.failed_action.is_some()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn retry(&mut self) -> Option<PendingGlossaryAction> {
        if !self.can_retry() {
            return None;
        }
        let action = self.failed_action.take()?;
        let operation_id = self.begin_action(action.clone());
        Some(PendingGlossaryAction {
            operation_id,
            action,
        })
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{
        GlossaryEntry, GlossaryEntryDraft, GlossaryEntryId, GlossaryOrigin, RequestId,
        SourceContext, TranslationLanguage,
    };

    use super::{GlossaryAction, GlossaryViewState};

    #[test]
    fn list_update_delete_are_applied_only_to_the_matching_operation() {
        let mut view = GlossaryViewState::default();
        let list = view.begin_list(Some("cooldown".to_owned()));
        assert!(!view.apply_list(RequestId::new(), vec![entry("wrong")]));
        assert!(view.apply_list(list, vec![entry("entry")]));
        assert_eq!(view.entries().len(), 1);

        let update = view.begin_action(GlossaryAction::Update {
            entry_id: GlossaryEntryId::from("entry"),
            draft: draft(),
        });
        assert!(view.apply_saved(update, entry("entry")));
        let delete = view.begin_action(GlossaryAction::Delete(GlossaryEntryId::from("entry")));
        assert!(view.apply_deleted(delete, &GlossaryEntryId::from("entry")));
        assert!(view.entries().is_empty());
    }

    #[test]
    fn retryable_failure_retains_the_pending_action() {
        let mut view = GlossaryViewState::default();
        let operation = view.begin_action(GlossaryAction::Delete(GlossaryEntryId::from("entry")));
        assert!(view.apply_failure(operation, "try again".to_owned(), true));
        assert!(view.can_retry());
        let retry = view.retry().expect("retry action");
        assert_ne!(retry.operation_id, operation);
        assert!(matches!(retry.action, GlossaryAction::Delete(_)));
    }

    #[test]
    fn retryable_update_retains_the_exact_user_draft() {
        let mut view = GlossaryViewState::default();
        let expected = draft();
        let operation = view.begin_action(GlossaryAction::Update {
            entry_id: GlossaryEntryId::from("entry"),
            draft: expected.clone(),
        });
        assert!(view.apply_failure(operation, "try again".to_owned(), true));

        let retry = view.retry().expect("retry action");
        assert!(matches!(
            retry.action,
            GlossaryAction::Update { ref draft, .. } if draft == &expected
        ));
    }

    fn draft() -> GlossaryEntryDraft {
        GlossaryEntryDraft {
            source_phrase: "cooldown".to_owned(),
            approved_translation: "время перезарядки".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            example: "Reduces cooldown".to_owned(),
            context: SourceContext::default(),
            origin: GlossaryOrigin::Corrected,
        }
    }

    fn entry(id: &str) -> GlossaryEntry {
        GlossaryEntry {
            entry_id: GlossaryEntryId::from(id),
            source_phrase: "cooldown".to_owned(),
            approved_translation: "время перезарядки".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            examples: vec!["Reduces cooldown".to_owned()],
            context: SourceContext::default(),
            origin: GlossaryOrigin::Corrected,
            created_unix_ms: 1,
            updated_unix_ms: 2,
        }
    }
}
