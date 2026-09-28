use rust_protocol::{PhysicalRect, ScreenBlockId};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{ScreenBlockOverlay, TranslationPair, TranslationPreview};

const MIN_CALLOUT_WIDTH: f32 = 180.0;
const MAX_CALLOUT_WIDTH: f32 = 320.0;
const MIN_READABLE_LINE_HEIGHT: f32 = 14.0;
const APPROXIMATE_GLYPH_WIDTH: f32 = 7.0;
const CALLOUT_GAP: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultViewMode {
    Translation,
    Original,
    Dual,
    OnImage,
}

impl ResultViewMode {
    fn from_ui(value: i32) -> Self {
        match value {
            1 => Self::Original,
            2 => Self::Dual,
            3 => Self::OnImage,
            _ => Self::Translation,
        }
    }

    fn as_ui(self) -> i32 {
        match self {
            Self::Translation => 0,
            Self::Original => 1,
            Self::Dual => 2,
            Self::OnImage => 3,
        }
    }
}

pub(crate) fn visible_mode(requested: ResultViewMode, pixels_available: bool) -> ResultViewMode {
    if requested == ResultViewMode::OnImage && !pixels_available {
        ResultViewMode::Translation
    } else {
        requested
    }
}

pub(crate) fn visible_ui_mode(requested: i32, pixels_available: bool) -> i32 {
    visible_mode(ResultViewMode::from_ui(requested), pixels_available).as_ui()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockLayoutInput {
    pub(crate) block_id: ScreenBlockId,
    pub(crate) source: String,
    pub(crate) translation: String,
    pub(crate) bounds: PhysicalRect,
    pub(crate) confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlockLayout {
    pub(crate) block_id: ScreenBlockId,
    pub(crate) source: String,
    pub(crate) translation: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) source_x: f32,
    pub(crate) source_y: f32,
    pub(crate) source_width: f32,
    pub(crate) source_height: f32,
    pub(crate) callout: bool,
    pub(crate) low_confidence: bool,
}

pub(crate) fn layout_blocks(
    crop: PhysicalRect,
    viewport_width: u32,
    viewport_height: u32,
    blocks: &[BlockLayoutInput],
) -> Vec<BlockLayout> {
    if crop.width == 0 || crop.height == 0 || viewport_width == 0 || viewport_height == 0 {
        return Vec::new();
    }
    let scale_x = viewport_width as f32 / crop.width as f32;
    let scale_y = viewport_height as f32 / crop.height as f32;
    let viewport_width = viewport_width as f32;
    let viewport_height = viewport_height as f32;

    blocks
        .iter()
        .map(|block| {
            let source_x = ((i64::from(block.bounds.x) - i64::from(crop.x)) as f32 * scale_x)
                .clamp(0.0, viewport_width);
            let source_y = ((i64::from(block.bounds.y) - i64::from(crop.y)) as f32 * scale_y)
                .clamp(0.0, viewport_height);
            let source_width = (block.bounds.width as f32 * scale_x)
                .max(1.0)
                .min(viewport_width.saturating_sub(source_x));
            let source_height = (block.bounds.height as f32 * scale_y)
                .max(1.0)
                .min(viewport_height.saturating_sub(source_y));
            let readable_columns = (source_width / APPROXIMATE_GLYPH_WIDTH).floor().max(1.0);
            let readable_lines = (source_height / MIN_READABLE_LINE_HEIGHT).floor().max(1.0);
            let capacity = (readable_columns * readable_lines) as usize;
            let callout = block.translation.chars().count() > capacity;

            let (x, y, width, height) = if callout {
                let desired_width =
                    (block.translation.chars().count() as f32 * APPROXIMATE_GLYPH_WIDTH / 3.0)
                        .clamp(MIN_CALLOUT_WIDTH, MAX_CALLOUT_WIDTH)
                        .min(viewport_width);
                let desired_lines = (block.translation.chars().count() as f32
                    / (desired_width / APPROXIMATE_GLYPH_WIDTH).max(1.0))
                .ceil()
                .max(1.0);
                let desired_height =
                    (desired_lines * MIN_READABLE_LINE_HEIGHT + 12.0).clamp(32.0, viewport_height);
                let right_x = source_x + source_width + CALLOUT_GAP;
                let left_x = source_x - desired_width - CALLOUT_GAP;
                let x = if right_x + desired_width <= viewport_width {
                    right_x
                } else if left_x >= 0.0 {
                    left_x
                } else {
                    source_x.min((viewport_width - desired_width).max(0.0))
                };
                let y = source_y.min((viewport_height - desired_height).max(0.0));
                (x, y, desired_width, desired_height)
            } else {
                (source_x, source_y, source_width, source_height)
            };

            BlockLayout {
                block_id: block.block_id.clone(),
                source: block.source.clone(),
                translation: block.translation.clone(),
                x,
                y,
                width,
                height,
                source_x,
                source_y,
                source_width,
                source_height,
                callout,
                low_confidence: block.confidence_milli < 650,
            }
        })
        .collect()
}

trait SaturatingSubF32 {
    fn saturating_sub(self, rhs: f32) -> f32;
}

impl SaturatingSubF32 for f32 {
    fn saturating_sub(self, rhs: f32) -> f32 {
        (self - rhs).max(0.0)
    }
}

pub(crate) fn apply_snapshot(
    preview: &TranslationPreview,
    snapshot: &ScreenResultSnapshot,
) -> Result<Vec<BlockLayout>, String> {
    let (viewport_width, viewport_height) = fitted_viewport(snapshot.crop_rect);
    let inputs = snapshot
        .blocks
        .iter()
        .map(|block| BlockLayoutInput {
            block_id: block.block_id.clone(),
            source: block.source.clone(),
            translation: block.translation.clone(),
            bounds: block.bounds,
            confidence_milli: block.confidence_milli,
        })
        .collect::<Vec<_>>();
    let layouts = layout_blocks(snapshot.crop_rect, viewport_width, viewport_height, &inputs);
    let rows = layouts
        .iter()
        .map(|block| ScreenBlockOverlay {
            block_id: block.block_id.as_str().into(),
            source: block.source.clone().into(),
            translation: block.translation.clone().into(),
            x: block.x,
            y: block.y,
            width: block.width,
            height: block.height,
            source_x: block.source_x,
            source_y: block.source_y,
            source_width: block.source_width,
            source_height: block.source_height,
            callout: block.callout,
            low_confidence: block.low_confidence,
        })
        .collect::<Vec<_>>();
    let pairs = snapshot
        .blocks
        .iter()
        .map(|block| TranslationPair {
            source: block.source.clone().into(),
            translation: block.translation.clone().into(),
        })
        .collect::<Vec<_>>();
    let translations = snapshot
        .blocks
        .iter()
        .map(|block| slint::SharedString::from(block.translation.as_str()))
        .collect::<Vec<_>>();
    preview.set_source_text(
        snapshot
            .blocks
            .iter()
            .map(|block| block.source.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
    );
    preview.set_translated_text(
        snapshot
            .blocks
            .iter()
            .map(|block| block.translation.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
    );
    preview.set_translated_paragraphs(ModelRc::new(VecModel::from(translations)));
    preview.set_dual_pairs(ModelRc::new(VecModel::from(pairs)));
    preview.set_screen_blocks(ModelRc::new(VecModel::from(rows)));
    preview.set_screen_content_width(viewport_width as f32);
    preview.set_screen_content_height(viewport_height as f32);
    preview.set_screen_image_expired(snapshot.crop.is_none());
    preview.set_screen_view_mode(visible_ui_mode(3, snapshot.crop.is_some()));
    preview.set_screen_selected_block(-1);
    preview.set_screen_editor_visible(false);
    preview.set_screen_save_error(false);
    preview.set_screen_image_active(true);
    preview.set_screen_crop_image(match snapshot.crop.as_deref() {
        Some(frame) => frame_image(frame)?,
        None => Image::default(),
    });
    Ok(layouts)
}

pub(crate) fn clear_snapshot(preview: &TranslationPreview) {
    preview.set_screen_image_active(false);
    preview.set_screen_crop_image(Image::default());
    preview.set_screen_blocks(ModelRc::new(VecModel::from(
        Vec::<ScreenBlockOverlay>::new(),
    )));
    preview.set_screen_editor_visible(false);
    preview.set_screen_image_expired(false);
    preview.set_screen_saved(false);
    preview.set_screen_save_error(false);
    preview.set_screen_selected_block(-1);
}

fn fitted_viewport(crop: PhysicalRect) -> (u32, u32) {
    if crop.width == 0 || crop.height == 0 {
        return (320, 200);
    }
    let fit = (960.0 / crop.width as f32)
        .min(576.0 / crop.height as f32)
        .min(1.0);
    let minimum = (320.0 / crop.width as f32)
        .max(200.0 / crop.height as f32)
        .min((960.0 / crop.width as f32).min(576.0 / crop.height as f32));
    let scale = fit.max(minimum);
    (
        (crop.width as f32 * scale).round().max(1.0) as u32,
        (crop.height as f32 * scale).round().max(1.0) as u32,
    )
}

fn frame_image(frame: &rust_platform_windows::screen_capture::RgbaFrame) -> Result<Image, String> {
    let row_bytes = frame
        .rect
        .width
        .checked_mul(4)
        .ok_or_else(|| "screen crop row is too wide".to_owned())? as usize;
    let packed = if frame.stride as usize == row_bytes {
        frame.pixels.clone()
    } else {
        let mut packed = Vec::with_capacity(row_bytes.saturating_mul(frame.rect.height as usize));
        for row in frame.pixels.chunks_exact(frame.stride as usize) {
            packed.extend_from_slice(
                row.get(..row_bytes)
                    .ok_or_else(|| "screen crop stride is too small".to_owned())?,
            );
        }
        packed
    };
    if packed.len() != row_bytes.saturating_mul(frame.rect.height as usize) {
        return Err("screen crop pixel buffer is incomplete".to_owned());
    }
    Ok(Image::from_rgba8(
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &packed,
            frame.rect.width,
            frame.rect.height,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use rust_protocol::{PhysicalRect, ScreenBlockId};

    use super::{BlockLayoutInput, ResultViewMode, layout_blocks, visible_mode, visible_ui_mode};

    #[test]
    fn block_geometry_scales_with_the_crop_and_long_text_uses_a_readable_callout() {
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("short"),
                source: "Menu".to_owned(),
                translation: "Меню".to_owned(),
                bounds: rect(100, 50, 200, 80),
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("long"),
                source: "Long source".to_owned(),
                translation:
                    "Очень длинный перевод, который нельзя уменьшать до нечитаемого размера"
                        .to_owned(),
                bounds: rect(700, 400, 80, 24),
                confidence_milli: 500,
            },
        ];

        let laid_out = layout_blocks(rect(0, 0, 1_000, 500), 500, 250, &blocks);
        assert_eq!((laid_out[0].x, laid_out[0].y), (50.0, 25.0));
        assert_eq!((laid_out[0].width, laid_out[0].height), (100.0, 40.0));
        assert!(!laid_out[0].callout);
        assert!(laid_out[1].callout);
        assert!(laid_out[1].width >= 180.0);
        assert!(laid_out[1].x + laid_out[1].width <= 500.0);
        assert!(laid_out[1].low_confidence);
    }

    #[test]
    fn expired_pixels_fall_back_to_the_text_projection() {
        assert_eq!(
            visible_mode(ResultViewMode::OnImage, false),
            ResultViewMode::Translation
        );
        assert_eq!(
            visible_mode(ResultViewMode::Dual, true),
            ResultViewMode::Dual
        );
        assert_eq!(visible_ui_mode(3, false), 0);
        assert_eq!(visible_ui_mode(1, false), 1);
        assert_eq!(visible_ui_mode(2, true), 2);
        assert_eq!(visible_ui_mode(99, true), 0);
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }
}
