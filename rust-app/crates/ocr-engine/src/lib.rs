use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use base64::Engine as _;
use image::RgbaImage;
use rust_protocol::{OcrLanguagePolicy, OcrLine};
use thiserror::Error;

mod reconstruct;

pub use reconstruct::{ReconstructedBlock, ReconstructedText, reconstruct_text};

#[cfg(feature = "ppocr")]
mod ppocr;

#[cfg(feature = "ppocr")]
pub use ppocr::{PpOcrAdapter, PpOcrModelPaths};

pub const MAX_OCR_IMAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrDocument {
    pub lines: Vec<OcrLine>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OcrError {
    #[error("OCR request was cancelled")]
    Cancelled,
    #[error("OCR image is invalid")]
    InvalidImage,
    #[error("OCR image exceeds the byte limit")]
    ImageTooLarge,
    #[error("OCR model is unavailable")]
    ModelUnavailable,
    #[error("OCR language is unavailable")]
    LanguageUnavailable,
    #[error("OCR engine failed")]
    Engine,
}

pub trait InferenceAdapter: Send + Sync {
    fn recognize(
        &self,
        image: &RgbaImage,
        language_policy: &OcrLanguagePolicy,
        cancellation: &CancellationToken,
    ) -> Result<OcrDocument, OcrError>;
}

#[derive(Clone)]
pub struct OcrEngine {
    adapter: Arc<dyn InferenceAdapter>,
}

impl OcrEngine {
    #[must_use]
    pub fn new(adapter: Arc<dyn InferenceAdapter>) -> Self {
        Self { adapter }
    }

    pub fn recognize(
        &self,
        image: &RgbaImage,
        language_policy: &OcrLanguagePolicy,
        cancellation: &CancellationToken,
    ) -> Result<OcrDocument, OcrError> {
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        if image.width() == 0 || image.height() == 0 {
            return Err(OcrError::InvalidImage);
        }
        let document = self
            .adapter
            .recognize(image, language_policy, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        Ok(document)
    }
}

impl std::fmt::Debug for OcrEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OcrEngine(..)")
    }
}

pub fn decode_rgba_request(
    width: u32,
    height: u32,
    stride: u32,
    rgba_base64: &str,
) -> Result<RgbaImage, OcrError> {
    if width == 0 || height == 0 {
        return Err(OcrError::InvalidImage);
    }
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(OcrError::ImageTooLarge)?;
    let stride = usize::try_from(stride).map_err(|_| OcrError::ImageTooLarge)?;
    if stride < row_bytes {
        return Err(OcrError::InvalidImage);
    }
    let encoded_bytes = stride
        .checked_mul(usize::try_from(height).map_err(|_| OcrError::ImageTooLarge)?)
        .ok_or(OcrError::ImageTooLarge)?;
    let contiguous_bytes = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| OcrError::ImageTooLarge)?)
        .ok_or(OcrError::ImageTooLarge)?;
    if encoded_bytes > MAX_OCR_IMAGE_BYTES || contiguous_bytes > MAX_OCR_IMAGE_BYTES {
        return Err(OcrError::ImageTooLarge);
    }
    let maximum_base64_len = encoded_bytes
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .and_then(|value| value.checked_add(4))
        .ok_or(OcrError::ImageTooLarge)?;
    if rgba_base64.len() > maximum_base64_len {
        return Err(OcrError::InvalidImage);
    }

    let mut decoded = vec![0_u8; encoded_bytes];
    let decoded_len = base64::engine::general_purpose::STANDARD
        .decode_slice(rgba_base64, &mut decoded)
        .map_err(|_| OcrError::InvalidImage)?;
    if decoded_len != encoded_bytes {
        return Err(OcrError::InvalidImage);
    }

    let pixels = if stride == row_bytes {
        decoded
    } else {
        let mut pixels = vec![0_u8; contiguous_bytes];
        for row in 0..usize::try_from(height).map_err(|_| OcrError::ImageTooLarge)? {
            let source_start = row * stride;
            let destination_start = row * row_bytes;
            pixels[destination_start..destination_start + row_bytes]
                .copy_from_slice(&decoded[source_start..source_start + row_bytes]);
        }
        pixels
    };
    RgbaImage::from_raw(width, height, pixels).ok_or(OcrError::InvalidImage)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{Rgba, RgbaImage};
    use rust_protocol::{OcrLanguagePolicy, OcrLine, PhysicalRect};

    use super::{
        CancellationToken, InferenceAdapter, OcrDocument, OcrEngine, OcrError, decode_rgba_request,
    };

    struct FakeAdapter {
        result: OcrDocument,
    }

    impl InferenceAdapter for FakeAdapter {
        fn recognize(
            &self,
            _image: &RgbaImage,
            _language_policy: &OcrLanguagePolicy,
            _cancellation: &CancellationToken,
        ) -> Result<OcrDocument, OcrError> {
            Ok(self.result.clone())
        }
    }

    fn engine_with(lines: Vec<OcrLine>) -> OcrEngine {
        OcrEngine::new(Arc::new(FakeAdapter {
            result: OcrDocument {
                lines,
                language: Some("en".to_owned()),
            },
        }))
    }

    #[test]
    fn preserves_inference_geometry() {
        let line = OcrLine {
            text: "fixture".to_owned(),
            bounds: PhysicalRect {
                x: 10,
                y: 12,
                width: 80,
                height: 16,
            },
            confidence_milli: 920,
        };
        let image = RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let document = engine_with(vec![line.clone()])
            .recognize(&image, &OcrLanguagePolicy::Auto, &CancellationToken::new())
            .unwrap();
        assert_eq!(document.lines, vec![line]);
    }

    #[test]
    fn cancellation_wins_before_inference() {
        let token = CancellationToken::new();
        token.cancel();
        let error = engine_with(Vec::new())
            .recognize(&RgbaImage::new(1, 1), &OcrLanguagePolicy::Auto, &token)
            .unwrap_err();
        assert_eq!(error, OcrError::Cancelled);
    }

    #[test]
    fn rejects_invalid_stride_and_dimensions() {
        assert_eq!(
            decode_rgba_request(2, 1, 7, "AAAAAAAAAAA="),
            Err(OcrError::InvalidImage)
        );
        assert_eq!(
            decode_rgba_request(0, 1, 0, ""),
            Err(OcrError::InvalidImage)
        );
    }

    #[test]
    fn rejects_invalid_base64() {
        assert_eq!(
            decode_rgba_request(1, 1, 4, "not base64"),
            Err(OcrError::InvalidImage)
        );
    }

    #[test]
    fn rejects_byte_cap_before_decoding() {
        let width = 65_536;
        let height = 65_536;
        assert_eq!(
            decode_rgba_request(width, height, width * 4, "AAAA"),
            Err(OcrError::ImageTooLarge)
        );
    }

    #[test]
    fn errors_never_contain_source_text_or_pixels() {
        for error in [
            OcrError::InvalidImage,
            OcrError::ImageTooLarge,
            OcrError::ModelUnavailable,
            OcrError::LanguageUnavailable,
            OcrError::Engine,
            OcrError::Cancelled,
        ] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for secret in ["fixture", "1, 2, 3", "base64"] {
                assert!(!display.contains(secret));
                assert!(!debug.contains(secret));
            }
        }
    }
}
