use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub const FLOW_SPHERE_IMAGE_SIZE: u32 = 116;
const SPECTRUM_BANDS: usize = 32;

#[derive(Clone)]
struct RenderRequest {
    phase: f32,
    energy: f32,
    impulse: f32,
    spectrum: [f32; SPECTRUM_BANDS],
    opacity: f32,
}

struct RenderedFrame {
    pixels: Vec<u8>,
}

pub struct FlowSphereRenderer {
    requests: Option<SyncSender<RenderRequest>>,
    frames: Receiver<RenderedFrame>,
    thread: Option<JoinHandle<()>>,
}

impl FlowSphereRenderer {
    pub fn start() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel::<RenderRequest>(1);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<RenderedFrame>(1);
        let thread = thread::Builder::new()
            .name("flow-sphere-renderer".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let frame = render_flow_sphere_buffer(
                        request.phase,
                        request.energy,
                        request.impulse,
                        &request.spectrum,
                        request.opacity,
                    );
                    let rendered = RenderedFrame {
                        pixels: frame.as_bytes().to_vec(),
                    };
                    match frame_tx.try_send(rendered) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            // Never accumulate animation latency.
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
            })
            .expect("flow sphere renderer thread");
        Self {
            requests: Some(request_tx),
            frames: frame_rx,
            thread: Some(thread),
        }
    }

    pub fn render_latest(
        &self,
        phase: f32,
        energy: f32,
        impulse: f32,
        spectrum: &[f32],
        opacity: f32,
    ) -> Option<Image> {
        let mut bands = [0.0; SPECTRUM_BANDS];
        for (target, source) in bands.iter_mut().zip(spectrum.iter().copied()) {
            *target = source;
        }
        if let Some(requests) = &self.requests {
            match requests.try_send(RenderRequest {
                phase,
                energy,
                impulse,
                spectrum: bands,
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
                FLOW_SPHERE_IMAGE_SIZE,
                FLOW_SPHERE_IMAGE_SIZE,
            );
            buffer.make_mut_bytes().copy_from_slice(&frame.pixels);
            Image::from_rgba8_premultiplied(buffer)
        })
    }
}

impl Drop for FlowSphereRenderer {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
struct SurfacePoint {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy)]
struct DeformedPoint {
    position: SurfacePoint,
    voice_response: f32,
}

#[derive(Clone, Copy)]
struct AudioShape {
    envelope: f32,
    impulse: f32,
    low: f32,
    mid: f32,
    high: f32,
}

impl AudioShape {
    fn from_signal(energy: f32, impulse: f32, spectrum: &[f32]) -> Self {
        let envelope = smoothstep(0.14, 0.62, energy.clamp(0.0, 1.0));
        let relative_scale = energy.max(0.08);
        Self {
            envelope,
            impulse: impulse.clamp(0.0, 1.0),
            low: spectrum_range_mean(spectrum, 0, 10) / relative_scale,
            mid: spectrum_range_mean(spectrum, 8, 23) / relative_scale,
            high: spectrum_range_mean(spectrum, 20, spectrum.len()) / relative_scale,
        }
        .clamped()
    }

    fn clamped(self) -> Self {
        Self {
            envelope: self.envelope.clamp(0.0, 1.0),
            impulse: self.impulse.clamp(0.0, 1.0),
            low: self.low.clamp(0.0, 1.0),
            mid: self.mid.clamp(0.0, 1.0),
            high: self.high.clamp(0.0, 1.0),
        }
    }
}

fn render_flow_sphere_buffer(
    phase: f32,
    energy: f32,
    impulse: f32,
    spectrum: &[f32],
    opacity: f32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buffer =
        SharedPixelBuffer::<Rgba8Pixel>::new(FLOW_SPHERE_IMAGE_SIZE, FLOW_SPHERE_IMAGE_SIZE);
    buffer.make_mut_bytes().fill(0);

    let fade = opacity.clamp(0.0, 1.0);
    let center = FLOW_SPHERE_IMAGE_SIZE as f32 / 2.0;
    let radius = 46.0;
    let time = phase * 0.055;
    let audio = AudioShape::from_signal(energy, impulse, spectrum);
    let width = buffer.width();
    let height = buffer.height();

    // One closed point-cloud surface produces both the interior and the rim.
    // Near the camera tangent, points from the front and back accumulate into
    // a thick silhouette; as the sphere turns, those same points move inward.
    let mut surface = RasterSurface {
        pixels: buffer.make_mut_bytes(),
        width,
        height,
    };
    for latitude_index in 0..35 {
        let latitude = -1.48 + latitude_index as f32 / 34.0 * 2.96;
        for longitude_index in 0..72 {
            let longitude =
                -std::f32::consts::PI + longitude_index as f32 / 72.0 * std::f32::consts::TAU;
            let deformed = deform_sphere(latitude, longitude, time, audio, spectrum);
            let point = deformed.position;
            let (x, y) = project(point, center, radius);
            let distance = ((x - center).powi(2) + (y - center).powi(2)).sqrt();
            if distance > radius * 1.055 {
                continue;
            }

            let vertical_mix = ((center - y) / (radius * 2.0) + 0.5).clamp(0.0, 1.0);
            let depth = ((point.z + 1.0) * 0.5).clamp(0.0, 1.0);
            let silhouette = (-point.z.abs() * 8.4).exp();
            let flow = (((longitude * 1.13 + latitude * 1.86 + time * 1.17).sin()
                + (longitude * 2.41 - latitude * 0.73 - time * 0.66).cos() * 0.42)
                * 0.5
                + 0.5)
                .clamp(0.0, 1.0);
            let surface_alpha = (0.11 + depth * 0.46) * (0.34 + flow * 0.66);
            let voice_presence = deformed.voice_response.clamp(0.0, 1.0).powf(0.68);
            let alpha = fade
                * (surface_alpha + voice_presence * 0.105)
                    .max(silhouette * (0.62 + flow * 0.32 + voice_presence * 0.22));
            let color = (
                (60.0 + vertical_mix * 195.0) as u8,
                255,
                (66.0 - vertical_mix * 36.0) as u8,
            );
            surface.draw_glowing_point(
                x,
                y,
                0.27 + depth * 0.09
                    + silhouette * (0.08 + voice_presence * 0.065)
                    + voice_presence * 0.04,
                color,
                alpha,
            );
        }
    }
    buffer
}

fn deform_sphere(
    latitude: f32,
    longitude: f32,
    time: f32,
    audio: AudioShape,
    spectrum: &[f32],
) -> DeformedPoint {
    let spectrum_position =
        (longitude / std::f32::consts::TAU + 0.5 + latitude.sin() * 0.075 + time * 0.018)
            .rem_euclid(1.0);
    let band = smoothed_spectrum_band(spectrum, spectrum_position);
    let band_shape = (band / audio.envelope.max(0.12)).clamp(0.0, 1.0);
    let voice_response = (audio.envelope * (0.38 + band_shape * 0.62)
        + audio.impulse * (0.24 + band_shape * 0.76))
        .clamp(0.0, 1.0);
    let broad_voice =
        audio.envelope * (audio.low * 0.27 + audio.mid * 0.48 + audio.high * 0.25).clamp(0.0, 1.0);
    let onset_lobe = ((((longitude * 1.37 + latitude * 2.16 - time * 1.8).sin() + 1.0) * 0.5)
        .powi(4)
        * audio.impulse
        * (0.45 + band_shape * 0.55))
        .clamp(0.0, 1.0);

    let warped_latitude = (latitude
        + (longitude * 1.52 - time * 0.91).sin() * (0.08 + voice_response * 0.14)
        + (longitude * 3.1 + time * 0.47).cos() * voice_response * 0.052
        + (longitude * 0.83 - time * 0.36).sin() * broad_voice * 0.045)
        .clamp(-1.54, 1.54);
    let warped_longitude = longitude
        + (latitude * 2.08 + time * 0.73).sin() * (0.22 + voice_response * 0.24)
        + (latitude * 4.2 - time * 1.08).cos() * voice_response * 0.075;
    let latitude_cos = warped_latitude.cos();
    let mut point = SurfacePoint {
        x: latitude_cos * warped_longitude.cos(),
        y: warped_latitude.sin(),
        z: latitude_cos * warped_longitude.sin(),
    };

    let fold = (longitude * 2.34 + latitude * 2.07 + time * 1.31).sin();
    let ripple = (longitude * 5.7 - latitude * 3.34 - time * 0.92).sin();
    let low_breath = (longitude * 0.84 - time * 0.58).sin() * audio.low;
    let mid_fold = (longitude * 2.61 + latitude * 1.47 + time * 0.82).sin() * audio.mid;
    let high_ripple = (longitude * 7.1 - latitude * 4.8 - time * 1.26).sin() * audio.high;
    let radial = 0.94
        + audio.envelope * 0.018
        + onset_lobe * 0.085
        + fold * (0.036 + voice_response * 0.105)
        + ripple * (0.012 + voice_response * 0.036)
        + audio.envelope * (low_breath * 0.036 + mid_fold * 0.048 + high_ripple * 0.019);
    point.x *= radial;
    point.y *= radial;
    point.z *= radial;
    point.z += fold * (0.07 + voice_response * 0.15) + onset_lobe * 0.055;
    point.x += (latitude * 2.4 - time * 1.2).cos() * voice_response * 0.072;
    point.y += (longitude * 1.7 + time * 1.8).sin() * voice_response * 0.096;

    DeformedPoint {
        position: rotate_z(
            rotate_x(
                rotate_y(point, time * 0.94 + 0.30),
                0.40 + (time * 0.52).sin() * 0.46,
            ),
            (time * 0.39).sin() * 0.96,
        ),
        voice_response,
    }
}

fn spectrum_range_mean(spectrum: &[f32], start: usize, end: usize) -> f32 {
    let end = end.min(spectrum.len());
    if start >= end {
        return 0.0;
    }
    spectrum[start..end].iter().sum::<f32>() / (end - start) as f32
}

fn smoothed_spectrum_band(spectrum: &[f32], position: f32) -> f32 {
    if spectrum.is_empty() {
        return 0.0;
    }
    let band_position = position.rem_euclid(1.0) * spectrum.len() as f32;
    let center = band_position.floor() as isize;
    let fraction = band_position.fract();
    let sample = |offset: isize| {
        let index = (center + offset).rem_euclid(spectrum.len() as isize) as usize;
        spectrum[index].clamp(0.0, 1.0)
    };
    let left = sample(-1) * 0.18 + sample(0) * 0.82;
    let right = sample(0) * 0.82 + sample(1) * 0.18;
    left + (right - left) * fraction
}

fn smoothstep(edge_0: f32, edge_1: f32, value: f32) -> f32 {
    let normalized = ((value - edge_0) / (edge_1 - edge_0)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

struct RasterSurface<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
}

impl RasterSurface<'_> {
    fn draw_glowing_point(&mut self, x: f32, y: f32, radius: f32, color: (u8, u8, u8), alpha: f32) {
        let center_x = x.round() as i32;
        let center_y = y.round() as i32;
        let sigma = radius.max(0.27);
        for pixel_y in (center_y - 1)..=(center_y + 1) {
            if pixel_y < 0 || pixel_y >= self.height as i32 {
                continue;
            }
            for pixel_x in (center_x - 1)..=(center_x + 1) {
                if pixel_x < 0 || pixel_x >= self.width as i32 {
                    continue;
                }
                let dx = pixel_x as f32 + 0.5 - x;
                let dy = pixel_y as f32 + 0.5 - y;
                let coverage = (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp();
                if coverage < 0.035 {
                    continue;
                }
                blend_premultiplied(
                    self.pixels,
                    (pixel_y as u32 * self.width + pixel_x as u32) as usize * 4,
                    color,
                    (alpha * coverage).clamp(0.0, 0.94),
                );
            }
        }
    }
}

fn blend_premultiplied(pixels: &mut [u8], offset: usize, color: (u8, u8, u8), source_alpha: f32) {
    let inverse = 1.0 - source_alpha;
    pixels[offset] = (color.0 as f32 * source_alpha + pixels[offset] as f32 * inverse) as u8;
    pixels[offset + 1] =
        (color.1 as f32 * source_alpha + pixels[offset + 1] as f32 * inverse) as u8;
    pixels[offset + 2] =
        (color.2 as f32 * source_alpha + pixels[offset + 2] as f32 * inverse) as u8;
    pixels[offset + 3] =
        (255.0 * source_alpha + pixels[offset + 3] as f32 * inverse).min(255.0) as u8;
}

fn project(point: SurfacePoint, center: f32, radius: f32) -> (f32, f32) {
    let perspective = 1.0 / (1.12 - point.z * 0.11);
    (
        center + point.x * radius * perspective,
        center + point.y * radius * perspective,
    )
}

fn rotate_x(point: SurfacePoint, angle: f32) -> SurfacePoint {
    let (sin, cos) = angle.sin_cos();
    SurfacePoint {
        x: point.x,
        y: point.y * cos - point.z * sin,
        z: point.y * sin + point.z * cos,
    }
}

fn rotate_y(point: SurfacePoint, angle: f32) -> SurfacePoint {
    let (sin, cos) = angle.sin_cos();
    SurfacePoint {
        x: point.x * cos + point.z * sin,
        y: point.y,
        z: -point.x * sin + point.z * cos,
    }
}

fn rotate_z(point: SurfacePoint, angle: f32) -> SurfacePoint {
    let (sin, cos) = angle.sin_cos();
    SurfacePoint {
        x: point.x * cos - point.y * sin,
        y: point.x * sin + point.y * cos,
        z: point.z,
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioShape, FLOW_SPHERE_IMAGE_SIZE, render_flow_sphere_buffer};

    #[test]
    fn sphere_frame_keeps_corners_transparent_and_draws_one_point_cloud() {
        let spectrum = (0..32)
            .map(|index| (index as f32 * 0.37).sin().abs())
            .collect::<Vec<_>>();
        let mut frame = render_flow_sphere_buffer(1.4, 0.72, 0.4, &spectrum, 1.0);
        let bytes = frame.make_mut_bytes();
        let corner_alpha = bytes[3];
        let visible_pixels = bytes.chunks_exact(4).filter(|pixel| pixel[3] > 10).count();
        assert_eq!(corner_alpha, 0);
        assert!(visible_pixels > FLOW_SPHERE_IMAGE_SIZE as usize * 4);
    }

    #[test]
    fn audio_shape_gates_room_noise_and_preserves_voice_energy() {
        let quiet = AudioShape::from_signal(0.08, 0.0, &[0.03; 32]);
        let voice = AudioShape::from_signal(0.54, 0.72, &[0.38; 32]);

        assert_eq!(quiet.envelope, 0.0);
        assert!(voice.envelope > 0.9);
        assert_eq!(voice.impulse, 0.72);
        assert!(voice.low > 0.6);
        assert!(voice.mid > 0.6);
        assert!(voice.high > 0.6);
    }

    #[test]
    fn active_voice_materially_changes_the_same_point_cloud() {
        let silent = render_flow_sphere_buffer(2.1, 0.08, 0.0, &[0.0; 32], 1.0);
        let spectrum = std::array::from_fn::<_, 32, _>(|index| {
            let position = index as f32 / 31.0;
            (0.22 + (position * std::f32::consts::TAU * 2.5).sin().abs() * 0.58).clamp(0.0, 1.0)
        });
        let active = render_flow_sphere_buffer(2.1, 0.68, 0.82, &spectrum, 1.0);
        let difference = silent
            .as_bytes()
            .iter()
            .zip(active.as_bytes())
            .map(|(left, right)| left.abs_diff(*right) as usize)
            .sum::<usize>();

        assert!(difference > 50_000);
    }
}
