use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use candle::{DType, Device, Tensor};
use candle_nn::{VarBuilder, ops::log_softmax};
use candle_transformers::models::marian;
use rust_protocol::{TranslationLanguage, TranslationTier};
use sentencepiece_rs::SentencePieceProcessor;

use crate::{TranslationDirectionDescriptor, TranslationPackDescriptor};

const MAX_INPUT_CHARACTERS: usize = 32_000;
const CHUNK_CHARACTERS: usize = 1_200;

pub struct TranslationEngine {
    pack: TranslationPackDescriptor,
    root: PathBuf,
    runtimes: HashMap<(TranslationLanguage, TranslationLanguage), MarianRuntime>,
}

impl TranslationEngine {
    pub fn new(pack: TranslationPackDescriptor, root: PathBuf) -> Self {
        Self {
            pack,
            root,
            runtimes: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn tier(&self) -> TranslationTier {
        self.pack.tier
    }

    pub fn warm(&mut self) -> Result<()> {
        for (source, target) in [
            (TranslationLanguage::Russian, TranslationLanguage::English),
            (TranslationLanguage::English, TranslationLanguage::Russian),
        ] {
            self.ensure_runtime(source, target)?;
        }
        Ok(())
    }

    pub fn translate(
        &mut self,
        text: &str,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
    ) -> Result<(String, TranslationLanguage, TranslationLanguage)> {
        if text.trim().is_empty() {
            bail!("text is empty");
        }
        if text.chars().count() > MAX_INPUT_CHARACTERS {
            bail!("text exceeds the {MAX_INPUT_CHARACTERS}-character local translation limit");
        }
        let source = source_language.unwrap_or_else(|| detect_language(text));
        let target = target_language.unwrap_or_else(|| source.opposite());
        if source == target {
            bail!("source and target languages are the same");
        }
        let runtime = self.ensure_runtime(source, target)?;
        let translated = translate_preserving_lines(runtime, text)?;
        let translated = normalize_single_word_repetition(text, &translated);
        Ok((translated, source, target))
    }

    fn ensure_runtime(
        &mut self,
        source: TranslationLanguage,
        target: TranslationLanguage,
    ) -> Result<&mut MarianRuntime> {
        let key = (source, target);
        if !self.runtimes.contains_key(&key) {
            let descriptor = self
                .pack
                .direction(source, target)
                .cloned()
                .ok_or_else(|| anyhow!("installed pack does not support this direction"))?;
            let runtime = MarianRuntime::load(&self.root.join(&descriptor.directory), &descriptor)
                .with_context(|| {
                    format!(
                        "could not load {} → {} translation model",
                        source.code(),
                        target.code()
                    )
                })?;
            self.runtimes.insert(key, runtime);
        }
        self.runtimes
            .get_mut(&key)
            .ok_or_else(|| anyhow!("translation runtime disappeared"))
    }
}

#[must_use]
pub fn detect_language(text: &str) -> TranslationLanguage {
    let mut cyrillic = 0_usize;
    let mut latin = 0_usize;
    for character in text.chars() {
        if ('\u{0400}'..='\u{052f}').contains(&character) {
            cyrillic += 1;
        } else if character.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cyrillic > 0 && cyrillic.saturating_mul(2) >= latin {
        TranslationLanguage::Russian
    } else {
        TranslationLanguage::English
    }
}

struct MarianRuntime {
    model: marian::MTModel,
    config: marian::Config,
    source_processor: SentencePieceProcessor,
    target_processor: SentencePieceProcessor,
    vocabulary: HashMap<String, u32>,
    inverse_vocabulary: Vec<String>,
    device: Device,
    beam_size: usize,
}

impl MarianRuntime {
    fn load(root: &Path, descriptor: &TranslationDirectionDescriptor) -> Result<Self> {
        let config: marian::Config = serde_json::from_slice(&fs::read(root.join("config.json"))?)
            .context("invalid Marian config")?;
        let source_processor = SentencePieceProcessor::open(root.join("source.spm"))
            .context("invalid source SentencePiece model")?;
        let target_processor = SentencePieceProcessor::open(root.join("target.spm"))
            .context("invalid target SentencePiece model")?;
        let vocabulary: HashMap<String, u32> =
            serde_json::from_slice(&fs::read(root.join("vocab.json"))?)
                .context("invalid Marian vocabulary")?;
        let max_id = vocabulary
            .values()
            .copied()
            .max()
            .ok_or_else(|| anyhow!("Marian vocabulary is empty"))? as usize;
        let mut inverse_vocabulary = vec![String::new(); max_id + 1];
        for (piece, id) in &vocabulary {
            if let Some(slot) = inverse_vocabulary.get_mut(*id as usize) {
                slot.clone_from(piece);
            }
        }

        let device = Device::Cpu;
        let weights = root.join(&descriptor.weight_file);
        let builder = if descriptor.weight_file.ends_with(".safetensors") {
            unsafe { VarBuilder::from_mmaped_safetensors(&[weights], DType::F32, &device)? }
        } else {
            VarBuilder::from_pth(weights, DType::F32, &device)?
        };
        let model = marian::MTModel::new(&config, builder)?;
        let beam_size = if config.encoder_layers <= 3 { 3 } else { 4 };
        Ok(Self {
            model,
            config,
            source_processor,
            target_processor,
            vocabulary,
            inverse_vocabulary,
            device,
            beam_size,
        })
    }

    fn translate_unit(&mut self, text: &str) -> Result<String> {
        let pieces = self.source_processor.encode(text)?;
        let unknown = self
            .vocabulary
            .get("<unk>")
            .copied()
            .unwrap_or(self.source_processor.unk_id() as u32);
        let mut source_ids = pieces
            .iter()
            .map(|piece| self.vocabulary.get(piece).copied().unwrap_or(unknown))
            .collect::<Vec<_>>();
        source_ids.push(self.config.eos_token_id);
        if source_ids.len() >= self.config.max_position_embeddings {
            bail!("translation unit is too long for this model");
        }

        self.model.reset_kv_cache();
        let source = Tensor::new(source_ids.as_slice(), &self.device)?.unsqueeze(0)?;
        let encoder = self.model.encoder().forward(&source, 0)?;
        let max_output = source_ids
            .len()
            .saturating_mul(3)
            .saturating_add(24)
            .min(self.config.max_position_embeddings.saturating_sub(1))
            .max(24);
        let output_ids = beam_search(
            &self.model,
            &encoder,
            &self.config,
            self.beam_size,
            max_output,
            &self.device,
        )?;
        let pieces = output_ids
            .into_iter()
            .filter(|id| {
                *id != self.config.decoder_start_token_id
                    && *id != self.config.eos_token_id
                    && *id != self.config.forced_eos_token_id
                    && *id != self.config.pad_token_id
            })
            .filter_map(|id| self.inverse_vocabulary.get(id as usize))
            .filter(|piece| !piece.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        self.target_processor
            .decode_pieces(&pieces)
            .map_err(Into::into)
    }
}

#[derive(Clone)]
struct Beam {
    tokens: Vec<u32>,
    score: f64,
    model: marian::MTModel,
    finished: bool,
}

fn beam_search(
    model: &marian::MTModel,
    encoder: &Tensor,
    config: &marian::Config,
    beam_size: usize,
    max_output: usize,
    device: &Device,
) -> Result<Vec<u32>> {
    let mut seed = model.clone();
    seed.reset_kv_cache();
    let mut beams = vec![Beam {
        tokens: vec![config.decoder_start_token_id],
        score: 0.0,
        model: seed,
        finished: false,
    }];

    for _ in 0..max_output {
        let mut candidates = Vec::with_capacity(beam_size * beam_size);
        for beam in beams {
            if beam.finished {
                candidates.push(beam);
                continue;
            }
            let mut decoded = beam.model;
            let past = beam.tokens.len().saturating_sub(1);
            let input = Tensor::new(&beam.tokens[past..], device)?.unsqueeze(0)?;
            let logits = decoded.decode(&input, encoder, past)?;
            let logits = logits.squeeze(0)?;
            let logits = logits.get(logits.dim(0)? - 1)?;
            let log_probabilities = log_softmax(&logits, 0)?.to_vec1::<f32>()?;
            for (token, token_score) in top_k(&log_probabilities, beam_size) {
                if token as u32 == config.pad_token_id
                    || token as u32 == config.decoder_start_token_id
                {
                    continue;
                }
                let mut tokens = beam.tokens.clone();
                tokens.push(token as u32);
                let finished = token as u32 == config.eos_token_id
                    || token as u32 == config.forced_eos_token_id;
                candidates.push(Beam {
                    tokens,
                    score: beam.score + f64::from(token_score),
                    model: decoded.clone(),
                    finished,
                });
            }
        }

        candidates.sort_by(|left, right| {
            normalized_score(right)
                .partial_cmp(&normalized_score(left))
                .unwrap_or(Ordering::Equal)
        });
        candidates.truncate(beam_size);
        if candidates.iter().all(|beam| beam.finished) {
            beams = candidates;
            break;
        }
        beams = candidates;
    }
    beams
        .into_iter()
        .max_by(|left, right| {
            normalized_score(left)
                .partial_cmp(&normalized_score(right))
                .unwrap_or(Ordering::Equal)
        })
        .map(|beam| beam.tokens)
        .ok_or_else(|| anyhow!("translation produced no hypotheses"))
}

fn normalized_score(beam: &Beam) -> f64 {
    let generated = beam.tokens.len().saturating_sub(1).max(1) as f64;
    beam.score / generated.powf(0.7)
}

fn top_k(values: &[f32], count: usize) -> Vec<(usize, f32)> {
    let mut indexed = values.iter().copied().enumerate().collect::<Vec<_>>();
    let retained = count.min(indexed.len());
    if retained == 0 {
        return Vec::new();
    }
    indexed.select_nth_unstable_by(retained - 1, |left, right| {
        right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal)
    });
    indexed.truncate(retained);
    indexed.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    indexed
}

fn translate_preserving_lines(runtime: &mut MarianRuntime, text: &str) -> Result<String> {
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (content, newline) = line.strip_suffix('\n').map_or((line, ""), |content| {
            (content.strip_suffix('\r').unwrap_or(content), "\n")
        });
        if content.trim().is_empty() {
            output.push_str(content);
            output.push_str(newline);
            continue;
        }
        let leading = content.len() - content.trim_start().len();
        let trailing = content.len() - content.trim_end().len();
        output.push_str(&content[..leading]);
        let body_end = content.len().saturating_sub(trailing);
        let body = &content[leading..body_end];
        let chunks = split_chunks(body, CHUNK_CHARACTERS);
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(runtime.translate_unit(chunk.trim())?.trim());
        }
        if trailing > 0 {
            output.push_str(&content[body_end..]);
        }
        output.push_str(newline);
    }
    Ok(output)
}

fn split_chunks(text: &str, max_characters: usize) -> Vec<String> {
    if text.chars().count() <= max_characters {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let additional = word.chars().count() + usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + additional > max_characters {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn normalize_single_word_repetition(source: &str, translated: &str) -> String {
    let source_words = lexical_spans(source);
    if source_words.len() != 1 {
        return translated.to_owned();
    }
    let translated_words = lexical_spans(translated);
    if translated_words.len() != 2 {
        return translated.to_owned();
    }
    let first = &translated[translated_words[0].0..translated_words[0].1];
    let second = &translated[translated_words[1].0..translated_words[1].1];
    if first.to_lowercase() != second.to_lowercase() {
        return translated.to_owned();
    }

    let mut normalized = String::with_capacity(translated.len());
    normalized.push_str(&translated[..translated_words[0].0]);
    normalized.push_str(first);
    normalized.push_str(&translated[translated_words[1].1..]);
    normalized
}

fn lexical_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(word_start) = start.take() {
            spans.push((word_start, index));
        }
    }
    if let Some(word_start) = start {
        spans.push((word_start, text.len()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use rust_protocol::TranslationLanguage;

    use super::{detect_language, normalize_single_word_repetition, split_chunks};

    #[test]
    fn language_detection_is_deterministic_for_the_supported_pair() {
        assert_eq!(
            detect_language("Это русский текст"),
            TranslationLanguage::Russian
        );
        assert_eq!(
            detect_language("This is English text"),
            TranslationLanguage::English
        );
        assert_eq!(
            detect_language("Версия Rust 1.88"),
            TranslationLanguage::Russian
        );
    }

    #[test]
    fn long_text_is_split_without_dropping_words() {
        let input = "one two three four five";
        let chunks = split_chunks(input, 8);
        assert_eq!(chunks.join(" "), input);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 8));
    }

    #[test]
    fn repeated_single_word_translation_is_collapsed_without_losing_terminal_punctuation() {
        assert_eq!(
            normalize_single_word_repetition("hello", "Привет, привет."),
            "Привет."
        );
        assert_eq!(
            normalize_single_word_repetition("TEXT", "TEXT TEXT"),
            "TEXT"
        );
        assert_eq!(
            normalize_single_word_repetition("текст", "text-text"),
            "text"
        );
    }

    #[test]
    fn repetition_normalization_never_changes_multiword_or_distinct_results() {
        assert_eq!(
            normalize_single_word_repetition("hello world", "Привет, привет."),
            "Привет, привет."
        );
        assert_eq!(
            normalize_single_word_repetition("hello", "Добрый день"),
            "Добрый день"
        );
        assert_eq!(
            normalize_single_word_repetition("hello", "Привет, привет, привет."),
            "Привет, привет, привет."
        );
    }
}
