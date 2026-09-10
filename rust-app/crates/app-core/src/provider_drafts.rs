use std::collections::HashMap;

use rust_protocol::RequestId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Speech,
    Translation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderDraftKey {
    pub kind: ProviderKind,
    pub provider_id: String,
}

impl ProviderDraftKey {
    #[must_use]
    pub fn new(kind: ProviderKind, provider_id: impl Into<String>) -> Self {
        Self {
            kind,
            provider_id: provider_id.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderDraft {
    pub secret: String,
    pub extra: String,
    pub pending_operation: Option<RequestId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftAcknowledgement {
    Ignored,
    Retained,
    Cleared,
}

/// Owns unsaved provider forms independently of the Slint repeater that draws
/// them. Status refreshes and row reorders may rebuild that repeater; they must
/// never destroy a key the user is still typing.
#[derive(Debug, Default)]
pub struct ProviderDraftStore {
    drafts: HashMap<ProviderDraftKey, ProviderDraft>,
}

impl ProviderDraftStore {
    #[must_use]
    pub fn get(&self, key: &ProviderDraftKey) -> Option<&ProviderDraft> {
        self.drafts.get(key)
    }

    pub fn edit(
        &mut self,
        key: ProviderDraftKey,
        secret: impl Into<String>,
        extra: impl Into<String>,
    ) {
        let draft = self.drafts.entry(key).or_default();
        draft.secret = secret.into();
        draft.extra = extra.into();
    }

    pub fn ensure(&mut self, key: ProviderDraftKey, extra: impl Into<String>) {
        self.drafts.entry(key).or_insert_with(|| ProviderDraft {
            extra: extra.into(),
            ..ProviderDraft::default()
        });
    }

    pub fn begin_save(&mut self, key: ProviderDraftKey, operation_id: RequestId) {
        self.drafts.entry(key).or_default().pending_operation = Some(operation_id);
    }

    pub fn cancel_save(&mut self, key: &ProviderDraftKey, operation_id: RequestId) {
        if self
            .drafts
            .get(key)
            .is_some_and(|draft| draft.pending_operation == Some(operation_id))
            && let Some(draft) = self.drafts.get_mut(key)
        {
            draft.pending_operation = None;
        }
    }

    pub fn acknowledge(
        &mut self,
        key: &ProviderDraftKey,
        operation_id: RequestId,
        succeeded: bool,
    ) -> DraftAcknowledgement {
        let Some(draft) = self.drafts.get(key) else {
            return DraftAcknowledgement::Ignored;
        };
        if draft.pending_operation != Some(operation_id) {
            return DraftAcknowledgement::Ignored;
        }
        if succeeded {
            self.drafts.remove(key);
            DraftAcknowledgement::Cleared
        } else {
            if let Some(draft) = self.drafts.get_mut(key) {
                draft.pending_operation = None;
            }
            DraftAcknowledgement::Retained
        }
    }

    pub fn discard(&mut self, key: &ProviderDraftKey) {
        self.drafts.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::RequestId;

    use super::{DraftAcknowledgement, ProviderDraftKey, ProviderDraftStore, ProviderKind};

    fn key(id: &str) -> ProviderDraftKey {
        ProviderDraftKey::new(ProviderKind::Speech, id)
    }

    #[test]
    fn a_projection_refresh_or_reorder_cannot_replace_a_draft() {
        let mut drafts = ProviderDraftStore::default();
        let groq = key("groq");
        drafts.edit(groq.clone(), "secret-in-progress", "account-in-progress");

        // Status and ordering are deliberately absent from the store's API.
        // Reading after either projection operation returns the same owner.
        assert_eq!(drafts.get(&groq).unwrap().secret, "secret-in-progress");
        assert_eq!(drafts.get(&groq).unwrap().extra, "account-in-progress");
    }

    #[test]
    fn only_the_matching_success_acknowledgement_clears_typed_text() {
        let mut drafts = ProviderDraftStore::default();
        let groq = key("groq");
        let expected = RequestId::new();
        drafts.edit(groq.clone(), "secret", "");
        drafts.begin_save(groq.clone(), expected);

        assert_eq!(
            drafts.acknowledge(&groq, RequestId::new(), true),
            DraftAcknowledgement::Ignored
        );
        assert!(drafts.get(&groq).is_some());
        assert_eq!(
            drafts.acknowledge(&groq, expected, false),
            DraftAcknowledgement::Retained
        );
        assert_eq!(drafts.get(&groq).unwrap().secret, "secret");

        drafts.begin_save(groq.clone(), expected);
        assert_eq!(
            drafts.acknowledge(&groq, expected, true),
            DraftAcknowledgement::Cleared
        );
        assert!(drafts.get(&groq).is_none());
    }
}
