use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use image::{DynamicImage, RgbaImage};
use oar_ocr::oarocr::{EdgeProcessor, TextCroppingProcessor};
use oar_ocr_core::{
    core::traits::{
        adapter::{AdapterBuilder, ModelAdapter},
        task::ImageTaskInput,
    },
    domain::{
        adapters::{
            TextDetectionAdapter, TextDetectionAdapterBuilder, TextRecognitionAdapter,
            TextRecognitionAdapterBuilder,
        },
        tasks::{TextDetectionConfig, TextRecognitionOutput},
    },
    processors::{BoundingBox, LimitType, sort_quad_boxes},
};
use rust_protocol::{OcrLanguagePolicy, OcrLine, PhysicalRect};

use crate::{CancellationToken, InferenceAdapter, OcrDocument, OcrError};

#[derive(Debug, Clone)]
pub struct PpOcrModelPaths {
    root: PathBuf,
}

impl PpOcrModelPaths {
    #[must_use]
    pub fn from_bundle_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn detector(&self) -> PathBuf {
        self.root
            .join("detector")
            .join("ch_PP-OCRv5_det_mobile.onnx")
    }

    fn recognizer(&self, family: ModelFamily) -> PathBuf {
        self.root.join("recognizers").join(family.recognizer_name())
    }

    fn dictionary(&self, family: ModelFamily) -> PathBuf {
        self.root
            .join("dictionaries")
            .join(family.dictionary_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModelFamily {
    English,
    Latin,
    EasternSlavic,
    ChineseGeneral,
    Korean,
    Devanagari,
    Arabic,
}

impl ModelFamily {
    const ALL: [Self; 7] = [
        Self::English,
        Self::Latin,
        Self::EasternSlavic,
        Self::ChineseGeneral,
        Self::Korean,
        Self::Devanagari,
        Self::Arabic,
    ];

    fn for_language(language: &str) -> Option<Self> {
        match language {
            "en" => Some(Self::English),
            "es" | "pt" | "pt-BR" | "de" | "fr" | "it" => Some(Self::Latin),
            "ru" | "uk" => Some(Self::EasternSlavic),
            "zh" | "zh-Hans" | "zh-Hant" | "ja" => Some(Self::ChineseGeneral),
            "ko" => Some(Self::Korean),
            "hi" => Some(Self::Devanagari),
            "ar" => Some(Self::Arabic),
            _ => None,
        }
    }

    fn recognizer_name(self) -> &'static str {
        match self {
            Self::English => "en_PP-OCRv5_rec_mobile.onnx",
            Self::Latin => "latin_PP-OCRv5_rec_mobile.onnx",
            Self::EasternSlavic => "eslav_PP-OCRv5_rec_mobile.onnx",
            Self::ChineseGeneral => "ch_PP-OCRv5_rec_mobile.onnx",
            Self::Korean => "korean_PP-OCRv5_rec_mobile.onnx",
            Self::Devanagari => "devanagari_PP-OCRv5_rec_mobile.onnx",
            Self::Arabic => "arabic_PP-OCRv5_rec_mobile.onnx",
        }
    }

    fn dictionary_name(self) -> &'static str {
        match self {
            Self::English => "ppocrv5_en_dict.txt",
            Self::Latin => "ppocrv5_latin_dict.txt",
            Self::EasternSlavic => "ppocrv5_eslav_dict.txt",
            Self::ChineseGeneral => "ppocrv5_dict.txt",
            Self::Korean => "ppocrv5_korean_dict.txt",
            Self::Devanagari => "ppocrv5_devanagari_dict.txt",
            Self::Arabic => "ppocrv5_arabic_dict.txt",
        }
    }
}

pub struct PpOcrAdapter {
    detector: TextDetectionAdapter,
    recognizers: Mutex<HashMap<ModelFamily, TextRecognitionAdapter>>,
    paths: PpOcrModelPaths,
}

impl std::fmt::Debug for PpOcrAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PpOcrAdapter")
            .field(
                "loaded_recognizer_count",
                &self
                    .recognizers
                    .lock()
                    .map(|recognizers| recognizers.len())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

#[derive(Debug)]
struct CroppedLine {
    bounds: BoundingBox,
    image: Arc<image::RgbImage>,
}

#[derive(Debug)]
struct FamilyRecognition {
    lines: Vec<OcrLine>,
    average_confidence: f32,
}

/// Longest image side the text detector works at. Larger images are scaled
/// down to it before detection. At 960 a 2152x1037 web page lost its small
/// letter-spaced captions ("LATEST NEWS", "VIEW ALL", "DOWNLOAD ON STEAM");
/// 2560 finds them for about 0.25 s more on the build machine (two languages).
pub const DETECTION_SIDE_LIMIT: u32 = 2560;

impl PpOcrAdapter {
    pub fn load(paths: &PpOcrModelPaths) -> Result<Self, OcrError> {
        Self::load_with_detector(paths, detector_config(DETECTION_SIDE_LIMIT))
    }

    fn load_with_detector(
        paths: &PpOcrModelPaths,
        detector_config: TextDetectionConfig,
    ) -> Result<Self, OcrError> {
        let detector = paths.detector();
        if !detector.is_file() {
            return Err(OcrError::ModelUnavailable);
        }
        for family in ModelFamily::ALL {
            let recognizer = paths.recognizer(family);
            let dictionary = paths.dictionary(family);
            if !recognizer.is_file() || !dictionary.is_file() {
                return Err(OcrError::ModelUnavailable);
            }
        }
        let detector = TextDetectionAdapterBuilder::new()
            .with_config(detector_config)
            .build(detector)
            .map_err(|_| OcrError::Engine)?;
        Ok(Self {
            detector,
            recognizers: Mutex::new(HashMap::new()),
            paths: paths.clone(),
        })
    }

    fn detect_lines(
        &self,
        image: &image::RgbImage,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CroppedLine>, OcrError> {
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let image = Arc::new(image.clone());
        let detection = self
            .detector
            .execute(
                ImageTaskInput::from_arc_images(vec![Arc::clone(&image)]),
                None,
            )
            .map_err(|_| OcrError::Engine)?;
        let boxes = sort_quad_boxes(
            &detection
                .detections
                .into_iter()
                .next()
                .unwrap_or_default()
                .into_iter()
                .map(|detection| detection.bbox)
                .collect::<Vec<_>>(),
        );
        let crops = TextCroppingProcessor::new(true)
            .process((image, boxes.clone()))
            .map_err(|_| OcrError::Engine)?;
        Ok(boxes
            .into_iter()
            .zip(crops)
            .filter_map(|(bounds, image)| image.map(|image| CroppedLine { bounds, image }))
            .collect())
    }

    fn recognize_family(
        &self,
        family: ModelFamily,
        crops: &[CroppedLine],
        image_width: u32,
        image_height: u32,
        cancellation: &CancellationToken,
    ) -> Result<FamilyRecognition, OcrError> {
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        if crops.is_empty() {
            return Ok(FamilyRecognition {
                lines: Vec::new(),
                average_confidence: -1.0,
            });
        }
        let mut recognizers = self.recognizers.lock().map_err(|_| OcrError::Engine)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = recognizers.entry(family) {
            let dictionary = std::fs::read_to_string(self.paths.dictionary(family))
                .map_err(|_| OcrError::ModelUnavailable)?;
            let recognizer = TextRecognitionAdapterBuilder::new()
                .character_dict(dictionary.lines().map(str::to_owned).collect())
                .build(self.paths.recognizer(family))
                .map_err(|_| OcrError::Engine)?;
            entry.insert(recognizer);
        }
        let recognition: TextRecognitionOutput = recognizers
            .get(&family)
            .ok_or(OcrError::Engine)?
            .execute(
                ImageTaskInput::from_arc_images(
                    crops.iter().map(|crop| Arc::clone(&crop.image)).collect(),
                ),
                None,
            )
            .map_err(|_| OcrError::Engine)?;
        drop(recognizers);
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let lines = crops
            .iter()
            .zip(recognition.texts.iter())
            .zip(recognition.scores.iter())
            .filter_map(|((crop, text), confidence)| {
                let text = postprocess_text(family, text);
                (!text.is_empty()).then(|| OcrLine {
                    text,
                    bounds: bounds_for(&crop.bounds, image_width, image_height),
                    confidence_milli: confidence_milli(Some(*confidence)),
                })
            })
            .collect::<Vec<_>>();
        let average_confidence = if lines.is_empty() {
            -1.0
        } else {
            lines
                .iter()
                .map(|line| f32::from(line.confidence_milli) / 1000.0)
                .sum::<f32>()
                / lines.len() as f32
        };
        Ok(FamilyRecognition {
            lines,
            average_confidence,
        })
    }
}

fn detector_config(side_limit: u32) -> TextDetectionConfig {
    TextDetectionConfig {
        unclip_ratio: 2.0,
        limit_side_len: Some(side_limit),
        limit_type: Some(LimitType::Max),
        max_side_len: Some(4000),
        ..TextDetectionConfig::default()
    }
}

fn better_family(
    best: Option<FamilyRecognition>,
    candidate: FamilyRecognition,
) -> Option<FamilyRecognition> {
    if best
        .as_ref()
        .is_none_or(|best| candidate.average_confidence > best.average_confidence)
    {
        Some(candidate)
    } else {
        best
    }
}

impl InferenceAdapter for PpOcrAdapter {
    fn recognize(
        &self,
        image: &RgbaImage,
        language_policy: &OcrLanguagePolicy,
        cancellation: &CancellationToken,
    ) -> Result<OcrDocument, OcrError> {
        let rgb = DynamicImage::ImageRgba8(image.clone()).to_rgb8();
        let crops = self.detect_lines(&rgb, cancellation)?;
        let (language, result) = match language_policy {
            OcrLanguagePolicy::Prefer(languages) => {
                let mut best: Option<(String, FamilyRecognition)> = None;
                for (language, family) in preferred_model_families(languages) {
                    let candidate = self.recognize_family(
                        family,
                        &crops,
                        image.width(),
                        image.height(),
                        cancellation,
                    )?;
                    if best.as_ref().is_none_or(|(_, best)| {
                        candidate.average_confidence > best.average_confidence
                    }) {
                        best = Some((language, candidate));
                    }
                }
                let (language, result) = best.ok_or(OcrError::LanguageUnavailable)?;
                (Some(language), result)
            }
            OcrLanguagePolicy::Auto => {
                let mut best: Option<FamilyRecognition> = None;
                for family in ModelFamily::ALL {
                    let candidate = self.recognize_family(
                        family,
                        &crops,
                        image.width(),
                        image.height(),
                        cancellation,
                    )?;
                    best = better_family(best, candidate);
                }
                (None, best.ok_or(OcrError::Engine)?)
            }
        };
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        Ok(OcrDocument {
            lines: result.lines,
            language,
        })
    }
}

fn preferred_model_families(languages: &[String]) -> Vec<(String, ModelFamily)> {
    let mut families = Vec::with_capacity(languages.len());
    for language in languages {
        let Some(family) = ModelFamily::for_language(language) else {
            continue;
        };
        if families
            .iter()
            .any(|(_, existing_family)| *existing_family == family)
        {
            continue;
        }
        families.push((language.clone(), family));
    }
    families
}

fn bounds_for(bounds: &BoundingBox, image_width: u32, image_height: u32) -> PhysicalRect {
    let points = &bounds.points;
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0.0, image_width as f32) as u32;
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .clamp(0.0, image_height as f32) as u32;
    PhysicalRect {
        x: i32::try_from(min_x).unwrap_or(i32::MAX),
        y: i32::try_from(min_y).unwrap_or(i32::MAX),
        width: max_x.saturating_sub(min_x),
        height: max_y.saturating_sub(min_y),
    }
}

fn confidence_milli(confidence: Option<f32>) -> u16 {
    (confidence.unwrap_or_default().clamp(0.0, 1.0) * 1000.0).round() as u16
}

fn postprocess_text(family: ModelFamily, text: &str) -> String {
    if family != ModelFamily::Arabic {
        return text.to_owned();
    }
    let bidi = unicode_bidi::BidiInfo::new(text, None);
    bidi.paragraphs
        .first()
        .map(|paragraph| {
            bidi.reorder_line(paragraph, paragraph.range.clone())
                .into_owned()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{ModelFamily, postprocess_text, preferred_model_families};

    #[test]
    fn unsupported_language_has_no_model_family() {
        assert_eq!(ModelFamily::for_language("xx"), None);
    }

    #[test]
    fn arabic_visual_order_is_restored_to_logical_order() {
        assert_eq!(
            postprocess_text(ModelFamily::Arabic, "ةيمويلا كماهم"),
            "مهامك اليومية"
        );
    }

    #[test]
    fn preferred_languages_keep_both_directions_without_duplicate_models() {
        assert_eq!(
            preferred_model_families(&["en".into(), "ru".into(), "uk".into()]),
            vec![
                ("en".into(), ModelFamily::English),
                ("ru".into(), ModelFamily::EasternSlavic),
            ]
        );
    }
}

#[cfg(test)]
mod detection_limit_probe {
    use std::{sync::Arc, time::Instant};

    use rust_protocol::OcrLanguagePolicy;

    use super::{PpOcrAdapter, PpOcrModelPaths, detector_config};
    use crate::{CancellationToken, OcrEngine};

    fn list(name: &str, default: &str) -> Vec<String> {
        std::env::var(name)
            .unwrap_or_else(|_| default.to_owned())
            .split(',')
            .map(str::to_owned)
            .collect()
    }

    /// Opt-in: runs the real models on one image with several detector
    /// settings. SAYFLEX_OCR_PROBE_BUNDLE = installed OCR bundle,
    /// SAYFLEX_OCR_PROBE_IMAGE = PNG; optional SAYFLEX_OCR_PROBE_LIMITS,
    /// SAYFLEX_OCR_PROBE_BOX (box thresholds), SAYFLEX_OCR_PROBE_SCORE and
    /// SAYFLEX_OCR_PROBE_LANGS (comma lists; empty LANGS = automatic).
    #[test]
    #[ignore = "opt-in: needs the OCR model bundle and a probe image"]
    fn compare_detection_limits_on_a_probe_image() {
        let bundle = std::env::var_os("SAYFLEX_OCR_PROBE_BUNDLE").expect("bundle");
        let image = std::env::var_os("SAYFLEX_OCR_PROBE_IMAGE").expect("image");
        let image = image::open(image).expect("probe image").to_rgba8();
        let paths = PpOcrModelPaths::from_bundle_root(bundle.into());
        let languages = list("SAYFLEX_OCR_PROBE_LANGS", "")
            .into_iter()
            .filter(|language| !language.is_empty())
            .collect::<Vec<_>>();
        let policy = if languages.is_empty() {
            OcrLanguagePolicy::Auto
        } else {
            OcrLanguagePolicy::Prefer(languages)
        };
        for limit in list("SAYFLEX_OCR_PROBE_LIMITS", "960,1600,2560") {
            for box_threshold in list("SAYFLEX_OCR_PROBE_BOX", "0.6") {
                for score_threshold in list("SAYFLEX_OCR_PROBE_SCORE", "0.3") {
                    let mut config = detector_config(limit.parse().unwrap());
                    config.box_threshold = box_threshold.parse().unwrap();
                    config.score_threshold = score_threshold.parse().unwrap();
                    let adapter = PpOcrAdapter::load_with_detector(&paths, config).unwrap();
                    let engine = OcrEngine::new(Arc::new(adapter));
                    // Warm-up, then timed run.
                    let _ = engine.recognize(&image, &policy, &CancellationToken::default());
                    let started = Instant::now();
                    let document = engine
                        .recognize(&image, &policy, &CancellationToken::default())
                        .unwrap();
                    let elapsed = started.elapsed();
                    let mut texts = document
                        .lines
                        .iter()
                        .map(|line| {
                            format!(
                                "{}@{},{}w{}h{} c{}",
                                line.text,
                                line.bounds.x,
                                line.bounds.y,
                                line.bounds.width,
                                line.bounds.height,
                                line.confidence_milli
                            )
                        })
                        .collect::<Vec<_>>();
                    texts.sort();
                    eprintln!(
                        "limit {limit} box {box_threshold} score {score_threshold}: {} lines in {elapsed:?}",
                        document.lines.len()
                    );
                    for text in texts {
                        eprintln!("    {text}");
                    }
                    // Visual rows exactly as the desktop builds its screen
                    // blocks, as tab-separated text for fixture scripts.
                    let reconstructed = crate::reconstruct_text(&document.lines, 350).unwrap();
                    for row in crate::reconstruct_visual_rows(&reconstructed.elements) {
                        let rect = |r: &rust_protocol::PhysicalRect| {
                            format!("{},{},{},{}", r.x, r.y, r.width, r.height)
                        };
                        eprintln!(
                            "ROW	{}	{}	{}	{}",
                            row.text,
                            rect(&row.bounds),
                            row.fragments.iter().map(rect).collect::<Vec<_>>().join(";"),
                            row.confidence_milli
                        );
                    }
                }
            }
        }
    }
}
