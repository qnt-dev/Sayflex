//! Pixel work for drawing a translation where the source text was: what the
//! source glyphs look like (colour, height, stroke, marks just outside the OCR
//! box such as quotes) and a copy of the capture whose text areas are filled
//! from the surrounding background, so the translation is drawn straight onto
//! the picture instead of onto a plate.

use rust_platform_windows::screen_capture::RgbaFrame;

/// A half-open rectangle in capture pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub(crate) left: i64,
    pub(crate) top: i64,
    pub(crate) right: i64,
    pub(crate) bottom: i64,
}

impl PixelRect {
    pub(crate) fn width(self) -> i64 {
        self.right - self.left
    }

    pub(crate) fn height(self) -> i64 {
        self.bottom - self.top
    }

    fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub(crate) fn inflate(self, horizontal: i64, vertical: i64) -> Self {
        Self {
            left: self.left - horizontal,
            top: self.top - vertical,
            right: self.right + horizontal,
            bottom: self.bottom + vertical,
        }
    }

    fn clip(self, width: i64, height: i64) -> Self {
        Self {
            left: self.left.clamp(0, width),
            top: self.top.clamp(0, height),
            right: self.right.clamp(0, width),
            bottom: self.bottom.clamp(0, height),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlyphMetrics {
    pub(crate) ink: [u8; 3],
    pub(crate) background: [u8; 3],
    /// Rows and columns holding ink, widened over marks just outside the OCR
    /// box that belong to the same text (an opening quote, a trailing dot).
    pub(crate) extent: PixelRect,
    /// The densest band of rows, half-open: the capital height of text in
    /// capitals, the x-height of lowercase text. Its bottom is the baseline.
    /// Ascenders, descenders, comma tails and a busy background around the
    /// text are all far sparser.
    pub(crate) core: (i64, i64),
    /// Median horizontal run of ink, a measure of the stroke weight.
    pub(crate) stroke: f32,
    /// More ink of the same colour a word space or so beyond the left and
    /// right ends: text of the same line the OCR did not return, which a
    /// wider translation must not cover.
    pub(crate) continues: (bool, bool),
    /// Where the plain background to the right of the text ends: up to here
    /// a longer translation covers nothing but that background.
    pub(crate) free_right: i64,
}

/// Pixels kept between the measured text and unrecognised text continuing
/// its line: more than the margin around a rebuilt area.
const CONTINUATION_CLEARANCE: i64 = 3;

/// Largest channel difference from the background colour that still counts
/// as the same plain background (a card edge or a panel border exceeds it).
const PLAIN_BACKGROUND: u8 = 10;

/// Below this the box holds no text that stands out from its surroundings.
const MIN_TEXT_CONTRAST: u8 = 40;

fn rgb(frame: &RgbaFrame, x: i64, y: i64) -> Option<[u8; 3]> {
    if x < 0 || y < 0 {
        return None;
    }
    frame
        .pixel(u32::try_from(x).ok()?, u32::try_from(y).ok()?)
        .map(|[red, green, blue, _]| [red, green, blue])
}

fn distance(left: [u8; 3], right: [u8; 3]) -> u8 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left.abs_diff(right))
        .max()
        .unwrap_or(0)
}

/// Median colour of a two-pixel ring just outside `rect`.
pub(crate) fn ring_median(frame: &RgbaFrame, rect: PixelRect) -> Option<[u8; 3]> {
    let mut channels: [Vec<u8>; 3] = Default::default();
    let mut take = |x: i64, y: i64| {
        if let Some(pixel) = rgb(frame, x, y) {
            for (channel, value) in channels.iter_mut().zip(pixel) {
                channel.push(value);
            }
        }
    };
    for offset in 1..=2 {
        for x in (rect.left - offset)..(rect.right + offset) {
            take(x, rect.top - offset);
            take(x, rect.bottom - 1 + offset);
        }
        for y in (rect.top - offset + 1)..(rect.bottom - 1 + offset) {
            take(rect.left - offset, y);
            take(rect.right - 1 + offset, y);
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

/// How the text inside `rect` is drawn. `reach` bounds how far left and right
/// marks outside the box may be claimed, so a neighbouring row never is.
pub(crate) fn glyph_metrics(
    frame: &RgbaFrame,
    rect: PixelRect,
    reach: (i64, i64),
) -> Option<GlyphMetrics> {
    let (width, height) = (i64::from(frame.rect.width), i64::from(frame.rect.height));
    let rect = rect.clip(width, height);
    if rect.is_empty() {
        return None;
    }
    let background = ring_median(frame, rect)?;
    let mut distances = Vec::with_capacity((rect.width() * rect.height()) as usize);
    for y in rect.top..rect.bottom {
        for x in rect.left..rect.right {
            distances.push(distance(rgb(frame, x, y)?, background));
        }
    }
    distances.sort_unstable();
    let strong = distances[(distances.len() - 1) * 95 / 100];
    if strong < MIN_TEXT_CONTRAST {
        return None;
    }
    let mut total = [0_u64; 3];
    let mut count = 0_u64;
    for y in rect.top..rect.bottom {
        for x in rect.left..rect.right {
            let pixel = rgb(frame, x, y)?;
            if u16::from(distance(pixel, background)) * 5 >= u16::from(strong) * 4 {
                for (sum, value) in total.iter_mut().zip(pixel) {
                    *sum += u64::from(value);
                }
                count += 1;
            }
        }
    }
    let ink = total.map(|sum| (sum / count.max(1)) as u8);
    let separation = u16::from(distance(ink, background));
    // An ink pixel is nearer the text colour than the background and clearly
    // off the background, so anti-aliased edges count and texture does not.
    let is_ink = |x: i64, y: i64| {
        rgb(frame, x, y).is_some_and(|pixel| {
            let from_background = distance(pixel, background);
            from_background > distance(pixel, ink)
                && u16::from(from_background) * 5 >= separation * 2
        })
    };

    let row_minimum = (rect.width() / 80).max(2);
    let counts = (rect.top..rect.bottom)
        .map(|y| (rect.left..rect.right).filter(|&x| is_ink(x, y)).count() as i64)
        .collect::<Vec<_>>();
    let row_at = |index: usize| rect.top + index as i64;
    let rows = (0..counts.len())
        .filter(|&index| counts[index] >= row_minimum)
        .collect::<Vec<_>>();
    let (mut top, mut bottom) = (row_at(*rows.first()?), row_at(*rows.last()?) + 1);
    let core =
        densest_band(&counts, row_minimum).map(|(first, end)| (row_at(first), row_at(end)))?;
    // Gaps are judged against the core (capital or x-height): the full
    // extent can include speckle of a busy background.
    let letter_height = core.1 - core.0;
    let column_minimum = if bottom - top >= 8 { 2 } else { 1 };
    let ink_column = |x: i64, top: i64, bottom: i64| {
        (top..bottom).filter(|&y| is_ink(x, y)).count() as i64 >= column_minimum
    };
    let inside = (rect.left..rect.right)
        .filter(|&x| ink_column(x, top, bottom))
        .collect::<Vec<_>>();
    let (mut left, mut right) = (*inside.first()?, *inside.last()? + 1);

    // Descenders, accents and underlines the box cut off: rows just above
    // or below it that still hold ink across the text's columns.
    let row_has_ink = |y: i64| y >= 0 && y < height && (left..right).any(|x| is_ink(x, y));
    let (rise, drop) = ((letter_height / 4).max(2), (letter_height * 2 / 5).max(2));
    if top == rect.top {
        while top > rect.top - rise && row_has_ink(top - 1) {
            top -= 1;
        }
    }
    if bottom == rect.bottom {
        while bottom < rect.bottom + drop && row_has_ink(bottom) {
            bottom += 1;
        }
    }
    let glyph_height = bottom - top;

    // Marks beside the box: walk outwards while the gaps stay letter-sized.
    let band_top = (top - glyph_height / 10).max(0);
    let band_bottom = (bottom + glyph_height / 10).min(height);
    // A mark belongs to the text while the gap to it is no wider than the
    // text's own spacing between letters (the wider gaps inside the box are
    // the few word spaces), and never wider than a letter is tall.
    let mut inner_gaps = Vec::new();
    let mut run = 0_i64;
    for x in left..right {
        if ink_column(x, top, bottom) {
            if run > 0 {
                inner_gaps.push(run);
            }
            run = 0;
        } else {
            run += 1;
        }
    }
    inner_gaps.sort_unstable();
    let letter_gap = inner_gaps
        .get(inner_gaps.len() * 3 / 4)
        .copied()
        .unwrap_or(0);
    let largest_gap = (letter_gap + 1).max(3).min(letter_height.max(3));
    let outer_left = (rect.left - letter_height).max(reach.0).max(0);
    let mut gap = 0;
    for x in (outer_left..rect.left).rev() {
        if ink_column(x, band_top, band_bottom) {
            left = x;
            gap = 0;
        } else {
            gap += 1;
            if gap > largest_gap {
                break;
            }
        }
    }
    let outer_right = (rect.right + letter_height).min(reach.1).min(width);
    gap = 0;
    for x in rect.right..outer_right {
        if ink_column(x, band_top, band_bottom) {
            right = x + 1;
            gap = 0;
        } else {
            gap += 1;
            if gap > largest_gap {
                break;
            }
        }
    }
    let lookout = letter_height * 2;
    let next_left = ((left - lookout).max(reach.0).max(0)..left)
        .rev()
        .find(|&x| ink_column(x, band_top, band_bottom));
    let next_right = (right..(right + lookout).min(reach.1).min(width))
        .find(|&x| ink_column(x, band_top, band_bottom));
    // A button's or panel's edge beside the text is a rule, not more text:
    // at most three columns, inked over the whole line, then nothing.
    let rule_right = next_right.filter(|&first| {
        let solid = |x: i64| {
            let rows = band_bottom - band_top;
            (band_top..band_bottom).filter(|&y| is_ink(x, y)).count() as i64 * 10 >= rows * 9
        };
        let columns = (first..first + 4).take_while(|&x| solid(x)).count() as i64;
        (1..=3).contains(&columns)
            && !(first + columns..first + columns + 3).any(|x| ink_column(x, band_top, band_bottom))
    });
    let next_right = next_right.filter(|_| rule_right.is_none());
    let continues = (next_left.is_some(), next_right.is_some());
    // Stay clear of text that continues the line, so the margin around the
    // rebuilt area never clips its first glyph; speckle in the gap must not
    // carry the extent up to it.
    let (inner_left, inner_right) = (*inside.first()?, *inside.last()? + 1);
    if let Some(next) = next_right {
        right = right.min(next - CONTINUATION_CLEARANCE).max(inner_right);
    }
    if let Some(next) = next_left {
        left = left.max(next + 1 + CONTINUATION_CLEARANCE).min(inner_left);
    }

    // Plain background beyond the text, skipping the last glyph's soft edge;
    // at most twice the text's width.
    let mut free_right = right;
    if !continues.1 {
        let limit = reach
            .1
            .min(width)
            .min(right + 2 * (right - left))
            .min(rule_right.map_or(i64::MAX, |rule| rule - 2));
        let plain = |x: i64| {
            (band_top..band_bottom).all(|y| {
                rgb(frame, x, y)
                    .is_some_and(|pixel| distance(pixel, background) <= PLAIN_BACKGROUND)
            })
        };
        let mut x = right + 2;
        while x < limit && plain(x) {
            x += 1;
        }
        // Whatever ended the plain run (an edge, an icon) keeps a gap.
        let end = if x < limit { x - 2 } else { x };
        if end > right {
            free_right = end;
        }
    }

    // Stroke width over solid ink only: anti-aliasing widens every stem by
    // a pixel or two, which would make regular text read as bold.
    let is_solid = |x: i64, y: i64| {
        rgb(frame, x, y).is_some_and(|pixel| {
            let from_background = u16::from(distance(pixel, background));
            from_background * 10 >= separation * 7
                && distance(pixel, ink) < distance(pixel, background)
        })
    };
    let mut runs = Vec::new();
    for y in core.0..core.1 {
        let mut run = 0_u32;
        for x in left..=right {
            if x < right && is_solid(x, y) {
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
    }
    runs.sort_unstable();
    let stroke = runs.get(runs.len() / 2).copied().unwrap_or(1) as f32;

    Some(GlyphMetrics {
        ink,
        background,
        extent: PixelRect {
            left,
            top,
            right,
            bottom,
        },
        stroke,
        continues,
        core,
        free_right,
    })
}

/// The longest run of rows holding at least 35% of the fullest row's ink,
/// bridging single thinner rows (a serif, a crossbar gap).
fn densest_band(counts: &[i64], minimum: i64) -> Option<(usize, usize)> {
    let fullest = counts.iter().copied().max()?;
    let threshold = minimum.max((fullest * 35 + 99) / 100);
    let dense = |index: usize| counts.get(index).is_some_and(|&count| count >= threshold);
    let mut best: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < counts.len() {
        if !dense(index) {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index + 1;
        while dense(end) || dense(end + 1) {
            end += if dense(end) { 1 } else { 2 };
        }
        if best.is_none_or(|(first, last)| end - start > last - first) {
            best = Some((start, end));
        }
        index = end;
    }
    best
}

/// The capture as packed RGBA, with every `region` filled from the
/// background around it. Pixels inside `protected` (text that stays) are
/// never painted over.
pub(crate) fn repaired_pixels(
    frame: &RgbaFrame,
    regions: &[PixelRect],
    protected: &[PixelRect],
) -> Vec<u8> {
    repaired_pixels_with_sampling_exclusions(frame, regions, protected, &[]).0
}

/// Extra exclusions affect background sampling only. Unlike `protected`,
/// they do not preserve pixels inside a replacement region. False means a
/// region had no observed background; that region retains its source pixels.
pub(crate) fn repaired_pixels_with_sampling_exclusions(
    frame: &RgbaFrame,
    regions: &[PixelRect],
    protected: &[PixelRect],
    sampling_exclusions: &[PixelRect],
) -> (Vec<u8>, bool) {
    let (width, height) = (i64::from(frame.rect.width), i64::from(frame.rect.height));
    let row_bytes = width as usize * 4;
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in frame
        .pixels
        .chunks(frame.stride as usize)
        .take(height as usize)
    {
        pixels.extend_from_slice(&row[..row_bytes.min(row.len())]);
    }
    let mut masked = vec![false; (width * height) as usize];
    let regions = regions
        .iter()
        .map(|region| region.clip(width, height))
        .filter(|region| !region.is_empty())
        .collect::<Vec<_>>();
    for region in &regions {
        for y in region.top..region.bottom {
            masked[(y * width + region.left) as usize..(y * width + region.right) as usize]
                .fill(true);
        }
    }
    // Background is sampled past every text box, replaced or kept; only
    // the replaced text is painted over.
    let mut blocked = masked.clone();
    for kept in protected {
        let kept = kept.clip(width, height);
        for y in kept.top..kept.bottom {
            let row = (y * width + kept.left) as usize..(y * width + kept.right) as usize;
            masked[row.clone()].fill(false);
            blocked[row].fill(true);
        }
    }
    for excluded in sampling_exclusions {
        let excluded = excluded.clip(width, height);
        if excluded.is_empty() {
            continue;
        }
        for y in excluded.top..excluded.bottom {
            blocked[(y * width + excluded.left) as usize..(y * width + excluded.right) as usize]
                .fill(true);
        }
    }
    let mut complete = true;
    for region in &regions {
        complete &= fill_region(frame, &masked, &blocked, *region, &mut pixels);
    }
    (pixels, complete)
}

type Side = Option<Vec<[f32; 3]>>;

/// Median of up to five background pixels found by walking from `(x, y)` in
/// direction `step`, past any text. The median ignores a thin border line
/// or a stray glyph edge next to the region.
fn side_sample(
    frame: &RgbaFrame,
    blocked: &[bool],
    (mut x, mut y): (i64, i64),
    step: (i64, i64),
    reach: i64,
) -> Option<[f32; 3]> {
    let width = i64::from(frame.rect.width);
    let height = i64::from(frame.rect.height);
    let inside = |x: i64, y: i64| x >= 0 && y >= 0 && x < width && y < height;
    let mut walked = 0;
    while inside(x, y) && blocked[(y * width + x) as usize] {
        x += step.0;
        y += step.1;
        walked += 1;
        if walked > reach {
            return None;
        }
    }
    let mut channels: [Vec<u8>; 3] = Default::default();
    for _ in 0..5 {
        if !inside(x, y) || blocked[(y * width + x) as usize] {
            break;
        }
        let pixel = rgb(frame, x, y)?;
        for (channel, value) in channels.iter_mut().zip(pixel) {
            channel.push(value);
        }
        x += step.0;
        y += step.1;
    }
    if channels[0].is_empty() {
        return None;
    }
    Some(channels.map(|mut channel| {
        channel.sort_unstable();
        f32::from(channel[channel.len() / 2])
    }))
}

/// Smooths one side of a region and closes its gaps from the nearest
/// sample; `None` when the side has no background at all (an image edge).
/// A median first, so an icon or a bright speck beside the text, narrower
/// than the window, does not bleed into the fill; then a box blur.
fn smoothed(samples: &[Option<[f32; 3]>], radius: usize) -> Side {
    if samples.iter().all(Option::is_none) {
        return None;
    }
    let window = |index: usize, radius: usize| {
        index.saturating_sub(radius)..(index + radius + 1).min(samples.len())
    };
    let medians = (0..samples.len())
        .map(|index| {
            let mut channels: [Vec<f32>; 3] = Default::default();
            for sample in samples[window(index, radius)].iter().flatten() {
                for (channel, value) in channels.iter_mut().zip(sample) {
                    channel.push(*value);
                }
            }
            (!channels[0].is_empty()).then(|| {
                channels.map(|mut channel| {
                    channel.sort_unstable_by(f32::total_cmp);
                    channel[channel.len() / 2]
                })
            })
        })
        .collect::<Vec<_>>();
    let mut out = Vec::with_capacity(samples.len());
    for index in 0..samples.len() {
        let mut sum = [0.0_f32; 3];
        let mut count = 0.0_f32;
        for sample in medians[window(index, (radius / 2).max(1))].iter().flatten() {
            for (total, value) in sum.iter_mut().zip(sample) {
                *total += value;
            }
            count += 1.0;
        }
        out.push((count > 0.0).then(|| sum.map(|total| total / count)));
    }
    let mut last = out.iter().flatten().next().copied();
    for value in &mut out {
        match value {
            Some(found) => last = Some(*found),
            None => *value = last,
        }
    }
    Some(out.into_iter().flatten().collect())
}

/// The typical spread of a side's raw samples around their smoothed values
/// (a median, so one bright object does not count), used as the grain of
/// the fill so it does not look flatter than its surroundings.
fn grain(raw: &[Option<[f32; 3]>], smooth: &Side) -> f32 {
    let Some(smooth) = smooth else {
        return 0.0;
    };
    let mut deviations = raw
        .iter()
        .zip(smooth)
        .filter_map(|(raw, smooth)| {
            raw.map(|raw| {
                raw.iter()
                    .zip(smooth)
                    .map(|(raw, smooth)| (raw - smooth).abs())
                    .sum::<f32>()
                    / 3.0
            })
        })
        .collect::<Vec<_>>();
    if deviations.is_empty() {
        return 0.0;
    }
    deviations.sort_unstable_by(f32::total_cmp);
    deviations[deviations.len() / 2]
}

fn noise(x: i64, y: i64) -> f32 {
    let mut hash = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 32;
    (hash & 0xffff) as f32 / 32_767.5 - 1.0
}

/// The four sides of a region as sampled from the background, each `None`
/// where the background lies further away than `reach`.
struct Sides {
    raw: [Vec<Option<[f32; 3]>>; 4],
}

impl Sides {
    fn sample(frame: &RgbaFrame, blocked: &[bool], region: PixelRect, reach: i64) -> Self {
        Self {
            raw: [
                (region.left..region.right)
                    .map(|x| side_sample(frame, blocked, (x, region.top - 1), (0, -1), reach))
                    .collect(),
                (region.left..region.right)
                    .map(|x| side_sample(frame, blocked, (x, region.bottom), (0, 1), reach))
                    .collect(),
                (region.top..region.bottom)
                    .map(|y| side_sample(frame, blocked, (region.left - 1, y), (-1, 0), reach))
                    .collect(),
                (region.top..region.bottom)
                    .map(|y| side_sample(frame, blocked, (region.right, y), (1, 0), reach))
                    .collect(),
            ],
        }
    }

    fn is_empty(&self) -> bool {
        self.raw.iter().flatten().all(Option::is_none)
    }
}

/// Fills a region from its four sides, each weighted by the inverse of its
/// distance: between two opposite sides that is a straight blend, so
/// gradients and edges crossing the text continue through it. A side is
/// used only where the background is near; background found only beyond
/// other text (a row above, a title below) belongs to somewhere else, and
/// the nearer sides — usually left and right of a line — carry the fill.
fn fill_region(
    frame: &RgbaFrame,
    masked: &[bool],
    blocked: &[bool],
    region: PixelRect,
    pixels: &mut [u8],
) -> bool {
    let width = i64::from(frame.rect.width);
    let mut sides = Sides::sample(frame, blocked, region, (region.height() / 2).clamp(4, 24));
    if sides.is_empty() {
        sides = Sides::sample(frame, blocked, region, 96);
    }
    if sides.is_empty() {
        // Neighbouring lines can block every axial ray while clean corners
        // remain. Use only observed, unblocked pixels; never infer a colour
        // from the text being removed or silently replace it with black.
        let Some(background) = unblocked_background(frame, blocked) else {
            return false;
        };
        for y in region.top..region.bottom {
            for x in region.left..region.right {
                let at = (y * width + x) as usize;
                if masked[at] {
                    pixels[at * 4..at * 4 + 3].copy_from_slice(&background);
                    pixels[at * 4 + 3] = 255;
                }
            }
        }
        return true;
    }
    let across = (region.height() / 3).clamp(2, 16) as usize;
    let along = (region.height() / 8).clamp(1, 6) as usize;
    let [raw_top, raw_bottom, raw_left, raw_right] = &sides.raw;
    let top = smoothed(raw_top, across);
    let bottom = smoothed(raw_bottom, across);
    let left = smoothed(raw_left, along);
    let right = smoothed(raw_right, along);
    let grains = [
        grain(raw_top, &top),
        grain(raw_bottom, &bottom),
        grain(raw_left, &left),
        grain(raw_right, &right),
    ];
    let present = [&top, &bottom, &left, &right]
        .iter()
        .filter(|side| side.is_some())
        .count()
        .max(1) as f32;
    let amplitude = (grains.iter().sum::<f32>() / present).min(10.0);

    for y in region.top..region.bottom {
        let row = (y - region.top) as usize;
        for x in region.left..region.right {
            if !masked[(y * width + x) as usize] {
                continue;
            }
            let column = (x - region.left) as usize;
            let candidates = [
                top.as_ref().map(|side| (side[column], y - region.top + 1)),
                bottom
                    .as_ref()
                    .map(|side| (side[column], region.bottom - y)),
                left.as_ref().map(|side| (side[row], x - region.left + 1)),
                right.as_ref().map(|side| (side[row], region.right - x)),
            ];
            let mut sum = [0.0_f32; 3];
            let mut weights = 0.0_f32;
            for (color, distance) in candidates.into_iter().flatten() {
                let weight = 1.0 / distance.max(1) as f32;
                for (total, value) in sum.iter_mut().zip(color) {
                    *total += value * weight;
                }
                weights += weight;
            }
            if weights == 0.0 {
                continue;
            }
            let grain = noise(x, y) * amplitude;
            let offset = ((y * width + x) * 4) as usize;
            for (channel, total) in sum.iter().enumerate() {
                pixels[offset + channel] =
                    (total / weights + grain).round().clamp(0.0, 255.0) as u8;
            }
            pixels[offset + 3] = 255;
        }
    }
    true
}

fn unblocked_background(frame: &RgbaFrame, blocked: &[bool]) -> Option<[u8; 3]> {
    let mut channels: [Vec<u8>; 3] = Default::default();
    let stride = blocked.len().div_ceil(4096).max(1);
    let width = frame.rect.width as usize;
    for index in (0..blocked.len()).step_by(stride) {
        if !blocked[index]
            && let Some(pixel) = frame.pixel((index % width) as u32, (index / width) as u32)
        {
            for (channel, value) in channels.iter_mut().zip(pixel) {
                channel.push(value);
            }
        }
    }
    if channels[0].is_empty() {
        // A narrow clean margin can fall between the bounded sample points.
        let index = blocked.iter().position(|blocked| !blocked)?;
        let [r, g, b, _] = frame.pixel((index % width) as u32, (index / width) as u32)?;
        return Some([r, g, b]);
    }
    Some(channels.map(|mut values| {
        let middle = values.len() / 2;
        *values.select_nth_unstable(middle).1
    }))
}

#[cfg(test)]
mod tests {
    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::PhysicalRect;

    use super::{
        PixelRect, glyph_metrics, repaired_pixels, repaired_pixels_with_sampling_exclusions,
    };

    fn frame(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> RgbaFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [red, green, blue] = paint(x, y);
                pixels.extend_from_slice(&[red, green, blue, 255]);
            }
        }
        RgbaFrame::try_new(
            PhysicalRect {
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

    fn rect(left: i64, top: i64, right: i64, bottom: i64) -> PixelRect {
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// White strokes on a dark gradient: three 4 px stems 20 px tall at
    /// x 40.., 60.., 80.., plus a quote-like mark at x 26..30 left of the box.
    fn caption(x: u32, y: u32) -> [u8; 3] {
        let background = [20 + (y / 4) as u8, 30, 40];
        let stem = (40..44).contains(&x) || (60..64).contains(&x) || (80..84).contains(&x);
        let quote = (26..30).contains(&x) && (20..27).contains(&y);
        if ((20..40).contains(&y) && stem) || quote {
            [240, 236, 228]
        } else {
            background
        }
    }

    #[test]
    fn glyph_metrics_find_the_ink_colour_height_and_a_quote_outside_the_box() {
        let frame = frame(140, 60, caption);

        let metrics = glyph_metrics(&frame, rect(36, 14, 90, 46), (0, 140)).unwrap();

        assert!(metrics.ink.iter().all(|&channel| channel >= 220));
        assert_eq!((metrics.extent.top, metrics.extent.bottom), (20, 40));
        assert_eq!(
            metrics.extent.left, 26,
            "the quote left of the box is claimed"
        );
        assert_eq!(metrics.extent.right, 84);
        assert_eq!(metrics.stroke, 4.0);
        assert_eq!(metrics.continues, (false, false));
        assert_eq!(metrics.core, (20, 40));
        // A gentle gradient is plain background, up to the image edge.
        assert_eq!(metrics.free_right, 140);
    }

    #[test]
    fn plain_background_beside_the_text_is_free_up_to_a_panel_edge() {
        // Light page; a darker panel starts at x 150.
        let frame = frame(220, 50, |x, y| {
            let stem = [20, 30, 40]
                .iter()
                .any(|left| (*left..left + 2).contains(&x));
            if (15..27).contains(&y) && stem {
                [40, 40, 40]
            } else if x >= 150 {
                [215, 215, 215]
            } else {
                [248, 248, 246]
            }
        });

        let metrics = glyph_metrics(&frame, rect(16, 10, 46, 32), (0, 220)).unwrap();

        assert_eq!(metrics.extent.right, 42);
        assert_eq!(metrics.free_right, 86, "twice the text's width at most");
        let near = glyph_metrics(&frame, rect(16, 10, 46, 32), (0, 70)).unwrap();
        assert_eq!(near.free_right, 70, "never past the next row");
        let panel = glyph_metrics(&frame, rect(56, 10, 86, 32), (0, 220));
        assert!(panel.is_none() || panel.is_some_and(|metrics| metrics.free_right <= 150));
    }

    #[test]
    fn the_core_band_ignores_a_busy_background_and_a_comma_tail() {
        // Row profile of "AUGUST 12, 2026" on a golden emblem (dota2.com):
        // background speckle above, the comma tail below the capitals.
        let counts = [
            6, 7, 7, 6, 6, 5, 4, 58, 58, 36, 35, 39, 44, 44, 40, 43, 56, 68, 2, 1, 0,
        ];

        assert_eq!(super::densest_band(&counts, 2), Some((7, 18)));
    }

    #[test]
    fn the_core_band_of_lowercase_text_is_its_x_height() {
        // "The International: Streams,": ascender rows, x-height, comma.
        let counts = [
            41, 47, 46, 36, 30, 31, 173, 208, 200, 153, 147, 141, 165, 163, 139, 142, 162, 204,
            196, 181, 4, 4, 3,
        ];

        assert_eq!(super::densest_band(&counts, 4), Some((6, 20)));
    }

    #[test]
    fn glyph_metrics_notice_text_of_the_same_line_beyond_the_box() {
        // "AUGUST 27, 2026" read as "AUGUST": the date's digits follow a
        // word space after the box.
        let frame = frame(160, 60, |x, y| {
            let stem = [40, 50, 60, 76, 86]
                .iter()
                .any(|left| (*left..left + 3).contains(&x));
            if (20..34).contains(&y) && stem {
                [200, 200, 200]
            } else {
                [20, 24, 30]
            }
        });

        let metrics = glyph_metrics(&frame, rect(36, 14, 66, 40), (0, 160)).unwrap();

        assert_eq!(metrics.extent.right, 63);
        assert_eq!(metrics.continues, (false, true));
    }

    #[test]
    fn glyph_metrics_never_claim_marks_beyond_the_neighbouring_row() {
        let frame = frame(140, 60, caption);

        let metrics = glyph_metrics(&frame, rect(36, 14, 90, 46), (32, 140)).unwrap();

        assert_eq!(metrics.extent.left, 40);
    }

    #[test]
    fn a_descender_cut_off_by_the_box_still_belongs_to_the_glyphs() {
        // Letters on rows 20..34 with a descender down to row 39; the OCR
        // box ends at row 36.
        let frame = frame(120, 60, |x, y| {
            let stems = [30, 40, 50, 60]
                .iter()
                .any(|left| (*left..left + 3).contains(&x));
            let descender = (40..43).contains(&x) && (34..39).contains(&y);
            if ((20..34).contains(&y) && stems) || descender {
                [230, 230, 230]
            } else {
                [20, 22, 26]
            }
        });

        let metrics = glyph_metrics(&frame, rect(26, 16, 70, 36), (0, 120)).unwrap();

        assert_eq!(metrics.core, (20, 34));
        assert_eq!(metrics.extent.bottom, 39);
    }

    #[test]
    fn anti_aliased_edges_do_not_thicken_the_stroke() {
        // 2 px stems with a half-tone pixel either side: regular text.
        let frame = frame(120, 60, |x, y| {
            let phase = (x + 10) % 10;
            if !(20..34).contains(&y) || !(20..100).contains(&x) {
                [20, 20, 20]
            } else if phase == 1 || phase == 2 {
                [220, 220, 220]
            } else if phase == 0 || phase == 3 {
                [150, 150, 150]
            } else {
                [20, 20, 20]
            }
        });

        let metrics = glyph_metrics(&frame, rect(16, 14, 104, 40), (0, 120)).unwrap();

        assert_eq!(metrics.stroke, 2.0);
    }

    #[test]
    fn a_button_edge_beside_the_text_is_not_more_text() {
        // Light text in a dark button whose 2 px light border is at x 60.
        let frame = frame(120, 50, |x, y| {
            let stem = [20, 30, 40]
                .iter()
                .any(|left| (*left..left + 2).contains(&x));
            if ((15..27).contains(&y) && stem) || (60..62).contains(&x) {
                [235, 235, 235]
            } else {
                [40, 32, 26]
            }
        });

        let metrics = glyph_metrics(&frame, rect(16, 10, 46, 32), (0, 120)).unwrap();

        assert_eq!(metrics.continues, (false, false));
        assert_eq!(metrics.free_right, 58, "up to the border, not over it");
    }

    #[test]
    fn plain_background_has_no_glyphs() {
        let frame = frame(80, 40, |_, _| [200, 200, 200]);

        assert_eq!(glyph_metrics(&frame, rect(10, 10, 60, 30), (0, 80)), None);
    }

    #[test]
    fn a_repaired_region_continues_the_background_gradient_and_hides_the_text() {
        let frame = frame(140, 60, caption);

        let pixels = repaired_pixels(&frame, &[rect(22, 16, 88, 44)], &[]);

        let at = |x: usize, y: usize| {
            let offset = (y * 140 + x) * 4;
            [pixels[offset], pixels[offset + 1], pixels[offset + 2]]
        };
        for (x, y) in [(42, 30), (62, 22), (28, 24), (82, 38)] {
            let [red, green, blue] = at(x, y);
            let expected = 20 + (y / 4) as u8;
            assert!(
                red.abs_diff(expected) <= 3,
                "({x},{y}) red {red} != {expected}"
            );
            assert!(green.abs_diff(30) <= 3 && blue.abs_diff(40) <= 3);
        }
        // Outside the region nothing changes.
        assert_eq!(at(100, 30), [27, 30, 40]);
    }

    #[test]
    fn a_fill_under_a_kept_row_samples_the_background_beyond_it() {
        // A replaced row directly below a kept white title: its top edge must
        // come from the background above the title, not from the title.
        let frame = frame(120, 80, |x, y| {
            if (10..30).contains(&y) && (20..100).contains(&x) && x % 6 < 3 {
                [250, 250, 250]
            } else {
                [30, 34, 40]
            }
        });

        let pixels = repaired_pixels(&frame, &[rect(20, 30, 100, 50)], &[rect(18, 8, 102, 30)]);

        for x in [24, 60, 96] {
            let offset = (32 * 120 + x) * 4;
            let red = pixels[offset];
            assert!(
                red <= 40,
                "x {x}: the kept title bled into the fill ({red})"
            );
        }
    }

    #[test]
    fn protected_text_keeps_its_pixels_inside_a_repaired_region() {
        let frame = frame(140, 60, caption);

        let pixels = repaired_pixels(&frame, &[rect(22, 16, 88, 44)], &[rect(58, 16, 66, 44)]);

        let offset = (30 * 140 + 61) * 4;
        assert_eq!(&pixels[offset..offset + 3], &[240, 236, 228]);
    }

    #[test]
    fn sampling_exclusion_does_not_preserve_ink_in_the_paint_target() {
        let frame = frame(100, 70, |x, y| {
            if ((25..75).contains(&x) && (25..38).contains(&y))
                || ((10..90).contains(&x) && (35..55).contains(&y))
            {
                [15; 3]
            } else {
                [230; 3]
            }
        });
        let (pixels, complete) = repaired_pixels_with_sampling_exclusions(
            &frame,
            &[rect(20, 20, 80, 40)],
            &[rect(48, 28, 52, 32)],
            &[rect(10, 35, 90, 55)],
        );
        assert!(complete);
        assert!(
            pixels[(37 * 100 + 50) * 4] >= 220,
            "overlapping source ink must still be erased"
        );
        assert_eq!(
            pixels[(30 * 100 + 50) * 4],
            15,
            "explicit protected pixels stay unchanged"
        );
        assert_eq!(
            pixels[(45 * 100 + 50) * 4],
            15,
            "sampling exclusion is not another paint target"
        );
        assert_eq!(frame.pixel(50, 37).unwrap()[0], 15);
    }

    #[test]
    fn blocked_sampling_rays_use_clean_corners_and_report_no_background_honestly() {
        let target = rect(20, 20, 40, 40);
        let exclusions = [
            rect(20, 0, 40, 20),
            rect(20, 40, 40, 60),
            rect(0, 20, 20, 40),
            rect(40, 20, 60, 40),
        ];
        let frame = frame(60, 60, |x, y| {
            if (20..40).contains(&x) || (20..40).contains(&y) {
                [15; 3]
            } else {
                [230; 3]
            }
        });
        let (pixels, complete) =
            repaired_pixels_with_sampling_exclusions(&frame, &[target], &[], &exclusions);
        assert!(complete);
        assert_eq!(pixels[(30 * 60 + 30) * 4], 230);
        assert_eq!(pixels[(10 * 60 + 30) * 4], 15);
        let (pixels, complete) =
            repaired_pixels_with_sampling_exclusions(&frame, &[target], &[], &[rect(0, 0, 60, 60)]);
        assert!(!complete);
        assert_eq!(pixels, frame.pixels);
    }
}
