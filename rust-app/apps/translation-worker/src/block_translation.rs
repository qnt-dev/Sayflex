use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};
use rust_protocol::{
    BlockCompletion, GlossaryEntry, ScreenTextBlock, SourceContext, TranslatedScreenBlock,
    TranslationBackendCapabilities,
};

/// Immutable input assembled before a backend request starts.  Keeping the
/// OCR blocks, context and glossary guidance together prevents the worker from
/// later reconstructing one from another or concatenating instructions into
/// user text.
pub(crate) struct BlockTranslationPlan {
    pub(crate) blocks: Vec<ScreenTextBlock>,
    pub(crate) context: SourceContext,
    pub(crate) glossary_matches: Vec<GlossaryEntry>,
    pub(crate) backend_capabilities: TranslationBackendCapabilities,
}

impl BlockTranslationPlan {
    pub(crate) fn new(
        blocks: Vec<ScreenTextBlock>,
        context: SourceContext,
        glossary_matches: Vec<GlossaryEntry>,
        backend_capabilities: TranslationBackendCapabilities,
    ) -> Result<Self> {
        if blocks.is_empty() {
            bail!("screen translation contains no blocks");
        }
        let mut ids = HashSet::with_capacity(blocks.len());
        for block in &blocks {
            if block.block_id.as_str().trim().is_empty() || block.source_text.trim().is_empty() {
                bail!("screen translation contains an empty block");
            }
            if !ids.insert(block.block_id.clone()) {
                bail!("screen translation contains a duplicate block id");
            }
        }
        Ok(Self {
            blocks,
            context,
            glossary_matches,
            backend_capabilities,
        })
    }
}

pub(crate) fn validate_completed_blocks(
    plan: &BlockTranslationPlan,
    blocks: Vec<TranslatedScreenBlock>,
) -> Result<Vec<TranslatedScreenBlock>> {
    if blocks.len() != plan.blocks.len() {
        bail!("translation returned a different number of screen blocks");
    }
    let mut by_id = HashMap::with_capacity(blocks.len());
    for block in blocks {
        if by_id.insert(block.block_id.clone(), block).is_some() {
            bail!("translation returned a duplicate screen block id");
        }
    }
    let mut ordered = Vec::with_capacity(plan.blocks.len());
    for source in &plan.blocks {
        let translated = by_id
            .remove(&source.block_id)
            .ok_or_else(|| anyhow!("translation omitted a screen block id"))?;
        if translated.completion == BlockCompletion::Completed {
            if translated.translated_text.trim().is_empty() {
                bail!("translation completed with an empty screen block");
            }
            validate_protected_values(&source.source_text, &translated.translated_text)?;
        }
        ordered.push(translated);
    }
    if !by_id.is_empty() {
        bail!("translation returned an unknown screen block id");
    }
    Ok(ordered)
}

fn validate_protected_values(source: &str, translated: &str) -> Result<()> {
    let expected = protected_values(source);
    let actual = protected_values(translated);
    if expected != actual {
        bail!("translation changed protected values: expected {expected:?}, received {actual:?}");
    }
    Ok(())
}

fn protected_values(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let chars = text.char_indices().collect::<Vec<_>>();
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
        if character.is_ascii_digit() || signed_number {
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
            values.push(text[start..end].to_owned());
            continue;
        }
        if character == '{'
            && let Some(relative_end) = text[start..].find('}')
        {
            let end = start + relative_end + 1;
            values.push(text[start..end].to_owned());
            while index < chars.len() && chars[index].0 < end {
                index += 1;
            }
            continue;
        }
        if matches!(character, '™' | '©' | '®') {
            values.push(character.to_string());
        }
        index += 1;
    }
    for token in text.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            matches!(
                character,
                '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '!' | '?'
            )
        });
        if token.starts_with("https://") || token.starts_with("http://") {
            values.push(token.to_owned());
        }
    }
    values.sort();
    values
}

#[cfg(test)]
mod tests {
    use rust_protocol::{
        BlockCompletion, PhysicalRect, ScreenBlockId, ScreenTextBlock, SourceContext,
        TranslatedScreenBlock, TranslationBackendCapabilities,
    };

    use super::{BlockTranslationPlan, validate_completed_blocks};

    fn source(id: &str, text: &str) -> ScreenTextBlock {
        ScreenTextBlock {
            block_id: ScreenBlockId::from(id),
            source_text: text.to_owned(),
            bounds: PhysicalRect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
            confidence_milli: 900,
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

    #[test]
    fn reordered_backend_output_is_restored_to_ocr_reading_order() {
        let plan = BlockTranslationPlan::new(
            vec![source("a", "Damage 25"), source("b", "Cooldown")],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();

        let ordered = validate_completed_blocks(
            &plan,
            vec![translated("b", "Перезарядка"), translated("a", "Урон 25")],
        )
        .unwrap();

        assert_eq!(ordered[0].block_id.as_str(), "a");
        assert_eq!(ordered[1].block_id.as_str(), "b");
    }

    #[test]
    fn missing_duplicate_unknown_and_damaged_protected_values_are_rejected() {
        let plan = BlockTranslationPlan::new(
            vec![
                source("a", "Open https://example.com for {count}: 25 ™"),
                source("b", "Cooldown"),
            ],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();

        assert!(validate_completed_blocks(&plan, vec![translated("a", "Открыть")]).is_err());
        assert!(
            validate_completed_blocks(
                &plan,
                vec![translated("a", "Открыть"), translated("a", "Повтор")]
            )
            .is_err()
        );
        assert!(
            validate_completed_blocks(
                &plan,
                vec![translated("a", "Открыть"), translated("unknown", "?")]
            )
            .is_err()
        );
        assert!(
            validate_completed_blocks(
                &plan,
                vec![
                    translated("a", "Открыть 25"),
                    translated("b", "Перезарядка")
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn limited_adapters_are_explicit_about_unsupported_context_features() {
        let limited = TranslationBackendCapabilities::limited();
        assert!(!limited.structured_blocks);
        assert!(!limited.context_instructions);
        assert!(!limited.glossary_guidance);
    }

    #[test]
    fn protected_numbers_are_compared_as_counted_tokens_not_substrings() {
        let repeated = BlockTranslationPlan::new(
            vec![source("numbers", "Damage -25, then 25 and 25")],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();

        assert!(
            validate_completed_blocks(
                &repeated,
                vec![translated("numbers", "Урон -125, затем 25 и 25")]
            )
            .is_err()
        );
        assert!(
            validate_completed_blocks(
                &repeated,
                vec![translated("numbers", "Урон -25, затем только 25")]
            )
            .is_err()
        );
        assert!(
            validate_completed_blocks(
                &repeated,
                vec![translated("numbers", "Урон -25, затем 25 и 25")]
            )
            .is_ok()
        );
    }
}
