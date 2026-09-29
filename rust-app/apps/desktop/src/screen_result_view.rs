use rust_protocol::{BlockCompletion, PhysicalRect, ScreenBlockId};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{ScreenBlockOverlay, ScreenMaskOverlay, ScreenTranslationWindow};

const MIN_FONT_SIZE: f32 = 12.0;
const FONT_SIZES: [f32; 3] = [14.0, 13.0, MIN_FONT_SIZE];
const CALLOUT_GAP: f32 = 8.0;
const TEXT_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_VERTICAL_PADDING: f32 = 6.0;
const MAX_IMAGE_SCALE: f32 = 1.75;

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
    pub(crate) fragments: Vec<CropLocalRect>,
    pub(crate) complete: bool,
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
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
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
    pub(crate) low_confidence: bool,
    pub(crate) replace_source: bool,
    pub(crate) placed: bool,
    pub(crate) not_placeable: bool,
    pub(crate) font_size: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MaskLayout {
    pub(crate) block_id: ScreenBlockId,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScreenResultLayoutPlan {
    pub(crate) blocks: Vec<BlockLayout>,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) content_width: u32,
    pub(crate) masks: Vec<MaskLayout>,
    pub(crate) unplaceable_rows: Vec<ScreenBlockId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LogicalWorkArea {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LogicalTextSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

pub(crate) trait ScreenTextMeasurer {
    fn measure(&self, text: &str, width: f32, font_size: f32) -> LogicalTextSize;
}

struct SlintScreenTextMeasurer<'a> {
    window: &'a ScreenTranslationWindow,
}

impl ScreenTextMeasurer for SlintScreenTextMeasurer<'_> {
    fn measure(&self, text: &str, width: f32, font_size: f32) -> LogicalTextSize {
        self.window.set_measure_text(text.into());
        self.window.set_measure_width(width.max(1.0));
        self.window
            .set_measure_font_size(font_size.max(MIN_FONT_SIZE));
        LogicalTextSize {
            width: self.window.get_measured_text_width(),
            height: self.window.get_measured_text_height(),
        }
    }
}

#[cfg(test)]
struct ApproximateTextMeasurer;

#[cfg(test)]
impl ScreenTextMeasurer for ApproximateTextMeasurer {
    fn measure(&self, text: &str, width: f32, font_size: f32) -> LogicalTextSize {
        let glyph_width = font_size * 0.56;
        let natural_width = text.chars().count() as f32 * glyph_width;
        let lines = (natural_width / width.max(1.0)).ceil().max(1.0);
        LogicalTextSize {
            width: natural_width.min(width),
            height: lines * font_size * 1.35,
        }
    }
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

#[cfg(test)]
pub(crate) fn layout_blocks(
    crop: CropPixelSize,
    viewport_width: u32,
    viewport_height: u32,
    blocks: &[BlockLayoutInput],
) -> Vec<BlockLayout> {
    plan_screen_result_layout(
        crop,
        blocks,
        LogicalWorkArea {
            width: viewport_width as f32 + 32.0,
            height: viewport_height as f32 + 70.0,
        },
        &ApproximateTextMeasurer,
    )
    .blocks
}

pub(crate) fn plan_screen_result_layout(
    crop: CropPixelSize,
    blocks: &[BlockLayoutInput],
    work_area: LogicalWorkArea,
    measurer: &dyn ScreenTextMeasurer,
) -> ScreenResultLayoutPlan {
    let (image_width, image_height) = fitted_viewport(crop, work_area);
    if crop.width == 0 || crop.height == 0 || image_width == 0 || image_height == 0 {
        return ScreenResultLayoutPlan {
            blocks: Vec::new(),
            image_width,
            image_height,
            content_width: image_width,
            masks: Vec::new(),
            unplaceable_rows: Vec::new(),
        };
    }
    let scale_x = image_width as f32 / crop.width as f32;
    let scale_y = image_height as f32 / crop.height as f32;
    let viewport = (image_width as f32, image_height as f32);
    let scaled_fragments = blocks
        .iter()
        .map(|block| {
            let fragments = if block.fragments.is_empty() {
                std::slice::from_ref(&block.bounds)
            } else {
                block.fragments.as_slice()
            };
            fragments
                .iter()
                .map(|fragment| scale_rect(*fragment, scale_x, scale_y, viewport))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let occupied_source = scaled_fragments
        .iter()
        .enumerate()
        .flat_map(|(row, fragments)| fragments.iter().copied().map(move |rect| (row, rect)))
        .collect::<Vec<_>>();
    let mut order = (0..blocks.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        blocks[*left]
            .bounds
            .y
            .cmp(&blocks[*right].bounds.y)
            .then_with(|| blocks[*left].bounds.x.cmp(&blocks[*right].bounds.x))
    });
    let mut placed_rects = Vec::<(f32, f32, f32, f32)>::new();
    let mut layouts = blocks
        .iter()
        .map(|block| {
            let source = scale_rect(block.bounds, scale_x, scale_y, viewport);
            let translatable_source = block.source.chars().any(char::is_alphabetic);
            let replace_source = block.complete
                && translation_needs_visual_replacement(&block.source, &block.translation);
            BlockLayout {
                block_id: block.block_id.clone(),
                source: block.source.clone(),
                translation: block.translation.clone(),
                x: source.0,
                y: source.1,
                width: source.2,
                height: source.3,
                source_x: source.0,
                source_y: source.1,
                source_width: source.2,
                source_height: source.3,
                callout: false,
                low_confidence: block.confidence_milli < 650,
                replace_source,
                placed: false,
                not_placeable: !block.complete && translatable_source,
                font_size: MIN_FONT_SIZE,
            }
        })
        .collect::<Vec<_>>();
    let mut masks = Vec::new();
    let mut unplaceable_rows = layouts
        .iter()
        .filter(|layout| layout.not_placeable)
        .map(|layout| layout.block_id.clone())
        .collect::<Vec<_>>();

    for index in order {
        if !layouts[index].replace_source {
            continue;
        }
        let source = (
            layouts[index].source_x,
            layouts[index].source_y,
            layouts[index].source_width,
            layouts[index].source_height,
        );
        let candidate = direct_candidates(source, viewport)
            .into_iter()
            .flat_map(|rect| {
                FONT_SIZES
                    .into_iter()
                    .map(move |font_size| (rect, font_size, false))
            })
            .chain(
                callout_candidates(source, viewport)
                    .into_iter()
                    .flat_map(|rect| {
                        FONT_SIZES
                            .into_iter()
                            .map(move |font_size| (rect, font_size, true))
                    }),
            )
            .find_map(|(rect, font_size, callout)| {
                measured_candidate(
                    &layouts[index].translation,
                    rect,
                    font_size,
                    measurer,
                    viewport,
                )
                .filter(|candidate| {
                    let source_collision = occupied_source.iter().any(|(owner, occupied)| {
                        let own_direct_fragment = !callout && *owner == index;
                        !own_direct_fragment && rectangles_overlap(*candidate, *occupied)
                    });
                    !source_collision
                        && !placed_rects
                            .iter()
                            .any(|occupied| rectangles_overlap(*candidate, *occupied))
                })
                .map(|candidate| (candidate, font_size, callout))
            });
        if let Some((candidate, font_size, callout)) = candidate {
            let row = &mut layouts[index];
            row.x = candidate.0;
            row.y = candidate.1;
            row.width = candidate.2;
            row.height = candidate.3;
            row.font_size = font_size;
            row.callout = callout;
            row.placed = true;
            placed_rects.push(candidate);
            masks.extend(scaled_fragments[index].iter().map(|fragment| MaskLayout {
                block_id: row.block_id.clone(),
                x: fragment.0,
                y: fragment.1,
                width: fragment.2,
                height: fragment.3,
            }));
        } else {
            layouts[index].not_placeable = true;
            unplaceable_rows.push(layouts[index].block_id.clone());
        }
    }

    ScreenResultLayoutPlan {
        blocks: layouts,
        image_width,
        image_height,
        content_width: image_width,
        masks,
        unplaceable_rows,
    }
}

fn scale_rect(
    rect: CropLocalRect,
    scale_x: f32,
    scale_y: f32,
    viewport: (f32, f32),
) -> (f32, f32, f32, f32) {
    let x = (rect.x as f32 * scale_x).clamp(0.0, viewport.0);
    let y = (rect.y as f32 * scale_y).clamp(0.0, viewport.1);
    let width = (rect.width as f32 * scale_x)
        .max(1.0)
        .min((viewport.0 - x).max(0.0));
    let height = (rect.height as f32 * scale_y)
        .max(1.0)
        .min((viewport.1 - y).max(0.0));
    (x, y, width, height)
}

fn direct_candidates(
    source: (f32, f32, f32, f32),
    viewport: (f32, f32),
) -> Vec<(f32, f32, f32, f32)> {
    let mut candidates = Vec::new();
    let widths = [
        source.2,
        (source.2 * 1.35).max(120.0),
        (source.2 * 1.75).max(180.0),
        240.0,
        320.0,
    ];
    for width in widths {
        let width = width.min(viewport.0);
        for x in [
            source.0,
            source.0 + source.2 - width,
            source.0 + (source.2 - width) / 2.0,
        ] {
            candidates.push((
                x.clamp(0.0, (viewport.0 - width).max(0.0)),
                source.1,
                width,
                source.3,
            ));
        }
    }
    deduplicate_rects(candidates)
}

fn callout_candidates(
    source: (f32, f32, f32, f32),
    viewport: (f32, f32),
) -> Vec<(f32, f32, f32, f32)> {
    let mut candidates = Vec::new();
    for width in [180.0_f32, 240.0, 320.0, viewport.0 * 0.45] {
        let width = width.min(viewport.0);
        for x in [
            source.0 + source.2 + CALLOUT_GAP,
            source.0 - width - CALLOUT_GAP,
            source.0,
            (viewport.0 - width).max(0.0),
        ] {
            for y in [
                source.1,
                source.1 + source.3 + CALLOUT_GAP,
                source.1 - 48.0 - CALLOUT_GAP,
                0.0,
            ] {
                candidates.push((
                    x.clamp(0.0, (viewport.0 - width).max(0.0)),
                    y.clamp(0.0, viewport.1),
                    width,
                    viewport.1,
                ));
            }
        }
        let mut y = 0.0;
        while y < viewport.1 {
            candidates.push((0.0, y, width, viewport.1));
            candidates.push(((viewport.0 - width).max(0.0), y, width, viewport.1));
            y += 24.0;
        }
    }
    deduplicate_rects(candidates)
}

fn measured_candidate(
    text: &str,
    candidate: (f32, f32, f32, f32),
    font_size: f32,
    measurer: &dyn ScreenTextMeasurer,
    viewport: (f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let text_width = (candidate.2 - TEXT_HORIZONTAL_PADDING).max(1.0);
    let measured = measurer.measure(text, text_width, font_size);
    if !measured.width.is_finite() || !measured.height.is_finite() {
        return None;
    }
    let width = candidate.2.max(measured.width + TEXT_HORIZONTAL_PADDING);
    let height = measured.height + TEXT_VERTICAL_PADDING;
    let y = candidate.1.clamp(0.0, (viewport.1 - height).max(0.0));
    (width <= viewport.0
        && height <= viewport.1
        && candidate.0 + width <= viewport.0 + 0.5
        && y + height <= viewport.1 + 0.5)
        .then_some((candidate.0, y, width, height))
}

fn deduplicate_rects(candidates: Vec<(f32, f32, f32, f32)>) -> Vec<(f32, f32, f32, f32)> {
    let mut unique = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let duplicate = unique.iter().any(|existing: &(f32, f32, f32, f32)| {
            (existing.0 - candidate.0).abs() < 0.5
                && (existing.1 - candidate.1).abs() < 0.5
                && (existing.2 - candidate.2).abs() < 0.5
        });
        if !duplicate {
            unique.push(candidate);
        }
    }
    unique
}

fn rectangles_overlap(left: (f32, f32, f32, f32), right: (f32, f32, f32, f32)) -> bool {
    left.0 < right.0 + right.2
        && right.0 < left.0 + left.2
        && left.1 < right.1 + right.3
        && right.1 < left.1 + left.3
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
            fragments: block
                .fragments
                .iter()
                .copied()
                .map(CropLocalRect::from_ocr_bounds)
                .collect(),
            complete: block.completion == BlockCompletion::Completed
                && block.failure_message.is_none(),
            confidence_milli: block.confidence_milli,
        })
        .collect::<Vec<_>>();
    let work_area = rust_platform_windows::screen_result_logical_work_area((
        snapshot.crop_rect.x,
        snapshot.crop_rect.y,
        snapshot.crop_rect.width,
        snapshot.crop_rect.height,
    ))
    .map(|(width, height)| LogicalWorkArea { width, height })
    .unwrap_or(LogicalWorkArea {
        width: 1280.0,
        height: 720.0,
    });
    let plan = plan_screen_result_layout(
        crop_size,
        &inputs,
        work_area,
        &SlintScreenTextMeasurer { window: preview },
    );
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
            low_confidence: block.low_confidence,
            replace_source: block.replace_source,
            placed: block.placed,
            not_placeable: block.not_placeable,
            font_size: block.font_size,
        })
        .collect::<Vec<_>>();
    let masks = plan
        .masks
        .iter()
        .map(|mask| ScreenMaskOverlay {
            block_id: mask.block_id.as_str().into(),
            x: mask.x,
            y: mask.y,
            width: mask.width,
            height: mask.height,
        })
        .collect::<Vec<_>>();
    preview.set_screen_blocks(ModelRc::new(VecModel::from(rows)));
    preview.set_screen_masks(ModelRc::new(VecModel::from(masks)));
    preview.set_screen_image_content_width(plan.image_width as f32);
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

pub(crate) fn editor_position(
    window: &ScreenTranslationWindow,
    block_index: usize,
) -> Option<(f32, f32)> {
    let block = window.get_screen_blocks().row_data(block_index)?;
    let content_width = window.get_screen_content_width().max(1.0);
    let content_height = window.get_screen_content_height().max(1.0);
    let editor_width = 330.0_f32.min((content_width - 16.0).max(1.0));
    let editor_height = 282.0_f32.min((content_height - 16.0).max(1.0));
    let right_candidate = block.x + block.width + 8.0;
    let left_candidate = block.x - editor_width - 8.0;
    let x = if right_candidate + editor_width <= content_width {
        right_candidate
    } else {
        left_candidate
    }
    .clamp(8.0, (content_width - editor_width - 8.0).max(8.0));
    let y = block
        .y
        .clamp(8.0, (content_height - editor_height - 8.0).max(8.0));
    Some((x, y))
}

fn fitted_viewport(crop: CropPixelSize, work_area: LogicalWorkArea) -> (u32, u32) {
    if crop.width == 0 || crop.height == 0 {
        return (320, 200);
    }
    let scale = MAX_IMAGE_SCALE
        .min((work_area.width - 32.0).max(1.0) / crop.width as f32)
        .min((work_area.height - 70.0).max(1.0) / crop.height as f32)
        .max(0.01);
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
        BlockLayoutInput, CropLocalRect, CropPixelSize, LogicalTextSize, LogicalWorkArea,
        MIN_FONT_SIZE, ResultViewMode, ScreenResultWindowMode, ScreenTextMeasurer, fitted_viewport,
        layout_blocks, plan_screen_result_layout, preserve_editor_on_snapshot_refresh,
        translation_needs_visual_replacement, visible_mode, visible_ui_mode,
    };

    struct FakeMeasurer {
        height_multiplier: f32,
    }

    impl ScreenTextMeasurer for FakeMeasurer {
        fn measure(&self, text: &str, width: f32, font_size: f32) -> LogicalTextSize {
            let glyph_width = font_size * 0.56;
            let lines = ((text.chars().count() as f32 * glyph_width) / width.max(1.0))
                .ceil()
                .max(1.0);
            LogicalTextSize {
                width: width.min(text.chars().count() as f32 * glyph_width),
                height: lines * font_size * 1.35 * self.height_multiplier,
            }
        }
    }

    fn normal_measurer() -> FakeMeasurer {
        FakeMeasurer {
            height_multiplier: 1.0,
        }
    }

    #[test]
    fn block_geometry_scales_with_the_crop_and_long_text_uses_a_readable_callout() {
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("short"),
                source: "Menu".to_owned(),
                translation: "Меню".to_owned(),
                bounds: local_rect(100, 50, 200, 80),
                fragments: vec![local_rect(100, 50, 200, 80)],
                complete: true,
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("long"),
                source: "Long source".to_owned(),
                translation:
                    "Очень длинный перевод, который нельзя уменьшать до нечитаемого размера"
                        .to_owned(),
                bounds: local_rect(700, 400, 80, 24),
                fragments: vec![local_rect(700, 400, 80, 24)],
                complete: true,
                confidence_milli: 500,
            },
        ];

        let laid_out = layout_blocks(crop_size(1_000, 500), 500, 250, &blocks);
        assert_eq!((laid_out[0].x, laid_out[0].y), (50.0, 25.0));
        assert_eq!(
            (laid_out[0].source_width, laid_out[0].source_height),
            (100.0, 40.0)
        );
        assert_eq!(laid_out[0].width, 100.0);
        assert!(laid_out[0].height >= MIN_FONT_SIZE);
        assert!(!laid_out[0].callout);
        assert!(laid_out[1].placed);
        assert!(laid_out[1].font_size >= MIN_FONT_SIZE);
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
                fragments: vec![local_rect(30, 13, 240, 120)],
                complete: true,
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("second"),
                source: "revalidated exact v3 lineage and its installed tree".to_owned(),
                translation: "Перепроверено точное происхождение v3, установленное дерево и пакет времени выполнения. Старый и новый контент остаются отдельной обязательной проверкой."
                    .to_owned(),
                bounds: local_rect(30, 133, 251, 117),
                fragments: vec![local_rect(30, 133, 251, 117)],
                complete: true,
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
    fn result_viewport_respects_the_monitor_work_area() {
        let (width, height) = fitted_viewport(
            crop_size(305, 255),
            LogicalWorkArea {
                width: 1280.0,
                height: 720.0,
            },
        );

        assert!(width <= 1248);
        assert!(height <= 650);
        assert_eq!((width, height), (534, 446));
    }

    #[test]
    fn adjacent_long_translations_never_overlap_or_hide_unplaceable_source() {
        let long =
            "A translated paragraph that needs a readable callout instead of tiny text ".repeat(4);
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("first"),
                source: "first".to_owned(),
                translation: long.clone(),
                bounds: local_rect(20, 20, 70, 20),
                fragments: vec![local_rect(20, 20, 70, 20)],
                complete: true,
                confidence_milli: 900,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("second"),
                source: "second".to_owned(),
                translation: long,
                bounds: local_rect(20, 45, 70, 20),
                fragments: vec![local_rect(20, 45, 70, 20)],
                complete: true,
                confidence_milli: 900,
            },
        ];

        let laid_out = layout_blocks(crop_size(440, 320), 440, 320, &blocks);

        assert!(
            laid_out
                .iter()
                .all(|block| block.placed || block.not_placeable)
        );
        assert!(
            laid_out
                .iter()
                .filter(|block| block.placed)
                .all(|block| block.font_size >= MIN_FONT_SIZE)
        );
        assert!(!overlaps(&laid_out[0], &laid_out[1]));
    }

    #[test]
    fn dense_results_report_unplaceable_rows_without_masking_them() {
        let long = "A translated paragraph that cannot fit inside its source rectangle ".repeat(5);
        let blocks = (0..8)
            .map(|index| BlockLayoutInput {
                block_id: ScreenBlockId::from(format!("block-{index}")),
                source: format!("source {index}"),
                translation: long.clone(),
                bounds: local_rect(0, index * 40, 440, 36),
                fragments: vec![local_rect(0, index * 40, 440, 36)],
                complete: true,
                confidence_milli: 900,
            })
            .collect::<Vec<_>>();

        let laid_out = layout_blocks(crop_size(440, 320), 440, 320, &blocks);

        assert!(laid_out.iter().any(|block| block.not_placeable));
        assert!(
            laid_out
                .iter()
                .all(|block| block.placed || block.not_placeable)
        );
    }

    #[test]
    fn dense_results_never_create_an_external_translation_column() {
        let long = "A translated paragraph that cannot fit inside its source rectangle ".repeat(5);
        let blocks = (0..8)
            .map(|index| BlockLayoutInput {
                block_id: ScreenBlockId::from(format!("block-{index}")),
                source: format!("source {index}"),
                translation: long.clone(),
                bounds: local_rect(0, index * 40, 440, 36),
                fragments: vec![local_rect(0, index * 40, 440, 36)],
                complete: true,
                confidence_milli: 900,
            })
            .collect::<Vec<_>>();

        let plan = plan_screen_result_layout(
            crop_size(440, 320),
            &blocks,
            LogicalWorkArea {
                width: 472.0,
                height: 390.0,
            },
            &normal_measurer(),
        );

        assert_eq!(plan.content_width, plan.image_width);
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.placed)
                .all(|block| block.x + block.width <= plan.image_width as f32)
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
                    fragments: vec![CropLocalRect {
                        x,
                        y,
                        width,
                        height,
                    }],
                    complete: true,
                    confidence_milli: 980,
                },
            )
            .collect()
    }

    #[test]
    fn nimbus_capture_masks_only_detected_text_rows_and_keeps_numeric_icon_values() {
        let inputs = nimbus_layout_inputs();

        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            LogicalWorkArea {
                width: 1280.0,
                height: 900.0,
            },
            &normal_measurer(),
        );
        let masked_area = plan
            .masks
            .iter()
            .map(|mask| mask.width * mask.height)
            .sum::<f32>();

        assert_eq!(plan.blocks.len(), 16);
        assert_eq!(plan.content_width, plan.image_width);
        assert!(masked_area < plan.image_width as f32 * plan.image_height as f32 * 0.25);
        for value in ["30", "45", "275"] {
            let block = plan
                .blocks
                .iter()
                .find(|block| block.source.trim() == value)
                .unwrap();
            assert!(!block.replace_source, "{value} must remain in the image");
        }
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.replace_source)
                .all(|block| block.placed || block.not_placeable)
        );
        assert!(
            plan.masks
                .iter()
                .all(|mask| mask.block_id.as_str() != "nimbus-13")
        );
        assert!(
            plan.masks
                .iter()
                .all(|mask| mask.block_id.as_str() != "nimbus-14")
        );
    }

    #[test]
    fn nimbus_renderer_measurement_can_refuse_a_visually_oversized_translation() {
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &nimbus_layout_inputs(),
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &FakeMeasurer {
                height_multiplier: 20.0,
            },
        );

        assert!(plan.blocks.iter().any(|block| block.not_placeable));
        for block in plan.blocks.iter().filter(|block| block.not_placeable) {
            assert!(
                plan.masks
                    .iter()
                    .all(|mask| mask.block_id != block.block_id)
            );
        }
    }

    #[test]
    fn nimbus_small_work_area_reports_unplaceable_rows_without_side_panel_or_clipping() {
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &nimbus_layout_inputs(),
            LogicalWorkArea {
                width: 360.0,
                height: 280.0,
            },
            &normal_measurer(),
        );

        assert_eq!(plan.content_width, plan.image_width);
        assert!(!plan.unplaceable_rows.is_empty());
        assert!(plan.blocks.iter().filter(|row| row.placed).all(|row| {
            row.x >= 0.0
                && row.y >= 0.0
                && row.x + row.width <= plan.image_width as f32
                && row.y + row.height <= plan.image_height as f32
        }));
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
        super::apply_snapshot(&preview, &snapshot).expect("snapshot should bind to the real view");
        preview
            .show()
            .expect("preview should show on testing backend");
        preview.window().set_size(LogicalSize::new(
            preview.get_screen_content_width(),
            preview.get_screen_content_height() + 38.0,
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
