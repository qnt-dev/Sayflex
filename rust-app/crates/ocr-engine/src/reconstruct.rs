use rust_protocol::{OcrFailureKind, OcrLine};

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
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedVisualRow {
    pub text: String,
    pub bounds: rust_protocol::PhysicalRect,
    pub fragments: Vec<rust_protocol::PhysicalRect>,
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

struct VisualRowBuilder {
    text: String,
    bounds: rust_protocol::PhysicalRect,
    fragments: Vec<rust_protocol::PhysicalRect>,
    confidence_total: u64,
    fragment_count: u64,
    contains_unmergeable_noise: bool,
}

impl VisualRowBuilder {
    fn new(element: &ReconstructedElement) -> Self {
        Self {
            text: element.text.clone(),
            bounds: element.bounds,
            fragments: vec![element.bounds],
            confidence_total: u64::from(element.confidence_milli),
            fragment_count: 1,
            contains_unmergeable_noise: is_unmergeable_noise(element),
        }
    }

    fn accepts(&self, element: &ReconstructedElement) -> bool {
        if self.contains_unmergeable_noise || is_unmergeable_noise(element) {
            return false;
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
        let horizontal_gap = if row_right < element_left {
            element_left.saturating_sub(row_right)
        } else if element_right < row_left {
            row_left.saturating_sub(element_right)
        } else {
            0
        };
        // A word space is a fraction of the text height, and the detector
        // rarely splits a line inside one. One and a half heights joined the
        // titles of two neighbouring news cards (51 px apart at 41 px) into a
        // single row, so one card was given the other's translation.
        let maximum_gap = 12_i64.max(smaller_height.saturating_mul(4) / 5);
        horizontal_gap <= maximum_gap
    }

    fn baseline_distance(&self, element: &ReconstructedElement) -> u64 {
        let row_baseline = i64::from(self.bounds.y).saturating_add(i64::from(self.bounds.height));
        let element_baseline =
            i64::from(element.bounds.y).saturating_add(i64::from(element.bounds.height));
        row_baseline.abs_diff(element_baseline)
    }

    fn push(&mut self, element: &ReconstructedElement) {
        append_visual_fragment(&mut self.text, &element.text);
        self.fragments.push(element.bounds);
        self.bounds = union_rect(self.bounds, element.bounds);
        self.confidence_total = self
            .confidence_total
            .saturating_add(u64::from(element.confidence_milli));
        self.fragment_count = self.fragment_count.saturating_add(1);
    }

    fn finish(self) -> ReconstructedVisualRow {
        ReconstructedVisualRow {
            text: self.text,
            bounds: self.bounds,
            fragments: self.fragments,
            confidence_milli: u16::try_from(self.confidence_total / self.fragment_count.max(1))
                .unwrap_or(u16::MAX),
        }
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
        let left = i64::from(line.source.bounds.x);
        let right = left.saturating_add(i64::from(line.source.bounds.width));
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
        let left = i64::from(line.source.bounds.x);
        let right = left.saturating_add(i64::from(line.source.bounds.width));
        let overlap = self.max_x.min(right).saturating_sub(self.min_x.max(left));
        let line_width = right.saturating_sub(left).max(1);
        let column_width = self.max_x.saturating_sub(self.min_x).max(1);
        left.abs_diff(self.anchor_x()) <= u64::try_from(tolerance).unwrap_or(u64::MAX)
            || overlap.saturating_mul(2) >= line_width.min(column_width)
    }

    fn push(&mut self, line: AcceptedLine<'a>) {
        let left = i64::from(line.source.bounds.x);
        let right = left.saturating_add(i64::from(line.source.bounds.width));
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
                },
            )
        })
        .collect::<Vec<_>>();
    if accepted.is_empty() {
        return Err(OcrFailureKind::NoText);
    }

    let median_height = median(accepted.iter().map(|line| line.source.bounds.height.max(1)));
    let median_width = median(accepted.iter().map(|line| line.source.bounds.width.max(1)));
    let column_tolerance = i64::from((median_height * 3).max(median_width * 2 / 5));

    accepted.sort_by_key(|line| (line.source.bounds.x, line.source.bounds.y, line.index));
    let mut columns: Vec<Column<'_>> = Vec::new();
    for line in accepted.iter().copied() {
        let target = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.accepts(line, column_tolerance))
            .min_by_key(|(_, column)| i64::from(line.source.bounds.x).abs_diff(column.anchor_x()))
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
            .sort_by_key(|line| (line.source.bounds.y, line.source.bounds.x, line.index));
        elements.extend(column.lines.iter().map(|line| ReconstructedElement {
            text: line.text.to_owned(),
            bounds: line.source.bounds,
            confidence_milli: line.source.confidence_milli,
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
            let previous_bottom = i64::from(previous.source.bounds.y)
                .saturating_add(i64::from(previous.source.bounds.height));
            let vertical_gap =
                i64::from(line.source.bounds.y).saturating_sub(previous_bottom) > paragraph_gap;
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
