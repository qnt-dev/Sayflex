use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};
use rust_protocol::{
    BlockCompletion, GlossaryEntry, ScreenTextBlock, SourceContext, TranslatedScreenBlock,
    TranslationBackendCapabilities,
};
use rust_translation_engine::protected_values_match;

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
            validate_partial_block(plan, &translated)?;
        }
        ordered.push(translated);
    }
    if !by_id.is_empty() {
        bail!("translation returned an unknown screen block id");
    }
    Ok(ordered)
}

pub(crate) fn validate_partial_block(
    plan: &BlockTranslationPlan,
    block: &TranslatedScreenBlock,
) -> Result<()> {
    let source = plan
        .blocks
        .iter()
        .find(|source| source.block_id == block.block_id)
        .ok_or_else(|| anyhow!("translation returned an unknown screen block id"))?;
    if block.completion != BlockCompletion::Completed || block.translated_text.trim().is_empty() {
        bail!("translation completed with an empty or incomplete screen block");
    }
    validate_protected_values(&source.source_text, &block.translated_text)
}

fn validate_protected_values(source: &str, translated: &str) -> Result<()> {
    if !protected_values_match(source, translated) {
        bail!("translation changed protected values (protected_values_changed)");
    }
    Ok(())
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
            quad: None,
            fragment_quads: Vec::new(),
            fragments: Vec::new(),
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
    fn named_month_date_is_valid_at_partial_and_final_worker_boundaries() {
        let plan = BlockTranslationPlan::new(
            vec![source("a", "February 10,2026. Collect within 30 days.")],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::limited(),
        )
        .unwrap();
        let block = translated("a", "10 февраля 2026. Заберите в течение 30 дней.");
        assert!(super::validate_partial_block(&plan, &block).is_ok());
        assert!(validate_completed_blocks(&plan, vec![block]).is_ok());
        for changed in [
            "11 февраля 2026, срок 30 дней",
            "10 февраля 2027, срок 30 дней",
        ] {
            assert!(super::validate_partial_block(&plan, &translated("a", changed)).is_err());
        }
    }

    #[test]
    fn attached_seconds_decimal_locale_is_valid_at_worker_partial_and_final_boundaries() {
        let plan = BlockTranslationPlan::new(
            vec![source("a", "lasts 4.6 seconds. 0.2s after")],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();
        let block = translated("a", "длится 4,6 секунды. Через 0,2 с");
        assert!(super::validate_partial_block(&plan, &block).is_ok());
        assert!(validate_completed_blocks(&plan, vec![block]).is_ok());
    }

    #[test]
    fn worker_rejects_changed_time_values_and_decimal_identifier_spelling() {
        for (input, output) in [
            ("0.2s", "0,3 с"),
            ("-0.2s", "0,2 с"),
            ("0.2s 0.2s", "0,2 с"),
            ("0.2s", "0,2%"),
            ("1.234s", "1,234с"),
            ("id0.2s", "id0,2s"),
            ("0.2serial", "0,2serial"),
            ("https://example.test/0.2s", "https://example.test/0,2s"),
            ("{delay_0.2s}", "{delay_0,2s}"),
        ] {
            let plan = BlockTranslationPlan::new(
                vec![source("a", input)],
                SourceContext::default(),
                Vec::new(),
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
            let block = translated("a", output);
            assert!(
                super::validate_partial_block(&plan, &block).is_err(),
                "{input:?}"
            );
            assert!(
                validate_completed_blocks(&plan, vec![block]).is_err(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn partial_validation_rejects_unsafe_rows_before_publication() {
        let plan = BlockTranslationPlan::new(
            vec![source("a", "Damage 25")],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();
        assert!(super::validate_partial_block(&plan, &translated("a", "Урон 25")).is_ok());
        for block in [
            translated("unknown", "Урон 25"),
            translated("a", "Урон 9"),
            translated("a", " "),
        ] {
            assert!(super::validate_partial_block(&plan, &block).is_err());
        }
        let mut failed = translated("a", "Урон 25");
        failed.completion = BlockCompletion::Failed;
        assert!(super::validate_partial_block(&plan, &failed).is_err());
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

    #[test]
    fn protected_value_failure_does_not_reveal_source_or_translation_tokens() {
        let plan = BlockTranslationPlan::new(
            vec![source(
                "a",
                "Open https://private.invalid/?token=SOURCE_SECRET {private_placeholder} 25",
            )],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();
        let error = validate_completed_blocks(
            &plan,
            vec![translated(
                "a",
                "Открыть https://other.invalid/?token=TRANSLATION_SECRET {changed_placeholder} 26",
            )],
        )
        .unwrap_err();

        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("protected_values_changed"),
            "{diagnostic}"
        );
        for private_value in [
            "SECRET",
            "private.invalid",
            "other.invalid",
            "placeholder",
            "25",
            "26",
        ] {
            assert!(!diagnostic.contains(private_value), "{diagnostic}");
        }
    }

    #[test]
    fn completed_blocks_accept_unambiguous_decimal_locale_changes() {
        let plan = BlockTranslationPlan::new(
            vec![source(
                "a",
                "Gain 35% evasion for 6.5 seconds, with -0.25 and +2.50%",
            )],
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::structured(),
        )
        .unwrap();
        let output = validate_completed_blocks(
            &plan,
            vec![translated(
                "a",
                "Уклонение 35% на 6,5 секунды, с -0,25 и +2,50%",
            )],
        )
        .unwrap();

        assert_eq!(output[0].completion, BlockCompletion::Completed);
        assert_eq!(
            output[0].translated_text,
            "Уклонение 35% на 6,5 секунды, с -0,25 и +2,50%"
        );
    }

    #[test]
    fn completed_blocks_keep_unchanged_numbers_when_unit_spacing_changes() {
        for (original, result) in [
            ("Duration 6.5s", "Длительность 6.5 секунды"),
            ("Duration 6,5s", "Длительность 6,5 секунды"),
            ("Duration 6.5 seconds", "Длительность 6.5с"),
            (
                "Duration 6.5s and 7.5 seconds",
                "Длительность 6.5 секунды и 7,5 секунды",
            ),
            ("Version v6.5 and value 6.5", "Версия v6.5 и значение 6,5"),
        ] {
            let plan = BlockTranslationPlan::new(
                vec![source("a", original)],
                SourceContext::default(),
                Vec::new(),
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
            assert!(validate_completed_blocks(&plan, vec![translated("a", result)]).is_ok());
        }
    }

    #[test]
    fn decimal_locale_validation_rejects_real_changes_and_ambiguous_tokens() {
        for (original, changed) in [
            ("Duration 6.5", "Длительность 6,6"),
            ("Change -6.5", "Изменить 6,5"),
            ("Change 6.5 and 6.5", "Изменить 6,5"),
            ("Rate 6.5%", "Ставка 6,5"),
            ("Cost 1,234", "Стоимость 1.234"),
            ("Cost 1,234.50", "Стоимость 1.234,50"),
            ("Date 12.10.2026", "Дата 12,10,2026"),
            ("Version v6.5", "Версия v6,5"),
            ("Item item_6.5", "Предмет item_6,5"),
            ("Build release-6.5", "Сборка release-6,5"),
            ("Build 6.5.exe", "Сборка 6,5.exe"),
            ("Version v6.5", "Версия 6,5"),
            (
                "Open https://example.test/6.5",
                "Открыть https://example.test/6,5",
            ),
            ("Value {amount6.5}", "Значение {amount6,5}"),
        ] {
            let plan = BlockTranslationPlan::new(
                vec![source("a", original)],
                SourceContext::default(),
                Vec::new(),
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();

            assert!(validate_completed_blocks(&plan, vec![translated("a", changed)]).is_err());
        }
    }
}
