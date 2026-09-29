// The owner is introduced before the block-translation and correction views
// that consume its full API. Keeping the complete mutation boundary here is
// intentional; the following plan slices wire each method without duplicating
// state in UI code.
#![allow(dead_code)]

use std::{collections::HashMap, sync::Arc};

use rust_ocr_engine::ReconstructedElement;
use rust_platform_windows::screen_capture::RgbaFrame;
use rust_protocol::{
    BlockCompletion, PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext,
    TranslatedScreenBlock,
};

const LOW_CONFIDENCE_MILLI: u16 = 650;
pub(crate) const MIN_ADMITTED_OCR_CONFIDENCE_MILLI: u16 = 350;
pub(crate) const DEFAULT_SCREENSHOT_RETENTION_MS: u64 = 24 * 60 * 60 * 1_000;

pub(crate) fn screen_blocks_from_ocr(elements: Vec<ReconstructedElement>) -> Vec<ScreenTextBlock> {
    elements
        .into_iter()
        .enumerate()
        .map(|(index, element)| ScreenTextBlock {
            block_id: ScreenBlockId::from(format!("screen-block-{index:04}")),
            source_text: element.text,
            bounds: element.bounds,
            confidence_milli: element.confidence_milli,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentError {
    EmptyDocument,
    EmptyBlockId,
    DuplicateBlockId,
    UnknownBlockId,
    MissingBlockId,
    RequestMismatch,
    RevisionMismatch,
    MachineResultAlreadyApplied,
    EmptyUserEdit,
    IncompleteBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreservationKind {
    Number,
    Url,
    Placeholder,
    ProtectedSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreservationWarning {
    pub(crate) kind: PreservationKind,
    pub(crate) token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserTranslationEdit {
    text: String,
}

impl UserTranslationEdit {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenBlockState {
    pub(crate) source: String,
    pub(crate) bounds: PhysicalRect,
    pub(crate) confidence_milli: u16,
    pub(crate) machine: Option<TranslatedScreenBlock>,
    pub(crate) user_edit: Option<UserTranslationEdit>,
    warnings: Vec<PreservationWarning>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScreenResultBlockSnapshot {
    pub(crate) block_id: ScreenBlockId,
    pub(crate) source: String,
    pub(crate) translation: String,
    pub(crate) completion: BlockCompletion,
    pub(crate) failure_message: Option<String>,
    pub(crate) bounds: PhysicalRect,
    pub(crate) confidence_milli: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct ScreenResultSnapshot {
    pub(crate) request_id: RequestId,
    pub(crate) revision: u64,
    pub(crate) crop: Option<Arc<RgbaFrame>>,
    pub(crate) crop_rect: PhysicalRect,
    pub(crate) blocks: Vec<ScreenResultBlockSnapshot>,
    pub(crate) context: SourceContext,
    pub(crate) created_unix_ms: u64,
    pub(crate) expires_unix_ms: u64,
}

impl ScreenBlockState {
    pub(crate) fn low_confidence(&self) -> bool {
        self.confidence_milli < LOW_CONFIDENCE_MILLI
    }

    pub(crate) fn warnings(&self) -> &[PreservationWarning] {
        &self.warnings
    }

    fn effective_translation(&self) -> Option<&str> {
        self.user_edit
            .as_ref()
            .map(|edit| edit.text.as_str())
            .or_else(|| {
                self.machine
                    .as_ref()
                    .map(|machine| machine.translated_text.as_str())
            })
    }

    fn is_safe_for_copy(&self) -> bool {
        self.user_edit.is_some()
            || self
                .machine
                .as_ref()
                .is_some_and(|machine| machine.completion == BlockCompletion::Completed)
    }

    fn refresh_warnings(&mut self) {
        self.warnings = self
            .effective_translation()
            .map(|translation| preservation_warnings(&self.source, translation))
            .unwrap_or_default();
    }
}

#[derive(Debug)]
pub(crate) struct ScreenTranslationDocument {
    request_id: RequestId,
    revision: u64,
    machine_base_revision: u64,
    machine_applied: bool,
    crop: Option<Arc<RgbaFrame>>,
    crop_rect: PhysicalRect,
    reading_order: Vec<ScreenBlockId>,
    blocks: HashMap<ScreenBlockId, ScreenBlockState>,
    pub(crate) context: SourceContext,
    pub(crate) created_unix_ms: u64,
    pub(crate) expires_unix_ms: u64,
}

pub(crate) struct PersistedScreenResult {
    pub(crate) request_id: RequestId,
    pub(crate) crop: Arc<RgbaFrame>,
    pub(crate) blocks: Vec<ScreenTextBlock>,
    pub(crate) translations: Vec<TranslatedScreenBlock>,
    pub(crate) context: SourceContext,
    pub(crate) revision: u64,
    pub(crate) created_unix_ms: u64,
    pub(crate) expires_unix_ms: u64,
}

impl ScreenTranslationDocument {
    pub(crate) fn new(
        request_id: RequestId,
        crop: Arc<RgbaFrame>,
        blocks: Vec<ScreenTextBlock>,
        context: SourceContext,
        created_unix_ms: u64,
        expires_unix_ms: u64,
    ) -> Result<Self, DocumentError> {
        if blocks.is_empty() {
            return Err(DocumentError::EmptyDocument);
        }
        let mut reading_order = Vec::with_capacity(blocks.len());
        let mut states = HashMap::with_capacity(blocks.len());
        for block in blocks {
            if block.block_id.as_str().trim().is_empty() {
                return Err(DocumentError::EmptyBlockId);
            }
            if states.contains_key(&block.block_id) {
                return Err(DocumentError::DuplicateBlockId);
            }
            reading_order.push(block.block_id.clone());
            states.insert(
                block.block_id,
                ScreenBlockState {
                    source: block.source_text,
                    bounds: block.bounds,
                    confidence_milli: block.confidence_milli,
                    machine: None,
                    user_edit: None,
                    warnings: Vec::new(),
                },
            );
        }
        let crop_rect = crop.rect;
        Ok(Self {
            request_id,
            revision: 0,
            machine_base_revision: 0,
            machine_applied: false,
            crop: Some(crop),
            crop_rect,
            reading_order,
            blocks: states,
            context,
            created_unix_ms,
            expires_unix_ms,
        })
    }

    pub(crate) fn restore(persisted: PersistedScreenResult) -> Result<Self, DocumentError> {
        let mut document = Self::new(
            persisted.request_id,
            persisted.crop,
            persisted.blocks,
            persisted.context,
            persisted.created_unix_ms,
            persisted.expires_unix_ms,
        )?;
        if !persisted.translations.is_empty() {
            document.apply_machine_blocks(persisted.request_id, 0, persisted.translations)?;
        }
        document.revision = document.revision.max(persisted.revision);
        Ok(document)
    }

    pub(crate) fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn crop_rect(&self) -> PhysicalRect {
        self.crop_rect
    }

    pub(crate) fn has_pixels(&self) -> bool {
        self.crop.is_some()
    }

    pub(crate) fn block(&self, block_id: &ScreenBlockId) -> Option<&ScreenBlockState> {
        self.blocks.get(block_id)
    }

    pub(crate) fn source_blocks(&self) -> Vec<ScreenTextBlock> {
        self.reading_order
            .iter()
            .filter_map(|block_id| {
                let state = self.blocks.get(block_id)?;
                Some(ScreenTextBlock {
                    block_id: block_id.clone(),
                    source_text: state.source.clone(),
                    bounds: state.bounds,
                    confidence_milli: state.confidence_milli,
                })
            })
            .collect()
    }

    pub(crate) fn source_text(&self) -> String {
        self.reading_order
            .iter()
            .filter_map(|block_id| self.blocks.get(block_id).map(|block| block.source.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub(crate) fn effective_translation(&self, block_id: &ScreenBlockId) -> Option<&str> {
        self.blocks.get(block_id)?.effective_translation()
    }

    pub(crate) fn copy_text(&self) -> Result<String, DocumentError> {
        let mut translations = Vec::with_capacity(self.reading_order.len());
        for block_id in &self.reading_order {
            let block = self
                .blocks
                .get(block_id)
                .ok_or(DocumentError::UnknownBlockId)?;
            if !block.is_safe_for_copy() {
                return Err(DocumentError::IncompleteBlock);
            }
            translations.push(
                block
                    .effective_translation()
                    .ok_or(DocumentError::IncompleteBlock)?,
            );
        }
        Ok(translations.join("\n\n"))
    }

    pub(crate) fn apply_machine_blocks(
        &mut self,
        request_id: RequestId,
        base_revision: u64,
        translated: Vec<TranslatedScreenBlock>,
    ) -> Result<(), DocumentError> {
        if request_id != self.request_id {
            return Err(DocumentError::RequestMismatch);
        }
        if base_revision != self.machine_base_revision {
            return Err(DocumentError::RevisionMismatch);
        }
        if self.machine_applied {
            return Err(DocumentError::MachineResultAlreadyApplied);
        }
        if translated.len() < self.blocks.len() {
            return Err(DocumentError::MissingBlockId);
        }
        let mut by_id = HashMap::with_capacity(translated.len());
        for block in translated {
            if !self.blocks.contains_key(&block.block_id) {
                return Err(DocumentError::UnknownBlockId);
            }
            if by_id.insert(block.block_id.clone(), block).is_some() {
                return Err(DocumentError::DuplicateBlockId);
            }
        }
        if by_id.len() != self.blocks.len() {
            return Err(DocumentError::MissingBlockId);
        }
        for block_id in &self.reading_order {
            let machine = by_id
                .remove(block_id)
                .ok_or(DocumentError::MissingBlockId)?;
            let state = self
                .blocks
                .get_mut(block_id)
                .ok_or(DocumentError::UnknownBlockId)?;
            state.machine = Some(machine);
            state.refresh_warnings();
        }
        self.machine_applied = true;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub(crate) fn save_user_edit(
        &mut self,
        block_id: &ScreenBlockId,
        expected_revision: u64,
        edit: UserTranslationEdit,
    ) -> Result<(), DocumentError> {
        if expected_revision != self.revision {
            return Err(DocumentError::RevisionMismatch);
        }
        if edit.text.trim().is_empty() {
            return Err(DocumentError::EmptyUserEdit);
        }
        let block = self
            .blocks
            .get_mut(block_id)
            .ok_or(DocumentError::UnknownBlockId)?;
        block.user_edit = Some(edit);
        block.refresh_warnings();
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub(crate) fn expire_pixels(&mut self, now_unix_ms: u64) -> Option<Arc<RgbaFrame>> {
        (now_unix_ms >= self.expires_unix_ms)
            .then(|| self.crop.take())
            .flatten()
    }

    pub(crate) fn result_snapshot(&self) -> Result<ScreenResultSnapshot, DocumentError> {
        let mut blocks = Vec::with_capacity(self.reading_order.len());
        for block_id in &self.reading_order {
            let state = self
                .blocks
                .get(block_id)
                .ok_or(DocumentError::UnknownBlockId)?;
            let (completion, failure_message) = if state.user_edit.is_some() {
                (BlockCompletion::Completed, None)
            } else {
                let machine = state
                    .machine
                    .as_ref()
                    .ok_or(DocumentError::IncompleteBlock)?;
                (machine.completion, machine.failure_message.clone())
            };
            blocks.push(ScreenResultBlockSnapshot {
                block_id: block_id.clone(),
                source: state.source.clone(),
                translation: state
                    .effective_translation()
                    .ok_or(DocumentError::IncompleteBlock)?
                    .to_owned(),
                completion,
                failure_message,
                bounds: state.bounds,
                confidence_milli: state.confidence_milli,
            });
        }
        Ok(ScreenResultSnapshot {
            request_id: self.request_id,
            revision: self.revision,
            crop: self.crop.clone(),
            crop_rect: self.crop_rect,
            blocks,
            context: self.context.clone(),
            created_unix_ms: self.created_unix_ms,
            expires_unix_ms: self.expires_unix_ms,
        })
    }
}

fn preservation_warnings(source: &str, translation: &str) -> Vec<PreservationWarning> {
    let mut warnings = Vec::new();
    for (kind, expected, actual) in [
        (
            PreservationKind::Number,
            number_tokens(source),
            number_tokens(translation),
        ),
        (
            PreservationKind::Url,
            url_tokens(source),
            url_tokens(translation),
        ),
        (
            PreservationKind::Placeholder,
            placeholder_tokens(source),
            placeholder_tokens(translation),
        ),
        (
            PreservationKind::ProtectedSymbol,
            protected_symbol_tokens(source),
            protected_symbol_tokens(translation),
        ),
    ] {
        let actual_counts = actual.into_iter().fold(
            std::collections::HashMap::<String, usize>::new(),
            |mut counts, token| {
                *counts.entry(token).or_default() += 1;
                counts
            },
        );
        let mut expected_counts = std::collections::HashMap::<String, usize>::new();
        for token in expected {
            let count = expected_counts.entry(token.clone()).or_default();
            *count += 1;
            if *count > actual_counts.get(&token).copied().unwrap_or_default()
                && !warnings.iter().any(|warning: &PreservationWarning| {
                    warning.kind == kind && warning.token == token
                })
            {
                warnings.push(PreservationWarning { kind, token });
            }
        }
    }
    warnings
}

fn number_tokens(text: &str) -> Vec<String> {
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (start, character) = chars[index];
        let signed_number = matches!(character, '+' | '-')
            && chars
                .get(index + 1)
                .is_some_and(|(_, next)| next.is_ascii_digit())
            && index
                .checked_sub(1)
                .and_then(|previous| chars.get(previous))
                .is_none_or(|(_, previous)| !previous.is_alphanumeric() && *previous != '_');
        if !character.is_ascii_digit() && !signed_number {
            index += 1;
            continue;
        }
        let mut end = start + character.len_utf8();
        index += 1;
        while index < chars.len() {
            let (byte, next) = chars[index];
            let numeric_separator = matches!(next, '.' | ',' | ':' | '-' | '/')
                && chars
                    .get(index + 1)
                    .is_some_and(|(_, following)| following.is_ascii_digit());
            if next.is_ascii_digit() || numeric_separator || next == '%' {
                end = byte + next.len_utf8();
                index += 1;
                if next == '%' {
                    break;
                }
            } else {
                break;
            }
        }
        tokens.push(text[start..end].to_owned());
    }
    tokens
}

fn url_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|character: char| ",.;:!?()[]{}\"'".contains(character)))
        .filter(|token| token.starts_with("https://") || token.starts_with("http://"))
        .map(str::to_owned)
        .collect()
}

fn placeholder_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find('{') {
        let start = cursor + start;
        let Some(end) = text[start + 1..].find('}') else {
            break;
        };
        let end = start + 1 + end;
        tokens.push(text[start..=end].to_owned());
        cursor = end + 1;
    }
    let bytes = text.as_bytes();
    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index] == b'%' && bytes[index + 1].is_ascii_alphabetic() {
            tokens.push(text[index..index + 2].to_owned());
        }
    }
    tokens
}

fn protected_symbol_tokens(text: &str) -> Vec<String> {
    text.chars()
        .filter(|character| matches!(character, '©' | '®' | '™'))
        .map(|character| character.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::{
        BlockCompletion, PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext,
        TranslatedScreenBlock,
    };

    use super::{
        DocumentError, PersistedScreenResult, PreservationKind, ScreenTranslationDocument,
        UserTranslationEdit, preservation_warnings, screen_blocks_from_ocr,
    };

    fn block(id: &str, source: &str, confidence_milli: u16) -> ScreenTextBlock {
        ScreenTextBlock {
            block_id: ScreenBlockId::from(id),
            source_text: source.to_owned(),
            bounds: PhysicalRect {
                x: -20,
                y: 10,
                width: 240,
                height: 32,
            },
            confidence_milli,
        }
    }

    fn translated(id: &str, text: &str) -> TranslatedScreenBlock {
        TranslatedScreenBlock {
            block_id: ScreenBlockId::from(id),
            translated_text: text.to_owned(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        }
    }

    fn crop() -> Arc<RgbaFrame> {
        Arc::new(
            RgbaFrame::try_new(
                PhysicalRect {
                    x: -20,
                    y: 10,
                    width: 2,
                    height: 1,
                },
                8,
                vec![1, 2, 3, 255, 4, 5, 6, 255],
            )
            .unwrap(),
        )
    }

    fn document() -> ScreenTranslationDocument {
        ScreenTranslationDocument::new(
            RequestId::new(),
            crop(),
            vec![block("a", "Damage 25", 980), block("b", "Cooldown", 610)],
            SourceContext::default(),
            1_000,
            2_000,
        )
        .unwrap()
    }

    #[test]
    fn persisted_effective_translation_restores_with_its_monotonic_revision() {
        let request_id = RequestId::new();
        let restored = ScreenTranslationDocument::restore(PersistedScreenResult {
            request_id,
            crop: crop(),
            blocks: vec![block("a", "Cooldown", 900)],
            translations: vec![translated("a", "перезарядка")],
            context: SourceContext::default(),
            revision: 7,
            created_unix_ms: 1_000,
            expires_unix_ms: 2_000,
        })
        .unwrap();

        assert_eq!(restored.revision(), 7);
        assert_eq!(restored.copy_text().unwrap(), "перезарядка");
        assert!(restored.result_snapshot().unwrap().crop.is_some());
    }

    #[test]
    fn ocr_reading_order_gets_stable_request_local_ids() {
        let blocks = screen_blocks_from_ocr(vec![
            rust_ocr_engine::ReconstructedElement {
                text: "first".to_owned(),
                bounds: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence_milli: 900,
            },
            rust_ocr_engine::ReconstructedElement {
                text: "second".to_owned(),
                bounds: PhysicalRect {
                    x: 20,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence_milli: 800,
            },
        ]);

        assert_eq!(blocks[0].block_id.as_str(), "screen-block-0000");
        assert_eq!(blocks[1].block_id.as_str(), "screen-block-0001");
        assert_eq!(blocks[1].source_text, "second");
        assert_eq!(blocks[1].bounds.x, 20);
    }

    #[test]
    fn machine_mapping_must_be_unique_complete_and_known() {
        let mut document = document();
        let request_id = document.request_id();

        assert_eq!(
            document.apply_machine_blocks(request_id, 0, vec![translated("a", "Урон 25")]),
            Err(DocumentError::MissingBlockId)
        );
        assert_eq!(
            document.apply_machine_blocks(
                request_id,
                0,
                vec![translated("a", "Урон 25"), translated("a", "Повтор")]
            ),
            Err(DocumentError::DuplicateBlockId)
        );
        assert_eq!(
            document.apply_machine_blocks(
                request_id,
                0,
                vec![translated("a", "Урон 25"), translated("unknown", "?")]
            ),
            Err(DocumentError::UnknownBlockId)
        );
    }

    #[test]
    fn stale_request_and_revision_are_rejected() {
        let mut document = document();
        assert_eq!(
            document.apply_machine_blocks(
                RequestId::new(),
                0,
                vec![translated("a", "Урон 25"), translated("b", "Перезарядка")]
            ),
            Err(DocumentError::RequestMismatch)
        );
        assert_eq!(
            document.save_user_edit(
                &ScreenBlockId::from("a"),
                7,
                UserTranslationEdit::new("Урон 25")
            ),
            Err(DocumentError::RevisionMismatch)
        );
        assert_eq!(
            document.apply_machine_blocks(
                document.request_id(),
                7,
                vec![translated("a", "Урон 25"), translated("b", "Перезарядка")]
            ),
            Err(DocumentError::RevisionMismatch)
        );
    }

    #[test]
    fn user_edit_wins_when_the_matching_machine_result_arrives_late() {
        let mut document = document();
        let request_id = document.request_id();
        document
            .save_user_edit(
                &ScreenBlockId::from("a"),
                0,
                UserTranslationEdit::new("Исправленный урон 25"),
            )
            .unwrap();

        document
            .apply_machine_blocks(
                request_id,
                0,
                vec![
                    translated("a", "Машинный урон 25"),
                    translated("b", "Перезарядка"),
                ],
            )
            .unwrap();

        assert_eq!(
            document.effective_translation(&ScreenBlockId::from("a")),
            Some("Исправленный урон 25")
        );
        assert_eq!(document.revision(), 2);
    }

    #[test]
    fn preservation_warnings_and_low_confidence_remain_block_scoped() {
        let request_id = RequestId::new();
        let mut document = ScreenTranslationDocument::new(
            request_id,
            crop(),
            vec![block(
                "a",
                "Open https://example.com for {count} items: 25 ™",
                620,
            )],
            SourceContext::default(),
            1_000,
            2_000,
        )
        .unwrap();
        document
            .apply_machine_blocks(request_id, 0, vec![translated("a", "Открыть список")])
            .unwrap();

        let state = document.block(&ScreenBlockId::from("a")).unwrap();
        assert!(state.low_confidence());
        for kind in [
            PreservationKind::Number,
            PreservationKind::Url,
            PreservationKind::Placeholder,
            PreservationKind::ProtectedSymbol,
        ] {
            assert!(state.warnings().iter().any(|warning| warning.kind == kind));
        }
    }

    #[test]
    fn preservation_warnings_compare_exact_counted_numbers_instead_of_substrings() {
        let warnings = preservation_warnings("25 25 -7", "125 25 +7");

        assert!(
            warnings.iter().any(|warning| {
                warning.kind == PreservationKind::Number && warning.token == "25"
            })
        );
        assert!(
            warnings.iter().any(|warning| {
                warning.kind == PreservationKind::Number && warning.token == "-7"
            })
        );
    }

    #[test]
    fn copy_projection_uses_edits_and_pixel_expiry_keeps_text() {
        let mut document = document();
        let request_id = document.request_id();
        document
            .apply_machine_blocks(
                request_id,
                0,
                vec![translated("a", "Урон 25"), translated("b", "Перезарядка")],
            )
            .unwrap();
        let revision = document.revision();
        document
            .save_user_edit(
                &ScreenBlockId::from("b"),
                revision,
                UserTranslationEdit::new("Время восстановления"),
            )
            .unwrap();

        assert_eq!(
            document.copy_text().unwrap(),
            "Урон 25\n\nВремя восстановления"
        );
        assert!(document.expire_pixels(1_999).is_none());
        assert!(document.expire_pixels(2_000).is_some());
        assert!(!document.has_pixels());
        assert_eq!(document.source_text(), "Damage 25\n\nCooldown");
        assert_eq!(
            document.copy_text().unwrap(),
            "Урон 25\n\nВремя восстановления"
        );
    }
}
