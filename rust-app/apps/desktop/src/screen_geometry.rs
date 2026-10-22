//! Small, deterministic transforms for OCR geometry. Work is done once when
//! a result changes; the live view uses Slint's existing text renderer.

use rust_platform_windows::screen_capture::RgbaFrame;
use rust_protocol::{PhysicalQuad, PhysicalRect};

use crate::screen_inpaint::{PixelRect, repaired_pixels_with_sampling_exclusions};

#[derive(Clone, Copy, Debug)]
pub(crate) struct OrientedBox {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) angle: f32,
    cosine: f32,
    sine: f32,
}

impl OrientedBox {
    pub(crate) fn new(quad: PhysicalQuad) -> Option<Self> {
        if !quad.is_valid() {
            return None;
        }
        let [a, b, c, d] = quad.points;
        let dx = ((b.x as f32 - a.x as f32) + (c.x as f32 - d.x as f32)) * 0.5;
        let dy = ((b.y as f32 - a.y as f32) + (c.y as f32 - d.y as f32)) * 0.5;
        let width = dx.hypot(dy);
        if width < 1.0 {
            return None;
        }
        let (cosine, sine) = (dx / width, dy / width);
        let height = (((d.y as f32 - a.y as f32) + (c.y as f32 - b.y as f32)) * cosine
            - ((d.x as f32 - a.x as f32) + (c.x as f32 - b.x as f32)) * sine)
            * 0.5;
        if height < 1.0 {
            return None;
        }
        Some(Self {
            x: a.x as f32,
            y: a.y as f32,
            width,
            height,
            angle: dy.atan2(dx).to_degrees(),
            cosine,
            sine,
        })
    }

    pub(crate) fn point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.x + x * self.cosine - y * self.sine,
            self.y + x * self.sine + y * self.cosine,
        )
    }

    pub(crate) fn local(self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = (x - self.x, y - self.y);
        (
            x * self.cosine + y * self.sine,
            -x * self.sine + y * self.cosine,
        )
    }

    pub(crate) fn rectified(self, frame: &RgbaFrame, margin: u32) -> Option<RgbaFrame> {
        if !self.width.is_finite()
            || !self.height.is_finite()
            || self.width > 16_000_000.0
            || self.height > 16_000_000.0
        {
            return None;
        }
        let padding = margin.checked_mul(2)?;
        let width = (self.width.ceil() as u32).checked_add(padding)?;
        let height = (self.height.ceil() as u32).checked_add(padding)?;
        if width.checked_mul(height)? > 16_000_000 {
            return None;
        }
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let (px, py) = self.point(x as f32 - margin as f32, y as f32 - margin as f32);
                pixels.extend_from_slice(&sample(frame, px, py));
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
        .ok()
    }
}

fn sample(frame: &RgbaFrame, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, frame.rect.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, frame.rect.height.saturating_sub(1) as f32);
    let (left, top) = (x.floor() as u32, y.floor() as u32);
    let right = (left + 1).min(frame.rect.width.saturating_sub(1));
    let bottom = (top + 1).min(frame.rect.height.saturating_sub(1));
    let (dx, dy) = (x - left as f32, y - top as f32);
    let a = frame.pixel(left, top).unwrap_or([0, 0, 0, 255]);
    let b = frame.pixel(right, top).unwrap_or(a);
    let c = frame.pixel(left, bottom).unwrap_or(a);
    let d = frame.pixel(right, bottom).unwrap_or(a);
    std::array::from_fn(|i| {
        ((f32::from(a[i]) * (1.0 - dx) + f32::from(b[i]) * dx) * (1.0 - dy)
            + (f32::from(c[i]) * (1.0 - dx) + f32::from(d[i]) * dx) * dy)
            .round() as u8
    })
}

pub(crate) fn contains(quad: PhysicalQuad, x: f32, y: f32) -> bool {
    quad.points
        .iter()
        .zip(quad.points.iter().cycle().skip(1))
        .take(4)
        .all(|(a, b)| {
            (b.x as f32 - a.x as f32) * (y - a.y as f32)
                - (b.y as f32 - a.y as f32) * (x - a.x as f32)
                >= -0.01
        })
}

/// Fill only the oriented source strips. Sampling and interpolation happen in
/// their upright coordinate system, so a long tilted line never erases a whole
/// axis-aligned rectangle or a neighbouring unchanged line.
pub(crate) fn repair_oriented(
    frame: &RgbaFrame,
    pixels: &mut [u8],
    quads: &[PhysicalQuad],
    protected: &[PhysicalQuad],
) -> Vec<PhysicalQuad> {
    let mut failed = Vec::new();
    for (index, quad) in quads.iter().enumerate() {
        let Some(oriented) = OrientedBox::new(*quad) else {
            failed.push(*quad);
            continue;
        };
        let margin = (oriented.height * 0.5).ceil().clamp(8.0, 64.0) as u32;
        let Some(patch) = oriented.rectified(frame, margin) else {
            failed.push(*quad);
            continue;
        };
        let region = PixelRect {
            left: i64::from(margin) - 1,
            top: i64::from(margin) - 1,
            right: i64::from(patch.rect.width - margin) + 1,
            bottom: i64::from(patch.rect.height - margin) + 1,
        };
        let mut sampling_exclusions = quads
            .iter()
            .enumerate()
            .filter_map(|(other_index, quad)| (other_index != index).then_some(quad))
            .chain(protected.iter())
            .filter(|quad| quad.is_valid())
            .map(|quad| {
                let local = quad
                    .points
                    .map(|point| oriented.local(point.x as f32, point.y as f32));
                let left = local
                    .iter()
                    .map(|point| point.0)
                    .fold(f32::INFINITY, f32::min);
                let top = local
                    .iter()
                    .map(|point| point.1)
                    .fold(f32::INFINITY, f32::min);
                let right = local
                    .iter()
                    .map(|point| point.0)
                    .fold(f32::NEG_INFINITY, f32::max);
                let bottom = local
                    .iter()
                    .map(|point| point.1)
                    .fold(f32::NEG_INFINITY, f32::max);
                // Include interpolation footprints and detector edge rounding.
                PixelRect {
                    left: left.floor() as i64 + i64::from(margin) - 2,
                    top: top.floor() as i64 + i64::from(margin) - 2,
                    right: right.ceil() as i64 + i64::from(margin) + 2,
                    bottom: bottom.ceil() as i64 + i64::from(margin) + 2,
                }
            })
            .collect::<Vec<_>>();
        // Rectification clamps off-image samples to the crop's edge. Those
        // replicated pixels are not observed background (a clipped signature
        // can have ink on that edge). Exclude them without changing paint.
        for y in 0..patch.rect.height {
            let (base_x, base_y) = oriented.point(-(margin as f32), y as f32 - margin as f32);
            let mut start = 0.0_f32;
            let mut end = patch.rect.width.saturating_sub(1) as f32;
            for (base, slope, maximum) in [
                (
                    base_x,
                    oriented.cosine,
                    frame.rect.width.saturating_sub(1) as f32,
                ),
                (
                    base_y,
                    oriented.sine,
                    frame.rect.height.saturating_sub(1) as f32,
                ),
            ] {
                if slope.abs() < 0.000001 {
                    if base < 0.0 || base > maximum {
                        end = -1.0;
                    }
                } else {
                    let a = -base / slope;
                    let b = (maximum - base) / slope;
                    start = start.max(a.min(b));
                    end = end.min(a.max(b));
                }
            }
            let (valid_left, valid_right) = if start <= end {
                (start.ceil().max(0.0) as i64, end.floor() as i64 + 1)
            } else {
                (i64::from(patch.rect.width), i64::from(patch.rect.width))
            };
            for (left, right) in [(0, valid_left), (valid_right, i64::from(patch.rect.width))] {
                if left < right {
                    sampling_exclusions.push(PixelRect {
                        left,
                        right,
                        top: i64::from(y),
                        bottom: i64::from(y) + 1,
                    });
                }
            }
        }
        let (fill, complete) =
            repaired_pixels_with_sampling_exclusions(&patch, &[region], &[], &sampling_exclusions);
        if !complete {
            failed.push(*quad);
            continue;
        }
        let Ok(repaired) = RgbaFrame::try_new(patch.rect, patch.stride, fill) else {
            failed.push(*quad);
            continue;
        };
        let left = quad.points.iter().map(|p| p.x).min().unwrap_or(0).max(0) as u32;
        let right = (quad.points.iter().map(|p| p.x).max().unwrap_or(0).max(0) as u32)
            .min(frame.rect.width);
        let top = quad.points.iter().map(|p| p.y).min().unwrap_or(0).max(0) as u32;
        let bottom = (quad.points.iter().map(|p| p.y).max().unwrap_or(0).max(0) as u32)
            .min(frame.rect.height);
        for y in top..bottom {
            for x in left..right {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                if !contains(*quad, px, py) || protected.iter().any(|q| contains(*q, px, py)) {
                    continue;
                }
                let (u, v) = oriented.local(px, py);
                let color = sample(&repaired, u + margin as f32, v + margin as f32);
                let at = ((y * frame.rect.width + x) * 4) as usize;
                if let Some(target) = pixels.get_mut(at..at + 4) {
                    target.copy_from_slice(&color);
                }
            }
        }
    }
    // Earlier successful strips can overlap a failed strip's padding. Restore
    // that entire polygon before the caller draws its explicit plate fallback.
    for quad in &failed {
        let left = quad.points.iter().map(|p| p.x).min().unwrap_or(0).max(0) as u32;
        let right = (quad.points.iter().map(|p| p.x).max().unwrap_or(0).max(0) as u32)
            .min(frame.rect.width);
        let top = quad.points.iter().map(|p| p.y).min().unwrap_or(0).max(0) as u32;
        let bottom = (quad.points.iter().map(|p| p.y).max().unwrap_or(0).max(0) as u32)
            .min(frame.rect.height);
        for y in top..bottom {
            for x in left..right {
                if contains(*quad, x as f32 + 0.5, y as f32 + 0.5)
                    && let Some(color) = frame.pixel(x, y)
                {
                    let at = ((y * frame.rect.width + x) * 4) as usize;
                    if let Some(target) = pixels.get_mut(at..at + 4) {
                        target.copy_from_slice(&color);
                    }
                }
            }
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_protocol::PhysicalPoint;

    fn tilted() -> PhysicalQuad {
        PhysicalQuad {
            points: [(20, 10), (120, 40), (114, 60), (14, 30)].map(|(x, y)| PhysicalPoint { x, y }),
        }
    }

    #[test]
    fn tilted_basis_keeps_actual_line_height_and_roundtrips_points() {
        let b = OrientedBox::new(tilted()).unwrap();
        assert!(b.height < 22.0 && b.height > 20.0);
        assert!((b.angle - 16.7).abs() < 0.1);
        let point = b.point(40.0, 11.0);
        let local = b.local(point.0, point.1);
        assert!((local.0 - 40.0).abs() < 0.001 && (local.1 - 11.0).abs() < 0.001);
    }

    #[test]
    fn extreme_valid_geometry_and_margin_are_rejected_before_allocation() {
        let quad = PhysicalQuad {
            points: [
                (i32::MIN, i32::MIN),
                (i32::MAX, i32::MIN),
                (i32::MAX, i32::MAX),
                (i32::MIN, i32::MAX),
            ]
            .map(|(x, y)| PhysicalPoint { x, y }),
        };
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            4,
            vec![0, 0, 0, 255],
        )
        .unwrap();
        assert!(
            OrientedBox::new(quad)
                .unwrap()
                .rectified(&frame, 8)
                .is_none()
        );
        assert!(
            OrientedBox::new(tilted())
                .unwrap()
                .rectified(&frame, u32::MAX)
                .is_none()
        );
    }

    #[test]
    fn tilted_repair_preserves_pixels_outside_quad_and_protected_text() {
        let rect = PhysicalRect {
            x: 0,
            y: 0,
            width: 140,
            height: 80,
        };
        let mut bytes = vec![230; (rect.width * rect.height * 4) as usize];
        for pixel in bytes.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        // Simulated ink well inside the sloping line and a neighbour in its AABB.
        for (x, y) in [(65, 32), (100, 13)] {
            let i = (y * 140 + x) * 4;
            bytes[i..i + 3].fill(0);
        }
        let frame = RgbaFrame::try_new(rect, 560, bytes.clone()).unwrap();
        assert!(repair_oriented(&frame, &mut bytes, &[tilted()], &[]).is_empty());
        assert!(bytes[(32 * 140 + 65) * 4] > 180);
        assert_eq!(
            &bytes[(13 * 140 + 100) * 4..(13 * 140 + 100) * 4 + 3],
            &[0, 0, 0]
        );
        for y in 0..80 {
            for x in 0..140 {
                if !contains(tilted(), x as f32 + 0.5, y as f32 + 0.5) {
                    let i = ((y * 140 + x) * 4) as usize;
                    assert_eq!(&bytes[i..i + 4], &frame.pixels[i..i + 4]);
                }
            }
        }
        let mut kept = frame.pixels.clone();
        assert!(repair_oriented(&frame, &mut kept, &[tilted()], &[tilted()]).is_empty());
        assert_eq!(kept, frame.pixels);
    }

    #[test]
    fn overlapping_tilted_strips_do_not_sample_neighbouring_dark_text() {
        let quad = |points: [(i32, i32); 4]| PhysicalQuad {
            points: points.map(|(x, y)| PhysicalPoint { x, y }),
        };
        let first = quad([(40, 20), (200, 52), (194, 82), (34, 50)]);
        let second = quad([(35, 44), (195, 76), (189, 106), (29, 74)]);
        let orientations = [first, second].map(|quad| OrientedBox::new(quad).unwrap());
        let rect = PhysicalRect {
            x: 0,
            y: 0,
            width: 240,
            height: 140,
        };
        let mut original = Vec::with_capacity((rect.width * rect.height * 4) as usize);
        for y in 0..rect.height {
            for x in 0..rect.width {
                let ink = orientations.iter().any(|line| {
                    let (u, v) = line.local(x as f32 + 0.5, y as f32 + 0.5);
                    (10.0..150.0).contains(&u) && (8.0..24.0).contains(&v) && (u as u32 % 10) < 7
                });
                let shade = if ink { 15 } else { 230 };
                original.extend_from_slice(&[shade, shade, shade, 255]);
            }
        }
        let frame = RgbaFrame::try_new(rect, rect.width * 4, original.clone()).unwrap();
        let mut repaired = original.clone();
        assert!(repair_oriented(&frame, &mut repaired, &[first, second], &[]).is_empty());
        for line in orientations {
            for (u, v) in [(60.0, 3.0), (80.0, 16.0), (100.0, 28.0)] {
                let (x, y) = line.point(u, v);
                let at = ((y.round() as u32 * rect.width + x.round() as u32) * 4) as usize;
                assert!(
                    repaired[at] >= 220,
                    "neighbour ink entered the fill at {x},{y}: {}",
                    repaired[at]
                );
            }
        }
        for y in 0..rect.height {
            for x in 0..rect.width {
                if ![first, second]
                    .iter()
                    .any(|quad| contains(*quad, x as f32 + 0.5, y as f32 + 0.5))
                {
                    let at = ((y * rect.width + x) * 4) as usize;
                    assert_eq!(&repaired[at..at + 4], &original[at..at + 4]);
                }
            }
        }
        assert_eq!(frame.pixels, original);
    }

    #[test]
    fn clipped_region_with_no_observed_background_returns_failure_and_keeps_source() {
        let quad = PhysicalQuad {
            points: [(0, 0), (10, 0), (10, 10), (0, 10)].map(|(x, y)| PhysicalPoint { x, y }),
        };
        let frame = RgbaFrame::try_new(
            PhysicalRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            40,
            vec![15; 400],
        )
        .unwrap();
        let mut pixels = frame.pixels.clone();
        assert_eq!(
            repair_oriented(&frame, &mut pixels, &[quad], &[]),
            vec![quad]
        );
        assert_eq!(pixels, frame.pixels);
    }
}
