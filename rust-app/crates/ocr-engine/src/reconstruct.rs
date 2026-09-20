use rust_protocol::{OcrFailureKind, OcrLine};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedText {
    pub text: String,
    pub accepted_line_count: usize,
    pub average_confidence_milli: u16,
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
    let text = columns
        .iter_mut()
        .map(|column| {
            column
                .lines
                .sort_by_key(|line| (line.source.bounds.y, line.source.bounds.x, line.index));
            render_column(&column.lines, paragraph_gap)
        })
        .filter(|column| !column.is_empty())
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
        accepted_line_count: accepted.len(),
        average_confidence_milli,
    })
}

fn render_column(lines: &[AcceptedLine<'_>], paragraph_gap: i64) -> String {
    let mut output = String::new();
    let mut previous: Option<AcceptedLine<'_>> = None;
    for line in lines {
        if let Some(previous) = previous {
            if lexical_hyphen_continuation(previous.text, line.text) {
                output.pop();
            } else {
                let previous_bottom = i64::from(previous.source.bounds.y)
                    .saturating_add(i64::from(previous.source.bounds.height));
                let gap = i64::from(line.source.bounds.y).saturating_sub(previous_bottom);
                output.push('\n');
                if gap > paragraph_gap {
                    output.push('\n');
                }
            }
        }
        output.push_str(line.text);
        previous = Some(*line);
    }
    output
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
