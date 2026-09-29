use rust_protocol::{PhysicalRect, ScreenBlockId};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{ScreenBlockOverlay, TranslationPair, TranslationPreview};

const MIN_CALLOUT_WIDTH: f32 = 180.0;
const MAX_CALLOUT_WIDTH: f32 = 320.0;
// 12 logical pixels is the smallest overlay font admitted by the Slint view.
// Reserve a full 18 pixels per wrapped line so measured glyphs and line
// spacing cannot spill into the next callout on high-DPI displays.
const MIN_READABLE_LINE_HEIGHT: f32 = 18.0;
const APPROXIMATE_GLYPH_WIDTH: f32 = 7.0;
const CALLOUT_GAP: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultViewMode {
    Translation,
    Original,
    Dual,
    OnImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenResultWindowMode {
    pinned: bool,
}

impl ScreenResultWindowMode {
    pub(crate) fn new(pinned: bool) -> Self {
        Self { pinned }
    }

    pub(crate) fn topmost(self) -> bool {
        self.pinned
    }
}

pub(crate) fn preserve_editor_on_snapshot_refresh(
    editor_visible: bool,
    image_available: bool,
) -> bool {
    editor_visible && image_available
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
    pub(crate) bounds: CropLocalRect,
    pub(crate) confidence_milli: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CropPixelSize {
    width: u32,
    height: u32,
}

impl CropPixelSize {
    pub(crate) fn from_screen_rect(rect: PhysicalRect) -> Self {
        Self {
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CropLocalRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl CropLocalRect {
    pub(crate) fn from_ocr_bounds(bounds: PhysicalRect) -> Self {
        Self {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        }
    }
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
    pub(crate) overflow_panel: bool,
    pub(crate) panel_order: i32,
    pub(crate) panel_left: bool,
    pub(crate) low_confidence: bool,
}

pub(crate) fn layout_blocks(
    crop: CropPixelSize,
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

    let mut layouts = blocks
        .iter()
        .map(|block| {
            let source_x = (block.bounds.x as f32 * scale_x).clamp(0.0, viewport_width);
            let source_y = (block.bounds.y as f32 * scale_y).clamp(0.0, viewport_height);
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
                overflow_panel: false,
                panel_order: -1,
                panel_left: false,
                low_confidence: block.confidence_milli < 650,
            }
        })
        .collect::<Vec<_>>();
    resolve_callout_collisions(&mut layouts, viewport_width, viewport_height);
    layouts
}

fn resolve_callout_collisions(
    layouts: &mut [BlockLayout],
    viewport_width: f32,
    viewport_height: f32,
) {
    let source_rects = layouts
        .iter()
        .enumerate()
        .map(|(index, block)| {
            (
                index,
                block.source_x,
                block.source_y,
                block.source_width,
                block.source_height,
            )
        })
        .collect::<Vec<_>>();
    let mut placed_callouts: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut callout_order = layouts
        .iter()
        .enumerate()
        .filter(|(_, block)| block.callout)
        .map(|(index, block)| (index, block.source_y, block.source_x))
        .collect::<Vec<_>>();
    callout_order.sort_by(|left, right| {
        left.1
            .total_cmp(&right.1)
            .then_with(|| left.2.total_cmp(&right.2))
    });

    for (index, _, _) in callout_order {
        let block = &layouts[index];
        let right = block.source_x + block.source_width + CALLOUT_GAP;
        let left = block.source_x - block.width - CALLOUT_GAP;
        let mut x_candidates = vec![block.x];
        if right + block.width <= viewport_width {
            x_candidates.push(right);
        }
        if left >= 0.0 {
            x_candidates.push(left);
        }
        x_candidates.push((viewport_width - block.width).max(0.0));
        x_candidates.sort_by(f32::total_cmp);
        x_candidates.dedup_by(|left, right| (*left - *right).abs() < 0.5);

        let mut y_candidates = vec![
            block
                .source_y
                .min((viewport_height - block.height).max(0.0)),
            0.0,
            (viewport_height - block.height).max(0.0),
        ];
        for (_, _, y, _, height) in &source_rects {
            y_candidates
                .push((y + height + CALLOUT_GAP).min((viewport_height - block.height).max(0.0)));
            y_candidates.push((y - block.height - CALLOUT_GAP).max(0.0));
        }
        for (_, y, _, height) in &placed_callouts {
            y_candidates
                .push((y + height + CALLOUT_GAP).min((viewport_height - block.height).max(0.0)));
            y_candidates.push((y - block.height - CALLOUT_GAP).max(0.0));
        }
        y_candidates.sort_by(f32::total_cmp);
        y_candidates.dedup_by(|left, right| (*left - *right).abs() < 0.5);

        let best = x_candidates
            .iter()
            .flat_map(|x| y_candidates.iter().map(move |y| (*x, *y)))
            .filter(|(x, y)| {
                let candidate = (*x, *y, block.width, block.height);
                !source_rects
                    .iter()
                    .filter(|(source_index, ..)| *source_index != index)
                    .any(|(_, x, y, width, height)| {
                        rectangles_overlap(candidate, (*x, *y, *width, *height))
                    })
                    && !placed_callouts
                        .iter()
                        .any(|occupied| rectangles_overlap(candidate, *occupied))
            })
            .min_by(|left, right| {
                let left_distance =
                    (left.0 - block.source_x).abs() + (left.1 - block.source_y).abs();
                let right_distance =
                    (right.0 - block.source_x).abs() + (right.1 - block.source_y).abs();
                left_distance.total_cmp(&right_distance)
            });
        if let Some((x, y)) = best {
            layouts[index].x = x;
            layouts[index].y = y;
            placed_callouts.push((x, y, layouts[index].width, layouts[index].height));
        } else {
            // There is no collision-free callout arrangement in the image.
            // Do not silently accept overlaps: the complete result will use
            // one measured, scrollable side panel instead.
            assign_overflow_panel(layouts, viewport_width, viewport_height);
            return;
        }
    }
}

fn assign_overflow_panel(layouts: &mut [BlockLayout], viewport_width: f32, viewport_height: f32) {
    let panel_width = (viewport_width * 0.48)
        .clamp(MIN_CALLOUT_WIDTH, MAX_CALLOUT_WIDTH)
        .min(viewport_width);
    let right_x = (viewport_width - panel_width - CALLOUT_GAP).max(0.0);
    let left_rect = (
        CALLOUT_GAP.min(viewport_width),
        0.0,
        panel_width,
        viewport_height,
    );
    let right_rect = (right_x, 0.0, panel_width, viewport_height);
    let overlap = |panel: (f32, f32, f32, f32)| {
        layouts
            .iter()
            .map(|block| {
                rectangle_intersection_area(
                    panel,
                    (
                        block.source_x,
                        block.source_y,
                        block.source_width,
                        block.source_height,
                    ),
                )
            })
            .sum::<f32>()
    };
    let panel_left = overlap(left_rect) < overlap(right_rect);
    let panel_x = if panel_left {
        left_rect.0
    } else {
        right_rect.0
    };
    let mut order = (0..layouts.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        layouts[*left]
            .source_y
            .total_cmp(&layouts[*right].source_y)
            .then_with(|| layouts[*left].source_x.total_cmp(&layouts[*right].source_x))
    });
    for (panel_order, index) in order.into_iter().enumerate() {
        let block = &mut layouts[index];
        block.overflow_panel = true;
        block.panel_order = i32::try_from(panel_order).unwrap_or(i32::MAX);
        block.panel_left = panel_left;
        block.x = panel_x;
        block.y = block.source_y;
        block.width = panel_width;
    }
}

fn rectangles_overlap(left: (f32, f32, f32, f32), right: (f32, f32, f32, f32)) -> bool {
    left.0 < right.0 + right.2
        && right.0 < left.0 + left.2
        && left.1 < right.1 + right.3
        && right.1 < left.1 + left.3
}

fn rectangle_intersection_area(left: (f32, f32, f32, f32), right: (f32, f32, f32, f32)) -> f32 {
    let width = (left.0 + left.2).min(right.0 + right.2) - left.0.max(right.0);
    let height = (left.1 + left.3).min(right.1 + right.3) - left.1.max(right.1);
    width.max(0.0) * height.max(0.0)
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
    let crop_size = CropPixelSize::from_screen_rect(snapshot.crop_rect);
    let (viewport_width, viewport_height) = fitted_viewport(crop_size);
    let inputs = snapshot
        .blocks
        .iter()
        .map(|block| BlockLayoutInput {
            block_id: block.block_id.clone(),
            source: block.source.clone(),
            translation: block.translation.clone(),
            bounds: CropLocalRect::from_ocr_bounds(block.bounds),
            confidence_milli: block.confidence_milli,
        })
        .collect::<Vec<_>>();
    let layouts = layout_blocks(crop_size, viewport_width, viewport_height, &inputs);
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
            overflow_panel: block.overflow_panel,
            panel_order: block.panel_order,
            panel_left: block.panel_left,
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
    preview.set_screen_dense_panel_active(layouts.iter().any(|block| block.overflow_panel));
    preview.set_screen_dense_panel_left(
        layouts
            .iter()
            .find(|block| block.overflow_panel)
            .is_some_and(|block| block.panel_left),
    );
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
    preview.set_screen_dense_panel_active(false);
    preview.set_screen_dense_panel_left(false);
    preview.set_screen_editor_visible(false);
    preview.set_screen_image_expired(false);
    preview.set_screen_saved(false);
    preview.set_screen_save_error(false);
    preview.set_screen_selected_block(-1);
}

fn fitted_viewport(crop: CropPixelSize) -> (u32, u32) {
    if crop.width == 0 || crop.height == 0 {
        return (320, 200);
    }
    let fit = (960.0 / crop.width as f32)
        .min(576.0 / crop.height as f32)
        .min(1.0);
    let minimum = (440.0 / crop.width as f32)
        .max(320.0 / crop.height as f32)
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
    use rust_protocol::ScreenBlockId;

    use super::{
        BlockLayoutInput, CropLocalRect, CropPixelSize, ResultViewMode, ScreenResultWindowMode,
        fitted_viewport, layout_blocks, preserve_editor_on_snapshot_refresh, visible_mode,
        visible_ui_mode,
    };

    #[test]
    fn block_geometry_scales_with_the_crop_and_long_text_uses_a_readable_callout() {
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("short"),
                source: "Menu".to_owned(),
                translation: "Меню".to_owned(),
                bounds: local_rect(100, 50, 200, 80),
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("long"),
                source: "Long source".to_owned(),
                translation:
                    "Очень длинный перевод, который нельзя уменьшать до нечитаемого размера"
                        .to_owned(),
                bounds: local_rect(700, 400, 80, 24),
                confidence_milli: 500,
            },
        ];

        let laid_out = layout_blocks(crop_size(1_000, 500), 500, 250, &blocks);
        assert_eq!((laid_out[0].x, laid_out[0].y), (50.0, 25.0));
        assert_eq!((laid_out[0].width, laid_out[0].height), (100.0, 40.0));
        assert!(!laid_out[0].callout);
        assert!(laid_out[1].callout);
        assert!(laid_out[1].width >= 180.0);
        assert!(laid_out[1].x + laid_out[1].width <= 500.0);
        assert!(laid_out[1].low_confidence);
    }

    #[test]
    fn crop_local_blocks_do_not_subtract_the_physical_screen_origin() {
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("first"),
                source: "Focused source contracts: 61 pass, 1 Windows symlink EPERM skip"
                    .to_owned(),
                translation: "Сфокусированные исходные контракты: 61 прошли, 1 Windows symlink EPERM пропуск. Восстановление продукта, копирование и проверка только для чтения."
                    .to_owned(),
                bounds: local_rect(30, 13, 240, 120),
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("second"),
                source: "revalidated exact v3 lineage and its installed tree".to_owned(),
                translation: "Перепроверено точное происхождение v3, установленное дерево и пакет времени выполнения. Старый и новый контент остаются отдельной обязательной проверкой."
                    .to_owned(),
                bounds: local_rect(30, 133, 251, 117),
                confidence_milli: 900,
            },
        ];

        let laid_out = layout_blocks(crop_size(305, 255), 305, 255, &blocks);

        assert_eq!((laid_out[0].source_x, laid_out[0].source_y), (30.0, 13.0));
        assert_eq!((laid_out[1].source_x, laid_out[1].source_y), (30.0, 133.0));
        assert!(laid_out[0].source_y + laid_out[0].source_height <= laid_out[1].source_y);
        assert!(!overlaps(&laid_out[0], &laid_out[1]));
    }

    #[test]
    fn compact_results_are_wide_enough_for_the_complete_header() {
        let (width, height) = fitted_viewport(crop_size(305, 255));

        assert!(width >= 440);
        assert!(height >= 320);
    }

    #[test]
    fn adjacent_long_translations_receive_non_overlapping_callouts() {
        let long =
            "A translated paragraph that needs a readable callout instead of tiny text ".repeat(4);
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("first"),
                source: "first".to_owned(),
                translation: long.clone(),
                bounds: local_rect(20, 20, 70, 20),
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("second"),
                source: "second".to_owned(),
                translation: long,
                bounds: local_rect(20, 45, 70, 20),
                confidence_milli: 900,
            },
        ];

        let laid_out = layout_blocks(crop_size(440, 320), 440, 320, &blocks);

        assert!(laid_out.iter().all(|block| block.callout));
        assert!(!overlaps(&laid_out[0], &laid_out[1]));
    }

    #[test]
    fn dense_results_fall_back_to_one_ordered_scroll_panel() {
        let long = "A translated paragraph that cannot fit inside its source rectangle ".repeat(5);
        let blocks = (0..8)
            .map(|index| BlockLayoutInput {
                block_id: ScreenBlockId::from(format!("block-{index}")),
                source: format!("source {index}"),
                translation: long.clone(),
                bounds: local_rect(0, index * 40, 440, 36),
                confidence_milli: 900,
            })
            .collect::<Vec<_>>();

        let laid_out = layout_blocks(crop_size(440, 320), 440, 320, &blocks);

        assert!(laid_out.iter().all(|block| block.overflow_panel));
        assert_eq!(
            laid_out
                .iter()
                .map(|block| block.panel_order)
                .collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
        assert!(laid_out.iter().all(|block| block.width >= 180.0));
    }

    fn overlaps(left: &super::BlockLayout, right: &super::BlockLayout) -> bool {
        left.x < right.x + right.width
            && right.x < left.x + left.width
            && left.y < right.y + right.height
            && right.y < left.y + left.height
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

    #[test]
    fn screen_result_window_is_ordinary_until_the_user_pins_it() {
        let ordinary = ScreenResultWindowMode::new(false);
        assert!(!ordinary.topmost());

        let pinned = ScreenResultWindowMode::new(true);
        assert!(pinned.topmost());
    }

    #[test]
    fn snapshot_refresh_preserves_only_a_live_inline_editor() {
        assert!(preserve_editor_on_snapshot_refresh(true, true));
        assert!(!preserve_editor_on_snapshot_refresh(false, true));
        assert!(!preserve_editor_on_snapshot_refresh(true, false));
    }

    fn local_rect(x: i32, y: i32, width: u32, height: u32) -> CropLocalRect {
        CropLocalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn crop_size(width: u32, height: u32) -> CropPixelSize {
        CropPixelSize { width, height }
    }
}
