use std::{collections::HashMap, path::PathBuf};

use image::{DynamicImage, RgbaImage};
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder, OAROCRResult};
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
    pipelines: HashMap<ModelFamily, OAROCR>,
}

impl std::fmt::Debug for PpOcrAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PpOcrAdapter")
            .field("pipeline_count", &self.pipelines.len())
            .finish()
    }
}

impl PpOcrAdapter {
    pub fn load(paths: &PpOcrModelPaths) -> Result<Self, OcrError> {
        let detector = paths.detector();
        if !detector.is_file() {
            return Err(OcrError::ModelUnavailable);
        }
        let mut pipelines = HashMap::new();
        for family in ModelFamily::ALL {
            let recognizer = paths.recognizer(family);
            let dictionary = paths.dictionary(family);
            if !recognizer.is_file() || !dictionary.is_file() {
                return Err(OcrError::ModelUnavailable);
            }
            let pipeline = OAROCRBuilder::new(&detector, &recognizer, &dictionary)
                .build()
                .map_err(|_| OcrError::Engine)?;
            pipelines.insert(family, pipeline);
        }
        Ok(Self { pipelines })
    }

    fn predict_family(
        &self,
        family: ModelFamily,
        image: &image::RgbImage,
        cancellation: &CancellationToken,
    ) -> Result<OAROCRResult, OcrError> {
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        self.pipelines
            .get(&family)
            .ok_or(OcrError::ModelUnavailable)?
            .predict(vec![image.clone()])
            .map_err(|_| OcrError::Engine)?
            .into_iter()
            .next()
            .ok_or(OcrError::Engine)
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
        let (family, language, result) = match language_policy {
            OcrLanguagePolicy::Prefer(languages) => {
                let (language, family) = languages
                    .iter()
                    .find_map(|language| {
                        ModelFamily::for_language(language).map(|family| (language, family))
                    })
                    .ok_or(OcrError::LanguageUnavailable)?;
                (
                    family,
                    Some(language.clone()),
                    self.predict_family(family, &rgb, cancellation)?,
                )
            }
            OcrLanguagePolicy::Auto => {
                let mut best: Option<(f32, ModelFamily, OAROCRResult)> = None;
                for family in ModelFamily::ALL {
                    let candidate = self.predict_family(family, &rgb, cancellation)?;
                    let score = candidate.average_confidence().unwrap_or(-1.0);
                    if best
                        .as_ref()
                        .is_none_or(|(best_score, _, _)| score > *best_score)
                    {
                        best = Some((score, family, candidate));
                    }
                }
                let (_, family, result) = best.ok_or(OcrError::Engine)?;
                (family, None, result)
            }
        };
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        Ok(OcrDocument {
            lines: result
                .text_regions
                .iter()
                .filter_map(|region| {
                    let text = region.text.as_deref()?;
                    let text = postprocess_text(family, text);
                    (!text.is_empty()).then(|| OcrLine {
                        text,
                        bounds: bounds_for(region, image.width(), image.height()),
                        confidence_milli: confidence_milli(region.confidence),
                    })
                })
                .collect(),
            language,
        })
    }
}

fn bounds_for(
    region: &oar_ocr::oarocr::TextRegion,
    image_width: u32,
    image_height: u32,
) -> PhysicalRect {
    let points = &region.bounding_box.points;
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
    use super::{ModelFamily, postprocess_text};

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
}
