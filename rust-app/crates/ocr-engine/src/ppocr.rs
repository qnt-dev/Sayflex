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
use rust_protocol::{OcrLanguagePolicy, OcrLine, PhysicalPoint, PhysicalQuad, PhysicalRect};

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

/// One family's reading of every detected line, in detection order. `None`
/// where the family read nothing.
#[derive(Debug)]
struct FamilyRecognition {
    lines: Vec<Option<OcrLine>>,
    average_confidence: f32,
}

/// A tall line read below this confidence is read again upside down.
const ROTATION_RETRY_CONFIDENCE_MILLI: u16 = 600;

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
        let readings = self.read_images(
            family,
            crops.iter().map(|crop| Arc::clone(&crop.image)).collect(),
            cancellation,
        )?;
        let lines = crops
            .iter()
            .zip(readings)
            .map(|(crop, (text, confidence))| {
                (!text.is_empty()).then(|| OcrLine {
                    text,
                    bounds: bounds_for(&crop.bounds, image_width, image_height),
                    quad: quad_for(&crop.bounds, image_width, image_height),
                    confidence_milli: confidence_milli(Some(confidence)),
                })
            })
            .collect::<Vec<_>>();
        let read = lines.iter().flatten().collect::<Vec<_>>();
        let average_confidence = if read.is_empty() {
            -1.0
        } else {
            read.iter()
                .map(|line| f32::from(line.confidence_milli) / 1000.0)
                .sum::<f32>()
                / read.len() as f32
        };
        Ok(FamilyRecognition {
            lines,
            average_confidence,
        })
    }

    /// Text and confidence for each image, read with one family's model.
    fn read_images(
        &self,
        family: ModelFamily,
        images: Vec<Arc<image::RgbImage>>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<(String, f32)>, OcrError> {
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
        let count = images.len();
        let recognition: TextRecognitionOutput = recognizers
            .get(&family)
            .ok_or(OcrError::Engine)?
            .execute(ImageTaskInput::from_arc_images(images), None)
            .map_err(|_| OcrError::Engine)?;
        drop(recognizers);
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let mut readings = recognition
            .texts
            .iter()
            .zip(recognition.scores.iter())
            .map(|(text, confidence)| {
                (
                    crate::normalize_homoglyphs(&postprocess_text(family, text)),
                    *confidence,
                )
            })
            .collect::<Vec<_>>();
        readings.resize(count, (String::new(), 0.0));
        Ok(readings)
    }

    /// A tall crop is turned 90 degrees before recognition, which reads text
    /// written top-to-bottom upside down; text rotated the other way stayed
    /// unread (CER 0.63 in the audit). Read weak tall lines again at 180
    /// degrees and keep the better reading.
    fn retry_rotated_lines(
        &self,
        crops: &[CroppedLine],
        lines: &mut [Option<(ModelFamily, OcrLine)>],
        cancellation: &CancellationToken,
    ) -> Result<(), OcrError> {
        for family in ModelFamily::ALL {
            let candidates = crops
                .iter()
                .zip(lines.iter())
                .enumerate()
                .filter(|(_, (crop, line))| {
                    let tall = crop.image.width() > 0 && is_tall(&crop.bounds);
                    tall && line.as_ref().is_some_and(|(line_family, line)| {
                        *line_family == family
                            && line.confidence_milli < ROTATION_RETRY_CONFIDENCE_MILLI
                    })
                })
                .map(|(index, (crop, _))| {
                    (
                        index,
                        Arc::new(image::imageops::rotate180(crop.image.as_ref())),
                    )
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let readings = self.read_images(
                family,
                candidates
                    .iter()
                    .map(|(_, image)| Arc::clone(image))
                    .collect(),
                cancellation,
            )?;
            for ((index, _), (text, confidence)) in candidates.into_iter().zip(readings) {
                let confidence = confidence_milli(Some(confidence));
                if let Some((_, line)) = lines[index].as_mut()
                    && !text.is_empty()
                    && confidence > line.confidence_milli
                {
                    line.text = text;
                    line.confidence_milli = confidence;
                    // The reading axes turned with the image.
                    if let Some(quad) = line.quad.as_mut() {
                        quad.points.rotate_left(2);
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_tall(bounds: &BoundingBox) -> bool {
    let xs = bounds.points.iter().map(|point| point.x);
    let ys = bounds.points.iter().map(|point| point.y);
    let width = xs.clone().fold(f32::NEG_INFINITY, f32::max) - xs.fold(f32::INFINITY, f32::min);
    let height = ys.clone().fold(f32::NEG_INFINITY, f32::max) - ys.fold(f32::INFINITY, f32::min);
    height >= width * 1.5
}

/// For each line the reading with the highest confidence among the families,
/// and the family that won most lines. One family per region read the Latin
/// words of a Russian screenshot with the Cyrillic model ("Status: rotoBo").
fn best_reading_per_line(
    readings: Vec<(ModelFamily, FamilyRecognition)>,
    line_count: usize,
) -> (Vec<Option<(ModelFamily, OcrLine)>>, Option<ModelFamily>) {
    let mut best: Vec<Option<(ModelFamily, OcrLine)>> = vec![None; line_count];
    let order = readings
        .iter()
        .map(|(family, _)| *family)
        .collect::<Vec<_>>();
    for (family, recognition) in readings {
        for (slot, line) in best.iter_mut().zip(recognition.lines) {
            let Some(line) = line else {
                continue;
            };
            if slot
                .as_ref()
                .is_none_or(|(_, current)| line.confidence_milli > current.confidence_milli)
            {
                *slot = Some((family, line));
            }
        }
    }
    let winner = order
        .iter()
        .copied()
        .max_by_key(|family| {
            let wins = best
                .iter()
                .flatten()
                .filter(|(winner, _)| winner == family)
                .count();
            // On a tie the earlier, more preferred family wins.
            (
                wins,
                std::cmp::Reverse(order.iter().position(|f| f == family)),
            )
        })
        .filter(|family| best.iter().flatten().any(|(winner, _)| winner == family));
    (best, winner)
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
    best: Option<(ModelFamily, FamilyRecognition)>,
    candidate: (ModelFamily, FamilyRecognition),
) -> Option<(ModelFamily, FamilyRecognition)> {
    if best
        .as_ref()
        .is_none_or(|(_, best)| candidate.1.average_confidence > best.average_confidence)
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
        let (language, mut lines) = match language_policy {
            OcrLanguagePolicy::Prefer(languages) => {
                // The user's languages: each line takes the family that reads
                // it best.
                let families = preferred_model_families(languages);
                if families.is_empty() {
                    return Err(OcrError::LanguageUnavailable);
                }
                let mut readings = Vec::with_capacity(families.len());
                for (_, family) in &families {
                    readings.push((
                        *family,
                        self.recognize_family(
                            *family,
                            &crops,
                            image.width(),
                            image.height(),
                            cancellation,
                        )?,
                    ));
                }
                let (lines, winner) = best_reading_per_line(readings, crops.len());
                let language = winner
                    .and_then(|winner| {
                        families
                            .iter()
                            .find(|(_, family)| *family == winner)
                            .map(|(language, _)| language.clone())
                    })
                    .or_else(|| families.first().map(|(language, _)| language.clone()));
                (language, lines)
            }
            OcrLanguagePolicy::Auto => {
                // Without a hint every family competes; mixing them line by
                // line would let an unrelated model win a line with a
                // confident misreading, so one family reads the region.
                let mut best: Option<(ModelFamily, FamilyRecognition)> = None;
                for family in ModelFamily::ALL {
                    let candidate = self.recognize_family(
                        family,
                        &crops,
                        image.width(),
                        image.height(),
                        cancellation,
                    )?;
                    best = better_family(best, (family, candidate));
                }
                let (family, result) = best.ok_or(OcrError::Engine)?;
                (
                    None,
                    result
                        .lines
                        .into_iter()
                        .map(|line| line.map(|line| (family, line)))
                        .collect(),
                )
            }
        };
        self.retry_rotated_lines(&crops, &mut lines, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        Ok(OcrDocument {
            lines: lines.into_iter().flatten().map(|(_, line)| line).collect(),
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

fn quad_for(bounds: &BoundingBox, image_width: u32, image_height: u32) -> Option<PhysicalQuad> {
    let mut sorted: [_; 4] = bounds.points.as_slice().try_into().ok()?;
    if sorted
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return None;
    }
    // Match oar-ocr 0.9.2 get_rotate_crop_image: it canonicalizes detector
    // point order and turns tall rectified crops 90 degrees counterclockwise.
    // The returned quad must describe the recognition image's reading axes.
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x));
    let (tl, bl) = if sorted[1].y < sorted[0].y {
        (1, 0)
    } else {
        (0, 1)
    };
    let (tr, br) = if sorted[3].y < sorted[2].y {
        (3, 2)
    } else {
        (2, 3)
    };
    let mut points = [sorted[tl], sorted[tr], sorted[br], sorted[bl]];
    let distance =
        |a: usize, b: usize| (points[a].x - points[b].x).hypot(points[a].y - points[b].y);
    let width = distance(0, 1).max(distance(2, 3)).round();
    let height = distance(0, 3).max(distance(1, 2)).round();
    if height >= width * 1.5 {
        points.rotate_left(1);
    }
    let quad = PhysicalQuad {
        points: points.map(|point| PhysicalPoint {
            x: point.x.round().clamp(0.0, image_width as f32) as i32,
            y: point.y.round().clamp(0.0, image_height as f32) as i32,
        }),
    };
    quad.is_within(bounds_for(bounds, image_width, image_height))
        .then_some(quad)
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
    fn detector_quad_retains_slope_and_clamps_crop_edges() {
        use oar_ocr_core::processors::{BoundingBox, Point};
        let bounds = BoundingBox::new(vec![
            Point::new(-0.2, 10.0),
            Point::new(90.0, 30.0),
            Point::new(88.0, 50.0),
            Point::new(-0.2, 30.0),
        ]);
        let quad = super::quad_for(&bounds, 100, 60).unwrap();
        assert_eq!(quad.points[0], rust_protocol::PhysicalPoint { x: 0, y: 10 });
        assert_eq!(
            quad.points[1],
            rust_protocol::PhysicalPoint { x: 90, y: 30 }
        );
        assert!(quad.is_within(super::bounds_for(&bounds, 100, 60)));
    }

    #[test]
    fn malformed_detector_quad_is_not_propagated() {
        use oar_ocr_core::processors::{BoundingBox, Point};
        assert_eq!(
            super::quad_for(&BoundingBox::new(vec![Point::new(1.0, 1.0); 4]), 10, 10),
            None
        );
        assert_eq!(
            super::quad_for(
                &BoundingBox::new(vec![Point::new(f32::NAN, 1.0); 4]),
                10,
                10
            ),
            None
        );
    }

    #[test]
    fn quad_corner_order_matches_crop_canonicalization() {
        use oar_ocr_core::processors::{BoundingBox, Point};
        let shuffled = BoundingBox::new(vec![
            Point::new(88.0, 50.0),
            Point::new(0.0, 10.0),
            Point::new(0.0, 30.0),
            Point::new(90.0, 30.0),
        ]);
        let quad = super::quad_for(&shuffled, 100, 60).unwrap();
        assert_eq!(quad.points[0], rust_protocol::PhysicalPoint { x: 0, y: 10 });
        assert_eq!(
            quad.points[1],
            rust_protocol::PhysicalPoint { x: 90, y: 30 }
        );
    }

    #[test]
    fn vertical_quad_follows_the_automatically_rotated_recognition_crop() {
        use oar_ocr_core::processors::BoundingBox;
        let bounds = BoundingBox::from_coords(10.0, 20.0, 30.0, 120.0);
        let quad = super::quad_for(&bounds, 100, 200).unwrap();
        assert_eq!(
            quad.points,
            [
                rust_protocol::PhysicalPoint { x: 30, y: 20 },
                rust_protocol::PhysicalPoint { x: 30, y: 120 },
                rust_protocol::PhysicalPoint { x: 10, y: 120 },
                rust_protocol::PhysicalPoint { x: 10, y: 20 },
            ]
        );
        assert!(quad.is_within(super::bounds_for(&bounds, 100, 200)));
    }

    fn line(text: &str, confidence_milli: u16) -> Option<rust_protocol::OcrLine> {
        Some(rust_protocol::OcrLine {
            text: text.into(),
            bounds: rust_protocol::PhysicalRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            quad: None,
            confidence_milli,
        })
    }

    #[test]
    fn each_line_takes_the_family_that_reads_it_best() {
        let english = super::FamilyRecognition {
            lines: vec![line("Status: rotoBo", 610), line("Settings", 970), None],
            average_confidence: 0.79,
        };
        let slavic = super::FamilyRecognition {
            lines: vec![
                line("Status: готово", 930),
                line("Settings", 900),
                line("Готово", 880),
            ],
            average_confidence: 0.90,
        };
        let (lines, winner) = super::best_reading_per_line(
            vec![
                (ModelFamily::English, english),
                (ModelFamily::EasternSlavic, slavic),
            ],
            3,
        );
        let texts = lines
            .iter()
            .map(|line| line.as_ref().map(|(_, line)| line.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            [Some("Status: готово"), Some("Settings"), Some("Готово")]
        );
        assert_eq!(winner, Some(ModelFamily::EasternSlavic));
    }

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
                    let rows = crate::reconstruct_visual_rows(&reconstructed.elements);
                    if let Some(output) = std::env::var_os("SAYFLEX_OCR_PROBE_OUTPUT_JSON") {
                        // A single detector configuration gives one deterministic
                        // replay file, with no profile or screenshot-store writes.
                        let blocks = crate::reconstruct_visual_paragraphs(&rows)
                            .into_iter()
                            .enumerate()
                            .map(|(index, row)| rust_protocol::ScreenTextBlock {
                                block_id: format!("screen-block-{index:04}").into(),
                                source_text: row.text,
                                bounds: row.bounds,
                                quad: row.quad,
                                fragments: row.fragments,
                                fragment_quads: row.fragment_quads,
                                confidence_milli: row.confidence_milli,
                            })
                            .collect::<Vec<_>>();
                        let rows = rows.iter().map(|row| serde_json::json!({
                            "text": row.text, "bounds": row.bounds, "quad": row.quad,
                            "fragments": row.fragments, "fragment_quads": row.fragment_quads,
                            "confidence_milli": row.confidence_milli,
                        })).collect::<Vec<_>>();
                        let value = serde_json::json!({
                            "width": image.width(), "height": image.height(),
                            "limit": limit, "lines": document.lines, "rows": rows, "blocks": blocks,
                        });
                        std::fs::write(output, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
                    }
                    for row in rows {
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
