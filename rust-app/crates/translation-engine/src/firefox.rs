use std::{path::Path, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use fxtranslate::{
    engine::{DecodeTermination, Engine},
    shortlist::Shortlist,
};
use rust_protocol::TranslationLanguage;

#[cfg(test)]
use crate::FirefoxDirectionDescriptor;
use crate::{TranslationBackendDescriptor, TranslationCancelled};

pub(crate) struct FirefoxRuntime {
    directions: Vec<LoadedDirection>,
}

struct LoadedDirection {
    source: TranslationLanguage,
    target: TranslationLanguage,
    engine: Engine,
}

#[derive(Debug)]
pub(crate) struct FirefoxTranslation {
    pub text: String,
    pub first_result_duration_ms: u64,
    pub translated_paragraphs: u32,
    pub completion: FragmentCompletion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentCompletion {
    Completed,
    LimitReached,
    Cancelled,
    Failed,
}

#[derive(Debug)]
struct ParagraphTranslation {
    text: String,
    translated_paragraphs: u32,
    completion: FragmentCompletion,
}

struct FragmentTranslation {
    text: String,
    completion: FragmentCompletion,
}

impl FragmentTranslation {
    #[cfg(test)]
    fn completed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            completion: FragmentCompletion::Completed,
        }
    }
}

impl FirefoxRuntime {
    pub(crate) fn load(backend: &TranslationBackendDescriptor, root: &Path) -> Result<Self> {
        let TranslationBackendDescriptor::Firefox { directions } = backend else {
            bail!("translation pack is not a Firefox compact pack");
        };
        let mut loaded = Vec::with_capacity(directions.len());
        for direction in directions {
            let model = root.join(&direction.model_file);
            let source_vocab = root.join(&direction.source_vocab_file);
            let target_vocab = root.join(&direction.target_vocab_file);
            let shortlist = root.join(&direction.shortlist_file);
            let engine = Engine::load_mmapped(&model, &source_vocab, &target_vocab)
                .map_err(|error| anyhow!(error))
                .with_context(|| {
                    format!(
                        "could not load compact {} to {} model",
                        direction.source.display_name(),
                        direction.target.display_name()
                    )
                })?;
            let shortlist = Shortlist::load(&shortlist).with_context(|| {
                format!(
                    "could not load compact translation shortlist: {}",
                    shortlist.display()
                )
            })?;
            loaded.push(LoadedDirection {
                source: direction.source,
                target: direction.target,
                engine: engine.with_shortlist(shortlist),
            });
        }
        Ok(Self { directions: loaded })
    }

    pub(crate) fn warm(&self) -> Result<()> {
        // A package can contain any of SayFlex's language pairs. A digit is a
        // valid language-neutral probe for every admitted model and avoids a
        // Russian/English warm-up accidentally rejecting another pair.
        for direction in &self.directions {
            let translated = direction.engine.translate_checked("1");
            if translated.text.trim().is_empty() {
                bail!("compact translation warm-up returned no text");
            }
            if !translated.is_complete() {
                bail!("compact translation warm-up reached its output token limit");
            }
        }
        Ok(())
    }

    pub(crate) fn translate_streaming(
        &self,
        text: &str,
        source: TranslationLanguage,
        target: TranslationLanguage,
        should_continue: impl Fn() -> bool,
        mut on_partial: impl FnMut(&str, u64, u32, u32) -> bool,
    ) -> Result<FirefoxTranslation> {
        let engine = self.engine(source, target)?;
        let started = Instant::now();
        let mut first_result_duration_ms = None;
        let translated = translate_paragraphs_with(
            text,
            source,
            target,
            should_continue,
            |_, _, paragraph| {
                let translated = engine.translate_long_checked(paragraph);
                Ok(FragmentTranslation {
                    text: translated.text,
                    completion: completion_from_terminations(&translated.terminations),
                })
            },
            |visible, index, count| {
                let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                first_result_duration_ms.get_or_insert(elapsed_ms);
                on_partial(visible, elapsed_ms, index, count)
            },
        )?;
        Ok(FirefoxTranslation {
            text: translated.text,
            first_result_duration_ms: first_result_duration_ms
                .unwrap_or_else(|| started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
            translated_paragraphs: translated.translated_paragraphs,
            completion: translated.completion,
        })
    }

    fn engine(&self, source: TranslationLanguage, target: TranslationLanguage) -> Result<&Engine> {
        self.directions
            .iter()
            .find(|direction| direction.source == source && direction.target == target)
            .map(|direction| &direction.engine)
            .ok_or_else(|| {
                anyhow!(
                    "compact package does not support {} to {}",
                    source.display_name(),
                    target.display_name()
                )
            })
    }
}

#[cfg(test)]
fn find_direction(
    directions: &[FirefoxDirectionDescriptor],
    source: TranslationLanguage,
    target: TranslationLanguage,
) -> Result<&FirefoxDirectionDescriptor> {
    directions
        .iter()
        .find(|direction| direction.source == source && direction.target == target)
        .ok_or_else(|| {
            anyhow!(
                "compact package does not support {} to {}",
                source.display_name(),
                target.display_name()
            )
        })
}

fn translate_paragraphs_with(
    text: &str,
    source: TranslationLanguage,
    target: TranslationLanguage,
    should_continue: impl Fn() -> bool,
    mut translate: impl FnMut(
        TranslationLanguage,
        TranslationLanguage,
        &str,
    ) -> Result<FragmentTranslation>,
    mut on_partial: impl FnMut(&str, u32, u32) -> bool,
) -> Result<ParagraphTranslation> {
    let units = paragraph_units(text);
    let paragraph_count = units
        .iter()
        .filter(|(body, _)| !body.trim().is_empty())
        .count();
    let paragraph_count = u32::try_from(paragraph_count).unwrap_or(u32::MAX);
    let mut output = String::with_capacity(text.len());
    let mut paragraph_index = 0_u32;
    for (body, separator) in units {
        if !should_continue() {
            return Err(TranslationCancelled.into());
        }
        if body.trim().is_empty() {
            output.push_str(body);
            output.push_str(separator);
            continue;
        }
        let leading_bytes = body.len() - body.trim_start().len();
        let trailing_bytes = body.len() - body.trim_end().len();
        let body_end = body.len().saturating_sub(trailing_bytes);
        let leading = &body[..leading_bytes];
        let content = &body[leading_bytes..body_end];
        let trailing = &body[body_end..];
        output.push_str(leading);
        if !content.is_empty() {
            let translated = translate(source, target, content)?;
            if translated.text.trim().is_empty() {
                bail!("compact translation runtime returned an empty result");
            }
            output.push_str(translated.text.trim());
            output.push_str(trailing);
            paragraph_index = paragraph_index.saturating_add(1);
            if translated.completion != FragmentCompletion::Completed {
                return Ok(ParagraphTranslation {
                    text: output,
                    translated_paragraphs: paragraph_index,
                    completion: translated.completion,
                });
            }
            if !on_partial(&output, paragraph_index, paragraph_count) {
                return Err(TranslationCancelled.into());
            }
        } else {
            output.push_str(trailing);
        }
        output.push_str(separator);
    }
    if output.trim().is_empty() {
        bail!("compact translation runtime returned an empty result");
    }
    Ok(ParagraphTranslation {
        text: output,
        translated_paragraphs: paragraph_index,
        completion: FragmentCompletion::Completed,
    })
}

fn completion_from_terminations(terminations: &[DecodeTermination]) -> FragmentCompletion {
    if terminations.contains(&DecodeTermination::LengthLimit) {
        FragmentCompletion::LimitReached
    } else {
        FragmentCompletion::Completed
    }
}

fn paragraph_units(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut units = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\n' {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < bytes.len() && bytes[index] == b'\n' {
            index += 1;
        }
        if index - run_start >= 2 {
            units.push((&text[start..run_start], &text[run_start..index]));
            start = index;
        }
    }
    units.push((&text[start..], ""));
    units
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use rust_protocol::TranslationLanguage;

    use super::{
        FragmentCompletion, FragmentTranslation, find_direction, translate_paragraphs_with,
    };
    use crate::{FirefoxDirectionDescriptor, TranslationCancelled};

    fn direction(
        source: TranslationLanguage,
        target: TranslationLanguage,
    ) -> FirefoxDirectionDescriptor {
        FirefoxDirectionDescriptor {
            source,
            target,
            model_file: "model.bin".to_owned(),
            source_vocab_file: "vocab.spm".to_owned(),
            target_vocab_file: "vocab.spm".to_owned(),
            shortlist_file: "lex.bin".to_owned(),
        }
    }

    #[test]
    fn compact_translation_preserves_blank_lines_and_emits_completed_paragraphs() {
        let mut partials = Vec::new();
        let output = translate_paragraphs_with(
            "One.\n\nTwo.",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            || true,
            |_, _, paragraph| match paragraph {
                "One." => Ok(FragmentTranslation::completed("Один.")),
                "Two." => Ok(FragmentTranslation::completed("Два.")),
                other => panic!("unexpected paragraph {other:?}"),
            },
            |text, index, count| {
                partials.push((text.to_owned(), index, count));
                true
            },
        )
        .unwrap();

        assert_eq!(output.text, "Один.\n\nДва.");
        assert_eq!(output.translated_paragraphs, 2);
        assert_eq!(output.completion, FragmentCompletion::Completed);
        assert_eq!(partials[0], ("Один.".to_owned(), 1, 2));
        assert_eq!(partials[1], ("Один.\n\nДва.".to_owned(), 2, 2));
    }

    #[test]
    fn compact_translation_preserves_ascii_whitespace_only_paragraphs() {
        let output = translate_paragraphs_with(
            "Hello\n\n \t\n\nWorld",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            || true,
            |_, _, paragraph| match paragraph {
                "Hello" => Ok(FragmentTranslation::completed("Привет")),
                "World" => Ok(FragmentTranslation::completed("Мир")),
                other => panic!("whitespace-only paragraph reached the engine: {other:?}"),
            },
            |_, _, _| true,
        )
        .unwrap();

        assert_eq!(output.text, "Привет\n\n \t\n\nМир");
        assert_eq!(output.translated_paragraphs, 2);
    }

    #[test]
    fn compact_translation_preserves_unicode_whitespace_only_paragraphs() {
        let output = translate_paragraphs_with(
            "Hello\n\n\u{2003}\u{00a0}\n\nWorld",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            || true,
            |_, _, paragraph| match paragraph {
                "Hello" => Ok(FragmentTranslation::completed("Привет")),
                "World" => Ok(FragmentTranslation::completed("Мир")),
                other => panic!("whitespace-only paragraph reached the engine: {other:?}"),
            },
            |_, _, _| true,
        )
        .unwrap();

        assert_eq!(output.text, "Привет\n\n\u{2003}\u{00a0}\n\nМир");
        assert_eq!(output.translated_paragraphs, 2);
    }

    #[test]
    fn compact_translation_stops_before_the_next_paragraph_when_cancelled() {
        let calls = Cell::new(0_u32);
        let result = translate_paragraphs_with(
            "One.\n\nTwo.",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            || calls.get() == 0,
            |_, _, _| {
                calls.set(calls.get() + 1);
                Ok(FragmentTranslation::completed("Один."))
            },
            |_, _, _| true,
        );

        assert!(
            result
                .unwrap_err()
                .downcast_ref::<TranslationCancelled>()
                .is_some()
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn length_limited_fragment_is_never_emitted_as_completed() {
        let mut partials = 0;
        let output = translate_paragraphs_with(
            "One.\n\nTwo.",
            TranslationLanguage::English,
            TranslationLanguage::Russian,
            || true,
            |_, _, _| {
                Ok(FragmentTranslation {
                    text: "Один".to_owned(),
                    completion: FragmentCompletion::LimitReached,
                })
            },
            |_, _, _| {
                partials += 1;
                true
            },
        )
        .unwrap();

        assert_eq!(output.completion, FragmentCompletion::LimitReached);
        assert_eq!(partials, 0);
        assert_eq!(output.translated_paragraphs, 1);
    }

    #[test]
    fn compact_runtime_rejects_a_direction_not_declared_by_the_pack() {
        let directions = vec![
            direction(TranslationLanguage::English, TranslationLanguage::Russian),
            direction(TranslationLanguage::Russian, TranslationLanguage::English),
        ];

        assert!(
            find_direction(
                &directions,
                TranslationLanguage::German,
                TranslationLanguage::Russian,
            )
            .is_err()
        );
    }
}
