use rust_protocol::{OcrFailureKind, OcrLine, PhysicalPoint, PhysicalQuad, PhysicalRect};

const MAX_DENSE_BLOCK_LINES: u64 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedText {
    pub text: String,
    pub blocks: Vec<ReconstructedBlock>,
    /// OCR rows in reading order, retaining the exact rectangles that were
    /// detected in the crop. `blocks` are semantic context groups; these
    /// elements are the only geometry suitable for replacing text in-place.
    pub elements: Vec<ReconstructedElement>,
    pub accepted_line_count: usize,
    pub average_confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedBlock {
    pub text: String,
    pub bounds: rust_protocol::PhysicalRect,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedElement {
    pub text: String,
    pub bounds: rust_protocol::PhysicalRect,
    pub quad: Option<PhysicalQuad>,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedVisualRow {
    pub text: String,
    pub bounds: rust_protocol::PhysicalRect,
    pub quad: Option<PhysicalQuad>,
    pub fragments: Vec<rust_protocol::PhysicalRect>,
    pub fragment_quads: Vec<PhysicalQuad>,
    pub confidence_milli: u16,
}

pub fn reconstruct_visual_rows(elements: &[ReconstructedElement]) -> Vec<ReconstructedVisualRow> {
    let mut rows: Vec<VisualRowBuilder> = Vec::new();
    for element in elements {
        let target = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.accepts(element))
            .min_by_key(|(_, row)| row.baseline_distance(element))
            .map(|(index, _)| index);
        if let Some(index) = target {
            rows[index].push(element);
        } else {
            rows.push(VisualRowBuilder::new(element));
        }
    }
    rows.into_iter().map(VisualRowBuilder::finish).collect()
}

/// Conservatively join wrapped document lines after detecting visual rows.
/// Short labels, separate columns, dates and unknown legacy geometry retain
/// their own blocks. Exact line polygons remain available for source masks.
pub fn reconstruct_visual_paragraphs(
    rows: &[ReconstructedVisualRow],
) -> Vec<ReconstructedVisualRow> {
    let mut paragraphs: Vec<ReconstructedVisualRow> = Vec::new();
    let mut previous: Option<&ReconstructedVisualRow> = None;
    let mut line_count = 0_u64;
    let mut previous_step: Option<f64> = None;
    for row in rows {
        let step = previous.and_then(|previous| {
            let frame = QuadFrame::new(previous.quad?);
            let a = frame.extent(previous.quad?);
            let b = frame.extent(row.quad?);
            Some((b.1 + b.3 - a.1 - a.3) * 0.5)
        });
        let joins = previous.is_some_and(|previous| paragraph_continues(previous, row))
            // Detector polygons include padding around the ink. Once a
            // paragraph establishes its spacing, a larger blank-line gap
            // must not be mistaken for another line of that paragraph.
            && previous_step.zip(step).is_none_or(|(previous, next)| next <= previous * 1.5)
            && line_count < MAX_DENSE_BLOCK_LINES
            && paragraphs.last().is_some_and(|paragraph| {
                paragraph.text.len() + row.text.len()
                    < rust_protocol::MAX_SCREEN_BLOCK_TEXT_BYTES
            });
        if joins {
            let paragraph = paragraphs.last_mut().expect("previous paragraph");
            let old_count = paragraph.fragments.len() as u64;
            let next_count = row.fragments.len() as u64;
            let confidence = u64::from(paragraph.confidence_milli) * old_count
                + u64::from(row.confidence_milli) * next_count;
            if lexical_hyphen_continuation(&paragraph.text, &row.text) {
                paragraph.text.pop();
                paragraph.text.push_str(row.text.trim());
            } else {
                append_visual_fragment(&mut paragraph.text, &row.text);
            }
            paragraph.fragments.extend_from_slice(&row.fragments);
            paragraph
                .fragment_quads
                .extend_from_slice(&row.fragment_quads);
            let quad = QuadFrame::new(paragraph.quad.expect("validated paragraph geometry"))
                .enclose(&paragraph.fragment_quads);
            paragraph.quad = Some(quad);
            paragraph.bounds =
                union_rect(union_rect(paragraph.bounds, row.bounds), quad_bounds(quad));
            paragraph.confidence_milli = (confidence / (old_count + next_count).max(1)) as u16;
            line_count += 1;
        } else {
            paragraphs.push(row.clone());
            line_count = 1;
        }
        previous_step = if joins { step } else { None };
        previous = Some(row);
    }
    paragraphs
}

fn paragraph_continues(previous: &ReconstructedVisualRow, next: &ReconstructedVisualRow) -> bool {
    let (Some(previous_quad), Some(next_quad)) = (previous.quad, next.quad) else {
        return false;
    };
    if !previous_quad.is_within(previous.bounds)
        || !next_quad.is_within(next.bounds)
        || previous.fragment_quads.is_empty()
        || next.fragment_quads.is_empty()
        || previous.fragment_quads.len() != previous.fragments.len()
        || next.fragment_quads.len() != next.fragments.len()
        || starts_list_item(&next.text)
    {
        return false;
    }
    let frame = QuadFrame::new(previous_quad);
    let other = QuadFrame::new(next_quad);
    if frame.x * other.x + frame.y * other.y < 3_f64.to_radians().cos() {
        return false;
    }
    let a = frame.extent(previous_quad);
    let b = frame.extent(next_quad);
    let height = (a.3 - a.1).max(1.0);
    let next_height = (b.3 - b.1).max(1.0);
    let step = (b.1 + b.3 - a.1 - a.3) * 0.5;
    let minimum_height = height.min(next_height);
    let maximum_height = height.max(next_height);
    // A short final line ends a paragraph. A full-width line may contain a
    // sentence ending, so punctuation alone is not a paragraph boundary.
    a.2 - a.0 >= height * 12.0
        && maximum_height <= minimum_height * 1.4
        && (a.0 - b.0).abs() <= maximum_height
        && step >= minimum_height * 0.75
        && step <= maximum_height * 1.8
}

struct VisualRowBuilder {
    text: String,
    bounds: rust_protocol::PhysicalRect,
    quad: Option<PhysicalQuad>,
    fragments: Vec<rust_protocol::PhysicalRect>,
    fragment_quads: Vec<PhysicalQuad>,
    confidence_total: u64,
    fragment_count: u64,
    contains_unmergeable_noise: bool,
}

impl VisualRowBuilder {
    fn new(element: &ReconstructedElement) -> Self {
        let quad = element.quad.filter(|quad| quad.is_within(element.bounds));
        Self {
            text: element.text.clone(),
            bounds: element.bounds,
            quad,
            fragments: vec![element.bounds],
            fragment_quads: quad.into_iter().collect(),
            confidence_total: u64::from(element.confidence_milli),
            fragment_count: 1,
            contains_unmergeable_noise: is_unmergeable_noise(element),
        }
    }

    fn accepts(&self, element: &ReconstructedElement) -> bool {
        if self.contains_unmergeable_noise || is_unmergeable_noise(element) {
            return false;
        }
        match (
            self.quad,
            element.quad.filter(|quad| quad.is_within(element.bounds)),
        ) {
            (Some(row), Some(next)) => return oriented_fragments_share_row(row, next),
            // Unknown orientation must not enlarge a known slanted line.
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        let row_top = i64::from(self.bounds.y);
        let row_bottom = row_top.saturating_add(i64::from(self.bounds.height));
        let element_top = i64::from(element.bounds.y);
        let element_bottom = element_top.saturating_add(i64::from(element.bounds.height));
        let vertical_intersection = row_bottom
            .min(element_bottom)
            .saturating_sub(row_top.max(element_top));
        let smaller_height = i64::from(self.bounds.height.min(element.bounds.height).max(1));
        if vertical_intersection.saturating_mul(100) < smaller_height.saturating_mul(45) {
            return false;
        }

        let row_left = i64::from(self.bounds.x);
        let row_right = row_left.saturating_add(i64::from(self.bounds.width));
        let element_left = i64::from(element.bounds.x);
        let element_right = element_left.saturating_add(i64::from(element.bounds.width));
        let horizontal_overlap = row_right.min(element_right) - row_left.max(element_left);
        if horizontal_overlap.saturating_mul(4)
            > i64::from(self.bounds.width.min(element.bounds.width))
        {
            return false;
        }
        let horizontal_gap = if row_right < element_left {
            element_left.saturating_sub(row_right)
        } else if element_right < row_left {
            row_left.saturating_sub(element_right)
        } else {
            0
        };
        // A word space is a fraction of the text height, and where the
        // detector does split a phrase its boxes touch or overlap. One and a
        // half heights joined the titles of two neighbouring news cards (51 px
        // apart at 41 px) and a caption with the link beside it (LATEST NEWS,
        // VIEW ALL: 15 px at 21 px) into single rows, so each was translated
        // as one phrase.
        let maximum_gap = 12_i64.max(smaller_height / 2);
        horizontal_gap <= maximum_gap
    }

    fn baseline_distance(&self, element: &ReconstructedElement) -> u64 {
        if let (Some(row), Some(next)) = (self.quad, element.quad) {
            let frame = QuadFrame::new(row);
            return (frame.extent(row).3 - frame.extent(next).3).abs().round() as u64;
        }
        let row_baseline = i64::from(self.bounds.y).saturating_add(i64::from(self.bounds.height));
        let element_baseline =
            i64::from(element.bounds.y).saturating_add(i64::from(element.bounds.height));
        row_baseline.abs_diff(element_baseline)
    }

    fn push(&mut self, element: &ReconstructedElement) {
        append_visual_fragment(&mut self.text, &element.text);
        self.fragments.push(element.bounds);
        self.bounds = union_rect(self.bounds, element.bounds);
        if let (Some(row), Some(next)) = (self.quad, element.quad) {
            self.fragment_quads.push(next);
            let quad = QuadFrame::new(row).enclose(&self.fragment_quads);
            self.bounds = union_rect(self.bounds, quad_bounds(quad));
            self.quad = Some(quad);
        }
        self.confidence_total = self
            .confidence_total
            .saturating_add(u64::from(element.confidence_milli));
        self.fragment_count = self.fragment_count.saturating_add(1);
    }

    fn finish(self) -> ReconstructedVisualRow {
        ReconstructedVisualRow {
            text: self.text,
            bounds: self.bounds,
            quad: self.quad,
            fragments: self.fragments,
            fragment_quads: self.fragment_quads,
            confidence_milli: u16::try_from(self.confidence_total / self.fragment_count.max(1))
                .unwrap_or(u16::MAX),
        }
    }
}

/// Use the text's own baseline/normal axes, not the tall screen-aligned
/// rectangle produced by an inclined line. Coordinates remain crop-local.
#[derive(Clone, Copy)]
struct QuadFrame {
    x: f64,
    y: f64,
}

impl QuadFrame {
    fn new(quad: PhysicalQuad) -> Self {
        let [tl, tr, br, bl] = quad.points;
        let x = f64::from(tr.x) - f64::from(tl.x) + f64::from(br.x) - f64::from(bl.x);
        let y = f64::from(tr.y) - f64::from(tl.y) + f64::from(br.y) - f64::from(bl.y);
        let length = x.hypot(y).max(1.0);
        Self {
            x: x / length,
            y: y / length,
        }
    }

    fn extent(self, quad: PhysicalQuad) -> (f64, f64, f64, f64) {
        quad.points.iter().fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |(left, top, right, bottom), point| {
                let x = f64::from(point.x) * self.x + f64::from(point.y) * self.y;
                let y = -f64::from(point.x) * self.y + f64::from(point.y) * self.x;
                (left.min(x), top.min(y), right.max(x), bottom.max(y))
            },
        )
    }

    fn enclose(self, quads: &[PhysicalQuad]) -> PhysicalQuad {
        let (left, top, right, bottom) = quads.iter().map(|quad| self.extent(*quad)).fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)),
        );
        PhysicalQuad {
            points: [(left, top), (right, top), (right, bottom), (left, bottom)].map(|(x, y)| {
                PhysicalPoint {
                    x: (x * self.x - y * self.y).round() as i32,
                    y: (x * self.y + y * self.x).round() as i32,
                }
            }),
        }
    }
}

fn oriented_fragments_share_row(row: PhysicalQuad, next: PhysicalQuad) -> bool {
    let frame = QuadFrame::new(row);
    let other = QuadFrame::new(next);
    // Five degrees permits detector noise, without joining different slopes.
    if frame.x * other.x + frame.y * other.y < 5_f64.to_radians().cos() {
        return false;
    }
    let a = frame.extent(row);
    let b = frame.extent(next);
    let height = (a.3 - a.1).min(b.3 - b.1).max(1.0);
    let vertical_overlap = a.3.min(b.3) - a.1.max(b.1);
    let horizontal_overlap = a.2.min(b.2) - a.0.max(b.0);
    let width = (a.2 - a.0).min(b.2 - b.0).max(1.0);
    vertical_overlap >= height * 0.45
        && horizontal_overlap <= width * 0.25
        && -horizontal_overlap <= 12_f64.max(height * 0.5)
}

fn quad_bounds(quad: PhysicalQuad) -> PhysicalRect {
    let left = quad
        .points
        .iter()
        .map(|point| point.x)
        .min()
        .unwrap_or_default();
    let top = quad
        .points
        .iter()
        .map(|point| point.y)
        .min()
        .unwrap_or_default();
    let right = quad
        .points
        .iter()
        .map(|point| point.x)
        .max()
        .unwrap_or(left);
    let bottom = quad.points.iter().map(|point| point.y).max().unwrap_or(top);
    PhysicalRect {
        x: left,
        y: top,
        width: u32::try_from(i64::from(right) - i64::from(left)).unwrap_or(u32::MAX),
        height: u32::try_from(i64::from(bottom) - i64::from(top)).unwrap_or(u32::MAX),
    }
}

fn is_unmergeable_noise(element: &ReconstructedElement) -> bool {
    element.confidence_milli < 650 && !element.text.chars().any(char::is_alphabetic)
}

fn append_visual_fragment(row: &mut String, fragment: &str) {
    let fragment = fragment.trim();
    if fragment.is_empty() {
        return;
    }
    let fragment_starts_with_closing_punctuation = fragment
        .chars()
        .next()
        .is_some_and(|character| ",.;:!?)]}".contains(character));
    let row_ends_with_opening_punctuation = row
        .chars()
        .next_back()
        .is_some_and(|character| "([{«".contains(character));
    if !row.is_empty()
        && !row.ends_with(char::is_whitespace)
        && !fragment_starts_with_closing_punctuation
        && !row_ends_with_opening_punctuation
    {
        row.push(' ');
    }
    row.push_str(fragment);
}

fn union_rect(
    left: rust_protocol::PhysicalRect,
    right: rust_protocol::PhysicalRect,
) -> rust_protocol::PhysicalRect {
    let union_left = i64::from(left.x).min(i64::from(right.x));
    let union_top = i64::from(left.y).min(i64::from(right.y));
    let union_right = i64::from(left.x)
        .saturating_add(i64::from(left.width))
        .max(i64::from(right.x).saturating_add(i64::from(right.width)));
    let union_bottom = i64::from(left.y)
        .saturating_add(i64::from(left.height))
        .max(i64::from(right.y).saturating_add(i64::from(right.height)));
    rust_protocol::PhysicalRect {
        x: i32::try_from(union_left).unwrap_or(i32::MIN),
        y: i32::try_from(union_top).unwrap_or(i32::MIN),
        width: u32::try_from(union_right.saturating_sub(union_left)).unwrap_or(u32::MAX),
        height: u32::try_from(union_bottom.saturating_sub(union_top)).unwrap_or(u32::MAX),
    }
}

#[derive(Clone, Copy)]
struct AcceptedLine<'a> {
    index: usize,
    text: &'a str,
    source: &'a OcrLine,
    reading_bounds: PhysicalRect,
}

#[derive(Default)]
struct Column<'a> {
    lines: Vec<AcceptedLine<'a>>,
    left_sum: i64,
    min_x: i64,
    max_x: i64,
}

impl<'a> Column<'a> {
    fn new(line: AcceptedLine<'a>) -> Self {
        let left = i64::from(line.reading_bounds.x);
        let right = left.saturating_add(i64::from(line.reading_bounds.width));
        Self {
            lines: vec![line],
            left_sum: left,
            min_x: left,
            max_x: right,
        }
    }

    fn anchor_x(&self) -> i64 {
        self.left_sum / i64::try_from(self.lines.len()).unwrap_or(1)
    }

    fn accepts(&self, line: AcceptedLine<'_>, tolerance: i64) -> bool {
        let left = i64::from(line.reading_bounds.x);
        let right = left.saturating_add(i64::from(line.reading_bounds.width));
        let overlap = self.max_x.min(right).saturating_sub(self.min_x.max(left));
        let line_width = right.saturating_sub(left).max(1);
        let column_width = self.max_x.saturating_sub(self.min_x).max(1);
        left.abs_diff(self.anchor_x()) <= u64::try_from(tolerance).unwrap_or(u64::MAX)
            || overlap.saturating_mul(2) >= line_width.min(column_width)
    }

    fn push(&mut self, line: AcceptedLine<'a>) {
        let left = i64::from(line.reading_bounds.x);
        let right = left.saturating_add(i64::from(line.reading_bounds.width));
        self.lines.push(line);
        self.left_sum = self.left_sum.saturating_add(left);
        self.min_x = self.min_x.min(left);
        self.max_x = self.max_x.max(right);
    }
}

pub fn reconstruct_text(
    lines: &[OcrLine],
    min_confidence_milli: u16,
) -> Result<ReconstructedText, OcrFailureKind> {
    let mut accepted = lines
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            let text = source.text.trim();
            (source.confidence_milli >= min_confidence_milli && !text.is_empty()).then_some(
                AcceptedLine {
                    index,
                    text,
                    source,
                    reading_bounds: source.bounds,
                },
            )
        })
        .collect::<Vec<_>>();
    if accepted.is_empty() {
        return Err(OcrFailureKind::NoText);
    }

    // A coherently tilted document needs deskewed reading order as well as
    // deskewed row grouping. Keep the physical rectangles/quads untouched.
    let mut angles = accepted
        .iter()
        .filter_map(|line| {
            line.source
                .quad
                .filter(|quad| quad.is_within(line.source.bounds))
                .map(|quad| {
                    let frame = QuadFrame::new(quad);
                    frame.y.atan2(frame.x)
                })
        })
        .collect::<Vec<_>>();
    angles.sort_by(f64::total_cmp);
    if let Some(&angle) = angles.get(angles.len() / 2) {
        let coherent = angles
            .iter()
            .filter(|other| (**other - angle).abs() <= 5_f64.to_radians())
            .count();
        if coherent * 2 >= accepted.len() && angle.abs() > 1_f64.to_radians() {
            let (y, x) = angle.sin_cos();
            let frame = QuadFrame { x, y };
            for line in &mut accepted {
                if let Some(quad) = line
                    .source
                    .quad
                    .filter(|quad| quad.is_within(line.source.bounds))
                {
                    let (left, top, right, bottom) = frame.extent(quad);
                    line.reading_bounds = PhysicalRect {
                        x: left.floor() as i32,
                        y: top.floor() as i32,
                        width: (right.ceil() - left.floor()).max(1.0) as u32,
                        height: (bottom.ceil() - top.floor()).max(1.0) as u32,
                    };
                }
            }
        }
    }

    let median_height = median(
        accepted
            .iter()
            .map(|line| line.reading_bounds.height.max(1)),
    );
    let median_width = median(accepted.iter().map(|line| line.reading_bounds.width.max(1)));
    let column_tolerance = i64::from((median_height * 3).max(median_width * 2 / 5));

    accepted.sort_by_key(|line| (line.reading_bounds.x, line.reading_bounds.y, line.index));
    let mut columns: Vec<Column<'_>> = Vec::new();
    for line in accepted.iter().copied() {
        let target = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.accepts(line, column_tolerance))
            .min_by_key(|(_, column)| i64::from(line.reading_bounds.x).abs_diff(column.anchor_x()))
            .map(|(index, _)| index);
        if let Some(index) = target {
            columns[index].push(line);
        } else {
            columns.push(Column::new(line));
        }
    }
    columns.sort_by_key(Column::anchor_x);

    let paragraph_gap = i64::from(median_height) * 7 / 5;
    let mut blocks = Vec::new();
    let mut elements = Vec::with_capacity(accepted.len());
    for column in &mut columns {
        column
            .lines
            .sort_by_key(|line| (line.reading_bounds.y, line.reading_bounds.x, line.index));
        elements.extend(column.lines.iter().map(|line| {
            ReconstructedElement {
                text: line.text.to_owned(),
                bounds: line.source.bounds,
                quad: line
                    .source
                    .quad
                    .filter(|quad| quad.is_within(line.source.bounds)),
                confidence_milli: line.source.confidence_milli,
            }
        }));
        blocks.extend(reconstruct_column(&column.lines, paragraph_gap));
    }
    let text = blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        return Err(OcrFailureKind::NoText);
    }

    let confidence_total = accepted
        .iter()
        .map(|line| u64::from(line.source.confidence_milli))
        .sum::<u64>();
    let average_confidence_milli =
        u16::try_from(confidence_total / u64::try_from(accepted.len()).unwrap_or(1))
            .unwrap_or(u16::MAX);
    Ok(ReconstructedText {
        text,
        blocks,
        elements,
        accepted_line_count: accepted.len(),
        average_confidence_milli,
    })
}

fn reconstruct_column(lines: &[AcceptedLine<'_>], paragraph_gap: i64) -> Vec<ReconstructedBlock> {
    let mut blocks = Vec::new();
    let mut current: Option<ReconstructedBlockBuilder> = None;
    let mut previous: Option<AcceptedLine<'_>> = None;
    for (index, line) in lines.iter().enumerate() {
        let lexical_continuation =
            previous.is_some_and(|previous| lexical_hyphen_continuation(previous.text, line.text));
        let begins_lexical_pair = lines
            .get(index + 1)
            .is_some_and(|next| lexical_hyphen_continuation(line.text, next.text));
        let starts_paragraph = previous.is_some_and(|previous| {
            let previous_bottom = i64::from(previous.reading_bounds.y)
                .saturating_add(i64::from(previous.reading_bounds.height));
            let vertical_gap =
                i64::from(line.reading_bounds.y).saturating_sub(previous_bottom) > paragraph_gap;
            let explicit_list_item = starts_list_item(line.text);
            let dense_block_limit = current.as_ref().is_some_and(|block| {
                block.line_count >= MAX_DENSE_BLOCK_LINES
                    || (begins_lexical_pair && block.line_count + 1 >= MAX_DENSE_BLOCK_LINES)
            });
            vertical_gap || explicit_list_item || dense_block_limit
        });
        if starts_paragraph && let Some(block) = current.take() {
            blocks.push(block.finish());
        }
        let new_block = current.is_none();
        let block = current.get_or_insert_with(|| ReconstructedBlockBuilder::new(*line));
        if previous.is_some() {
            if lexical_continuation {
                block.text.pop();
            } else if !starts_paragraph {
                block.text.push('\n');
            }
        }
        if !new_block {
            block.include(*line);
        }
        block.text.push_str(line.text);
        previous = Some(*line);
    }
    if let Some(block) = current {
        blocks.push(block.finish());
    }
    blocks
}

fn starts_list_item(text: &str) -> bool {
    let text = text.trim_start();
    ["- ", "• ", "‣ ", "◦ ", "▪ ", "– ", "— "]
        .iter()
        .any(|marker| text.starts_with(marker))
}

struct ReconstructedBlockBuilder {
    text: String,
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    confidence_total: u64,
    line_count: u64,
}

impl ReconstructedBlockBuilder {
    fn new(line: AcceptedLine<'_>) -> Self {
        let bounds = line.source.bounds;
        Self {
            text: String::new(),
            left: i64::from(bounds.x),
            top: i64::from(bounds.y),
            right: i64::from(bounds.x).saturating_add(i64::from(bounds.width)),
            bottom: i64::from(bounds.y).saturating_add(i64::from(bounds.height)),
            confidence_total: u64::from(line.source.confidence_milli),
            line_count: 1,
        }
    }

    fn include(&mut self, line: AcceptedLine<'_>) {
        let bounds = line.source.bounds;
        self.left = self.left.min(i64::from(bounds.x));
        self.top = self.top.min(i64::from(bounds.y));
        self.right = self
            .right
            .max(i64::from(bounds.x).saturating_add(i64::from(bounds.width)));
        self.bottom = self
            .bottom
            .max(i64::from(bounds.y).saturating_add(i64::from(bounds.height)));
        self.confidence_total = self
            .confidence_total
            .saturating_add(u64::from(line.source.confidence_milli));
        self.line_count = self.line_count.saturating_add(1);
    }

    fn finish(self) -> ReconstructedBlock {
        ReconstructedBlock {
            text: self.text,
            bounds: rust_protocol::PhysicalRect {
                x: i32::try_from(self.left).unwrap_or(i32::MIN),
                y: i32::try_from(self.top).unwrap_or(i32::MIN),
                width: u32::try_from(self.right.saturating_sub(self.left)).unwrap_or(u32::MAX),
                height: u32::try_from(self.bottom.saturating_sub(self.top)).unwrap_or(u32::MAX),
            },
            confidence_milli: u16::try_from(self.confidence_total / self.line_count.max(1))
                .unwrap_or(u16::MAX),
        }
    }
}

fn lexical_hyphen_continuation(previous: &str, next: &str) -> bool {
    let Some(prefix) = previous.strip_suffix('-') else {
        return false;
    };
    let previous_word = prefix
        .rsplit(|character: char| !character.is_alphabetic())
        .next()
        .unwrap_or_default();
    let Some(next_character) = next.chars().next() else {
        return false;
    };
    !previous_word.is_empty()
        && previous_word.chars().all(char::is_alphabetic)
        && !previous_word.chars().any(char::is_uppercase)
        && next_character.is_alphabetic()
        && !next_character.is_uppercase()
}

fn median(values: impl Iterator<Item = u32>) -> u32 {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}
