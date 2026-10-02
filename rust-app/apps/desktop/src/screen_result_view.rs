use rust_protocol::{BlockCompletion, PhysicalRect, ScreenBlockId};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{
    ScreenBlockOverlay, ScreenConnectorOverlay, ScreenMaskOverlay, ScreenTranslationWindow,
};

const MIN_FONT_SIZE: f32 = 12.0;
const BODY_FONT_SIZE: f32 = 14.0;
type LayoutRect = (f32, f32, f32, f32);
const TEXT_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_VERTICAL_PADDING: f32 = 2.0;
const MAX_IMAGE_SCALE: f32 = 1.0;
const MAX_VERTICAL_NUDGE: f32 = 10.0;

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
    pub(crate) display_translation: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) source_x: f32,
    pub(crate) source_y: f32,
    pub(crate) source_width: f32,
    pub(crate) source_height: f32,
    pub(crate) low_confidence: bool,
    pub(crate) replace_source: bool,
    pub(crate) placed: bool,
    pub(crate) callout: bool,
    pub(crate) not_placeable: bool,
    pub(crate) editable_source: bool,
    pub(crate) overflow: bool,
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
pub(crate) struct ConnectorLayout {
    pub(crate) block_id: ScreenBlockId,
    pub(crate) start_x: f32,
    pub(crate) start_y: f32,
    pub(crate) end_x: f32,
    pub(crate) end_y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScreenResultLayoutPlan {
    pub(crate) blocks: Vec<BlockLayout>,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) content_width: u32,
    pub(crate) content_height: u32,
    pub(crate) viewport_height: u32,
    pub(crate) masks: Vec<MaskLayout>,
    pub(crate) connectors: Vec<ConnectorLayout>,
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
        ResultViewMode::OnImage,
        LogicalWorkArea {
            width: viewport_width as f32 + 32.0,
            height: viewport_height as f32 + 70.0,
        },
        &ApproximateTextMeasurer,
    )
    .blocks
}

#[cfg(test)]
pub(crate) fn plan_screen_result_layout(
    crop: CropPixelSize,
    blocks: &[BlockLayoutInput],
    projection: ResultViewMode,
    work_area: LogicalWorkArea,
    measurer: &dyn ScreenTextMeasurer,
) -> ScreenResultLayoutPlan {
    plan_screen_result_layout_with_frame(crop, blocks, projection, work_area, measurer, None)
}

pub(crate) fn plan_screen_result_layout_with_frame(
    crop: CropPixelSize,
    blocks: &[BlockLayoutInput],
    projection: ResultViewMode,
    work_area: LogicalWorkArea,
    measurer: &dyn ScreenTextMeasurer,
    _frame: Option<&rust_platform_windows::screen_capture::RgbaFrame>,
) -> ScreenResultLayoutPlan {
    let (image_width, image_height) = fitted_viewport(crop, work_area);
    let mut plan = ScreenResultLayoutPlan {
        blocks: Vec::new(),
        image_width,
        image_height,
        content_width: image_width,
        content_height: image_height,
        viewport_height: image_height,
        masks: Vec::new(),
        connectors: Vec::new(),
        unplaceable_rows: Vec::new(),
    };
    if crop.width == 0 || crop.height == 0 || image_width == 0 || image_height == 0 {
        return plan;
    }
    let scale_x = image_width as f32 / crop.width as f32;
    let scale_y = image_height as f32 / crop.height as f32;
    let viewport = (image_width as f32, image_height as f32);
    let overlay_mode = matches!(projection, ResultViewMode::OnImage | ResultViewMode::Dual);
    for (index, block) in blocks.iter().enumerate() {
        let source = scale_rect(block.bounds, scale_x, scale_y, viewport);
        let replace_source = overlay_mode
            && block.complete
            && translation_needs_visual_replacement(&block.source, &block.translation);
        let mut row = BlockLayout {
            block_id: block.block_id.clone(),
            source: block.source.clone(),
            translation: block.translation.clone(),
            display_translation: block.translation.clone(),
            x: source.0,
            y: source.1,
            width: source.2,
            height: source.3,
            source_x: source.0,
            source_y: source.1,
            source_width: source.2,
            source_height: source.3,
            low_confidence: block.confidence_milli < 650,
            replace_source,
            placed: false,
            callout: false,
            not_placeable: overlay_mode
                && !block.complete
                && block.source.chars().any(char::is_alphabetic),
            editable_source: overlay_mode
                && block.complete
                && !replace_source
                && block.source.chars().any(char::is_alphabetic),
            overflow: false,
            font_size: MIN_FONT_SIZE,
        };
        if replace_source {
            let preferred = preferred_font_size(block, scale_y);
            let adjacent_value = adjacent_unchanged_value(block, blocks.get(index + 1))
                .map(|value| (value, &blocks[index + 1]));
            let value_rect = adjacent_value
                .map(|(_, value)| scale_rect(value.bounds, scale_x, scale_y, viewport));
            let text_source = value_rect.map_or(source, |value| {
                (
                    source.0,
                    source.1,
                    source.2.min((value.0 - source.0).max(1.0)),
                    source.3,
                )
            });
            let (mut rect, mut font_size, mut fitted) = in_place_candidate(
                &projected_text(&row, projection),
                text_source,
                viewport,
                preferred,
                measurer,
            );
            // Keep a nearby unchanged number in the screenshot when the label
            // leaves it visible. If an edit already includes that value, mask
            // the original even when the shorter label does not overlap it.
            if let (Some((value, value_block)), Some(value_rect)) = (adjacent_value, value_rect) {
                let overlaps_value = rectangles_overlap(rect, value_rect);
                let includes_value = translation_ends_with_value(&block.translation, value);
                if overlaps_value && !includes_value {
                    row.display_translation = format!("{} {value}", block.translation);
                    (rect, font_size, fitted) = in_place_candidate(
                        &projected_text(&row, projection),
                        text_source,
                        viewport,
                        preferred,
                        measurer,
                    );
                }
                if overlaps_value || includes_value {
                    plan.masks.push(MaskLayout {
                        block_id: value_block.block_id.clone(),
                        x: value_rect.0,
                        y: value_rect.1,
                        width: value_rect.2,
                        height: value_rect.3,
                    });
                }
            }
            if let Some(lower_edge) = plan
                .blocks
                .iter()
                .filter(|prior| prior.placed && horizontal_overlap(rect, prior))
                .filter(|prior| rect.1 < prior.y + prior.height && rect.1 + rect.3 > prior.y)
                .map(|prior| prior.y + prior.height + 2.0)
                .max_by(f32::total_cmp)
                && lower_edge + rect.3 <= viewport.1
                && lower_edge - source.1 <= MAX_VERTICAL_NUDGE
            {
                rect.1 = lower_edge;
            }
            row.x = rect.0;
            row.y = rect.1;
            row.width = rect.2;
            row.height = rect.3;
            row.font_size = font_size;
            row.placed = true;
            row.overflow = !fitted;

            // Preserve the original glyph masks even if the new phrase is
            // shorter. The label paints its own measured, local underlay.
            let fragments = if block.fragments.is_empty() {
                std::slice::from_ref(&block.bounds)
            } else {
                block.fragments.as_slice()
            };
            plan.masks.extend(fragments.iter().map(|fragment| {
                let mut rect = scale_rect(*fragment, scale_x, scale_y, viewport);
                if row.display_translation == row.translation
                    && let Some(value) = value_rect
                    && rect.0 < value.0
                {
                    rect.2 = rect.2.min((value.0 - rect.0).max(0.0));
                }
                MaskLayout {
                    block_id: block.block_id.clone(),
                    x: rect.0,
                    y: rect.1,
                    width: rect.2,
                    height: rect.3,
                }
            }));
        }
        if row.not_placeable {
            plan.unplaceable_rows.push(row.block_id.clone());
        }
        plan.blocks.push(row);
    }
    plan
}

fn adjacent_unchanged_value<'a>(
    label: &BlockLayoutInput,
    next: Option<&'a BlockLayoutInput>,
) -> Option<&'a str> {
    let value = next?;
    if !label.source.trim_end().ends_with(':')
        || !value.complete
        || value.source != value.translation
        || value.source.is_empty()
        || !value
            .source
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
    {
        return None;
    }
    let label_right = label.bounds.x + label.bounds.width as i32;
    let gap = value.bounds.x - label_right;
    let baseline = value.bounds.y - label.bounds.y;
    (-8..=20).contains(&gap).then_some(())?;
    (-6..=6)
        .contains(&baseline)
        .then_some(value.source.as_str())
}

fn translation_ends_with_value(translation: &str, value: &str) -> bool {
    let Some(prefix) = translation.trim_end().strip_suffix(value) else {
        return false;
    };
    prefix.chars().last().is_none_or(|character| {
        character.is_whitespace() || matches!(character, ':' | '=' | '(' | '[')
    })
}

fn horizontal_overlap(rect: LayoutRect, prior: &BlockLayout) -> bool {
    rect.0 < prior.x + prior.width && rect.0 + rect.2 > prior.x
}

fn rectangles_overlap(left: LayoutRect, right: LayoutRect) -> bool {
    left.0 < right.0 + right.2
        && left.0 + left.2 > right.0
        && left.1 < right.1 + right.3
        && left.1 + left.3 > right.1
}

fn preferred_font_size(block: &BlockLayoutInput, scale: f32) -> f32 {
    let heading = block.source.chars().any(char::is_alphabetic)
        && block
            .source
            .chars()
            .filter(|character| character.is_alphabetic())
            .all(char::is_uppercase)
        && block.source.chars().count() <= 36
        && block.bounds.width >= 100
        && block.bounds.height >= 27;
    if heading {
        (20.0 * scale).max(MIN_FONT_SIZE)
    } else {
        (BODY_FONT_SIZE * scale).max(MIN_FONT_SIZE)
    }
}

/// Prefer the source rectangle, then grow beside it. OCR rectangles may
/// overlap already; another row is not a reason to detach this translation.
fn in_place_candidate(
    text: &str,
    source: LayoutRect,
    viewport: (f32, f32),
    preferred_font_size: f32,
    measurer: &dyn ScreenTextMeasurer,
) -> (LayoutRect, f32, bool) {
    let font_sizes = [
        preferred_font_size,
        (preferred_font_size - 2.0).max(MIN_FONT_SIZE),
        BODY_FONT_SIZE.min(preferred_font_size),
        MIN_FONT_SIZE,
    ];
    for font_size in font_sizes {
        if let Some(rect) = measured_candidate(text, source, font_size, measurer, viewport) {
            return (rect, font_size, true);
        }
    }

    let remaining = (
        (viewport.0 - source.0).max(1.0),
        (viewport.1 - source.1).max(1.0),
    );
    for font_size in font_sizes {
        if let Some(rect) = grown_candidate(text, source, remaining, font_size, measurer) {
            return (rect, font_size, true);
        }
    }

    // At an image edge, move only as far as required to keep the complete
    // measured label inside the image; it remains over its source region.
    for font_size in font_sizes {
        if let Some(mut rect) = grown_candidate(text, source, viewport, font_size, measurer) {
            rect.0 = source.0.min((viewport.0 - rect.2).max(0.0));
            rect.1 = source.1.min((viewport.1 - rect.3).max(0.0));
            return (rect, font_size, true);
        }
    }

    // A pathological paragraph can exceed the entire image at the readable
    // minimum. Clip its local label, while retaining the complete string for
    // the editor and text projection. Never create an external callout.
    (
        (source.0, source.1, remaining.0, remaining.1),
        MIN_FONT_SIZE,
        false,
    )
}

fn grown_candidate(
    text: &str,
    source: LayoutRect,
    available: (f32, f32),
    font_size: f32,
    measurer: &dyn ScreenTextMeasurer,
) -> Option<LayoutRect> {
    // Five bounded measurements find a compact local box without reading or
    // searching screenshot pixels on the UI thread.
    [1.0, 1.25, 1.5, 2.0, f32::INFINITY]
        .into_iter()
        .filter_map(|growth| {
            let requested_width = (source.2 * growth).min(available.0).max(1.0);
            let measured = measurer.measure(
                text,
                (requested_width - TEXT_HORIZONTAL_PADDING).max(1.0),
                font_size,
            );
            let width = source
                .2
                .min(available.0)
                .max(measured.width + TEXT_HORIZONTAL_PADDING);
            let height = measured.height + TEXT_VERTICAL_PADDING;
            (measured.width.is_finite()
                && measured.height.is_finite()
                && measured.height > 0.0
                && width <= requested_width + 0.5
                && width <= available.0
                && height <= available.1)
                .then_some((source.0, source.1, width, height))
        })
        .min_by(|left, right| {
            left.3
                .total_cmp(&right.3)
                .then_with(|| (left.2 * left.3).total_cmp(&(right.2 * right.3)))
        })
}

fn projected_text(row: &BlockLayout, projection: ResultViewMode) -> String {
    match projection {
        ResultViewMode::Dual => format!("{}\n{}", row.source, row.display_translation),
        _ => row.display_translation.clone(),
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
    let y = candidate.1;
    (width <= viewport.0
        && height <= viewport.1
        && width <= candidate.2 + 0.5
        && height <= candidate.3 + 0.5
        && candidate.0 + width <= viewport.0 + 0.5
        && y + height <= viewport.1 + 0.5)
        .then_some((candidate.0, y, width, height))
}

pub(crate) fn apply_snapshot(
    preview: &ScreenTranslationWindow,
    snapshot: &ScreenResultSnapshot,
) -> Result<Vec<BlockLayout>, String> {
    let requested_mode = preview.get_screen_view_mode();
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
    let plan = plan_screen_result_layout_with_frame(
        crop_size,
        &inputs,
        ResultViewMode::from_ui(requested_mode),
        work_area,
        &SlintScreenTextMeasurer { window: preview },
        snapshot.crop.as_deref(),
    );
    let layouts = &plan.blocks;
    let rows = layouts
        .iter()
        .map(|block| ScreenBlockOverlay {
            block_id: block.block_id.as_str().into(),
            source: block.source.clone().into(),
            translation: block.display_translation.clone().into(),
            x: block.x,
            y: block.y,
            width: block.width,
            height: block.height,
            source_x: block.source_x,
            source_y: block.source_y,
            source_width: block.source_width,
            source_height: block.source_height,
            low_confidence: block.low_confidence,
            replace_source: block.replace_source,
            placed: block.placed,
            callout: block.callout,
            not_placeable: block.not_placeable,
            editable_source: block.editable_source,
            overflow: block.overflow,
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
    let connectors = plan
        .connectors
        .iter()
        .map(|connector| ScreenConnectorOverlay {
            block_id: connector.block_id.as_str().into(),
            start_x: connector.start_x,
            start_y: connector.start_y,
            end_x: connector.end_x,
            end_y: connector.end_y,
        })
        .collect::<Vec<_>>();
    preview.set_screen_blocks(ModelRc::new(VecModel::from(rows)));
    preview.set_screen_masks(ModelRc::new(VecModel::from(masks)));
    preview.set_screen_connectors(ModelRc::new(VecModel::from(connectors)));
    preview.set_screen_image_content_width(plan.image_width as f32);
    preview.set_screen_image_content_height(plan.image_height as f32);
    preview.set_screen_total_content_height(plan.content_height as f32);
    preview.set_screen_content_width(plan.content_width as f32);
    preview.set_screen_content_height(plan.viewport_height as f32);
    preview.set_screen_image_expired(snapshot.crop.is_none());
    preview.set_screen_view_mode(visible_ui_mode(requested_mode, snapshot.crop.is_some()));
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
    let visible_width = window.get_screen_visible_width().max(1.0);
    let visible_height = window.get_screen_visible_height().max(1.0);
    let editor_width = 330.0_f32.min((visible_width - 16.0).max(1.0));
    let editor_height = 282.0_f32.min((visible_height - 16.0).max(1.0));
    let source_x = block.x - window.get_screen_scroll_x();
    let right_candidate = source_x + block.width + 8.0;
    let left_candidate = source_x - editor_width - 8.0;
    let x = if right_candidate + editor_width <= visible_width {
        right_candidate
    } else {
        left_candidate
    }
    .clamp(8.0, (visible_width - editor_width - 8.0).max(8.0));
    let y = (block.y - window.get_screen_scroll_offset())
        .clamp(8.0, (visible_height - editor_height - 8.0).max(8.0));
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
    use std::cell::RefCell;

    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::{PhysicalRect, ScreenBlockId};

    use super::{
        BlockLayoutInput, CropLocalRect, CropPixelSize, LogicalTextSize, LogicalWorkArea,
        MIN_FONT_SIZE, ResultViewMode, ScreenResultWindowMode, ScreenTextMeasurer, fitted_viewport,
        layout_blocks, plan_screen_result_layout, plan_screen_result_layout_with_frame,
        preserve_editor_on_snapshot_refresh, translation_needs_visual_replacement, visible_mode,
        visible_ui_mode,
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
    fn in_place_arc_lightning_keeps_overlapping_ocr_rows_at_their_source() {
        // Recorded Arc Lightning description: the OCR rectangles already overlap.
        let blocks = [
            BlockLayoutInput {
                block_id: ScreenBlockId::from("description"),
                source: "Hurls a bolt of lightning that leaps through nearby enemy units that"
                    .into(),
                translation:
                    "Бросает молнию, которая перепрыгивает через nearby вражеские юниты, что".into(),
                bounds: local_rect(134, 44, 506, 32),
                fragments: vec![local_rect(134, 44, 506, 32)],
                complete: true,
                confidence_milli: 980,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("damage"),
                source: "deal damage.".into(),
                translation: "наносит урон.".into(),
                bounds: local_rect(133, 66, 108, 33),
                fragments: vec![local_rect(133, 66, 108, 33)],
                complete: true,
                confidence_milli: 980,
            },
        ];
        let plan = plan_screen_result_layout(
            crop_size(673, 427),
            &blocks,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 705.0,
                height: 497.0,
            },
            &normal_measurer(),
        );

        for (row, (x, y)) in plan.blocks.iter().zip([(134.0, 44.0), (133.0, 66.0)]) {
            assert!(row.placed && !row.callout && !row.not_placeable);
            assert_eq!(row.x, x);
            assert!(row.y >= y && row.y - y < 12.0);
        }
        assert!(!overlaps(&plan.blocks[0], &plan.blocks[1]));
        assert_eq!((plan.content_width, plan.content_height), (673, 427));
        assert!(plan.connectors.is_empty());
        assert_eq!(plan.masks.len(), 2);
    }

    #[test]
    fn in_place_translation_grows_from_its_source_without_an_external_panel() {
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("immunity"),
            source: "PIERCES SPELL IMMUNITY: No".into(),
            translation: "ПРОБИВАЕТ ИММУНИТЕТ К МАГИИ: Нет".into(),
            bounds: local_rect(331, 153, 205, 23),
            fragments: vec![local_rect(331, 153, 187, 23), local_rect(512, 158, 24, 15)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout(
            crop_size(673, 427),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 705.0,
                height: 497.0,
            },
            &normal_measurer(),
        );
        let row = &plan.blocks[0];
        assert!(row.placed && !row.callout && !row.not_placeable);
        assert_eq!((row.x, row.y), (331.0, 153.0));
        assert!(row.width > 205.0 || row.height > 23.0);
        let measured = normal_measurer().measure(&row.translation, row.width - 8.0, row.font_size);
        assert!(measured.height + super::TEXT_VERTICAL_PADDING <= row.height + 0.5);
        assert!(row.x + row.width <= 673.0 && row.y + row.height <= 427.0);
        assert_eq!((plan.content_width, plan.content_height), (673, 427));
        assert!(plan.connectors.is_empty());
    }

    #[test]
    fn source_headings_keep_a_larger_type_scale_on_native_size_images() {
        let heading = BlockLayoutInput {
            block_id: ScreenBlockId::from("arc-title"),
            source: "ARC LIGHTNING".into(),
            translation: "АРКОВЫЙ МОЛНИЯ".into(),
            bounds: local_rect(136, 17, 211, 30),
            fragments: vec![local_rect(136, 17, 211, 30)],
            complete: true,
            confidence_milli: 980,
        };
        let body = BlockLayoutInput {
            block_id: ScreenBlockId::from("arc-body"),
            source: "Hurls a bolt of lightning that leaps through nearby enemy units that".into(),
            translation: "Бросает молнию, которая перепрыгивает через nearby вражеские юниты, что"
                .into(),
            bounds: local_rect(134, 44, 506, 32),
            fragments: vec![local_rect(134, 44, 506, 32)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout(
            crop_size(673, 427),
            &[heading, body],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 1280.0,
                height: 720.0,
            },
            &normal_measurer(),
        );
        assert_eq!((plan.image_width, plan.image_height), (673, 427));
        assert!(plan.blocks[0].font_size >= 18.0);
        assert!(plan.blocks[0].font_size > plan.blocks[1].font_size);
        assert!(!overlaps(&plan.blocks[0], &plan.blocks[1]));
    }

    struct RecordingMeasurer {
        measured: RefCell<Vec<String>>,
    }

    impl ScreenTextMeasurer for RecordingMeasurer {
        fn measure(&self, text: &str, width: f32, font_size: f32) -> LogicalTextSize {
            self.measured.borrow_mut().push(text.to_owned());
            FakeMeasurer {
                height_multiplier: 1.0,
            }
            .measure(text, width, font_size)
        }
    }

    #[test]
    fn dual_mode_measures_the_actual_source_and_translation_projection() {
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("dual"),
            source: "Cooldown".to_owned(),
            translation: "Перезарядка".to_owned(),
            bounds: local_rect(10, 10, 220, 60),
            fragments: vec![local_rect(10, 10, 220, 60)],
            complete: true,
            confidence_milli: 980,
        };
        let measurer = RecordingMeasurer {
            measured: RefCell::new(Vec::new()),
        };

        let plan = plan_screen_result_layout(
            crop_size(300, 120),
            &[block],
            ResultViewMode::Dual,
            LogicalWorkArea {
                width: 557.0,
                height: 280.0,
            },
            &measurer,
        );

        assert!(plan.blocks[0].placed);
        assert!(
            measurer
                .measured
                .borrow()
                .iter()
                .any(|text| text == "Cooldown\nПерезарядка")
        );
    }

    #[test]
    fn overflow_grows_a_local_box_without_detaching_the_translation() {
        let long = "A translation that needs more room than its source region can safely provide"
            .repeat(4);
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("safe"),
            source: "Short".to_owned(),
            translation: long,
            bounds: local_rect(40, 40, 80, 24),
            fragments: vec![local_rect(40, 40, 80, 24)],
            complete: true,
            confidence_milli: 980,
        };

        let plan = plan_screen_result_layout(
            crop_size(400, 200),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 432.0,
                height: 270.0,
            },
            &normal_measurer(),
        );

        let row = &plan.blocks[0];
        assert!(row.placed && !row.callout);
        assert_eq!((row.x, row.y), (40.0, 40.0));
        assert_eq!(plan.content_height, plan.image_height);
        assert_eq!(plan.masks.len(), 1);
        assert_eq!(plan.masks[0].x, row.source_x);
    }

    #[test]
    fn translated_zeus_rows_are_visible_even_when_russian_exceeds_ocr_boxes() {
        let blocks = vec![
            BlockLayoutInput {
                block_id: ScreenBlockId::from("description"),
                source: "Hurls a bolt of lightning that leaps through nearby enemy units that"
                    .to_owned(),
                translation:
                    "Бросает молнию, которая перепрыгивает через ближайших вражеских юнитов"
                        .to_owned(),
                bounds: local_rect(135, 47, 506, 31),
                fragments: vec![local_rect(135, 47, 506, 31)],
                complete: true,
                confidence_milli: 956,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("description-continuation"),
                source: "deal damage.".to_owned(),
                translation: "наносит урон.".to_owned(),
                bounds: local_rect(135, 68, 108, 33),
                fragments: vec![local_rect(135, 68, 108, 33)],
                complete: true,
                confidence_milli: 969,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("damage-type"),
                source: "DAMAGE TYPE: Magical".to_owned(),
                translation: "ТИП УРОНА: Магический".to_owned(),
                bounds: local_rect(30, 198, 159, 21),
                fragments: vec![local_rect(30, 198, 159, 21)],
                complete: true,
                confidence_milli: 980,
            },
            BlockLayoutInput {
                block_id: ScreenBlockId::from("mana-number"),
                source: "85 / 90 / 95 / 100".to_owned(),
                translation: "85 / 90 / 95 / 100".to_owned(),
                bounds: local_rect(510, 350, 130, 24),
                fragments: vec![local_rect(510, 350, 130, 24)],
                complete: true,
                confidence_milli: 980,
            },
        ];
        let plan = plan_screen_result_layout(
            crop_size(678, 429),
            &blocks,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 710.0,
                height: 500.0,
            },
            &normal_measurer(),
        );

        for id in ["description", "damage-type"] {
            let block = plan
                .blocks
                .iter()
                .find(|block| block.block_id.as_str() == id)
                .unwrap();
            assert!(block.placed, "{id} must have a visible Russian translation");
            assert!(
                !block.not_placeable,
                "{id} must not fall back to outlined English"
            );
            assert!(plan.masks.iter().any(|mask| mask.block_id.as_str() == id));
        }
        assert!(!plan.blocks[3].replace_source);
        assert!(
            plan.masks
                .iter()
                .all(|mask| mask.block_id.as_str() != "mana-number")
        );
        assert!(plan.connectors.is_empty());
        assert!(plan.blocks.iter().all(|row| !row.callout));
    }

    #[test]
    fn in_place_translation_does_not_search_for_a_distant_dark_patch() {
        let mut pixels = vec![16_u8; 400 * 250 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for y in 0..90 {
            for x in 0..90 {
                let pixel = &mut pixels[(y * 400 + x) * 4..][..4];
                pixel[..3].fill(240);
            }
        }
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 400,
                height: 250,
            },
            400 * 4,
            pixels,
        )
        .unwrap();
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("long-description"),
            source: "Short source".to_owned(),
            translation:
                "Длинный перевод текста, которому нужно больше места, но значок закрывать нельзя"
                    .to_owned(),
            bounds: local_rect(100, 25, 180, 22),
            fragments: vec![local_rect(100, 25, 180, 22)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout_with_frame(
            crop_size(400, 250),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 432.0,
                height: 320.0,
            },
            &normal_measurer(),
            Some(&frame),
        );
        let row = &plan.blocks[0];
        assert!(!row.callout && row.placed);
        assert_eq!((row.x, row.y), (100.0, 25.0));
        assert!(row.y + row.height <= plan.image_height as f32);
        assert!(row.x >= 90.0 || row.y >= 90.0);
        assert_eq!(plan.content_height, plan.image_height);
    }

    #[test]
    fn bright_image_keeps_translation_at_the_source() {
        let mut pixels = vec![240_u8; 400 * 250 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 400,
                height: 250,
            },
            400 * 4,
            pixels,
        )
        .unwrap();
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("long-description"),
            source: "Short source".to_owned(),
            translation:
                "Длинный перевод текста, которому нужно больше места, чем есть у исходной строки"
                    .to_owned(),
            bounds: local_rect(100, 25, 180, 22),
            fragments: vec![local_rect(100, 25, 180, 22)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout_with_frame(
            crop_size(400, 250),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 432.0,
                height: 320.0,
            },
            &normal_measurer(),
            Some(&frame),
        );
        let row = &plan.blocks[0];
        assert!(!row.callout && row.placed);
        assert_eq!((row.x, row.y), (100.0, 25.0));
        assert_eq!(plan.content_height, plan.image_height);
        assert!(plan.viewport_height <= 250);
    }

    #[test]
    fn dual_in_place_label_reserves_height_for_both_source_and_translation() {
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("long-description"),
            source: "An English sentence that spans several lines when shown in a narrow card"
                .to_owned(),
            translation: "Русское предложение, которое также занимает несколько строк".to_owned(),
            bounds: local_rect(100, 25, 100, 20),
            fragments: vec![local_rect(100, 25, 100, 20)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout(
            crop_size(300, 180),
            &[block],
            ResultViewMode::Dual,
            LogicalWorkArea {
                width: 332.0,
                height: 250.0,
            },
            &normal_measurer(),
        );
        let row = &plan.blocks[0];
        assert!(row.placed && !row.callout);
        let rendered = format!("{}\n{}", row.source, row.translation);
        let measured = normal_measurer().measure(
            &rendered,
            (row.width - super::TEXT_HORIZONTAL_PADDING).max(1.0),
            row.font_size,
        );
        assert!(
            row.height >= measured.height + super::TEXT_VERTICAL_PADDING,
            "dual label must not clip the source or its translation"
        );
    }

    #[test]
    fn textured_image_does_not_move_the_translation_away_from_its_source() {
        let mut pixels = vec![0_u8; 400 * 250 * 4];
        for y in 0..250 {
            for x in 0..400 {
                let value = if ((x / 3) + (y / 3)) % 2 == 0 { 16 } else { 48 };
                let pixel = &mut pixels[(y * 400 + x) * 4..][..4];
                pixel[..3].fill(value);
                pixel[3] = 255;
            }
        }
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 400,
                height: 250,
            },
            400 * 4,
            pixels,
        )
        .unwrap();
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("text"),
            source: "Short".to_owned(),
            translation:
                "Достаточно длинный русский перевод, который не помещается на исходной строке"
                    .to_owned(),
            bounds: local_rect(100, 25, 150, 20),
            fragments: vec![local_rect(100, 25, 150, 20)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout_with_frame(
            crop_size(400, 250),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 432.0,
                height: 320.0,
            },
            &normal_measurer(),
            Some(&frame),
        );
        assert!(plan.blocks[0].placed && !plan.blocks[0].callout);
        assert_eq!((plan.blocks[0].x, plan.blocks[0].y), (100.0, 25.0));
    }

    #[test]
    fn original_projection_keeps_original_image_size_without_hidden_callouts() {
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("text"),
            source: "Short".to_owned(),
            translation: "Длинный перевод, который не помещается на месте исходной надписи"
                .to_owned(),
            bounds: local_rect(20, 20, 80, 20),
            fragments: vec![local_rect(20, 20, 80, 20)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout(
            crop_size(400, 250),
            &[block],
            ResultViewMode::Original,
            LogicalWorkArea {
                width: 432.0,
                height: 320.0,
            },
            &normal_measurer(),
        );
        assert_eq!(plan.content_height, plan.image_height);
        assert!(plan.connectors.is_empty());
        assert!(plan.masks.is_empty());
    }

    #[test]
    fn snapshot_renders_a_local_underlay_and_preserves_original_pixels() {
        use std::sync::Arc;

        use rust_protocol::{BlockCompletion, RequestId, SourceContext};
        use slint::{ComponentHandle, LogicalSize, Model};

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
        let crop_rect = PhysicalRect {
            x: 0,
            y: 0,
            width: 400,
            height: 250,
        };
        let mut pixels = vec![240_u8; 400 * 250 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let frame = RgbaFrame::try_new(crop_rect, 400 * 4, pixels).unwrap();
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: RequestId::new(),
            revision: 1,
            crop: Some(Arc::new(frame)),
            crop_rect,
            blocks: vec![crate::screen_result::ScreenResultBlockSnapshot {
                block_id: ScreenBlockId::from("overflow"),
                source: "Short source".to_owned(),
                translation: "Длинный перевод для читаемой строки на исходном изображении "
                    .repeat(4),
                completion: BlockCompletion::Completed,
                failure_message: None,
                bounds: PhysicalRect {
                    x: 100,
                    y: 25,
                    width: 180,
                    height: 22,
                },
                fragments: vec![PhysicalRect {
                    x: 100,
                    y: 25,
                    width: 180,
                    height: 22,
                }],
                confidence_milli: 980,
            }],
            context: SourceContext::default(),
            created_unix_ms: 1,
            expires_unix_ms: u64::MAX,
        };
        let window = crate::ScreenTranslationWindow::new().unwrap();
        super::apply_snapshot(&window, &snapshot).unwrap();
        let row = window.get_screen_blocks().row_data(0).unwrap();

        assert!(row.placed && !row.callout);
        assert_eq!(
            window.get_screen_total_content_height(),
            window.get_screen_image_content_height()
        );
        assert_eq!(window.get_screen_connectors().row_count(), 0);
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(
            window.get_screen_content_width(),
            window.get_screen_content_height() + 38.0,
        ));
        let rendered = window.window().take_snapshot().unwrap();
        let toolbar_pixel = (18 * rendered.width() as usize + 300) * 4;
        assert_eq!(rendered.as_bytes()[toolbar_pixel + 3], 255);
        // The measured label has an opaque local underlay; distant pixels are
        // still the original screenshot, not a panel or connector background.
        let sample = |image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: usize, y: usize| {
            let offset = (y * image.width() as usize + x) * 4;
            image.as_bytes()[offset..offset + 3].to_vec()
        };
        let label_x = (row.x + row.width - 3.0) as usize;
        let label_y = (row.y + row.height - 3.0 + 38.0) as usize;
        assert!(
            sample(&rendered, label_x, label_y)
                .into_iter()
                .all(|channel| channel < 65)
        );
        // This corner is outside both the source glyph mask and even the
        // expanded label. The bottom right can legitimately be underlaid by
        // this deliberately oversized translation.
        let outside_x = 10;
        let outside_image_y = 10;
        assert!(row.x > outside_x as f32 || row.y > outside_image_y as f32);
        let outside_y = outside_image_y + 38;
        assert_eq!(sample(&rendered, outside_x, outside_y), [240, 240, 240]);

        window.set_screen_view_mode(1);
        super::apply_snapshot(&window, &snapshot).unwrap();
        let original = window.window().take_snapshot().unwrap();
        assert_eq!(sample(&original, label_x, label_y), [240, 240, 240]);
        assert_eq!(window.get_screen_masks().row_count(), 0);
    }

    #[test]
    fn screen_result_southeast_corner_requests_resize() {
        use std::{cell::Cell, rc::Rc};

        use slint::{
            ComponentHandle, LogicalPosition, LogicalSize,
            platform::{PointerEventButton, WindowEvent},
        };

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
        let window = crate::ScreenTranslationWindow::new().unwrap();
        let requested = Rc::new(Cell::new(false));
        let requested_on_resize = Rc::clone(&requested);
        window.on_begin_resize(move || requested_on_resize.set(true));
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(400.0, 300.0));
        let position = LogicalPosition::new(395.0, 295.0);
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        assert!(
            requested.get(),
            "the visible corner should initiate a resize"
        );
    }

    #[test]
    fn block_geometry_scales_and_long_text_grows_within_the_image() {
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
        assert!(laid_out[1].placed && !laid_out[1].callout);
        assert!(laid_out[1].x + laid_out[1].width <= 500.0);
        assert!(laid_out[1].y + laid_out[1].height <= 250.0);
        assert!(laid_out[1].font_size >= MIN_FONT_SIZE);
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
        assert_eq!((laid_out[0].x, laid_out[0].y), (30.0, 13.0));
        assert_eq!((laid_out[1].x, laid_out[1].y), (30.0, 133.0));
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
        assert_eq!((width, height), (305, 255));
    }

    #[test]
    fn adjacent_long_translations_remain_near_their_source_even_if_they_overlap() {
        let long =
            "A translated paragraph that needs readable text near its original place ".repeat(4);
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
        assert!(overlaps(&laid_out[0], &laid_out[1]));
        assert_eq!((laid_out[0].x, laid_out[0].y), (20.0, 20.0));
        assert_eq!(laid_out[1].x, 20.0);
        assert!((45.0..=55.0).contains(&laid_out[1].y));
        assert!(
            laid_out
                .iter()
                .all(|row| !row.callout && !row.not_placeable)
        );
    }

    #[test]
    fn dense_results_keep_every_complete_translation_within_the_image() {
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

        assert!(laid_out.iter().all(|block| block.placed));
        assert!(laid_out.iter().all(|row| !row.callout));
        assert!(laid_out.iter().all(|row| row.y + row.height <= 320.0));
        assert!(laid_out.iter().all(|row| row.translation == long));
    }

    #[test]
    fn dense_underlays_never_nudge_text_far_from_its_source() {
        let blocks = (0..10)
            .map(|index| BlockLayoutInput {
                block_id: ScreenBlockId::from(format!("dense-{index}")),
                source: format!("source {index}"),
                translation: "A longer translation near the same source row".into(),
                bounds: local_rect(20, index * 12, 180, 11),
                fragments: vec![local_rect(20, index * 12, 180, 11)],
                complete: true,
                confidence_milli: 900,
            })
            .collect::<Vec<_>>();
        let rows = layout_blocks(crop_size(240, 220), 240, 220, &blocks);
        assert!(
            rows.iter()
                .all(|row| row.placed && row.y - row.source_y <= 10.0)
        );
        assert!(rows.iter().all(|row| row.y >= row.source_y));
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
            ResultViewMode::OnImage,
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

    #[test]
    fn unchanged_alphabetic_text_keeps_pixels_and_an_edit_target() {
        let plan = plan_screen_result_layout(
            crop_size(300, 120),
            &[BlockLayoutInput {
                block_id: ScreenBlockId::from("proper-name"),
                source: "Nimbus".into(),
                translation: "Nimbus".into(),
                bounds: local_rect(20, 20, 100, 24),
                fragments: vec![local_rect(20, 20, 100, 24)],
                complete: true,
                confidence_milli: 970,
            }],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 500.0,
                height: 300.0,
            },
            &normal_measurer(),
        );
        assert!(!plan.blocks[0].placed);
        assert!(plan.blocks[0].editable_source);
        assert!(plan.masks.is_empty());
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
    fn adjacent_unchanged_nimbus_duration_remains_visible_with_translated_label() {
        let inputs = nimbus_layout_inputs();
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &normal_measurer(),
        );
        let label = &plan.blocks[7];
        let value = &plan.blocks[8];
        assert_eq!(label.translation, "ДЛИТЕЛЬНОСТЬ:");
        assert_eq!(label.display_translation, "ДЛИТЕЛЬНОСТЬ: 30");
        assert!(label.placed && !label.overflow);
        assert_eq!(value.translation, "30");
        assert!(!value.replace_source && !value.placed);
        assert_eq!(
            plan.masks
                .iter()
                .filter(|mask| mask.block_id == value.block_id)
                .count(),
            1
        );
    }

    #[test]
    fn adjacent_number_is_only_repainted_when_the_image_label_covers_it() {
        let mut inputs = nimbus_layout_inputs();
        inputs[7].translation = "Длительность".into(); // User edit has no colon.
        let long = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &normal_measurer(),
        );
        assert_eq!(long.blocks[7].display_translation, "Длительность 30");
        assert_eq!(
            long.masks
                .iter()
                .filter(|mask| mask.block_id == long.blocks[8].block_id)
                .count(),
            1
        );

        inputs[7].translation = "Срок".into();
        let short = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &normal_measurer(),
        );
        assert_eq!(short.blocks[7].display_translation, "Срок");
        assert!(
            short
                .masks
                .iter()
                .all(|mask| mask.block_id != short.blocks[8].block_id)
        );

        let text = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::Translation,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &normal_measurer(),
        );
        assert_eq!(text.blocks[7].display_translation, "Срок");
        assert_eq!(text.blocks[8].display_translation, "30");
        assert!(text.masks.is_empty());
    }

    #[test]
    fn edited_label_with_its_own_adjacent_value_is_not_duplicated() {
        let mut inputs = nimbus_layout_inputs();
        for (translation, expected) in [
            ("Продолжительность: 30", "Продолжительность: 30"),
            ("Продолжительность:30", "Продолжительность:30"),
            ("Продолжительность:130", "Продолжительность:130 30"),
            ("Срок: 30", "Срок: 30"),
            ("Срок:30", "Срок:30"),
        ] {
            inputs[7].translation = translation.into();
            let plan = plan_screen_result_layout(
                crop_size(682, 466),
                &inputs,
                ResultViewMode::OnImage,
                LogicalWorkArea {
                    width: 740.0,
                    height: 540.0,
                },
                &normal_measurer(),
            );
            assert_eq!(plan.blocks[7].translation, translation);
            assert_eq!(plan.blocks[7].display_translation, expected);
            assert_eq!(plan.blocks[8].translation, "30");
            if translation.starts_with("Срок:") {
                let label = &plan.blocks[7];
                let value = &plan.blocks[8];
                assert!(!super::rectangles_overlap(
                    (label.x, label.y, label.width, label.height),
                    (
                        value.source_x,
                        value.source_y,
                        value.source_width,
                        value.source_height
                    ),
                ));
            }
            assert_eq!(
                plan.masks
                    .iter()
                    .filter(|mask| mask.block_id == plan.blocks[8].block_id)
                    .count(),
                1
            );
        }

        for translation in ["Продолжительность:30", "Срок:30"] {
            inputs[7].translation = translation.into();
            for mode in [
                ResultViewMode::Translation,
                ResultViewMode::Original,
                ResultViewMode::Dual,
            ] {
                let plan = plan_screen_result_layout(
                    crop_size(682, 466),
                    &inputs,
                    mode,
                    LogicalWorkArea {
                        width: 740.0,
                        height: 540.0,
                    },
                    &normal_measurer(),
                );
                assert_eq!(plan.blocks[7].display_translation, translation);
                assert_eq!(plan.blocks[8].translation, "30");
                if mode == ResultViewMode::Dual {
                    assert_eq!(
                        plan.masks
                            .iter()
                            .filter(|mask| mask.block_id == plan.blocks[8].block_id)
                            .count(),
                        1
                    );
                } else {
                    assert!(plan.masks.is_empty());
                }
            }
        }
    }

    #[test]
    fn nimbus_capture_masks_only_detected_text_rows_and_keeps_numeric_icon_values() {
        let inputs = nimbus_layout_inputs();

        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::OnImage,
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
    fn impossible_measured_text_is_clipped_locally_without_discarding_its_data() {
        let mut inputs = nimbus_layout_inputs();
        inputs[0].translation =
            "A pathological paragraph that cannot fit this screenshot ".repeat(80);
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &inputs,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &FakeMeasurer {
                height_multiplier: 20.0,
            },
        );

        assert!(plan.blocks.iter().all(|block| !block.callout));
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.replace_source)
                .all(|block| block.placed)
        );
        assert_eq!(plan.content_height, plan.image_height);
        assert!(plan.blocks.iter().any(|block| block.overflow));
        for (row, input) in plan.blocks.iter().zip(inputs) {
            assert_eq!(row.translation, input.translation);
            assert!(row.y + row.height <= plan.image_height as f32 + 0.5);
        }
    }

    #[test]
    fn nimbus_small_work_area_keeps_all_translation_labels_inside_the_image() {
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &nimbus_layout_inputs(),
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 360.0,
                height: 280.0,
            },
            &normal_measurer(),
        );

        assert_eq!(plan.content_width, plan.image_width);
        assert!(
            plan.blocks
                .iter()
                .filter(|row| row.replace_source)
                .all(|row| row.placed)
        );
        assert_eq!(plan.content_height, plan.image_height);
        assert!(plan.blocks.iter().all(|row| !row.callout));
        assert!(plan.blocks.iter().filter(|row| row.placed).all(|row| {
            row.x >= 0.0
                && row.y >= 0.0
                && row.x + row.width <= plan.image_width as f32
                && row.y + row.height <= plan.content_height as f32
        }));
    }

    #[test]
    fn render_recorded_screen_result_when_requested() {
        use std::{fs, io::BufReader, io::BufWriter, sync::Arc};

        use rust_platform_windows::screen_capture::RgbaFrame;
        use rust_protocol::PhysicalRect;
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
        let metadata_path = std::env::var_os("SAYFLEX_SCREEN_SAMPLE_METADATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| sample_path.with_extension("json"));
        let metadata: crate::screenshot_store::ScreenshotMetadata =
            serde_json::from_reader(BufReader::new(
                fs::File::open(&metadata_path).expect("matching screenshot metadata should open"),
            ))
            .expect("screenshot metadata should decode");
        assert_eq!((metadata.width, metadata.height), (info.width, info.height));
        assert!(
            !metadata.translations.is_empty(),
            "render evidence needs completed translations"
        );
        let crop_rect = PhysicalRect {
            x: metadata.crop_x,
            y: metadata.crop_y,
            width: info.width,
            height: info.height,
        };
        let frame = RgbaFrame::try_new(crop_rect, info.width * 4, pixels)
            .expect("sample pixels should form a capture frame");
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: metadata.request_id,
            revision: metadata.revision,
            crop: Some(Arc::new(frame)),
            crop_rect,
            blocks: metadata
                .blocks
                .iter()
                .map(|block| {
                    let translation = metadata
                        .translations
                        .iter()
                        .find(|translation| translation.block_id == block.block_id)
                        .expect("every recorded source block needs its translation result");
                    crate::screen_result::ScreenResultBlockSnapshot {
                        block_id: block.block_id.clone(),
                        source: block.source_text.clone(),
                        translation: translation.translated_text.clone(),
                        completion: translation.completion,
                        failure_message: translation.failure_message.clone(),
                        bounds: block.bounds,
                        fragments: block.fragments.clone(),
                        confidence_milli: block.confidence_milli,
                    }
                })
                .collect(),
            context: metadata.context.clone(),
            created_unix_ms: metadata.created_unix_ms,
            expires_unix_ms: metadata.expires_unix_ms,
        };

        let preview = crate::ScreenTranslationWindow::new().expect("preview should instantiate");
        if let Ok(mode) = std::env::var("SAYFLEX_SCREEN_SAMPLE_MODE") {
            let mode = match mode.as_str() {
                "translation" => 0,
                "original" => 1,
                "dual" => 2,
                "on-image" => 3,
                _ => panic!("unknown sample view mode: {mode}"),
            };
            preview.set_screen_view_mode(mode);
        }
        super::apply_snapshot(&preview, &snapshot).expect("snapshot should bind to the real view");
        preview
            .show()
            .expect("preview should show on testing backend");
        let qa_viewport = std::env::var("SAYFLEX_SCREEN_SAMPLE_VIEWPORT")
            .ok()
            .and_then(|value| {
                let (width, height) = value.split_once('x')?;
                Some((width.parse::<f32>().ok()?, height.parse::<f32>().ok()?))
            })
            .unwrap_or((
                preview.get_screen_content_width(),
                preview.get_screen_content_height() + 38.0,
            ));
        preview
            .window()
            .set_size(LogicalSize::new(qa_viewport.0, qa_viewport.1));
        let rendered = preview
            .window()
            .take_snapshot()
            .expect("screen result should render");

        let output_dir = std::path::Path::new(&output_dir);
        fs::create_dir_all(output_dir).expect("sample output directory should be creatable");
        fs::copy(sample_path, output_dir.join("screen-original.png"))
            .expect("original evidence should copy");
        let file = fs::File::create(output_dir.join("screen-result.png"))
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
