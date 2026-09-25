use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

use serde::{Deserialize, Serialize};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub(crate) const PULSE_MATRIX_IMAGE_SIZE: u32 = 128;
const PULSE_MATRIX_RASTER_SCALE: u32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PulseMatrixPalette {
    #[default]
    Cyan,
    GraphiteSilver,
    RedAmber,
}

impl PulseMatrixPalette {
    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Cyan),
            1 => Some(Self::GraphiteSilver),
            2 => Some(Self::RedAmber),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::Cyan => 0,
            Self::GraphiteSilver => 1,
            Self::RedAmber => 2,
        }
    }
}

#[derive(Clone, Copy)]
struct RenderRequest {
    phase: f32,
    energy: f32,
    palette: PulseMatrixPalette,
    opacity: f32,
}

struct RenderedFrame {
    pixels: Vec<u8>,
}

pub(crate) struct PulseMatrixRenderer {
    requests: Option<SyncSender<RenderRequest>>,
    frames: Receiver<RenderedFrame>,
    thread: Option<JoinHandle<()>>,
}

impl PulseMatrixRenderer {
    pub(crate) fn start() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel::<RenderRequest>(1);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<RenderedFrame>(1);
        let thread = thread::Builder::new()
            .name("pulse-matrix-renderer".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let frame = render_pulse_matrix_buffer(
                        request.phase,
                        request.energy,
                        request.palette,
                        request.opacity,
                    );
                    let rendered = RenderedFrame {
                        pixels: frame.as_bytes().to_vec(),
                    };
                    match frame_tx.try_send(rendered) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            // Animation frames are disposable; latency is not.
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
            })
            .expect("pulse matrix renderer thread");
        Self {
            requests: Some(request_tx),
            frames: frame_rx,
            thread: Some(thread),
        }
    }

    pub(crate) fn render_latest(
        &self,
        phase: f32,
        energy: f32,
        palette: PulseMatrixPalette,
        opacity: f32,
    ) -> Option<Image> {
        if let Some(requests) = &self.requests {
            match requests.try_send(RenderRequest {
                phase,
                energy,
                palette,
                opacity,
            }) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return None,
            }
        }

        let mut latest = None;
        while let Ok(frame) = self.frames.try_recv() {
            latest = Some(frame);
        }
        latest.map(|frame| {
            let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(
                PULSE_MATRIX_IMAGE_SIZE,
                PULSE_MATRIX_IMAGE_SIZE,
            );
            buffer.make_mut_bytes().copy_from_slice(&frame.pixels);
            Image::from_rgba8_premultiplied(buffer)
        })
    }
}

impl Drop for PulseMatrixRenderer {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn render_pulse_matrix_buffer(
    phase: f32,
    energy: f32,
    palette: PulseMatrixPalette,
    opacity: f32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let raster_size = PULSE_MATRIX_IMAGE_SIZE * PULSE_MATRIX_RASTER_SCALE;
    let raster_scale = PULSE_MATRIX_RASTER_SCALE as f32;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(raster_size, raster_size);
    buffer.make_mut_bytes().fill(0);

    let fade = opacity.clamp(0.0, 1.0);
    let center = raster_size as f32 / 2.0;
    let level = energy.clamp(0.0, 1.0);
    let colors = palette_colors(palette);
    let breath = 1.0 + level * 0.025 + (phase * 0.42).sin() * 0.003;
    let radius = 45.5 * breath * raster_scale;
    let mut canvas = RasterCanvas {
        pixels: buffer.make_mut_bytes(),
        width: raster_size,
        height: raster_size,
    };

    // Tight, dark teal hairlines from the approved reference. They remain
    // separate rings instead of becoming a broad glow cloud.
    for ring in 0..6 {
        let travel = (phase * 0.82 - ring as f32 * 0.72).sin();
        let ring_radius =
            radius * (1.055 + ring as f32 * 0.038) + travel * (0.65 + level * 1.25) * raster_scale;
        let alpha = (0.54 - ring as f32 * 0.073 + level * 0.035) * fade;
        let line_width = if ring == 0 { 1.25 } else { 0.9 } * raster_scale;
        // Two coincident hairlines preserve the approved geometry while
        // guaranteeing contrast on both bright and dark desktop content.
        canvas.draw_ring(
            ring_radius,
            line_width + 0.75 * raster_scale,
            colors.ring_shadow,
            alpha * 0.94,
        );
        canvas.draw_ring(
            ring_radius,
            (line_width * 0.48).max(0.48 * raster_scale),
            colors.ring_highlight,
            alpha * 0.82,
        );
    }

    // Nearly-black glass core with cyan energy held inside the circumference.
    let edge_fade_width = 1.5 * raster_scale;
    for y in 0..raster_size {
        for x in 0..raster_size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > radius + edge_fade_width {
                continue;
            }
            let normalized = distance / radius;
            let edge_coverage =
                1.0 - smoothstep(radius - edge_fade_width, radius + edge_fade_width, distance);
            let rim = smoothstep(0.55, 1.0, normalized).powf(1.55);
            let lower = smoothstep(-0.35, 0.95, dy / radius);
            let side = (dx / radius).abs().powf(1.4);
            let energy_bias = (0.78 + lower * 0.16 + side * 0.06).clamp(0.0, 1.0);
            let mut color = mix_rgb(colors.center, colors.shell, rim * energy_bias);
            let inner_line =
                smoothstep(0.92, 0.96, normalized) * (1.0 - smoothstep(0.976, 1.006, normalized));
            color = mix_rgb(color, colors.highlight, inner_line * 0.72);
            color = mix_rgb(color, colors.edge, smoothstep(0.982, 1.025, normalized));
            canvas.blend_pixel(x as i32, y as i32, color, fade * edge_coverage);
        }
    }

    let spacing = radius * 0.128;
    for row in -3_i32..=3 {
        for column in -3_i32..=3 {
            let distance = ((column * column + row * row) as f32).sqrt();
            if distance > 3.55 {
                continue;
            }
            let random = fractional(
                ((column as f32 + 7.23) * 12.9898 + (row as f32 + 5.91) * 78.233).sin()
                    * 43_758.547,
            );
            let angle_bias = (phase * 1.35 + column as f32 * 0.76 - row as f32 * 0.34).sin();
            let radial_wave = (phase * 1.9 - distance * 1.42 + random * 1.8).sin();
            let voice = (level * 1.32 + radial_wave * 0.25 + angle_bias * 0.11).max(0.0);
            let center_weight = (1.0 - distance / 5.2).max(0.2);
            let visibility = (voice * center_weight + 0.08 - distance * 0.012).clamp(0.0, 1.0);
            let gate = 0.12 + random * 0.3;
            let appeared = ((visibility - gate) / (1.0 - gate)).clamp(0.0, 1.0);
            let idle = (0.1 - distance * 0.017).max(0.0);
            let alpha = (idle + appeared * 0.94).min(1.0) * fade;
            if alpha < 0.025 {
                continue;
            }

            let x = center + column as f32 * spacing;
            let y = center + row as f32 * spacing;
            let dot_radius = radius * (0.012 + center_weight * 0.013 + appeared * 0.018);
            canvas.draw_disc(
                x,
                y,
                dot_radius + (2.0 + appeared * 2.5) * raster_scale,
                colors.dot_glow,
                alpha * colors.dot_glow_alpha,
            );
            canvas.draw_disc(x, y, dot_radius, colors.dot, alpha);
        }
    }

    downsample_premultiplied(buffer, PULSE_MATRIX_RASTER_SCALE)
}

#[derive(Clone, Copy)]
struct PulseMatrixColors {
    ring_shadow: [u8; 3],
    ring_highlight: [u8; 3],
    center: [u8; 3],
    shell: [u8; 3],
    highlight: [u8; 3],
    edge: [u8; 3],
    dot_glow: [u8; 3],
    dot_glow_alpha: f32,
    dot: [u8; 3],
}

fn palette_colors(palette: PulseMatrixPalette) -> PulseMatrixColors {
    match palette {
        PulseMatrixPalette::Cyan => PulseMatrixColors {
            ring_shadow: [0, 34, 40],
            ring_highlight: [177, 244, 246],
            center: [2, 6, 8],
            shell: [28, 174, 183],
            highlight: [48, 205, 215],
            edge: [1, 8, 10],
            dot_glow: [212, 247, 249],
            dot_glow_alpha: 0.12,
            dot: [242, 247, 247],
        },
        PulseMatrixPalette::GraphiteSilver => PulseMatrixColors {
            ring_shadow: [31, 37, 41],
            ring_highlight: [232, 236, 238],
            center: [247, 248, 248],
            shell: [150, 158, 162],
            highlight: [226, 230, 231],
            edge: [28, 33, 36],
            dot_glow: [17, 19, 20],
            dot_glow_alpha: 0.035,
            dot: [7, 9, 10],
        },
        PulseMatrixPalette::RedAmber => PulseMatrixColors {
            ring_shadow: [76, 9, 8],
            ring_highlight: [255, 151, 58],
            center: [8, 2, 2],
            shell: [207, 55, 18],
            highlight: [255, 116, 40],
            edge: [24, 2, 1],
            dot_glow: [255, 187, 116],
            dot_glow_alpha: 0.12,
            dot: [255, 240, 220],
        },
    }
}

fn downsample_premultiplied(
    source: SharedPixelBuffer<Rgba8Pixel>,
    scale: u32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let target_width = source.width() / scale;
    let target_height = source.height() / scale;
    let source_width = source.width();
    let source_bytes = source.as_bytes();
    let mut target = SharedPixelBuffer::<Rgba8Pixel>::new(target_width, target_height);
    let target_bytes = target.make_mut_bytes();
    let sample_count = scale * scale;

    for target_y in 0..target_height {
        for target_x in 0..target_width {
            let target_offset = ((target_y * target_width + target_x) * 4) as usize;
            let mut totals = [0_u32; 4];
            for sample_y in 0..scale {
                for sample_x in 0..scale {
                    let source_x = target_x * scale + sample_x;
                    let source_y = target_y * scale + sample_y;
                    let source_offset = ((source_y * source_width + source_x) * 4) as usize;
                    for channel in 0..4 {
                        totals[channel] += source_bytes[source_offset + channel] as u32;
                    }
                }
            }
            for channel in 0..4 {
                target_bytes[target_offset + channel] =
                    ((totals[channel] + sample_count / 2) / sample_count) as u8;
            }
        }
    }

    target
}

struct RasterCanvas<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
}

impl RasterCanvas<'_> {
    fn draw_ring(&mut self, radius: f32, line_width: f32, color: [u8; 3], alpha: f32) {
        let center = self.width as f32 / 2.0;
        let half_width = line_width * 0.5;
        let reach = radius + half_width + 1.0;
        let minimum = (center - reach).floor().max(0.0) as i32;
        let maximum = (center + reach).ceil().min(self.width as f32 - 1.0) as i32;
        for y in minimum..=maximum {
            for x in minimum..=maximum {
                let dx = x as f32 + 0.5 - center;
                let dy = y as f32 + 0.5 - center;
                let edge = ((dx * dx + dy * dy).sqrt() - radius).abs();
                let coverage = (half_width + 0.75 - edge).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend_pixel(x, y, color, alpha * coverage);
                }
            }
        }
    }

    fn draw_disc(&mut self, center_x: f32, center_y: f32, radius: f32, color: [u8; 3], alpha: f32) {
        let minimum_x = (center_x - radius - 1.0).floor().max(0.0) as i32;
        let maximum_x = (center_x + radius + 1.0)
            .ceil()
            .min(self.width as f32 - 1.0) as i32;
        let minimum_y = (center_y - radius - 1.0).floor().max(0.0) as i32;
        let maximum_y = (center_y + radius + 1.0)
            .ceil()
            .min(self.height as f32 - 1.0) as i32;
        for y in minimum_y..=maximum_y {
            for x in minimum_x..=maximum_x {
                let dx = x as f32 + 0.5 - center_x;
                let dy = y as f32 + 0.5 - center_y;
                let distance = (dx * dx + dy * dy).sqrt();
                let coverage = (radius + 0.75 - distance).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend_pixel(x, y, color, alpha * coverage);
                }
            }
        }
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let source_alpha = alpha.clamp(0.0, 1.0);
        if source_alpha <= 0.0 {
            return;
        }
        let offset = ((y as u32 * self.width + x as u32) * 4) as usize;
        let destination_alpha = self.pixels[offset + 3] as f32 / 255.0;
        let inverse = 1.0 - source_alpha;
        for (channel, source) in color.into_iter().enumerate() {
            let destination = self.pixels[offset + channel] as f32 / 255.0;
            let premultiplied = source as f32 / 255.0 * source_alpha + destination * inverse;
            self.pixels[offset + channel] = (premultiplied * 255.0).round() as u8;
        }
        let out_alpha = source_alpha + destination_alpha * inverse;
        self.pixels[offset + 3] = (out_alpha * 255.0).round() as u8;
    }
}

fn mix_rgb(from: [u8; 3], to: [u8; 3], amount: f32) -> [u8; 3] {
    let amount = amount.clamp(0.0, 1.0);
    std::array::from_fn(|index| {
        (from[index] as f32 + (to[index] as f32 - from[index] as f32) * amount).round() as u8
    })
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let normalized = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

fn fractional(value: f32) -> f32 {
    value - value.floor()
}

#[cfg(test)]
mod tests {
    use super::{PULSE_MATRIX_IMAGE_SIZE, PulseMatrixPalette, render_pulse_matrix_buffer};

    fn luma(red: f32, green: f32, blue: f32) -> f32 {
        red * 0.2126 + green * 0.7152 + blue * 0.0722
    }

    fn ring_contrast_samples(palette: PulseMatrixPalette, background: u8) -> usize {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, palette, 1.0);
        let center = PULSE_MATRIX_IMAGE_SIZE as f32 / 2.0;
        let background = f32::from(background) / 255.0;
        let background_luma = luma(background, background, background);
        frame
            .as_bytes()
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, pixel)| {
                let x = (*index as u32 % PULSE_MATRIX_IMAGE_SIZE) as f32 + 0.5;
                let y = (*index as u32 / PULSE_MATRIX_IMAGE_SIZE) as f32 + 0.5;
                let distance = ((x - center).powi(2) + (y - center).powi(2)).sqrt();
                if !(48.0..=60.5).contains(&distance) || pixel[3] == 0 {
                    return false;
                }
                let alpha = f32::from(pixel[3]) / 255.0;
                let composite =
                    |channel: u8| f32::from(channel) / 255.0 + background * (1.0 - alpha);
                (luma(
                    composite(pixel[0]),
                    composite(pixel[1]),
                    composite(pixel[2]),
                ) - background_luma)
                    .abs()
                    >= 0.12
            })
            .count()
    }

    #[test]
    fn every_pulse_matrix_palette_has_a_stable_saved_value() {
        for (index, palette, serialized) in [
            (0, PulseMatrixPalette::Cyan, "\"cyan\""),
            (1, PulseMatrixPalette::GraphiteSilver, "\"graphite_silver\""),
            (2, PulseMatrixPalette::RedAmber, "\"red_amber\""),
        ] {
            assert_eq!(PulseMatrixPalette::from_index(index), Some(palette));
            assert_eq!(palette.index(), index);
            assert_eq!(serde_json::to_string(&palette).unwrap(), serialized);
        }
        assert_eq!(PulseMatrixPalette::from_index(3), None);
    }

    #[test]
    fn every_ring_palette_remains_visible_on_dark_and_light_backgrounds() {
        for palette in [
            PulseMatrixPalette::Cyan,
            PulseMatrixPalette::GraphiteSilver,
            PulseMatrixPalette::RedAmber,
        ] {
            let on_dark = ring_contrast_samples(palette, 18);
            let on_light = ring_contrast_samples(palette, 238);
            assert!(
                on_dark >= 20,
                "{palette:?} rings vanish on dark backgrounds: {on_dark} samples"
            );
            assert!(
                on_light >= 20,
                "{palette:?} rings vanish on light backgrounds: {on_light} samples"
            );
        }
    }

    #[test]
    fn graphite_silver_uses_black_dots_on_a_light_orb() {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, PulseMatrixPalette::GraphiteSilver, 1.0);
        let pixels = frame.as_bytes();
        let center = PULSE_MATRIX_IMAGE_SIZE / 2;
        let center_offset = ((center * PULSE_MATRIX_IMAGE_SIZE + center) * 4) as usize;
        let between_dots_offset =
            (((center + 3) * PULSE_MATRIX_IMAGE_SIZE + center + 3) * 4) as usize;
        let center_dot = &pixels[center_offset..center_offset + 4];
        let light_surface = &pixels[between_dots_offset..between_dots_offset + 4];

        assert!(
            center_dot[0] < 90 && center_dot[1] < 90 && center_dot[2] < 90,
            "graphite matrix dot is not black: {center_dot:?}"
        );
        assert!(
            light_surface[0] > 175 && light_surface[1] > 175 && light_surface[2] > 175,
            "graphite orb is not light behind the dots: {light_surface:?}"
        );
    }

    #[test]
    fn red_amber_shell_is_visibly_distinct_from_cyan() {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, PulseMatrixPalette::RedAmber, 1.0);
        let red_amber_pixels = frame
            .as_bytes()
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[0] > pixel[1].saturating_add(35)
                    && pixel[1] > pixel[2].saturating_add(8)
                    && pixel[3] > 80
            })
            .count();

        assert!(
            red_amber_pixels >= 250,
            "red-amber shell disappeared: {red_amber_pixels}"
        );
    }

    #[test]
    fn frame_has_transparent_corners_tight_teal_rings_and_a_bright_dot_matrix() {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, PulseMatrixPalette::Cyan, 1.0);
        assert_eq!(frame.width(), PULSE_MATRIX_IMAGE_SIZE);
        assert_eq!(frame.height(), PULSE_MATRIX_IMAGE_SIZE);

        let pixels = frame.as_bytes();
        for (x, y) in [
            (0, 0),
            (PULSE_MATRIX_IMAGE_SIZE - 1, 0),
            (0, PULSE_MATRIX_IMAGE_SIZE - 1),
            (PULSE_MATRIX_IMAGE_SIZE - 1, PULSE_MATRIX_IMAGE_SIZE - 1),
        ] {
            let offset = ((y * PULSE_MATRIX_IMAGE_SIZE + x) * 4 + 3) as usize;
            assert_eq!(pixels[offset], 0, "corner ({x}, {y}) must stay transparent");
        }

        let bright_dots = pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 175 && pixel[1] > 175 && pixel[2] > 175 && pixel[3] > 80)
            .count();
        let teal_energy = pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[1] > pixel[0].saturating_add(18) && pixel[2] > pixel[0])
            .count();
        assert!(
            bright_dots >= 8,
            "the dot matrix disappeared: {bright_dots}"
        );
        assert!(
            teal_energy >= 300,
            "the teal shell disappeared: {teal_energy}"
        );
    }

    #[test]
    fn voice_energy_changes_the_matrix_without_moving_the_orb() {
        let quiet = render_pulse_matrix_buffer(2.0, 0.08, PulseMatrixPalette::Cyan, 1.0);
        let speaking = render_pulse_matrix_buffer(2.0, 0.92, PulseMatrixPalette::Cyan, 1.0);

        let changed = quiet
            .as_bytes()
            .iter()
            .zip(speaking.as_bytes())
            .filter(|(left, right)| left != right)
            .count();
        assert!(
            changed >= 180,
            "voice response is visually inert: {changed}"
        );
    }

    #[test]
    fn main_orb_edge_has_subpixel_coverage_instead_of_stair_steps() {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, PulseMatrixPalette::Cyan, 1.0);
        let pixels = frame.as_bytes();
        let center = PULSE_MATRIX_IMAGE_SIZE as f32 / 2.0;
        let breath = 1.0 + 0.72 * 0.025 + (1.25_f32 * 0.42).sin() * 0.003;
        let radius = 45.5 * breath;
        let mut smooth_edge_pixels = 0;

        for y in 0..PULSE_MATRIX_IMAGE_SIZE {
            for x in 0..PULSE_MATRIX_IMAGE_SIZE {
                let dx = x as f32 + 0.5 - center;
                let dy = y as f32 + 0.5 - center;
                let distance = (dx * dx + dy * dy).sqrt();
                if (distance - radius).abs() > 1.0 {
                    continue;
                }

                let offset = ((y * PULSE_MATRIX_IMAGE_SIZE + x) * 4) as usize;
                let pixel = &pixels[offset..offset + 4];
                if (24..=230).contains(&pixel[3]) && pixel[1] > 60 && pixel[2] > 60 {
                    smooth_edge_pixels += 1;
                }
            }
        }

        assert!(
            smooth_edge_pixels >= 24,
            "orb edge is still quantized to whole pixels: {smooth_edge_pixels} smooth samples"
        );
    }

    #[test]
    fn main_orb_edge_fades_across_multiple_output_pixels() {
        let frame = render_pulse_matrix_buffer(1.25, 0.72, PulseMatrixPalette::Cyan, 1.0);
        let pixels = frame.as_bytes();
        let center = PULSE_MATRIX_IMAGE_SIZE as f32 / 2.0;
        let breath = 1.0 + 0.72 * 0.025 + (1.25_f32 * 0.42).sin() * 0.003;
        let radius = 45.5 * breath;
        let y = PULSE_MATRIX_IMAGE_SIZE / 2;
        let mut partially_covered = 0;

        for x in 0..PULSE_MATRIX_IMAGE_SIZE {
            let distance = x as f32 + 0.5 - center;
            if distance < 0.0 || (distance - radius).abs() > 1.6 {
                continue;
            }
            let offset = ((y * PULSE_MATRIX_IMAGE_SIZE + x) * 4) as usize;
            if (1..=254).contains(&pixels[offset + 3]) {
                partially_covered += 1;
            }
        }

        assert!(
            partially_covered >= 3,
            "main orb edge changes too abruptly: only {partially_covered} partially covered pixels"
        );
    }
}
