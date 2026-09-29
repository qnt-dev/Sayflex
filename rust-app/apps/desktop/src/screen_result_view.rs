use rust_protocol::{PhysicalRect, ScreenBlockId};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{ScreenBlockOverlay, ScreenTranslationWindow};

const MIN_CALLOUT_WIDTH: f32 = 180.0;
const MAX_CALLOUT_WIDTH: f32 = 320.0;
// 12 logical pixels is the smallest overlay font admitted by the Slint view.
// Reserve a full 18 pixels per wrapped line so measured glyphs and line
// spacing cannot spill into the next callout on high-DPI displays.
const MIN_READABLE_LINE_HEIGHT: f32 = 18.0;
const APPROXIMATE_GLYPH_WIDTH: f32 = 7.0;
const CALLOUT_GAP: f32 = 8.0;
const DENSE_PANEL_MIN_WIDTH: u32 = 220;
const DENSE_PANEL_MAX_WIDTH: u32 = 320;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultViewMode {
    Translation,
    Original,
    Dual,
    OnImage,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenResultWindowMode {
    pinned: bool,
}

#[cfg(test)]
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
    pub(crate) low_confidence: bool,
    pub(crate) replace_source: bool,
    /// The source pixels may be covered only when the translated text is
    /// rendered on the image. External-panel rows keep the screenshot intact.
    pub(crate) mask_source: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScreenResultLayoutPlan {
    pub(crate) blocks: Vec<BlockLayout>,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) content_width: u32,
    pub(crate) panel_width: u32,
    pub(crate) dense_panel_active: bool,
}

pub(crate) fn translation_needs_visual_replacement(source: &str, translation: &str) -> bool {
    let source = source.trim();
    let translation = translation.trim();
    if source.is_empty()
        || translation.is_empty()
        || source.to_lowercase() == translation.to_lowercase()
    {
        return false;
    }
    source.chars().any(char::is_alphabetic)
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
            let replace_source =
                translation_needs_visual_replacement(&block.source, &block.translation);
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
            let callout = replace_source && block.translation.chars().count() > capacity;

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
                low_confidence: block.confidence_milli < 650,
                replace_source,
                mask_source: replace_source,
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
            assign_overflow_panel(layouts, viewport_width);
            return;
        }
    }
}

fn assign_overflow_panel(layouts: &mut [BlockLayout], viewport_width: f32) {
    let panel_width = (viewport_width * 0.48)
        .clamp(MIN_CALLOUT_WIDTH, MAX_CALLOUT_WIDTH)
        .min(viewport_width);
    let panel_x = (viewport_width - panel_width - CALLOUT_GAP).max(0.0);
    let mut order = layouts
        .iter()
        .enumerate()
        .filter(|(_, block)| block.replace_source)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    order.sort_by(|left, right| {
        layouts[*left]
            .source_y
            .total_cmp(&layouts[*right].source_y)
            .then_with(|| layouts[*left].source_x.total_cmp(&layouts[*right].source_x))
    });
    for (panel_order, index) in order.into_iter().enumerate() {
        let block = &mut layouts[index];
        block.overflow_panel = true;
        block.mask_source = false;
        block.panel_order = i32::try_from(panel_order).unwrap_or(i32::MAX);
        block.x = panel_x;
        block.y = block.source_y;
        block.width = panel_width;
    }
}

pub(crate) fn plan_screen_result_layout(
    crop: CropPixelSize,
    blocks: &[BlockLayoutInput],
) -> ScreenResultLayoutPlan {
    let (image_width, image_height) = fitted_viewport(crop);
    let mut layouts = layout_blocks(crop, image_width, image_height, blocks);
    if !layouts.iter().any(|block| block.overflow_panel) {
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
        for (panel_order, (index, _, _)) in callout_order.into_iter().enumerate() {
            let block = &mut layouts[index];
            block.overflow_panel = true;
            block.mask_source = false;
            block.panel_order = i32::try_from(panel_order).unwrap_or(i32::MAX);
        }
    }
    let dense_panel_active = layouts.iter().any(|block| block.overflow_panel);
    let panel_width = if dense_panel_active {
        (image_width * 2 / 5).clamp(DENSE_PANEL_MIN_WIDTH, DENSE_PANEL_MAX_WIDTH)
    } else {
        0
    };
    if dense_panel_active {
        let panel_x = image_width.saturating_add(CALLOUT_GAP as u32) as f32;
        for block in layouts.iter_mut().filter(|block| block.overflow_panel) {
            block.x = panel_x;
            block.width = panel_width as f32;
        }
    }
    let content_width = image_width.saturating_add(if dense_panel_active {
        CALLOUT_GAP as u32 + panel_width
    } else {
        0
    });
    ScreenResultLayoutPlan {
        blocks: layouts,
        image_width,
        image_height,
        content_width,
        panel_width,
        dense_panel_active,
    }
}

fn rectangles_overlap(left: (f32, f32, f32, f32), right: (f32, f32, f32, f32)) -> bool {
    left.0 < right.0 + right.2
        && right.0 < left.0 + left.2
        && left.1 < right.1 + right.3
        && right.1 < left.1 + left.3
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
    preview: &ScreenTranslationWindow,
    snapshot: &ScreenResultSnapshot,
) -> Result<Vec<BlockLayout>, String> {
    let crop_size = CropPixelSize::from_screen_rect(snapshot.crop_rect);
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
    let plan = plan_screen_result_layout(crop_size, &inputs);
    let layouts = &plan.blocks;
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
            low_confidence: block.low_confidence,
            replace_source: block.replace_source,
            mask_source: block.mask_source,
        })
        .collect::<Vec<_>>();
    preview.set_screen_blocks(ModelRc::new(VecModel::from(rows)));
    preview.set_screen_dense_panel_active(plan.dense_panel_active);
    preview.set_screen_image_content_width(plan.image_width as f32);
    preview.set_screen_dense_panel_width(plan.panel_width as f32);
    preview.set_screen_content_width(plan.content_width as f32);
    preview.set_screen_content_height(plan.image_height as f32);
    preview.set_screen_image_expired(snapshot.crop.is_none());
    preview.set_screen_view_mode(visible_ui_mode(3, snapshot.crop.is_some()));
    preview.set_screen_selected_block(-1);
    preview.set_screen_editor_visible(false);
    preview.set_screen_save_error(false);
    preview.set_screen_crop_image(match snapshot.crop.as_deref() {
        Some(frame) => frame_image(frame)?,
        None => Image::default(),
    });
    Ok(layouts.to_vec())
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
        fitted_viewport, layout_blocks, plan_screen_result_layout,
        preserve_editor_on_snapshot_refresh, translation_needs_visual_replacement, visible_mode,
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

    #[test]
    fn dense_panel_is_reserved_beside_the_image_instead_of_covering_it() {
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

        let plan = plan_screen_result_layout(crop_size(440, 320), &blocks);

        assert!(plan.dense_panel_active);
        assert!(plan.content_width > plan.image_width);
        assert!(plan.panel_width >= 180);
        assert!(
            plan.blocks
                .iter()
                .all(|block| !block.overflow_panel || block.x >= plan.image_width as f32 + 8.0)
        );
    }

    #[test]
    fn standalone_numbers_and_unchanged_text_remain_visible_from_the_original_image() {
        assert!(!translation_needs_visual_replacement("45", "45"));
        assert!(!translation_needs_visual_replacement("275", "275"));
        assert!(!translation_needs_visual_replacement(
            "https://example.com",
            "https://example.com"
        ));
        assert!(translation_needs_visual_replacement(
            "RADIUS: 450",
            "РАДИУС: 450"
        ));
    }

    fn nimbus_layout_inputs() -> Vec<BlockLayoutInput> {
        let rows = [
            ("NIMBUS", "НИМБУС", 140, 23, 111, 30),
            (
                "SCEPTER GRANTS NEW ABILITY",
                "СКИПЕТР ДАЁТ НОВУЮ СПОСОБНОСТЬ",
                145,
                56,
                254,
                28,
            ),
            (
                "Creates a storm cloud anywhere on the map that automatically casts",
                "Создаёт грозовое облако в выбранной точке карты",
                141,
                84,
                511,
                26,
            ),
            (
                "Lightning Bolt on nearby enemies.",
                "Оно автоматически выпускает молнии в ближайших врагов.",
                142,
                106,
                259,
                26,
            ),
            ("ABILITY: Point Target", "ЦЕЛЬ: точка", 34, 166, 143, 20),
            (
                "PIERCES SPELL IMMUNITY: No",
                "ПРОБИВАЕТ НЕВОСПРИИМЧИВОСТЬ: нет",
                338,
                166,
                207,
                20,
            ),
            (
                "DAMAGE TYPE: Magical",
                "ТИП УРОНА: магический",
                34,
                187,
                161,
                19,
            ),
            ("DURATION:", "ДЛИТЕЛЬНОСТЬ:", 34, 228, 88, 19),
            ("30", "30", 116, 229, 25, 16),
            (
                "LIGHTNING BOLT BASE COOLDOWN: 2.5",
                "БАЗОВАЯ ПЕРЕЗАРЯДКА МОЛНИИ: 2.5",
                34,
                252,
                284,
                19,
            ),
            ("RADIUS: 450", "РАДИУС: 450", 34, 276, 92, 20),
            (
                "HITS TO DESTROY: 8",
                "УДАРОВ ДЛЯ УНИЧТОЖЕНИЯ: 8",
                35,
                301,
                148,
                17,
            ),
            (
                "CREEP HITS TO DESTROY: 16",
                "УДАРОВ КРИПОВ ДЛЯ УНИЧТОЖЕНИЯ: 16",
                35,
                324,
                202,
                20,
            ),
            ("45", "45", 60, 362, 22, 17),
            ("275", "275", 619, 363, 29, 17),
            (
                "Where the thunder god's ire grows, storm clouds quickly gather.",
                "Там, где растёт гнев бога грома, быстро собираются грозовые облака.",
                47,
                408,
                371,
                23,
            ),
        ];
        rows.into_iter()
            .enumerate()
            .map(
                |(index, (source, translation, x, y, width, height))| BlockLayoutInput {
                    block_id: ScreenBlockId::from(format!("nimbus-{index}")),
                    source: source.to_owned(),
                    translation: translation.to_owned(),
                    bounds: CropLocalRect {
                        x,
                        y,
                        width,
                        height,
                    },
                    confidence_milli: 980,
                },
            )
            .collect()
    }

    #[test]
    fn nimbus_capture_masks_only_detected_text_rows_and_keeps_numeric_icon_values() {
        let inputs = nimbus_layout_inputs();

        let plan = plan_screen_result_layout(crop_size(682, 466), &inputs);
        let masked_area = plan
            .blocks
            .iter()
            .filter(|block| block.mask_source)
            .map(|block| block.source_width * block.source_height)
            .sum::<f32>();

        assert_eq!(plan.blocks.len(), 16);
        assert!(plan.dense_panel_active);
        assert!(masked_area < 682.0 * 466.0 * 0.25);
        for value in ["30", "45", "275"] {
            let block = plan
                .blocks
                .iter()
                .find(|block| block.source.trim() == value)
                .unwrap();
            assert!(!block.replace_source, "{value} must remain in the image");
            assert!(!block.overflow_panel);
        }
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.overflow_panel)
                .all(|block| block.x >= plan.image_width as f32 + 8.0)
        );
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.overflow_panel)
                .all(|block| !block.mask_source),
            "translations rendered in the external panel must not erase source pixels"
        );
    }

    #[test]
    fn render_recorded_nimbus_result_when_requested() {
        use std::{fs, io::BufReader, io::BufWriter, sync::Arc};

        use rust_platform_windows::screen_capture::RgbaFrame;
        use rust_protocol::{BlockCompletion, PhysicalRect, RequestId, SourceContext};
        use slint::{ComponentHandle, LogicalSize};

        let Some(sample_path) = std::env::var_os("SAYFLEX_SCREEN_SAMPLE_PNG") else {
            return;
        };
        let Some(output_dir) = std::env::var_os("SAYFLEX_SCREEN_SAMPLE_OUTPUT") else {
            return;
        };
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .expect("testing platform should initialize once");

        let sample_path = std::path::Path::new(&sample_path);
        let decoder = png::Decoder::new(BufReader::new(
            fs::File::open(sample_path).expect("sample PNG should open"),
        ));
        let mut reader = decoder.read_info().expect("sample PNG should decode");
        let mut pixels = vec![0_u8; reader.output_buffer_size().unwrap()];
        let info = reader
            .next_frame(&mut pixels)
            .expect("sample PNG should contain one frame");
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        pixels.truncate(info.buffer_size());
        let crop_rect = PhysicalRect {
            x: 0,
            y: 0,
            width: info.width,
            height: info.height,
        };
        let frame = RgbaFrame::try_new(crop_rect, info.width * 4, pixels)
            .expect("sample pixels should form a capture frame");
        let inputs = nimbus_layout_inputs();
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: RequestId::new(),
            revision: 1,
            crop: Some(Arc::new(frame)),
            crop_rect,
            blocks: inputs
                .into_iter()
                .map(|input| crate::screen_result::ScreenResultBlockSnapshot {
                    block_id: input.block_id,
                    source: input.source,
                    translation: input.translation,
                    completion: BlockCompletion::Completed,
                    failure_message: None,
                    bounds: PhysicalRect {
                        x: input.bounds.x,
                        y: input.bounds.y,
                        width: input.bounds.width,
                        height: input.bounds.height,
                    },
                    fragments: vec![PhysicalRect {
                        x: input.bounds.x,
                        y: input.bounds.y,
                        width: input.bounds.width,
                        height: input.bounds.height,
                    }],
                    confidence_milli: input.confidence_milli,
                })
                .collect(),
            context: SourceContext {
                application_id: Some("dota2".to_owned()),
                nearby_text: Some("Nimbus item card".to_owned()),
                ..SourceContext::default()
            },
            created_unix_ms: 1,
            expires_unix_ms: u64::MAX,
        };

        let preview = crate::ScreenTranslationWindow::new().expect("preview should instantiate");
        let plan = super::plan_screen_result_layout(crop_size(682, 466), &nimbus_layout_inputs());
        super::apply_snapshot(&preview, &snapshot).expect("snapshot should bind to the real view");
        preview
            .show()
            .expect("preview should show on testing backend");
        preview.window().set_size(LogicalSize::new(
            plan.content_width as f32,
            plan.image_height as f32 + 38.0,
        ));
        let rendered = preview
            .window()
            .take_snapshot()
            .expect("screen result should render");

        let output_dir = std::path::Path::new(&output_dir);
        fs::create_dir_all(output_dir).expect("sample output directory should be creatable");
        fs::copy(sample_path, output_dir.join("nimbus-original.png"))
            .expect("original evidence should copy");
        let file = fs::File::create(output_dir.join("nimbus-result.png"))
            .expect("result evidence should be creatable");
        let mut encoder =
            png::Encoder::new(BufWriter::new(file), rendered.width(), rendered.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("result PNG header should be valid")
            .write_image_data(rendered.as_bytes())
            .expect("result PNG should be written");
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
