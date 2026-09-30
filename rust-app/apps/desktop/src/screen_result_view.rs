use rust_protocol::{BlockCompletion, PhysicalRect, ScreenBlockId};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::screen_result::ScreenResultSnapshot;
use crate::{
    ScreenBlockOverlay, ScreenConnectorOverlay, ScreenMaskOverlay, ScreenTranslationWindow,
};

const MIN_FONT_SIZE: f32 = 12.0;
const FONT_SIZES: [f32; 3] = [14.0, 13.0, MIN_FONT_SIZE];
// The layout runs on the UI thread. A distant empty patch is less useful than
// the guaranteed below-image fallback, so bound full pixel checks per row.
const MAX_QUIET_CALLOUT_CANDIDATES: usize = 96;
const MAX_QUIET_CANDIDATES_PER_LAYOUT: usize = 256;
const MAX_QUIET_PIXELS_PER_LAYOUT: usize = 2_000_000;
type LayoutRect = (f32, f32, f32, f32);
type OccupiedSource = (usize, LayoutRect);

struct QuietSearchImage<'a> {
    frame: &'a rust_platform_windows::screen_capture::RgbaFrame,
    crop: CropPixelSize,
    viewport: (f32, f32),
}

struct QuietSearchBudget {
    candidates_left: usize,
    pixels_left: usize,
}

impl QuietSearchBudget {
    fn for_layout() -> Self {
        Self {
            candidates_left: MAX_QUIET_CANDIDATES_PER_LAYOUT,
            pixels_left: MAX_QUIET_PIXELS_PER_LAYOUT,
        }
    }

    fn take_candidate(&mut self) -> bool {
        if self.candidates_left == 0 || self.pixels_left == 0 {
            return false;
        }
        self.candidates_left -= 1;
        true
    }

    fn take_pixels(&mut self, count: usize) -> bool {
        if count > self.pixels_left {
            return false;
        }
        self.pixels_left -= count;
        true
    }
}
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
    pub(crate) low_confidence: bool,
    pub(crate) replace_source: bool,
    pub(crate) placed: bool,
    pub(crate) callout: bool,
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
    frame: Option<&rust_platform_windows::screen_capture::RgbaFrame>,
) -> ScreenResultLayoutPlan {
    let mut budget = QuietSearchBudget::for_layout();
    plan_screen_result_layout_with_budget(
        crop,
        blocks,
        projection,
        work_area,
        measurer,
        frame,
        &mut budget,
    )
}

fn plan_screen_result_layout_with_budget(
    crop: CropPixelSize,
    blocks: &[BlockLayoutInput],
    projection: ResultViewMode,
    work_area: LogicalWorkArea,
    measurer: &dyn ScreenTextMeasurer,
    frame: Option<&rust_platform_windows::screen_capture::RgbaFrame>,
    quiet_search_budget: &mut QuietSearchBudget,
) -> ScreenResultLayoutPlan {
    let (image_width, image_height) = fitted_viewport(crop, work_area);
    if crop.width == 0 || crop.height == 0 || image_width == 0 || image_height == 0 {
        return ScreenResultLayoutPlan {
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
            let overlay_mode = matches!(projection, ResultViewMode::OnImage | ResultViewMode::Dual);
            let replace_source = overlay_mode
                && block.complete
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
                low_confidence: block.confidence_milli < 650,
                replace_source,
                placed: false,
                callout: false,
                not_placeable: overlay_mode && !block.complete && translatable_source,
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
        let measured_text = projected_text(&layouts[index], projection);
        let candidate = FONT_SIZES.into_iter().find_map(|font_size| {
            measured_candidate(&measured_text, source, font_size, measurer, viewport)
                .filter(|candidate| {
                    let source_collision = occupied_source.iter().any(|(owner, occupied)| {
                        *owner != index && rectangles_overlap(*candidate, *occupied)
                    });
                    !source_collision
                        && !placed_rects
                            .iter()
                            .any(|occupied| rectangles_overlap(*candidate, *occupied))
                })
                .map(|candidate| (candidate, font_size))
        });
        if let Some((candidate, font_size)) = candidate {
            let row = &mut layouts[index];
            row.x = candidate.0;
            row.y = candidate.1;
            row.width = candidate.2;
            row.height = candidate.3;
            row.font_size = font_size;
            row.placed = true;
            placed_rects.push(candidate);
            masks.extend(scaled_fragments[index].iter().map(|fragment| MaskLayout {
                block_id: row.block_id.clone(),
                x: fragment.0,
                y: fragment.1,
                width: fragment.2,
                height: fragment.3,
            }));
        }
    }

    // An OCR row is not a text-layout box: the translated phrase can be much
    // longer than its original. Keep its source glyphs masked and give the
    // translation readable space below the image when no in-place fit exists.
    let mut content_height = image_height as f32;
    for index in 0..layouts.len() {
        if !layouts[index].replace_source || layouts[index].placed {
            continue;
        }
        let font_size = FONT_SIZES[0];
        let measured_text = projected_text(&layouts[index], projection);
        let source = (
            layouts[index].source_x,
            layouts[index].source_y,
            layouts[index].source_width,
            layouts[index].source_height,
        );
        let quiet_candidate = frame.and_then(|frame| {
            [520.0_f32, 360.0, 240.0]
                .into_iter()
                .filter_map(|preferred_width| {
                    let width = preferred_width.min((image_width as f32 - 16.0).max(1.0));
                    let measured = measurer.measure(
                        &measured_text,
                        (width - TEXT_HORIZONTAL_PADDING).max(1.0),
                        font_size,
                    );
                    let height = measured.height + TEXT_VERTICAL_PADDING + 8.0;
                    (measured.height.is_finite() && height <= image_height as f32 - 16.0)
                        .then_some((width, height))
                })
                .find_map(|(width, height)| {
                    find_quiet_callout(
                        QuietSearchImage {
                            frame,
                            crop,
                            viewport,
                        },
                        (width, height),
                        source,
                        &occupied_source,
                        &placed_rects,
                        quiet_search_budget,
                    )
                })
        });
        if let Some(candidate) = quiet_candidate {
            let row = &mut layouts[index];
            row.x = candidate.0;
            row.y = candidate.1;
            row.width = candidate.2;
            row.height = candidate.3;
            row.font_size = font_size;
            row.placed = true;
            row.callout = true;
            placed_rects.push(candidate);
            masks.extend(scaled_fragments[index].iter().map(|fragment| MaskLayout {
                block_id: row.block_id.clone(),
                x: fragment.0,
                y: fragment.1,
                width: fragment.2,
                height: fragment.3,
            }));
            continue;
        }

        let width = (image_width as f32 - 16.0).clamp(1.0, 560.0);
        let measured = measurer.measure(
            &measured_text,
            (width - TEXT_HORIZONTAL_PADDING).max(1.0),
            font_size,
        );
        if !measured.height.is_finite() || measured.height <= 0.0 {
            layouts[index].not_placeable = true;
            unplaceable_rows.push(layouts[index].block_id.clone());
            continue;
        }
        let height = measured.height + TEXT_VERTICAL_PADDING + 8.0;
        let x = layouts[index]
            .source_x
            .clamp(8.0, (image_width as f32 - width - 8.0).max(8.0));
        content_height += 10.0;
        let row = &mut layouts[index];
        row.x = x;
        row.y = content_height;
        row.width = width;
        row.height = height;
        row.font_size = font_size;
        row.placed = true;
        row.callout = true;
        masks.extend(scaled_fragments[index].iter().map(|fragment| MaskLayout {
            block_id: row.block_id.clone(),
            x: fragment.0,
            y: fragment.1,
            width: fragment.2,
            height: fragment.3,
        }));
        content_height += height;
    }
    if content_height > image_height as f32 {
        content_height += 8.0;
    }
    let content_height = content_height.ceil().min(u32::MAX as f32) as u32;
    let viewport_height = content_height.min((work_area.height - 70.0).max(1.0) as u32);
    let connectors = layouts
        .iter()
        .filter(|row| row.callout)
        .map(callout_connector)
        .collect();

    ScreenResultLayoutPlan {
        blocks: layouts,
        image_width,
        image_height,
        content_width: image_width,
        content_height,
        viewport_height,
        masks,
        connectors,
        unplaceable_rows,
    }
}

fn projected_text(row: &BlockLayout, projection: ResultViewMode) -> String {
    match projection {
        ResultViewMode::Dual => format!("{}\n{}", row.source, row.translation),
        _ => row.translation.clone(),
    }
}

fn callout_connector(row: &BlockLayout) -> ConnectorLayout {
    let source_centre_x = row.source_x + row.source_width / 2.0;
    let source_centre_y = row.source_y + row.source_height / 2.0;
    let target_centre_x = row.x + row.width / 2.0;
    let target_centre_y = row.y + row.height / 2.0;
    let horizontal =
        (target_centre_x - source_centre_x).abs() > (target_centre_y - source_centre_y).abs();
    let (start_x, start_y, end_x, end_y) = if horizontal {
        if target_centre_x >= source_centre_x {
            (
                row.source_x + row.source_width,
                source_centre_y,
                row.x,
                target_centre_y,
            )
        } else {
            (
                row.source_x,
                source_centre_y,
                row.x + row.width,
                target_centre_y,
            )
        }
    } else if target_centre_y >= source_centre_y {
        (
            source_centre_x,
            row.source_y + row.source_height,
            target_centre_x,
            row.y,
        )
    } else {
        (
            source_centre_x,
            row.source_y,
            target_centre_x,
            row.y + row.height,
        )
    };
    ConnectorLayout {
        block_id: row.block_id.clone(),
        start_x,
        start_y,
        end_x,
        end_y,
    }
}

fn find_quiet_callout(
    image: QuietSearchImage<'_>,
    size: (f32, f32),
    source: (f32, f32, f32, f32),
    occupied_source: &[OccupiedSource],
    placed: &[LayoutRect],
    budget: &mut QuietSearchBudget,
) -> Option<LayoutRect> {
    let QuietSearchImage {
        frame,
        crop,
        viewport,
    } = image;
    if budget.candidates_left == 0 || budget.pixels_left == 0 {
        return None;
    }
    let max_x = (viewport.0 - size.0).floor() as i32;
    let max_y = (viewport.1 - size.1).floor() as i32;
    if max_x < 8 || max_y < 8 {
        return None;
    }
    enum ProbeResult {
        Found(LayoutRect),
        Continue,
        Exhausted,
    }
    let mut examined = 0;
    let mut probe = |x: f32, y: f32| {
        if examined >= MAX_QUIET_CALLOUT_CANDIDATES || !budget.take_candidate() {
            return ProbeResult::Exhausted;
        }
        examined += 1;
        let candidate = (x, y, size.0, size.1);
        if x >= 8.0
            && y >= 8.0
            && x + size.0 <= viewport.0 - 8.0
            && y + size.1 <= viewport.1 - 8.0
            && !occupied_source
                .iter()
                .any(|(_, rect)| rectangles_overlap(candidate, *rect))
            && !placed
                .iter()
                .any(|rect| rectangles_overlap(candidate, *rect))
            && quiet_image_rect(frame, crop, viewport, candidate, budget)
        {
            ProbeResult::Found(candidate)
        } else {
            ProbeResult::Continue
        }
    };
    let source_centre = (source.0 + source.2 / 2.0, source.1 + source.3 / 2.0);
    let preferred = [
        (source.0, source.1 + source.3 + 8.0),
        (source.0, source.1 - size.1 - 8.0),
        (source.0 + source.2 + 8.0, source.1),
        (source.0 - size.0 - 8.0, source.1),
    ];
    for (x, y) in preferred {
        match probe(x, y) {
            ProbeResult::Found(rect) => return Some(rect),
            ProbeResult::Exhausted => return None,
            ProbeResult::Continue => {}
        }
    }
    let anchor_x = (source_centre.0 - size.0 / 2.0).round() as i32;
    let anchor_y = (source_centre.1 - size.1 / 2.0).round() as i32;
    let max_radius = (viewport.0.max(viewport.1) / 16.0).ceil() as i32 + 2;
    // Visit nearby positions in expanding rings. Never build and sort a full
    // desktop-sized grid for every OCR row, and prefer the below-image fallback
    // once the useful local search budget is exhausted.
    for radius in 0..=max_radius {
        if radius == 0 {
            match probe(anchor_x as f32, anchor_y as f32) {
                ProbeResult::Found(rect) => return Some(rect),
                ProbeResult::Exhausted => return None,
                ProbeResult::Continue => {}
            }
        } else {
            for dx in -radius..=radius {
                for y in [anchor_y - radius * 16, anchor_y + radius * 16] {
                    match probe((anchor_x + dx * 16) as f32, y as f32) {
                        ProbeResult::Found(rect) => return Some(rect),
                        ProbeResult::Exhausted => return None,
                        ProbeResult::Continue => {}
                    }
                }
            }
            for dy in (-radius + 1)..radius {
                for x in [anchor_x - radius * 16, anchor_x + radius * 16] {
                    match probe(x as f32, (anchor_y + dy * 16) as f32) {
                        ProbeResult::Found(rect) => return Some(rect),
                        ProbeResult::Exhausted => return None,
                        ProbeResult::Continue => {}
                    }
                }
            }
        }
    }
    None
}

fn quiet_image_rect(
    frame: &rust_platform_windows::screen_capture::RgbaFrame,
    crop: CropPixelSize,
    viewport: (f32, f32),
    rect: (f32, f32, f32, f32),
    budget: &mut QuietSearchBudget,
) -> bool {
    if frame.rect.width != crop.width || frame.rect.height != crop.height {
        return false;
    }
    let source_left = (rect.0 / viewport.0 * crop.width as f32).floor().max(0.0) as usize;
    let source_top = (rect.1 / viewport.1 * crop.height as f32).floor().max(0.0) as usize;
    let source_right = ((rect.0 + rect.2) / viewport.0 * crop.width as f32)
        .ceil()
        .min(crop.width as f32) as usize;
    let source_bottom = ((rect.1 + rect.3) / viewport.1 * crop.height as f32)
        .ceil()
        .min(crop.height as f32) as usize;
    if !budget.take_pixels(
        source_right
            .saturating_sub(source_left)
            .saturating_mul(source_bottom.saturating_sub(source_top)),
    ) {
        return false;
    }
    let mut baseline: Option<[u8; 3]> = None;
    for y in source_top..source_bottom {
        for x in source_left..source_right {
            let offset = y
                .saturating_mul(frame.stride as usize)
                .saturating_add(x.saturating_mul(4));
            let Some(pixel) = frame.pixels.get(offset..offset.saturating_add(3)) else {
                return false;
            };
            if pixel.iter().copied().max().unwrap_or(255) > 56 {
                return false;
            }
            if let Some(base) = baseline {
                if pixel
                    .iter()
                    .zip(base)
                    .any(|(channel, base)| channel.abs_diff(base) > 10)
                {
                    return false;
                }
            } else {
                baseline = Some([pixel[0], pixel[1], pixel[2]]);
            }
        }
    }
    true
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
            translation: block.translation.clone().into(),
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
    let y = (block.y - window.get_screen_scroll_offset())
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
    fn overflow_uses_a_separate_callout_without_covering_other_image_regions() {
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
        assert!(row.placed && row.callout);
        assert!(row.y >= plan.image_height as f32);
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
        assert_eq!(
            plan.connectors.len(),
            plan.blocks.iter().filter(|block| block.callout).count()
        );
        for connector in &plan.connectors {
            let source = plan
                .blocks
                .iter()
                .find(|block| block.block_id == connector.block_id)
                .unwrap();
            assert!(source.callout);
            assert!(connector.start_x >= source.source_x);
            assert!(connector.start_x <= source.source_x + source.source_width);
            assert!(connector.start_y >= source.source_y);
            assert!(connector.start_y <= source.source_y + source.source_height);
            assert!(connector.end_x >= source.x);
            assert!(connector.end_x <= source.x + source.width);
            assert!(connector.end_y >= source.y);
            assert!(connector.end_y <= source.y + source.height);
        }
    }

    #[test]
    fn long_translation_uses_a_quiet_image_area_but_never_covers_bright_content() {
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
        assert!(row.callout && row.placed);
        assert!(row.y + row.height <= plan.image_height as f32);
        assert!(row.x >= 90.0 || row.y >= 90.0);
        assert!(!super::rectangles_overlap(
            (row.x, row.y, row.width, row.height),
            (0.0, 0.0, 90.0, 90.0),
        ));
    }

    #[test]
    fn nonquiet_image_sends_overflow_below_the_original_pixels() {
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
        assert!(row.callout && row.placed);
        assert!(row.y >= plan.image_height as f32);
        assert!(plan.content_height > plan.image_height);
        assert!(plan.viewport_height <= 250);
    }

    #[test]
    fn dual_callout_reserves_height_for_both_source_and_translation() {
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
        assert!(row.callout);
        let rendered = format!("{}\n{}", row.source, row.translation);
        let measured = normal_measurer().measure(
            &rendered,
            (row.width - super::TEXT_HORIZONTAL_PADDING).max(1.0),
            row.font_size,
        );
        assert!(
            row.height >= measured.height + super::TEXT_VERTICAL_PADDING,
            "dual callout must not clip the source or its translation"
        );
    }

    #[test]
    fn dark_but_textured_content_is_not_treated_as_empty_space() {
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
        assert!(plan.blocks[0].callout);
        assert!(plan.blocks[0].y >= plan.image_height as f32);
    }

    #[test]
    fn quiet_area_check_detects_a_thin_bright_detail_between_grid_samples() {
        let mut pixels = vec![16_u8; 100 * 100 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let detail = &mut pixels[(14 * 100 + 14) * 4..][..4];
        detail[..3].fill(245);
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            100 * 4,
            pixels,
        )
        .unwrap();

        let mut budget = super::QuietSearchBudget::for_layout();
        assert!(!super::quiet_image_rect(
            &frame,
            crop_size(100, 100),
            (100.0, 100.0),
            (10.0, 10.0, 50.0, 50.0),
            &mut budget,
        ));
    }

    #[test]
    fn quiet_area_pixel_budget_is_shared_and_falls_back_conservatively() {
        let mut pixels = vec![16_u8; 100 * 100 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            100 * 4,
            pixels,
        )
        .unwrap();
        let mut budget = super::QuietSearchBudget {
            candidates_left: 2,
            pixels_left: 2_500,
        };
        let candidate = (0.0, 0.0, 50.0, 50.0);
        assert!(super::quiet_image_rect(
            &frame,
            crop_size(100, 100),
            (100.0, 100.0),
            candidate,
            &mut budget,
        ));
        assert_eq!(budget.pixels_left, 0);
        assert!(!super::quiet_image_rect(
            &frame,
            crop_size(100, 100),
            (100.0, 100.0),
            candidate,
            &mut budget,
        ));
        assert_eq!(budget.pixels_left, 0);
        let mut candidate_budget = super::QuietSearchBudget {
            candidates_left: 1,
            pixels_left: 1,
        };
        assert!(candidate_budget.take_candidate());
        assert!(!candidate_budget.take_candidate());
    }

    #[test]
    fn dense_result_shares_one_candidate_budget_across_rows_and_widths() {
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
        let blocks = [25, 100]
            .into_iter()
            .enumerate()
            .map(|(index, y)| BlockLayoutInput {
                block_id: ScreenBlockId::from(format!("row-{index}")),
                source: "Short".to_owned(),
                translation: "Длинный перевод, который не помещается в исходную строку".to_owned(),
                bounds: local_rect(100, y, 100, 20),
                fragments: vec![local_rect(100, y, 100, 20)],
                complete: true,
                confidence_milli: 980,
            })
            .collect::<Vec<_>>();
        let mut budget = super::QuietSearchBudget {
            candidates_left: 5,
            pixels_left: 100_000,
        };
        let plan = super::plan_screen_result_layout_with_budget(
            crop_size(400, 250),
            &blocks,
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 432.0,
                height: 320.0,
            },
            &normal_measurer(),
            Some(&frame),
            &mut budget,
        );
        assert_eq!(budget.candidates_left, 0);
        assert_eq!(plan.connectors.len(), 2);
        assert!(
            plan.blocks
                .iter()
                .all(|row| row.callout && row.y >= plan.image_height as f32)
        );
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
    fn snapshot_binds_a_visible_callout_and_scrollable_canvas_to_the_real_window() {
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
                translation: "Длинный перевод для читаемой выноски под исходным изображением "
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

        assert!(row.placed && row.callout);
        assert!(
            window.get_screen_total_content_height() > window.get_screen_image_content_height()
        );
        assert!(window.get_screen_content_height() <= window.get_screen_total_content_height());
        let connector = window.get_screen_connectors().row_data(0).unwrap();
        window.show().unwrap();
        window.window().set_size(LogicalSize::new(
            window.get_screen_content_width(),
            window.get_screen_content_height() + 38.0,
        ));
        let rendered = window.window().take_snapshot().unwrap();
        let x = ((connector.start_x + connector.end_x) / 2.0).round() as i32;
        let y = ((connector.start_y + connector.end_y) / 2.0 + 38.0).round() as i32;
        let pixels = rendered.as_bytes();
        let mut found_line_pixel = false;
        for py in (y - 4).max(0)..=(y + 4).min(rendered.height() as i32 - 1) {
            for px in (x - 4).max(0)..=(x + 4).min(rendered.width() as i32 - 1) {
                let offset = (py as usize * rendered.width() as usize + px as usize) * 4;
                let pixel = &pixels[offset..offset + 3];
                if pixel[0] > pixel[1].saturating_add(25) && pixel[1] > pixel[2].saturating_add(35)
                {
                    found_line_pixel = true;
                }
            }
        }
        assert!(
            found_line_pixel,
            "the source-to-callout line must be visibly painted"
        );
    }

    #[test]
    fn block_geometry_scales_and_long_text_remains_readable_in_a_callout() {
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
        assert!(laid_out[1].placed && laid_out[1].callout);
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
    fn dense_results_place_every_complete_translation_in_the_scrollable_canvas() {
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
        assert!(laid_out.iter().any(|block| block.callout));
        for (index, left) in laid_out.iter().enumerate() {
            for right in laid_out.iter().skip(index + 1) {
                if left.callout && right.callout {
                    assert!(!overlaps(left, right));
                }
            }
        }
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
    fn nimbus_renderer_measurement_sends_oversized_text_to_scrollable_callouts() {
        let plan = plan_screen_result_layout(
            crop_size(682, 466),
            &nimbus_layout_inputs(),
            ResultViewMode::OnImage,
            LogicalWorkArea {
                width: 740.0,
                height: 540.0,
            },
            &FakeMeasurer {
                height_multiplier: 20.0,
            },
        );

        assert!(plan.blocks.iter().any(|block| block.callout));
        assert!(
            plan.blocks
                .iter()
                .filter(|block| block.replace_source)
                .all(|block| block.placed)
        );
        assert!(plan.content_height > plan.viewport_height);
    }

    #[test]
    fn nimbus_small_work_area_keeps_all_callouts_inside_a_scrollable_canvas() {
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
        assert!(plan.content_height > plan.viewport_height);
        assert!(plan.blocks.iter().filter(|row| row.placed).all(|row| {
            row.x >= 0.0
                && row.y >= 0.0
                && row.x + row.width <= plan.image_width as f32
                && row.y + row.height <= plan.content_height as f32
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
