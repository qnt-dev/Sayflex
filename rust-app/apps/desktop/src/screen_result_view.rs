use rust_protocol::{BlockCompletion, PhysicalQuad, PhysicalRect, ScreenBlockId};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_geometry::{OrientedBox, repair_oriented};
use crate::screen_inpaint::{GlyphMetrics, PixelRect, glyph_metrics, repaired_pixels};
use crate::screen_result::ScreenResultSnapshot;
use crate::{ScreenBlockOverlay, ScreenConnectorOverlay, ScreenTranslationWindow};

const MIN_FONT_SIZE: f32 = 12.0;
const MAX_FONT_SIZE: f32 = 120.0;
type LayoutRect = (f32, f32, f32, f32);
const TEXT_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_VERTICAL_PADDING: f32 = 2.0;
const MAX_IMAGE_SCALE: f32 = 1.0;
const MAX_VERTICAL_NUDGE: f32 = 10.0;
/// Share of a label's height that a neighbour's line box may cover: the
/// leading above and below the glyphs.
const LEADING_OVERLAP: f32 = 0.25;
/// Step-downs from the source's own type size before the translation wraps.
const FONT_LADDER: [f32; 6] = [1.0, 0.94, 0.88, 0.82, 0.76, 0.7];
/// A translation may run this much wider than its source when nothing else
/// is in the way.
const WIDTH_ALLOWANCE: f32 = 1.05;
/// Width given to the measurer for a line that must not wrap.
const UNBOUNDED_WIDTH: f32 = 100_000.0;
const REGULAR_WEIGHT: i32 = 560;
const BOLD_WEIGHT: i32 = 700;
/// Median stroke run over the em size from which the source reads as bold,
/// and the fewest pixels that can (anti-aliasing spreads a thin stem over
/// two).
const BOLD_STROKE_RATIO: f32 = 0.11;
const BOLD_STROKE_MINIMUM: f32 = 3.0;
/// The labels' font (Segoe UI): capital height, x-height and the ascent's
/// share of the line height, in ems.
const CAPITAL_HEIGHT: f32 = 0.70;
const X_HEIGHT: f32 = 0.52;
const ASCENT_SHARE: f32 = 0.811;

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
    pub(crate) quad: Option<PhysicalQuad>,
    pub(crate) fragment_quads: Vec<PhysicalQuad>,
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
    pub(crate) source_quad: Option<PhysicalQuad>,
    pub(crate) low_confidence: bool,
    pub(crate) replace_source: bool,
    pub(crate) placed: bool,
    pub(crate) callout: bool,
    pub(crate) not_placeable: bool,
    pub(crate) editable_source: bool,
    pub(crate) overflow: bool,
    pub(crate) font_size: f32,
    pub(crate) bold: bool,
    /// The source text's own colours, when the picture shows them.
    pub(crate) angle: f32,
    pub(crate) source_ink: Option<[u8; 3]>,
    pub(crate) source_background: Option<[u8; 3]>,
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
    pub(crate) oriented_masks: Vec<PhysicalQuad>,
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
    fn measure(&self, text: &str, width: f32, font_size: f32, bold: bool) -> LogicalTextSize;
}

struct SlintScreenTextMeasurer<'a> {
    window: &'a ScreenTranslationWindow,
}

impl ScreenTextMeasurer for SlintScreenTextMeasurer<'_> {
    fn measure(&self, text: &str, width: f32, font_size: f32, bold: bool) -> LogicalTextSize {
        self.window.set_measure_text(text.into());
        self.window.set_measure_width(width.max(1.0));
        self.window
            .set_measure_font_size(font_size.max(MIN_FONT_SIZE));
        self.window
            .set_measure_font_weight(if bold { BOLD_WEIGHT } else { REGULAR_WEIGHT });
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
    fn measure(&self, text: &str, width: f32, font_size: f32, _bold: bool) -> LogicalTextSize {
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
    frame: Option<&rust_platform_windows::screen_capture::RgbaFrame>,
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
        oriented_masks: Vec::new(),
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
            source_quad: block.quad,
            low_confidence: block.confidence_milli < 650,
            replace_source,
            placed: false,
            callout: false,
            not_placeable: overlay_mode && !block.complete,
            editable_source: overlay_mode
                && block.complete
                && !replace_source
                && block.source.chars().any(char::is_alphabetic),
            overflow: false,
            font_size: MIN_FONT_SIZE,
            bold: false,
            angle: 0.0,
            source_ink: None,
            source_background: None,
        };
        if replace_source
            && let Some(oriented) = block.quad.and_then(OrientedBox::new)
            && oriented.angle.abs() >= 2.0
        {
            place_oriented_block(
                &mut row,
                block,
                oriented,
                (scale_x, scale_y),
                projection,
                measurer,
                frame,
            );
            plan.oriented_masks
                .extend(if block.fragment_quads.is_empty() {
                    block.quad.into_iter().collect::<Vec<_>>()
                } else {
                    block.fragment_quads.clone()
                });
            plan.blocks.push(row);
            continue;
        }
        if replace_source {
            // The source glyphs, when the picture is at hand, set the size,
            // weight and colour of the translation and where it starts.
            let metrics = frame.and_then(|frame| {
                glyph_metrics(
                    frame,
                    pixel_rect(block.bounds),
                    horizontal_reach(index, blocks),
                )
            });
            let em = source_em(&block.source, block.bounds, metrics.as_ref());
            let preferred = (em * scale_y).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
            let glyphs = metrics.as_ref().map_or(source, |metrics| {
                scale_pixel_rect(metrics.extent, scale_x, scale_y, viewport)
            });
            row.bold = metrics.as_ref().is_some_and(|metrics| {
                metrics.stroke >= (em * BOLD_STROKE_RATIO).max(BOLD_STROKE_MINIMUM)
            });
            row.source_ink = metrics.as_ref().map(|metrics| metrics.ink);
            row.source_background = metrics.as_ref().map(|metrics| metrics.background);
            let presented = presented_translation(&block.source, &block.translation);
            row.display_translation.clone_from(&presented);
            let adjacent_value = adjacent_unchanged_value(block, blocks.get(index + 1))
                .map(|value| (value, &blocks[index + 1]));
            let value_rect = adjacent_value
                .map(|(_, value)| scale_rect(value.bounds, scale_x, scale_y, viewport));
            let free_placement = Placement::new(
                index,
                blocks,
                source,
                metrics.as_ref().map(|metrics| Glyphs {
                    rect: glyphs,
                    baseline: metrics.core.1 as f32 * scale_y,
                    continues_right: metrics.continues.1,
                    free_right: metrics.free_right as f32 * scale_x,
                }),
                (scale_x, scale_y),
                viewport,
                projection == ResultViewMode::Dual,
            );
            let mut placement = free_placement;
            if let Some(value) = value_rect {
                placement.right = placement.right.min(value.0 - 2.0).max(placement.x + 1.0);
            }
            let (mut rect, mut font_size, mut fitted) = in_place_candidate(
                &projected_text(&row, projection),
                &placement,
                viewport,
                preferred,
                row.bold,
                measurer,
            );
            // Keep a nearby unchanged number in the screenshot when the label
            // leaves it visible. If an edit already includes that value, mask
            // the original even when the shorter label does not overlap it.
            if let (Some((value, value_block)), Some(value_rect)) = (adjacent_value, value_rect) {
                let overlaps_value = rectangles_overlap(rect, value_rect);
                let includes_value = translation_ends_with_value(&block.translation, value);
                if overlaps_value && !includes_value {
                    row.display_translation = format!("{presented} {value}");
                    (rect, font_size, fitted) = in_place_candidate(
                        &projected_text(&row, projection),
                        &free_placement,
                        viewport,
                        preferred,
                        row.bold,
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
            // Line boxes of close rows share their leading, which is
            // invisible on a transparent label; only a real overlap of the
            // text moves a label down. DUAL's plates are opaque.
            let leading = placement.leading_overlap();
            if let Some(lower_edge) = plan
                .blocks
                .iter()
                .filter(|prior| prior.placed && horizontal_overlap(rect, prior))
                .filter(|prior| {
                    let shared =
                        (rect.1 + rect.3).min(prior.y + prior.height) - rect.1.max(prior.y);
                    shared > leading * rect.3.min(prior.height)
                })
                .map(|prior| prior.y + prior.height + 2.0)
                .max_by(f32::total_cmp)
                && lower_edge + rect.3 <= viewport.1
                && lower_edge - rect.1 <= placement.largest_nudge(rect.3)
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
            // shorter; the background under them is rebuilt from the picture.
            let fragments = if block.fragments.is_empty() {
                std::slice::from_ref(&block.bounds)
            } else {
                block.fragments.as_slice()
            };
            plan.masks.extend(fragments.iter().map(|fragment| {
                let mut rect = scale_rect(*fragment, scale_x, scale_y, viewport);
                if row.display_translation == presented
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
            // Marks of the same text just outside the OCR box (quotes, a
            // closing dot, a descender) go with it instead of being left
            // behind.
            if let Some(metrics) = &metrics {
                let bounds = pixel_rect(block.bounds);
                if metrics.extent.left < bounds.left
                    || metrics.extent.right > bounds.right
                    || metrics.extent.top < bounds.top
                    || metrics.extent.bottom > bounds.bottom
                {
                    plan.masks.push(MaskLayout {
                        block_id: block.block_id.clone(),
                        x: glyphs.0,
                        y: glyphs.1,
                        width: glyphs.2,
                        height: glyphs.3,
                    });
                }
            }
        }
        if row.not_placeable {
            plan.unplaceable_rows.push(row.block_id.clone());
        }
        plan.blocks.push(row);
    }
    plan
}

fn place_oriented_block(
    row: &mut BlockLayout,
    block: &BlockLayoutInput,
    oriented: OrientedBox,
    scale: (f32, f32),
    projection: ResultViewMode,
    measurer: &dyn ScreenTextMeasurer,
    frame: Option<&rust_platform_windows::screen_capture::RgbaFrame>,
) {
    // Use one actual line for glyph metrics. A paragraph's tall envelope is
    // never interpreted as the height of one enormous font.
    let line = block
        .fragment_quads
        .first()
        .copied()
        .and_then(OrientedBox::new)
        .unwrap_or(oriented);
    let patch = frame.and_then(|frame| line.rectified(frame, 8));
    let line_bounds = CropLocalRect {
        x: 8,
        y: 8,
        width: line.width.ceil() as u32,
        height: line.height.ceil() as u32,
    };
    let metrics = patch.as_ref().and_then(|patch| {
        glyph_metrics(
            patch,
            pixel_rect(line_bounds),
            (8, 8 + i64::from(line_bounds.width)),
        )
    });
    let em = source_em(&block.source, line_bounds, metrics.as_ref());
    row.bold = metrics
        .as_ref()
        .is_some_and(|m| m.stroke >= (em * BOLD_STROKE_RATIO).max(BOLD_STROKE_MINIMUM));
    row.source_ink = metrics.as_ref().map(|m| m.ink);
    row.source_background = metrics.as_ref().map(|m| m.background).or_else(|| {
        patch
            .as_ref()
            .and_then(|patch| crate::screen_inpaint::ring_median(patch, pixel_rect(line_bounds)))
    });
    row.display_translation = presented_translation(&block.source, &block.translation);
    row.x = oriented.x * scale.0;
    row.y = oriented.y * scale.1;
    row.width = (oriented.width * scale.0).max(1.0);
    row.height = (oriented.height * scale.1).max(1.0);
    row.angle = oriented.angle;
    let text = projected_text(row, projection);
    let text_width = (row.width - TEXT_HORIZONTAL_PADDING).max(1.0);
    let text_height = (row.height - TEXT_VERTICAL_PADDING).max(1.0);
    let fits = |font| {
        let size = measurer.measure(&text, text_width, font, row.bold);
        // Slint reports intrinsic (unwrapped) preferred width but measures
        // preferred height at the supplied wrapping width. Requiring that
        // intrinsic width to fit shrank whole paragraphs to the minimum font.
        size.width.is_finite()
            && size.height.is_finite()
            && size.height > 0.0
            && size.height <= text_height + 0.5
    };
    let mut low = MIN_FONT_SIZE;
    let mut high = (em * scale.1).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    for _ in 0..8 {
        let mid = (low + high) * 0.5;
        if fits(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    row.font_size = low;
    row.placed = true;
    row.overflow = !fits(low);
}

fn pixel_rect(rect: CropLocalRect) -> PixelRect {
    let left = i64::from(rect.x);
    let top = i64::from(rect.y);
    PixelRect {
        left,
        top,
        right: left + i64::from(rect.width),
        bottom: top + i64::from(rect.height),
    }
}

fn scale_pixel_rect(
    rect: PixelRect,
    scale_x: f32,
    scale_y: f32,
    viewport: (f32, f32),
) -> LayoutRect {
    scale_rect(
        CropLocalRect {
            x: i32::try_from(rect.left).unwrap_or(i32::MAX),
            y: i32::try_from(rect.top).unwrap_or(i32::MAX),
            width: u32::try_from(rect.width()).unwrap_or(0),
            height: u32::try_from(rect.height()).unwrap_or(0),
        },
        scale_x,
        scale_y,
        viewport,
    )
}

/// How far, in capture pixels, marks beside row `index` may belong to it:
/// up to the rows on its left and right on the same line.
fn horizontal_reach(index: usize, blocks: &[BlockLayoutInput]) -> (i64, i64) {
    let row = pixel_rect(blocks[index].bounds);
    let mut reach = (0, i64::MAX);
    for (other_index, other) in blocks.iter().enumerate() {
        if other_index == index {
            continue;
        }
        let other = pixel_rect(other.bounds);
        if row.bottom.min(other.bottom) - row.top.max(other.top) <= 0 {
            continue;
        }
        if other.right <= row.left {
            reach.0 = reach.0.max(other.right);
        } else if other.left >= row.right {
            reach.1 = reach.1.min(other.left);
        }
    }
    reach
}

/// The em size, in capture pixels, of the font that drew the source, such
/// that the translation's capitals (or lowercase) are as tall as the
/// source's.
fn source_em(source: &str, bounds: CropLocalRect, metrics: Option<&GlyphMetrics>) -> f32 {
    match metrics {
        Some(metrics) => {
            let core = (metrics.core.1 - metrics.core.0) as f32;
            // Ink above the densest band, a quarter of it or more: the band
            // is the x-height under capitals and ascenders.
            let shelf = (metrics.core.0 - metrics.extent.top) as f32 >= core / 4.0;
            core / core_height(source, shelf)
        }
        // Without the picture: an OCR box is roughly twice its text's em.
        None => bounds.height as f32 * 0.55,
    }
}

/// What the densest band of the source's rows measures, in ems: the
/// x-height of mostly lowercase text, where the picture shows the taller
/// letters above it or the text has none; the capital height otherwise.
/// Glyphs of uncased scripts fill most of the em.
fn core_height(text: &str, shelf: bool) -> f32 {
    const ASCENDERS: &str = "bdfhkltбф";
    let letters = text
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    let lowercase = letters
        .iter()
        .filter(|letter| letter.is_lowercase())
        .count();
    let cased = letters
        .iter()
        .filter(|letter| letter.is_lowercase() || letter.is_uppercase())
        .count();
    let tall = text.chars().any(|character| {
        character.is_uppercase() || character.is_ascii_digit() || ASCENDERS.contains(character)
    });
    if !letters.is_empty() && cased == 0 {
        0.88
    } else if lowercase > 0 && lowercase * 2 >= letters.len() && (shelf || !tall) {
        X_HEIGHT
    } else {
        CAPITAL_HEIGHT
    }
}

/// The translation as drawn on the picture: in capitals when the source is
/// set in capitals, with typographic quotes in Cyrillic text.
fn presented_translation(source: &str, translation: &str) -> String {
    let letters = source
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    let capitals = letters.len() >= 2 && letters.iter().all(|letter| letter.is_uppercase());
    let text = if capitals {
        translation.to_uppercase()
    } else {
        translation.to_owned()
    };
    let cyrillic = text
        .chars()
        .any(|character| matches!(character, '\u{0400}'..='\u{04ff}'));
    if !cyrillic {
        return text;
    }
    let mut presented = String::with_capacity(text.len());
    let mut previous: Option<char> = None;
    for character in text.chars() {
        let replacement = match character {
            '"' | '\u{201c}' | '\u{201d}' | '\u{201e}' => {
                if previous.is_none_or(|previous| {
                    previous.is_whitespace() || matches!(previous, '(' | '[' | '\u{ab}')
                }) {
                    '\u{ab}'
                } else {
                    '\u{bb}'
                }
            }
            other => other,
        };
        presented.push(replacement);
        previous = Some(replacement);
    }
    presented
}

/// The measured ink of a source row, in layout coordinates.
#[derive(Debug, Clone, Copy)]
struct Glyphs {
    rect: LayoutRect,
    baseline: f32,
    /// More text of the same line follows, unrecognised.
    continues_right: bool,
    /// The plain background beside the text ends here.
    free_right: f32,
}

/// Where a translation may go: from `x`, its first line on the source's
/// baseline (or on its box, `band_top` and `band_height`, without the
/// picture), no further right than `right` and no lower than `bottom`.
/// `stacked` labels (DUAL: source over translation, on a plate) start at
/// the top of the source's box instead.
#[derive(Debug, Clone, Copy)]
struct Placement {
    x: f32,
    top: f32,
    band_top: f32,
    band_height: f32,
    baseline: Option<f32>,
    right: f32,
    bottom: f32,
    source_height: f32,
    stacked: bool,
}

impl Placement {
    fn new(
        index: usize,
        blocks: &[BlockLayoutInput],
        source: LayoutRect,
        glyphs: Option<Glyphs>,
        (scale_x, scale_y): (f32, f32),
        viewport: (f32, f32),
        stacked: bool,
    ) -> Self {
        let (x, glyph_right) = match glyphs {
            Some(Glyphs { rect, .. }) => (
                (rect.0 - TEXT_HORIZONTAL_PADDING / 2.0).max(0.0),
                rect.0 + rect.2 + TEXT_HORIZONTAL_PADDING / 2.0,
            ),
            None => (source.0, source.0 + source.2),
        };
        let band = glyphs.map_or(source, |glyphs| glyphs.rect);
        let width = (source.0 + source.2).max(glyph_right) - x;
        let mut right = match glyphs {
            // Text the OCR missed follows on the same line: stay within the
            // source's own glyphs rather than draw over it.
            Some(Glyphs {
                continues_right: true,
                ..
            }) => glyph_right,
            // Plain background beside the text takes a longer translation
            // at the source's size.
            Some(Glyphs { free_right, .. }) => (x + width * WIDTH_ALLOWANCE).max(free_right),
            None => x + width * WIDTH_ALLOWANCE,
        }
        .min(viewport.0);
        let mut bottom = viewport.1;
        for (other_index, other) in blocks.iter().enumerate() {
            if other_index == index {
                continue;
            }
            let other = scale_rect(other.bounds, scale_x, scale_y, viewport);
            let shared_height =
                (source.1 + source.3).min(other.1 + other.3) - source.1.max(other.1);
            if shared_height >= 0.45 * source.3.min(other.3) {
                if other.0 >= source.0 + source.2 - 1.0 {
                    right = right.min(other.0 - 4.0);
                }
            } else if other.1 >= source.1 + source.3 / 2.0
                && other.0 < right
                && other.0 + other.2 > x
            {
                bottom = bottom.min(other.1);
            }
        }
        Self {
            x,
            top: source.1,
            band_top: band.1,
            band_height: band.3,
            baseline: glyphs.map(|glyphs| glyphs.baseline),
            right: right.max(x + 1.0),
            bottom,
            source_height: source.3,
            stacked,
        }
    }

    /// How far a label may move down to clear the one above: a little on
    /// the picture, where it must stay on its source; a whole label in
    /// DUAL, where an opaque plate would otherwise hide the line above.
    fn largest_nudge(&self, height: f32) -> f32 {
        if self.stacked {
            height.max(MAX_VERTICAL_NUDGE)
        } else {
            MAX_VERTICAL_NUDGE
        }
    }

    /// Share of a label's height the next row may cover.
    fn leading_overlap(&self) -> f32 {
        if self.stacked { 0.0 } else { LEADING_OVERLAP }
    }

    /// Top of a label whose text lines are `line` high: its first baseline
    /// on the source's. Without the picture, one line of the source's
    /// height is centred on its box and a taller box is followed from its
    /// top.
    fn label_top(&self, line: f32) -> f32 {
        if self.stacked {
            return self.top;
        }
        if let Some(baseline) = self.baseline {
            return baseline - line * ASCENT_SHARE - TEXT_VERTICAL_PADDING / 2.0;
        }
        let padded = line + TEXT_VERTICAL_PADDING;
        if self.band_height <= padded {
            self.band_top + self.band_height / 2.0 - padded / 2.0
        } else {
            self.band_top
        }
    }
}

/// The source's type size first, a little smaller next, then wrapped at the
/// source's width, then smaller still. Overlapping OCR rows are not a reason
/// to detach a translation from its source.
fn in_place_candidate(
    text: &str,
    placement: &Placement,
    viewport: (f32, f32),
    preferred_font_size: f32,
    bold: bool,
    measurer: &dyn ScreenTextMeasurer,
) -> (LayoutRect, f32, bool) {
    let mut sizes = FONT_LADDER
        .iter()
        .map(|factor| (preferred_font_size * factor).max(MIN_FONT_SIZE))
        .collect::<Vec<_>>();
    sizes.dedup();
    for &size in &sizes {
        if let Some(rect) = single_line(text, placement, viewport, size, bold, measurer) {
            return (rect, size, true);
        }
    }
    for &size in &sizes {
        if let Some(rect) = wrapped(text, placement, viewport, size, bold, measurer) {
            return (rect, size, true);
        }
    }
    let mut size = sizes.last().copied().unwrap_or(MIN_FONT_SIZE) * 0.9;
    while size > MIN_FONT_SIZE {
        if let Some(rect) = single_line(text, placement, viewport, size, bold, measurer) {
            return (rect, size, true);
        }
        size *= 0.9;
    }
    for wrap in [false, true] {
        let candidate = if wrap {
            wrapped(text, placement, viewport, MIN_FONT_SIZE, bold, measurer)
        } else {
            single_line(text, placement, viewport, MIN_FONT_SIZE, bold, measurer)
        };
        if let Some(rect) = candidate {
            return (rect, MIN_FONT_SIZE, true);
        }
    }

    // Nothing fits beside the source: grow downwards within the image.
    let source = (
        placement.x,
        placement.top,
        (placement.right - placement.x).max(1.0),
        placement.source_height,
    );
    let remaining = (
        (viewport.0 - source.0).max(1.0),
        (viewport.1 - source.1).max(1.0),
    );
    if let Some(rect) = grown_candidate(text, source, remaining, MIN_FONT_SIZE, bold, measurer) {
        return (rect, MIN_FONT_SIZE, true);
    }
    // At an image edge, move only as far as required to keep the complete
    // measured label inside the image; it remains over its source region.
    if let Some(mut rect) = grown_candidate(text, source, viewport, MIN_FONT_SIZE, bold, measurer) {
        rect.0 = source.0.min((viewport.0 - rect.2).max(0.0));
        rect.1 = source.1.min((viewport.1 - rect.3).max(0.0));
        return (rect, MIN_FONT_SIZE, true);
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

/// The text on one line (or its own explicit lines), centred on the source.
fn single_line(
    text: &str,
    placement: &Placement,
    viewport: (f32, f32),
    font_size: f32,
    bold: bool,
    measurer: &dyn ScreenTextMeasurer,
) -> Option<LayoutRect> {
    let measured = measurer.measure(text, UNBOUNDED_WIDTH, font_size, bold);
    let width = measured.width + TEXT_HORIZONTAL_PADDING;
    let height = measured.height + TEXT_VERTICAL_PADDING;
    let lines = text.lines().count().max(1) as f32;
    let y = placement
        .label_top(measured.height / lines)
        .clamp(0.0, (viewport.1 - height).max(0.0));
    // The row below may share this label's leading, not its text.
    let clear_below = y + height <= placement.bottom + placement.leading_overlap() * height + 0.5;
    (measured.width.is_finite()
        && measured.height.is_finite()
        && measured.height > 0.0
        && placement.x + width <= placement.right + 0.5
        && height <= viewport.1
        && clear_below)
        .then_some((placement.x, y, width, height))
}

/// The text wrapped within the source's width, its first line where the
/// source line was, if the lines below stay clear of the next row.
fn wrapped(
    text: &str,
    placement: &Placement,
    viewport: (f32, f32),
    font_size: f32,
    bold: bool,
    measurer: &dyn ScreenTextMeasurer,
) -> Option<LayoutRect> {
    let available = placement.right - placement.x;
    let measured = measurer.measure(
        text,
        (available - TEXT_HORIZONTAL_PADDING).max(1.0),
        font_size,
        bold,
    );
    let line = measurer
        .measure("Ag", UNBOUNDED_WIDTH, font_size, bold)
        .height;
    let width = measured.width + TEXT_HORIZONTAL_PADDING;
    let height = measured.height + TEXT_VERTICAL_PADDING;
    let y = placement.label_top(line).max(0.0);
    (measured.width.is_finite()
        && measured.height.is_finite()
        && measured.height > 0.0
        && width <= available + 0.5
        && y + height <= placement.bottom.min(viewport.1) + 0.5
        && height <= (placement.source_height * 2.2).max(2.0 * line + TEXT_VERTICAL_PADDING) + 0.5)
        .then_some((placement.x, y, width, height))
}

fn grown_candidate(
    text: &str,
    source: LayoutRect,
    available: (f32, f32),
    font_size: f32,
    bold: bool,
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
                bold,
            );
            let width = (measured.width + TEXT_HORIZONTAL_PADDING).min(available.0);
            let height = measured.height + TEXT_VERTICAL_PADDING;
            (measured.width.is_finite()
                && measured.height.is_finite()
                && measured.height > 0.0
                && width <= requested_width + 0.5
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

pub(crate) fn apply_snapshot(
    preview: &ScreenTranslationWindow,
    snapshot: &ScreenResultSnapshot,
) -> Result<Vec<BlockLayout>, String> {
    let requested_mode = preview.get_screen_view_mode();
    preview.set_screen_copied(false);
    let translating = preview.get_screen_translating();
    preview.set_screen_failed_blocks(if translating {
        0
    } else {
        snapshot
            .blocks
            .iter()
            .filter(|block| {
                block.completion != BlockCompletion::Completed || block.failure_message.is_some()
            })
            .count() as i32
    });
    let crop_size = CropPixelSize::from_screen_rect(snapshot.crop_rect);
    let inputs = snapshot
        .blocks
        .iter()
        .map(|block| BlockLayoutInput {
            quad: block.quad,
            fragment_quads: block.fragment_quads.clone(),
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
    .map(|(width, height)| LogicalWorkArea {
        width,
        height: height - preview.get_screen_notice_height(),
    })
    .unwrap_or(LogicalWorkArea {
        width: 1280.0,
        height: 720.0 - preview.get_screen_notice_height(),
    });
    let mut plan = plan_screen_result_layout_with_frame(
        crop_size,
        &inputs,
        ResultViewMode::from_ui(requested_mode),
        work_area,
        &SlintScreenTextMeasurer { window: preview },
        snapshot.crop.as_deref(),
    );
    let frame = snapshot.crop.as_deref();
    let scale = frame.map(|frame| {
        (
            plan.image_width as f32 / frame.rect.width.max(1) as f32,
            plan.image_height as f32 / frame.rect.height.max(1) as f32,
        )
    });
    let repaired = frame
        .zip(scale)
        .filter(|_| !plan.masks.is_empty() || !plan.oriented_masks.is_empty())
        .map(|(frame, scale)| {
            let (image, failed_quads) = repaired_image(frame, &plan, scale);
            // If the picture offers no safe background sample, retain its
            // pixels and use the existing opaque label instead of guessing.
            for block in &mut plan.blocks {
                if inputs.iter().any(|input| {
                    input.block_id == block.block_id
                        && (input
                            .fragment_quads
                            .iter()
                            .any(|quad| failed_quads.contains(quad))
                            || input.quad.is_some_and(|quad| failed_quads.contains(&quad)))
                }) {
                    block.callout = true;
                }
            }
            image
        });
    let layouts = &plan.blocks;
    let rows = layouts
        .iter()
        .map(|block| {
            let background = block.source_background.or_else(|| {
                frame.zip(scale).and_then(|(frame, scale)| {
                    surrounding_color(frame, (block.x, block.y, block.width, block.height), scale)
                })
            });
            let ink = label_ink(block.source_ink, background);
            let incomplete = inputs
                .iter()
                .any(|input| input.block_id == block.block_id && !input.complete);
            ScreenBlockOverlay {
                block_id: block.block_id.as_str().into(),
                source: block.source.clone().into(),
                translation: block.display_translation.clone().into(),
                failed: !translating && incomplete,
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
                not_placeable: block.not_placeable && !(translating && incomplete),
                editable_source: block.editable_source,
                overflow: block.overflow,
                font_size: block.font_size,
                angle: block.angle,
                font_weight: if block.bold {
                    BOLD_WEIGHT
                } else {
                    REGULAR_WEIGHT
                },
                // Shown behind DUAL's two lines, which take more room than
                // the source and must stay readable over what they cover.
                plate: slint_color(background.unwrap_or(DEFAULT_PLATE)),
                ink: slint_color(ink),
                shadow: shadow_for(ink),
            }
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
    preview.set_screen_use_repaired_image(repaired.is_some());
    preview.set_screen_repaired_image(repaired.unwrap_or_default());
    Ok(layouts.to_vec())
}

/// The capture with the replaced text rebuilt from its background: under the
/// source glyphs, with a margin for their anti-aliased edges, except where
/// text that stays unchanged is drawn. A wider translation is drawn over the
/// untouched picture beside it.
fn repaired_image(
    frame: &rust_platform_windows::screen_capture::RgbaFrame,
    plan: &ScreenResultLayoutPlan,
    scale: (f32, f32),
) -> (Image, Vec<PhysicalQuad>) {
    let to_pixels = |(x, y, width, height): LayoutRect| PixelRect {
        left: (x / scale.0).floor() as i64,
        top: (y / scale.1).floor() as i64,
        right: ((x + width) / scale.0).ceil() as i64,
        bottom: ((y + height) / scale.1).ceil() as i64,
    };
    let regions = plan
        .masks
        .iter()
        .map(|mask| {
            to_pixels((mask.x, mask.y, mask.width, mask.height)).inflate(MASK_MARGIN, MASK_MARGIN)
        })
        .collect::<Vec<_>>();
    let protected = plan
        .blocks
        .iter()
        .filter(|block| !block.replace_source)
        .filter(|block| {
            plan.masks
                .iter()
                .all(|mask| mask.block_id != block.block_id)
        })
        .map(|block| {
            to_pixels((
                block.source_x,
                block.source_y,
                block.source_width,
                block.source_height,
            ))
        })
        .collect::<Vec<_>>();
    let mut pixels = repaired_pixels(frame, &regions, &protected);
    let protected_quads = plan
        .blocks
        .iter()
        .filter(|block| !block.replace_source)
        .map(|block| {
            if let Some(quad) = block.source_quad {
                return quad;
            }
            let r = to_pixels((
                block.source_x,
                block.source_y,
                block.source_width,
                block.source_height,
            ));
            PhysicalQuad {
                points: [
                    (r.left, r.top),
                    (r.right, r.top),
                    (r.right, r.bottom),
                    (r.left, r.bottom),
                ]
                .map(|(x, y)| rust_protocol::PhysicalPoint {
                    x: x as i32,
                    y: y as i32,
                }),
            }
        })
        .collect::<Vec<_>>();
    let failed = repair_oriented(frame, &mut pixels, &plan.oriented_masks, &protected_quads);
    (
        Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &pixels,
            frame.rect.width,
            frame.rect.height,
        )),
        failed,
    )
}

/// The design's plate, kept where the picture offers no colour: a callout
/// placed below the image, or a result whose pixels have expired.
const DEFAULT_PLATE: [u8; 3] = [0x12, 0x0e, 0x09];
/// Pixels added around a rebuilt text area for the anti-aliased fringe of
/// glyphs the OCR box stops at. Detection boxes already keep a margin; a
/// wider one would eat the background between close rows.
const MASK_MARGIN: i64 = 2;
const DARK_INK: [u8; 3] = [0x14, 0x14, 0x14];
const LIGHT_INK: [u8; 3] = [0xf3, 0xe9, 0xcf];
/// Luminance difference from which the source's own text colour still reads
/// on the rebuilt background.
const MIN_INK_CONTRAST: f32 = 0.3;

/// The colour around `rect` (layout coordinates) in the captured picture: the
/// median of a two-pixel ring just outside it. A fixed dark plate looked like
/// a sticker on a game interface and like a black box on a light page.
fn surrounding_color(
    frame: &rust_platform_windows::screen_capture::RgbaFrame,
    rect: LayoutRect,
    scale: (f32, f32),
) -> Option<[u8; 3]> {
    let width = i64::from(frame.rect.width);
    let height = i64::from(frame.rect.height);
    let to_pixel = |value: f32, factor: f32| (value / factor.max(f32::EPSILON)).floor() as i64;
    let left = to_pixel(rect.0, scale.0);
    let top = to_pixel(rect.1, scale.1);
    let right = to_pixel(rect.0 + rect.2, scale.0);
    let bottom = to_pixel(rect.1 + rect.3, scale.1);
    let mut channels: [Vec<u8>; 3] = Default::default();
    let mut take = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= width || y >= height {
            return;
        }
        if let Some([red, green, blue, _]) = frame.pixel(x as u32, y as u32) {
            channels[0].push(red);
            channels[1].push(green);
            channels[2].push(blue);
        }
    };
    for offset in 1..=2 {
        for x in (left - offset)..=(right + offset) {
            take(x, top - offset);
            take(x, bottom + offset);
        }
        for y in (top - offset)..=(bottom + offset) {
            take(left - offset, y);
            take(right + offset, y);
        }
    }
    if channels[0].len() < 8 {
        return None;
    }
    Some(channels.map(|mut channel| {
        channel.sort_unstable();
        channel[channel.len() / 2]
    }))
}

fn luminance(color: [u8; 3]) -> f32 {
    (0.2126 * f32::from(color[0]) + 0.7152 * f32::from(color[1]) + 0.0722 * f32::from(color[2]))
        / 255.0
}

/// Dark ink on a light plate, the design's cream on a dark one.
fn ink_for(plate: [u8; 3]) -> [u8; 3] {
    if luminance(plate) > 0.55 {
        DARK_INK
    } else {
        LIGHT_INK
    }
}

/// The source's own text colour where it still reads on its background;
/// otherwise the plain ink for that background.
fn label_ink(source_ink: Option<[u8; 3]>, background: Option<[u8; 3]>) -> [u8; 3] {
    let background = background.unwrap_or(DEFAULT_PLATE);
    match source_ink {
        Some(ink) if (luminance(ink) - luminance(background)).abs() >= MIN_INK_CONTRAST => ink,
        _ => ink_for(background),
    }
}

/// Light text drawn straight on a picture keeps a soft dark edge, so it
/// stays readable where the rebuilt background is busy.
fn shadow_for(ink: [u8; 3]) -> slint::Color {
    if luminance(ink) > 0.5 {
        slint::Color::from_argb_u8(140, 0, 0, 0)
    } else {
        slint::Color::from_argb_u8(0, 0, 0, 0)
    }
}

fn slint_color([red, green, blue]: [u8; 3]) -> slint::Color {
    slint::Color::from_rgb_u8(red, green, blue)
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
        fn measure(&self, text: &str, width: f32, font_size: f32, _bold: bool) -> LogicalTextSize {
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
    fn tilted_paragraph_uses_line_height_for_font_and_exact_strips_for_masks() {
        use rust_protocol::{PhysicalPoint, PhysicalQuad};
        let quad = |points: [(i32, i32); 4]| PhysicalQuad {
            points: points.map(|(x, y)| PhysicalPoint { x, y }),
        };
        let first = quad([(100, 80), (700, 200), (694, 230), (94, 110)]);
        let second = quad([(92, 120), (692, 240), (686, 270), (86, 150)]);
        let envelope = quad([(100, 80), (700, 200), (686, 270), (86, 150)]);
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("paragraph"),
            source: "A paragraph with two separate lines of text".into(),
            translation: "Перевод абзаца сохраняет наклон обеих строк текста.".into(),
            bounds: local_rect(86, 80, 614, 190),
            fragments: vec![local_rect(94, 80, 606, 150), local_rect(86, 120, 606, 150)],
            quad: Some(envelope),
            fragment_quads: vec![first, second],
            complete: true,
            confidence_milli: 960,
        };
        let plan = plan_screen_result_layout(
            crop_size(900, 500),
            std::slice::from_ref(&block),
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 1000.0,
                height: 700.0,
            },
            &normal_measurer(),
        );
        let row = &plan.blocks[0];
        assert!(row.placed && !row.overflow);
        assert!((row.angle - 11.31).abs() < 0.1);
        assert!(
            row.font_size < 25.0,
            "the paragraph AABB must not define font size"
        );
        assert!(plan.masks.is_empty());
        assert_eq!(plan.oriented_masks, vec![first, second]);
        let original = plan_screen_result_layout(
            crop_size(900, 500),
            &[block],
            ResultViewMode::Original,
            LogicalWorkArea {
                width: 1000.0,
                height: 700.0,
            },
            &normal_measurer(),
        );
        assert!(original.oriented_masks.is_empty());
        assert!(!original.blocks[0].placed);
    }

    #[test]
    fn tilted_wrapped_paragraph_uses_actual_slint_height_without_shrinking_to_minimum() {
        use rust_protocol::{PhysicalPoint, PhysicalQuad};
        use slint::ComponentHandle;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("skia".into()),
            },
        )))
        .unwrap();
        let window = crate::ScreenTranslationWindow::new().unwrap();
        window.show().unwrap();
        let quad = |points: [(i32, i32); 4]| PhysicalQuad {
            points: points.map(|(x, y)| PhysicalPoint { x, y }),
        };
        let first = quad([(100, 80), (700, 200), (692, 240), (92, 120)]);
        let second = quad([(90, 130), (690, 250), (682, 290), (82, 170)]);
        let block = BlockLayoutInput {
            block_id: ScreenBlockId::from("wrapped"),
            source: "A paragraph containing several words which need to wrap into two lines.".into(),
            translation: "Перевод длинного абзаца должен сохранять читаемый размер шрифта и переноситься на следующую строку.".into(),
            bounds: local_rect(82, 80, 618, 210),
            fragments: vec![local_rect(92, 80, 608, 160), local_rect(82, 130, 608, 160)],
            quad: Some(quad([(100, 80), (700, 200), (682, 290), (82, 170)])),
            fragment_quads: vec![first, second],
            complete: true,
            confidence_milli: 960,
        };
        let measurer = super::SlintScreenTextMeasurer { window: &window };
        let measured = measurer.measure(&block.translation, 600.0, 22.0, false);
        assert!(
            measured.width > 600.0,
            "fixture must exercise Slint intrinsic width"
        );
        assert!(measured.height < 90.0, "wrapped text fits the paragraph");
        let plan = plan_screen_result_layout(
            crop_size(900, 500),
            &[block],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 1000.0,
                height: 700.0,
            },
            &measurer,
        );
        assert!(
            plan.blocks[0].font_size > 18.0,
            "a fitting paragraph must remain readable"
        );
        assert!(!plan.blocks[0].overflow);
    }

    #[test]
    fn failed_numeric_block_keeps_its_pixels_and_has_a_visible_failure_marker() {
        let blocks = [BlockLayoutInput {
            quad: None,
            fragment_quads: Vec::new(),
            block_id: ScreenBlockId::from("cooldown"),
            source: "30".into(),
            translation: String::new(),
            bounds: local_rect(100, 25, 35, 26),
            fragments: vec![local_rect(100, 25, 35, 26)],
            complete: false,
            confidence_milli: 1000,
        }];
        let plan = plan_screen_result_layout(
            crop_size(483, 232),
            &blocks,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 1280.0,
                height: 720.0,
            },
            &normal_measurer(),
        );
        assert!(
            plan.blocks[0].not_placeable,
            "every failed block needs a visible marker"
        );
        assert!(!plan.blocks[0].replace_source);
        assert!(
            plan.masks.is_empty(),
            "failed source pixels must remain intact"
        );
    }

    #[test]
    fn in_place_arc_lightning_keeps_overlapping_ocr_rows_at_their_source() {
        // Recorded Arc Lightning description: the OCR rectangles already overlap.
        let blocks = [
            BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
        assert!(!text_collides(&plan.blocks[0], &plan.blocks[1]));
        assert_eq!((plan.content_width, plan.content_height), (673, 427));
        assert!(plan.connectors.is_empty());
        assert_eq!(plan.masks.len(), 2);
    }

    #[test]
    fn in_place_translation_grows_from_its_source_without_an_external_panel() {
        let block = BlockLayoutInput {
            quad: None,
            fragment_quads: Vec::new(),
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
        let measured =
            normal_measurer().measure(&row.translation, row.width - 8.0, row.font_size, false);
        assert!(measured.height + super::TEXT_VERTICAL_PADDING <= row.height + 0.5);
        assert!(row.x + row.width <= 673.0 && row.y + row.height <= 427.0);
        assert_eq!((plan.content_width, plan.content_height), (673, 427));
        assert!(plan.connectors.is_empty());
    }

    /// A dark capture with light vertical strokes standing in for glyphs.
    fn strokes(width: u32, height: u32, rows: &[(u32, u32, u32, u32, u32)]) -> RgbaFrame {
        let mut frame = page(width, height, [18, 20, 26]);
        for &(top, bottom, left, right, stroke) in rows {
            for y in top..bottom {
                for x in (left..right).filter(|x| (x - left) % 10 < stroke) {
                    let offset = (y * frame.stride + x * 4) as usize;
                    frame.pixels[offset..offset + 3].copy_from_slice(&[236, 232, 222]);
                }
            }
        }
        frame
    }

    #[test]
    fn translations_take_the_size_and_weight_of_the_source_glyphs() {
        // A capitals heading with 21 px glyphs and heavy strokes above body
        // text with 12 px glyphs (ascenders and descenders) and thin ones.
        let frame = strokes(673, 427, &[(20, 41, 140, 340, 4), (54, 66, 140, 600, 1)]);
        let heading = BlockLayoutInput {
            quad: None,
            fragment_quads: Vec::new(),
            block_id: ScreenBlockId::from("arc-title"),
            source: "ARC LIGHTNING".into(),
            translation: "Дуговая молния".into(),
            bounds: local_rect(136, 14, 211, 33),
            fragments: vec![local_rect(136, 14, 211, 33)],
            complete: true,
            confidence_milli: 980,
        };
        let body = BlockLayoutInput {
            quad: None,
            fragment_quads: Vec::new(),
            block_id: ScreenBlockId::from("arc-body"),
            source: "Hurls a bolt of lightning that leaps through nearby enemy units".into(),
            translation: "Бросает молнию, которая перепрыгивает через ближайших врагов".into(),
            bounds: local_rect(134, 49, 470, 22),
            fragments: vec![local_rect(134, 49, 470, 22)],
            complete: true,
            confidence_milli: 980,
        };
        let plan = plan_screen_result_layout_with_frame(
            crop_size(673, 427),
            &[heading, body],
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 1280.0,
                height: 720.0,
            },
            &normal_measurer(),
            Some(&frame),
        );
        assert_eq!((plan.image_width, plan.image_height), (673, 427));
        let (heading, body) = (&plan.blocks[0], &plan.blocks[1]);
        // 21 px capitals are a 29 px font; it may step down to fit.
        assert!(
            heading.font_size >= 0.7 * 21.0 / 0.72,
            "{}",
            heading.font_size
        );
        assert!(heading.font_size > body.font_size);
        assert!(heading.bold && !body.bold);
        assert_eq!(heading.display_translation, "ДУГОВАЯ МОЛНИЯ");
        assert_eq!(heading.source_ink, Some([236, 232, 222]));
        // Each label starts where its glyphs start and sits on their
        // baseline (one line: the label's height is that line plus padding).
        assert_eq!(heading.x, 136.0);
        let line = heading.height - super::TEXT_VERTICAL_PADDING;
        let baseline = heading.y + super::TEXT_VERTICAL_PADDING / 2.0 + super::ASCENT_SHARE * line;
        assert!(
            (baseline - 41.0).abs() <= 1.0,
            "heading baseline {baseline}"
        );
        assert!(!text_collides(heading, body));
    }

    struct RecordingMeasurer {
        measured: RefCell<Vec<String>>,
    }

    impl ScreenTextMeasurer for RecordingMeasurer {
        fn measure(&self, text: &str, width: f32, font_size: f32, bold: bool) -> LogicalTextSize {
            self.measured.borrow_mut().push(text.to_owned());
            FakeMeasurer {
                height_multiplier: 1.0,
            }
            .measure(text, width, font_size, bold)
        }
    }

    #[test]
    fn dual_mode_measures_the_actual_source_and_translation_projection() {
        let block = BlockLayoutInput {
            quad: None,
            fragment_quads: Vec::new(),
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
            quad: None,
            fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("description-continuation"),
                source: "deal damage.".to_owned(),
                translation: "наносит урон.".to_owned(),
                bounds: local_rect(135, 68, 108, 33),
                fragments: vec![local_rect(135, 68, 108, 33)],
                complete: true,
                confidence_milli: 969,
            },
            BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("damage-type"),
                source: "DAMAGE TYPE: Magical".to_owned(),
                translation: "ТИП УРОНА: Магический".to_owned(),
                bounds: local_rect(30, 198, 159, 21),
                fragments: vec![local_rect(30, 198, 159, 21)],
                complete: true,
                confidence_milli: 980,
            },
            BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
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
            quad: None,
            fragment_quads: Vec::new(),
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
            quad: None,
            fragment_quads: Vec::new(),
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
            quad: None,
            fragment_quads: Vec::new(),
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
            false,
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
            quad: None,
            fragment_quads: Vec::new(),
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
            quad: None,
            fragment_quads: Vec::new(),
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
    fn snapshot_rebuilds_the_background_under_replaced_text_and_keeps_the_original() {
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
        // Dark "glyphs" in the source row: the underlay must hide them.
        for y in 30..42 {
            for x in 110..270 {
                let offset = (y * 400 + x) * 4;
                pixels[offset..offset + 3].copy_from_slice(&[30, 30, 30]);
            }
        }
        let frame = RgbaFrame::try_new(crop_rect, 400 * 4, pixels).unwrap();
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: RequestId::new(),
            revision: 1,
            crop: Some(Arc::new(frame)),
            crop_rect,
            blocks: vec![crate::screen_result::ScreenResultBlockSnapshot {
                quad: None,
                fragment_quads: Vec::new(),
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
        // The picture under the translation is the capture with the source
        // glyphs rebuilt from the page around them: no plate, no dark text.
        assert!(window.get_screen_use_repaired_image());
        let repaired = window
            .get_screen_repaired_image()
            .to_rgba8()
            .expect("the rebuilt capture is a pixel buffer");
        for (x, y) in [(110, 30), (190, 36), (269, 41)] {
            assert_eq!(
                sample(&repaired, x, y),
                [240, 240, 240],
                "glyph pixel ({x},{y})"
            );
        }
        // The translation is drawn straight on it in the source's own ink.
        assert_eq!(row.ink, slint::Color::from_rgb_u8(30, 30, 30));
        assert_eq!(
            row.shadow.alpha(),
            0,
            "dark ink on a light page needs no shadow"
        );
        // This corner is outside both the source glyphs and the label.
        let outside_x = 10;
        let outside_image_y = 10;
        assert!(row.x > outside_x as f32 || row.y > outside_image_y as f32);
        let outside_y = outside_image_y + 38;
        assert_eq!(sample(&rendered, outside_x, outside_y), [240, 240, 240]);

        // ORIG shows the capture itself, glyphs included.
        window.set_screen_view_mode(1);
        super::apply_snapshot(&window, &snapshot).unwrap();
        let original = window.window().take_snapshot().unwrap();
        assert!(!window.get_screen_use_repaired_image());
        assert_eq!(sample(&original, 190, 36 + 38), [30, 30, 30]);
    }

    #[test]
    fn snapshot_without_safe_background_keeps_pixels_and_renders_opaque_fallback() {
        use std::sync::Arc;

        use rust_protocol::{
            BlockCompletion, PhysicalPoint, PhysicalQuad, RequestId, SourceContext,
        };
        use slint::{ComponentHandle, LogicalSize, Model};

        // Headless Skia renders the production rotation; the minimal software
        // renderer ignores it and could not verify this label's actual plate.
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("skia".into()),
            },
        )))
        .unwrap();
        let mut frame = page(400, 250, [240, 240, 240]);
        for y in 115..135 {
            for x in 90..110 {
                let offset = ((y * 400 + x) * 4) as usize;
                frame.pixels[offset..offset + 3].copy_from_slice(&[20, 30, 40]);
            }
        }
        let original = frame.pixels.clone();
        let crop_rect = frame.rect;
        // The rotated source occupies every observed pixel. Its apparent
        // rectification margin is off-image replication, not usable background.
        let quad = PhysicalQuad {
            points: [(400, 0), (400, 250), (0, 250), (0, 0)].map(|(x, y)| PhysicalPoint { x, y }),
        };
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: RequestId::new(),
            revision: 1,
            crop: Some(Arc::new(frame)),
            crop_rect,
            blocks: vec![crate::screen_result::ScreenResultBlockSnapshot {
                block_id: ScreenBlockId::from("no-background"),
                source: "Y".into(),
                translation: "X".into(),
                completion: BlockCompletion::Completed,
                failure_message: None,
                bounds: crop_rect,
                fragments: vec![crop_rect],
                quad: Some(quad),
                fragment_quads: vec![quad],
                confidence_milli: 980,
            }],
            context: SourceContext::default(),
            created_unix_ms: 1,
            expires_unix_ms: u64::MAX,
        };
        let window = crate::ScreenTranslationWindow::new().unwrap();
        window.set_screen_view_mode(3);
        super::apply_snapshot(&window, &snapshot).unwrap();
        let row = window.get_screen_blocks().row_data(0).unwrap();
        assert!(row.placed && row.callout && !row.failed && !row.not_placeable);
        assert_eq!(row.angle, 90.0);
        assert_eq!(row.plate.alpha(), 255);
        assert_eq!(window.get_screen_failed_blocks(), 0);
        assert_eq!(snapshot.crop.as_ref().unwrap().pixels, original);
        for image in [
            window.get_screen_crop_image(),
            window.get_screen_repaired_image(),
        ] {
            assert_eq!(image.to_rgba8().unwrap().as_bytes(), original.as_slice());
        }
        assert!(window.get_screen_use_repaired_image());

        window.show().unwrap();
        window.window().set_size(LogicalSize::new(
            window.get_screen_content_width(),
            window.get_screen_content_height() + 38.0,
        ));
        let sample = |image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            // Inside the source marker, far from the rotated single glyph.
            let offset = ((125 + 38) * image.width() as usize + 100) * 4;
            image.as_bytes()[offset..offset + 4].to_vec()
        };
        let rendered = window.window().take_snapshot().unwrap();
        assert_eq!(
            sample(&rendered),
            [row.plate.red(), row.plate.green(), row.plate.blue(), 255],
            "the actual translated label must cover retained source ink opaquely"
        );
        assert_ne!(sample(&rendered), [20, 30, 40, 255]);

        window.set_screen_view_mode(1);
        super::apply_snapshot(&window, &snapshot).unwrap();
        assert!(!window.get_screen_use_repaired_image());
        let original_view = window.window().take_snapshot().unwrap();
        assert_eq!(sample(&original_view), [20, 30, 40, 255]);
    }

    fn page(width: u32, height: u32, background: [u8; 3]) -> RgbaFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&[background[0], background[1], background[2], 255]);
        }
        RgbaFrame::try_new(
            rust_protocol::PhysicalRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            width * 4,
            pixels,
        )
        .unwrap()
    }

    #[test]
    fn a_label_on_a_light_page_takes_the_page_colour_and_dark_ink() {
        let mut frame = page(120, 60, [250, 250, 248]);
        // Dark glyphs inside the row must not colour the plate.
        for y in 20..30 {
            for x in 30..60 {
                let offset = (y * frame.stride + x * 4) as usize;
                frame.pixels[offset..offset + 3].copy_from_slice(&[20, 20, 20]);
            }
        }

        let plate = super::surrounding_color(&frame, (30.0, 20.0, 30.0, 10.0), (1.0, 1.0)).unwrap();

        assert_eq!(plate, [250, 250, 248]);
        assert_eq!(super::ink_for(plate), super::DARK_INK);
    }

    #[test]
    fn a_label_on_a_dark_interface_keeps_the_light_ink() {
        let frame = page(120, 60, [17, 20, 26]);
        let plate = super::surrounding_color(&frame, (10.0, 10.0, 40.0, 12.0), (1.0, 1.0)).unwrap();

        assert_eq!(plate, [17, 20, 26]);
        assert_eq!(super::ink_for(plate), super::LIGHT_INK);
    }

    #[test]
    fn a_label_outside_the_picture_has_no_sampled_colour() {
        // A callout placed below the image: the design plate applies.
        let frame = page(120, 60, [250, 250, 248]);
        assert_eq!(
            super::surrounding_color(&frame, (0.0, 90.0, 100.0, 20.0), (1.0, 1.0)),
            None
        );
    }

    #[test]
    fn an_expired_result_says_copied_once_the_copy_is_confirmed() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
        let window = crate::ScreenTranslationWindow::new().unwrap();
        window.set_screen_image_expired(true);
        window.show().unwrap();
        assert!(
            ElementHandle::find_by_accessible_label(&window, "COPY TEXT")
                .next()
                .is_some()
        );

        // What the desktop does when the input host answers TextCopied.
        window.set_screen_copied(true);

        assert!(
            ElementHandle::find_by_accessible_label(&window, "COPIED")
                .next()
                .is_some(),
            "the production window must show the confirmation"
        );
    }

    #[test]
    fn partial_result_exposes_retry_and_disables_it_while_translating() {
        use rust_protocol::{BlockCompletion, RequestId, SourceContext};
        use slint::{
            ComponentHandle, LogicalPosition, LogicalSize, Model,
            platform::{PointerEventButton, WindowEvent},
        };
        use std::{cell::Cell, rc::Rc, sync::Arc};
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
            width: 480,
            height: 240,
        };
        let crop = RgbaFrame::try_new(crop_rect, 480 * 4, vec![40; 480 * 240 * 4]).unwrap();
        let snapshot = crate::screen_result::ScreenResultSnapshot {
            request_id: RequestId::new(),
            revision: 1,
            crop: Some(Arc::new(crop)),
            crop_rect,
            blocks: [
                (
                    "title",
                    "Tier 4 Artifact",
                    "Артефакт 4 уровня",
                    BlockCompletion::Completed,
                ),
                ("cooldown", "30", "", BlockCompletion::Failed),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (id, source, translation, completion))| {
                crate::screen_result::ScreenResultBlockSnapshot {
                    quad: None,
                    fragment_quads: Vec::new(),
                    block_id: ScreenBlockId::from(id),
                    source: source.into(),
                    translation: translation.into(),
                    completion,
                    failure_message: (completion == BlockCompletion::Failed)
                        .then(|| "protected_values_changed".into()),
                    bounds: PhysicalRect {
                        x: 20,
                        y: 25 + i as i32 * 60,
                        width: 200,
                        height: 30,
                    },
                    fragments: vec![],
                    confidence_milli: 990,
                }
            })
            .collect(),
            context: SourceContext::default(),
            created_unix_ms: 1,
            expires_unix_ms: u64::MAX,
        };
        let window = crate::ScreenTranslationWindow::new().unwrap();
        super::apply_snapshot(&window, &snapshot).unwrap();
        assert_eq!(window.get_screen_failed_blocks(), 1);
        let rows = window.get_screen_blocks();
        assert!(rows.row_data(0).unwrap().placed);
        assert!(rows.row_data(1).unwrap().failed && rows.row_data(1).unwrap().not_placeable);
        let count = Rc::new(Cell::new(0));
        let clicks = count.clone();
        window.on_screen_translation_retry_requested(move || clicks.set(clicks.get() + 1));
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(600.0, 420.0));
        window.window().take_snapshot().unwrap();
        let click = || {
            let position = LogicalPosition::new(545.0, 64.0);
            window
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position });
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        };
        click();
        assert_eq!(
            count.get(),
            1,
            "visible retry button dispatches one request"
        );
        window.set_screen_translating(true);
        click();
        assert_eq!(
            count.get(),
            1,
            "processing must not dispatch a duplicate retry"
        );
        window.set_screen_translating(false);
        window.set_screen_translation_retry_enabled(false);
        click();
        assert_eq!(
            count.get(),
            1,
            "restored results cannot overwrite lost edit provenance"
        );
    }

    #[test]
    fn busy_image_disables_save_and_delete_until_translation_finishes() {
        use slint::{
            ComponentHandle, LogicalPosition, LogicalSize,
            platform::{PointerEventButton, WindowEvent},
        };
        use std::{cell::Cell, rc::Rc};

        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
        let window = crate::ScreenTranslationWindow::new().unwrap();
        let actions = Rc::new(Cell::new([0, 0]));
        let saved = Rc::clone(&actions);
        window.on_screen_save_file_requested(move || {
            let mut counts = saved.get();
            counts[0] += 1;
            saved.set(counts);
        });
        let deleted = Rc::clone(&actions);
        window.on_screen_delete_image_requested(move || {
            let mut counts = deleted.get();
            counts[1] += 1;
            deleted.set(counts);
        });
        window.set_screen_translating(true);
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(600.0, 420.0));
        window.window().take_snapshot().unwrap();
        let click = |x| {
            let position = LogicalPosition::new(x, 20.0);
            window
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position });
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        };
        click(214.0);
        click(255.0);
        assert_eq!(
            actions.get(),
            [0, 0],
            "pending image actions must not dispatch"
        );
        window.set_screen_translating(false);
        click(214.0);
        click(255.0);
        assert_eq!(
            actions.get(),
            [1, 1],
            "finished image keeps its save/delete actions"
        );
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
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("short"),
                source: "Menu".to_owned(),
                translation: "Меню".to_owned(),
                bounds: local_rect(100, 50, 200, 80),
                fragments: vec![local_rect(100, 50, 200, 80)],
                complete: true,
                confidence_milli: 900,
            },
            BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
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
        // The label is as wide as its text, never a plate the width of the box.
        assert!(laid_out[0].width < 100.0 && laid_out[0].width > 8.0);
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
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
                block_id: ScreenBlockId::from("first"),
                source: "first".to_owned(),
                translation: long.clone(),
                bounds: local_rect(20, 20, 70, 20),
                fragments: vec![local_rect(20, 20, 70, 20)],
                complete: true,
                confidence_milli: 900,
            },
            BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
        // Labels taller than these 11 px rows are centred on them, so a label
        // may start above its row, but never by more than half its height.
        assert!(
            rows.iter()
                .all(|row| row.placed && row.y - row.source_y <= 10.0)
        );
        assert!(
            rows.iter()
                .all(|row| row.y >= row.source_y - row.height / 2.0)
        );
    }

    #[test]
    fn dense_results_never_create_an_external_translation_column() {
        let long = "A translated paragraph that cannot fit inside its source rectangle ".repeat(5);
        let blocks = (0..8)
            .map(|index| BlockLayoutInput {
                quad: None,
                fragment_quads: Vec::new(),
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
                quad: None,
                fragment_quads: Vec::new(),
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
                    quad: None,
                    fragment_quads: Vec::new(),
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
        // The source label is set in capitals, so is its translation.
        assert_eq!(long.blocks[7].display_translation, "ДЛИТЕЛЬНОСТЬ 30");
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
        assert_eq!(short.blocks[7].display_translation, "СРОК");
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
            // Drawn in capitals like the source label.
            assert_eq!(plan.blocks[7].display_translation, expected.to_uppercase());
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
                let shown = if mode == ResultViewMode::Dual {
                    translation.to_uppercase()
                } else {
                    translation.to_owned()
                };
                assert_eq!(plan.blocks[7].display_translation, shown);
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
        // Slint's minimal software renderer ignores arbitrary rotation. Use
        // the headless Skia renderer for evidence of angled labels.
        let renderer =
            std::env::var("SAYFLEX_SCREEN_SAMPLE_RENDERER").unwrap_or_else(|_| "skia".to_owned());
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some(renderer.into()),
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
                        quad: block.quad,
                        fragment_quads: block.fragment_quads.clone(),
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
        let layouts = super::apply_snapshot(&preview, &snapshot)
            .expect("snapshot should bind to the real view");
        if let Some(frame) = snapshot.crop.as_deref() {
            for (index, (block, layout)) in snapshot.blocks.iter().zip(&layouts).enumerate() {
                let inputs = snapshot
                    .blocks
                    .iter()
                    .map(|block| super::BlockLayoutInput {
                        quad: None,
                        fragment_quads: Vec::new(),
                        block_id: block.block_id.clone(),
                        source: block.source.clone(),
                        translation: block.translation.clone(),
                        bounds: super::CropLocalRect::from_ocr_bounds(block.bounds),
                        fragments: Vec::new(),
                        complete: true,
                        confidence_milli: block.confidence_milli,
                    })
                    .collect::<Vec<_>>();
                let metrics = crate::screen_inpaint::glyph_metrics(
                    frame,
                    super::pixel_rect(inputs[index].bounds),
                    super::horizontal_reach(index, &inputs),
                );
                eprintln!(
                    "{:?} box={:?} glyphs={:?} -> label=({:.0},{:.0},{:.0},{:.0}) font={:.1} bold={} text={:?}",
                    block.source,
                    block.bounds,
                    metrics,
                    layout.x,
                    layout.y,
                    layout.width,
                    layout.height,
                    layout.font_size,
                    layout.bold,
                    layout.display_translation
                );
            }
        }
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

    /// Whether two labels' text collides, beyond the leading their line
    /// boxes may share.
    fn text_collides(left: &super::BlockLayout, right: &super::BlockLayout) -> bool {
        let shared = (left.y + left.height).min(right.y + right.height) - left.y.max(right.y);
        overlaps(left, right) && shared > super::LEADING_OVERLAP * left.height.min(right.height)
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
