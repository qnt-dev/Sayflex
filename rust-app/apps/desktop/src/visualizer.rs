//! The recording indicator and the visualizers it can draw.
//!
//! Everything here is presentation of one signal: a peak level and a spectrum
//! arriving about thirty-one times a second. It is separated from the session
//! logic that produces that signal because the two change for entirely
//! different reasons — a new visualizer is a drawing decision, not a dictation
//! one — and because this is the part that can be exercised without a
//! microphone.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use rust_protocol::AUDIO_SPECTRUM_BANDS;
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use pet_orb::OrbActor;
use pet_particles::Vec2;
use pet_render::{Canvas, Sprite, SpriteSource};

use crate::{
    RecordingIndicator, SettingsWindow, SlateLine, flow_sphere, pulse_matrix::PulseMatrixPalette,
};

pub(crate) const INDICATOR_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(crate) const INDICATOR_BAR_PROFILE: [f32; 10] =
    [0.25, 0.4, 0.62, 0.82, 1.0, 1.0, 0.82, 0.62, 0.4, 0.25];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VisualizerStyle {
    VoiceOrb,
    SymmetricSpectrum,
    FluidRibbon,
    #[default]
    BroadcastMic,
    CircularSpectrum,
    #[serde(alias = "flow_sphere_preview")]
    FlowSphere,
    #[serde(alias = "writing_slate_preview")]
    WritingSlate,
    CompanionOrb,
    Off,
    PulseMatrix,
}

/// Native composition is an HWND-creation-time contract, not a property to
/// toggle while a recording indicator is already alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordingSurface {
    Panel,
    Transparent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CircularSpectrumPalette {
    #[default]
    Sunset,
    Arctic,
    Monochrome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BroadcastMicPalette {
    #[default]
    White,
    Blue,
    Red,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WritingSlateSurface {
    #[default]
    DarkPanel,
    MatteGlass,
    BlueGlass,
}

impl CircularSpectrumPalette {
    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Sunset),
            1 => Some(Self::Arctic),
            2 => Some(Self::Monochrome),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::Sunset => 0,
            Self::Arctic => 1,
            Self::Monochrome => 2,
        }
    }
}

impl BroadcastMicPalette {
    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::White),
            1 => Some(Self::Blue),
            2 => Some(Self::Red),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::White => 0,
            Self::Blue => 1,
            Self::Red => 2,
        }
    }
}

impl WritingSlateSurface {
    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::DarkPanel),
            1 => Some(Self::MatteGlass),
            2 => Some(Self::BlueGlass),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::DarkPanel => 0,
            Self::MatteGlass => 1,
            Self::BlueGlass => 2,
        }
    }
}

impl VisualizerStyle {
    pub(crate) const fn released_or_default(self) -> Self {
        match self {
            Self::VoiceOrb | Self::SymmetricSpectrum | Self::FluidRibbon => Self::BroadcastMic,
            released => released,
        }
    }

    pub(crate) const fn from_selectable_index(index: i32) -> Option<Self> {
        match Self::from_index(index) {
            Some(Self::VoiceOrb | Self::SymmetricSpectrum | Self::FluidRibbon) => None,
            style => style,
        }
    }

    pub(crate) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::VoiceOrb),
            1 => Some(Self::SymmetricSpectrum),
            2 => Some(Self::FluidRibbon),
            3 => Some(Self::BroadcastMic),
            4 => Some(Self::CircularSpectrum),
            5 => Some(Self::FlowSphere),
            6 => Some(Self::WritingSlate),
            7 => Some(Self::CompanionOrb),
            8 => Some(Self::Off),
            9 => Some(Self::PulseMatrix),
            _ => None,
        }
    }

    pub(crate) const fn index(self) -> i32 {
        match self {
            Self::VoiceOrb => 0,
            Self::SymmetricSpectrum => 1,
            Self::FluidRibbon => 2,
            Self::BroadcastMic => 3,
            Self::CircularSpectrum => 4,
            Self::FlowSphere => 5,
            Self::WritingSlate => 6,
            Self::CompanionOrb => 7,
            Self::Off => 8,
            Self::PulseMatrix => 9,
        }
    }

    pub(crate) const fn recording_surface(self) -> Option<RecordingSurface> {
        match self {
            Self::BroadcastMic
            | Self::CircularSpectrum
            | Self::FlowSphere
            | Self::WritingSlate
            | Self::CompanionOrb
            | Self::PulseMatrix => Some(RecordingSurface::Transparent),
            Self::VoiceOrb | Self::SymmetricSpectrum | Self::FluidRibbon => {
                Some(RecordingSurface::Panel)
            }
            Self::Off => None,
        }
    }

    pub(crate) const fn uses_transparent_window(self) -> bool {
        matches!(
            self.recording_surface(),
            Some(RecordingSurface::Transparent)
        )
    }

    pub(crate) const fn uses_external_renderer(self) -> bool {
        matches!(self, Self::CompanionOrb)
    }

    pub(crate) const fn has_live_settings_preview(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum IndicatorMode {
    #[default]
    Hidden,
    Listening,
    Processing,
    Success,
}

pub(crate) struct IndicatorAnimation {
    pub(crate) bars: Rc<VecModel<f32>>,
    pub(crate) spectrum: Rc<VecModel<f32>>,
    pub(crate) slate_lines: Rc<VecModel<SlateLine>>,
    pub(crate) style: VisualizerStyle,
    pub(crate) mode: IndicatorMode,
    pub(crate) target_level: f32,
    pub(crate) display_level: f32,
    pub(crate) target_spectrum: [f32; AUDIO_SPECTRUM_BANDS],
    pub(crate) display_spectrum: [f32; AUDIO_SPECTRUM_BANDS],
    pub(crate) phase: f32,
    pub(crate) mode_progress: f32,
    pub(crate) flow_voice_impulse: f32,
    pub(crate) flow_sphere_palette: flow_sphere::FlowSpherePalette,
    pub(crate) pulse_matrix_palette: PulseMatrixPalette,
    pub(crate) last_frame: Instant,
    pub(crate) flow_sphere_renderer: Option<flow_sphere::FlowSphereRenderer>,
    pub(crate) pulse_matrix_renderer: Option<crate::pulse_matrix::PulseMatrixRenderer>,
    companion_preview_renderer: Option<CompanionPreviewRenderer>,
    pub(crate) writing_slate: WritingSlateAnimation,
}

const COMPANION_PREVIEW_WIDTH: u32 = 290;
const COMPANION_PREVIEW_HEIGHT: u32 = 168;
const COMPANION_PREVIEW_SCALE: f32 = 0.115;
const COMPANION_PREVIEW_GROW_START: f32 = 0.08;
const COMPANION_PREVIEW_GROW_FRAMES: usize = 18;
const COMPANION_PREVIEW_MOTION_RADIUS: f32 = 15.0;

/// The settings page embeds the same autonomous actor and particle renderer as
/// the companion process. Only the canvas size and scale differ: this is a
/// preview, not a second desktop overlay.
struct CompanionPreviewRenderer {
    actor: OrbActor,
    growth_frames: Vec<Sprite>,
    canvas: Canvas,
    anchor: Vec2,
}

impl CompanionPreviewRenderer {
    fn new() -> Result<Self, String> {
        let source =
            SpriteSource::load_bytes(include_bytes!("../../../../desktop-pet/assets/orb.png"), 10)
                .map_err(|error| format!("could not decode the companion orb: {error}"))?;
        let growth_frames = (0..COMPANION_PREVIEW_GROW_FRAMES)
            .map(|index| {
                let progress = index as f32 / (COMPANION_PREVIEW_GROW_FRAMES - 1) as f32;
                let scale = COMPANION_PREVIEW_SCALE
                    * (COMPANION_PREVIEW_GROW_START
                        + (1.0 - COMPANION_PREVIEW_GROW_START) * progress);
                source.scaled(scale)
            })
            .collect::<Vec<_>>();
        let sprite = growth_frames
            .last()
            .expect("the companion preview has at least one growth frame");
        let anchor = Vec2::new(
            COMPANION_PREVIEW_WIDTH as f32 / 2.0,
            COMPANION_PREVIEW_HEIGHT as f32 / 2.0,
        );
        let rest = anchor + Vec2::new(COMPANION_PREVIEW_MOTION_RADIUS, 0.0);
        let actor = OrbActor::new(
            rest,
            COMPANION_PREVIEW_MOTION_RADIUS,
            sprite.width() as f32 / 2.0,
            COMPANION_PREVIEW_SCALE,
        );
        Ok(Self {
            actor,
            growth_frames,
            canvas: Canvas::new(COMPANION_PREVIEW_WIDTH, COMPANION_PREVIEW_HEIGHT),
            anchor,
        })
    }

    fn render(&mut self, level: f32) -> Image {
        self.actor
            .update(1.0 / 60.0, self.anchor, 1.0 + level.clamp(0.0, 1.0) * 0.5);
        self.canvas.clear();
        self.actor.draw_behind(&mut self.canvas);
        self.actor
            .draw_sprite(&mut self.canvas, &self.growth_frames);
        self.actor.draw_front(&mut self.canvas);
        self.actor.draw_flash(&mut self.canvas);

        let mut pixels =
            SharedPixelBuffer::<Rgba8Pixel>::new(COMPANION_PREVIEW_WIDTH, COMPANION_PREVIEW_HEIGHT);
        for (source, target) in self
            .canvas
            .pixels()
            .chunks_exact(4)
            .zip(pixels.make_mut_bytes().chunks_exact_mut(4))
        {
            // pet_render is premultiplied BGRA for UpdateLayeredWindow; Slint
            // accepts the same premultiplication with RGBA channel order.
            target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        Image::from_rgba8_premultiplied(pixels)
    }
}

#[derive(Clone)]
pub(crate) struct WritingSlateLine {
    pub(crate) x: f32,
    pub(crate) width: f32,
    pub(crate) target_width: f32,
    pub(crate) y: f32,
    pub(crate) target_y: f32,
    pub(crate) opacity: f32,
    pub(crate) glow: f32,
    pub(crate) color_index: i32,
    pub(crate) active: bool,
}

#[derive(Default)]
pub(crate) struct WritingSlateAnimation {
    pub(crate) lines: Vec<WritingSlateLine>,
    pub(crate) voice_active: bool,
    pub(crate) silence_seconds: f32,
    pub(crate) next_line_delay: f32,
    pub(crate) sequence_index: usize,
    pub(crate) cursor_x: f32,
    pub(crate) cursor_y: f32,
}

impl WritingSlateAnimation {
    const CONTENT_WIDTH: f32 = 98.0;
    const SCROLL_Y: f32 = 88.0;
    const ROW_STEP: f32 = 8.5;
    const SEGMENT_GAP: f32 = 4.0;
    const COLOR_SEQUENCE: [i32; 12] = [0, 1, 2, 1, 0, 2, 0, 2, 1, 2, 0, 1];
    const WIDTH_SEQUENCE: [f32; 12] = [
        18.0, 27.0, 12.0, 22.0, 15.0, 30.0, 20.0, 10.0, 25.0, 16.0, 29.0, 13.0,
    ];
    const CADENCE_SEQUENCE: [f32; 12] = [
        0.072, 0.060, 0.086, 0.104, 0.066, 0.080, 0.058, 0.096, 0.070, 0.088, 0.062, 0.100,
    ];

    pub(crate) fn reset(&mut self) {
        self.lines.clear();
        self.voice_active = false;
        self.silence_seconds = 0.0;
        self.next_line_delay = 0.0;
        self.sequence_index = 0;
        self.cursor_x = 0.0;
        self.cursor_y = 0.0;
    }

    pub(crate) fn stop_input(&mut self) {
        self.voice_active = false;
        self.silence_seconds = 0.0;
        self.commit_active();
    }

    pub(crate) fn advance(
        &mut self,
        frame_scale: f32,
        mode: IndicatorMode,
        energy: f32,
    ) -> Vec<SlateLine> {
        let elapsed_seconds = 0.016 * frame_scale;
        self.next_line_delay = (self.next_line_delay - elapsed_seconds).max(0.0);

        for line in &mut self.lines {
            let y_response = 1.0 - 0.72_f32.powf(frame_scale);
            line.y += (line.target_y - line.y) * y_response;
            line.glow *= 0.90_f32.powf(frame_scale);
            let target_opacity = if line.target_y < 0.0 {
                0.0
            } else if line.active {
                1.0
            } else {
                0.78
            };
            let opacity_response = 1.0 - 0.68_f32.powf(frame_scale);
            line.opacity += (target_opacity - line.opacity) * opacity_response;
        }

        if mode == IndicatorMode::Listening {
            let threshold = if self.voice_active { 0.22 } else { 0.34 };
            let speaking = energy >= threshold;
            if speaking {
                self.voice_active = true;
                self.silence_seconds = 0.0;
                if !self.lines.last().is_some_and(|line| line.active) && self.next_line_delay <= 0.0
                {
                    self.start_line();
                }
                let reached_target =
                    if let Some(line) = self.lines.last_mut().filter(|line| line.active) {
                        let fill_response = 1.0 - (1.0 - (0.22 + energy * 0.10)).powf(frame_scale);
                        line.width += (line.target_width - line.width) * fill_response;
                        line.glow = line.glow.max((0.34 + energy * 0.66).clamp(0.0, 1.0));
                        line.opacity = (line.opacity + 0.20 * frame_scale).min(1.0);
                        line.width >= line.target_width - 0.45
                    } else {
                        false
                    };
                if reached_target {
                    self.commit_active();
                    let cadence =
                        self.sequence_index.saturating_sub(1) % Self::CADENCE_SEQUENCE.len();
                    self.next_line_delay = Self::CADENCE_SEQUENCE[cadence];
                }
            } else {
                self.voice_active = false;
                self.silence_seconds += elapsed_seconds;
                if self.silence_seconds >= 0.16 {
                    self.commit_active();
                }
            }
        } else {
            self.stop_input();
        }

        self.lines
            .retain(|line| line.target_y > -Self::ROW_STEP && line.opacity > 0.02);
        self.lines
            .iter()
            .map(|line| SlateLine {
                x: line.x,
                width: line.width,
                y: line.y,
                opacity: line.opacity,
                glow: line.glow,
                color_index: line.color_index,
            })
            .collect()
    }

    pub(crate) fn start_line(&mut self) {
        let sequence = self.sequence_index % Self::COLOR_SEQUENCE.len();
        let target_width = Self::WIDTH_SEQUENCE[sequence];
        if self.cursor_x > 0.0 && self.cursor_x + target_width > Self::CONTENT_WIDTH {
            self.cursor_x = 0.0;
            self.cursor_y += Self::ROW_STEP;
        }
        if self.cursor_y >= Self::SCROLL_Y {
            for line in &mut self.lines {
                line.target_y -= Self::ROW_STEP;
            }
            self.cursor_y -= Self::ROW_STEP;
        }
        self.lines.retain(|line| line.target_y > -Self::ROW_STEP);

        let x = self.cursor_x;
        let target_y = self.cursor_y;
        self.sequence_index = self.sequence_index.wrapping_add(1);
        self.lines.push(WritingSlateLine {
            x,
            width: 3.0,
            target_width,
            y: target_y + 2.5,
            target_y,
            opacity: 0.34,
            glow: 1.0,
            color_index: Self::COLOR_SEQUENCE[sequence],
            active: true,
        });
        self.cursor_x += target_width + Self::SEGMENT_GAP;
    }

    pub(crate) fn commit_active(&mut self) {
        if let Some(line) = self.lines.last_mut().filter(|line| line.active) {
            line.active = false;
            line.width = line.width.max(6.0).min(line.target_width);
            line.opacity = line.opacity.max(0.78);
            line.glow = line.glow.max(0.18);
        }
    }
}

pub(crate) struct IndicatorFrame {
    pub(crate) heights: Vec<f32>,
    pub(crate) spectrum_heights: Vec<f32>,
    pub(crate) signal_opacity: f32,
    pub(crate) disc_opacity: f32,
    pub(crate) orb_size: f32,
    pub(crate) orb_glow: f32,
    pub(crate) ribbon_y1: f32,
    pub(crate) ribbon_y2: f32,
    pub(crate) ribbon_y3: f32,
    pub(crate) ribbon_y4: f32,
    pub(crate) mic_edge_height: f32,
    pub(crate) mic_outer_height: f32,
    pub(crate) mic_middle_height: f32,
    pub(crate) mic_inner_height: f32,
    pub(crate) mic_near_height: f32,
    pub(crate) visual_scale: f32,
    pub(crate) visual_opacity: f32,
}

impl IndicatorAnimation {
    pub(crate) fn new(
        bars: Rc<VecModel<f32>>,
        spectrum: Rc<VecModel<f32>>,
        slate_lines: Rc<VecModel<SlateLine>>,
    ) -> Self {
        Self {
            bars,
            spectrum,
            slate_lines,
            style: VisualizerStyle::default(),
            mode: IndicatorMode::Hidden,
            target_level: 0.08,
            display_level: 0.08,
            target_spectrum: [0.0; AUDIO_SPECTRUM_BANDS],
            display_spectrum: [0.0; AUDIO_SPECTRUM_BANDS],
            phase: 0.0,
            mode_progress: 0.0,
            flow_voice_impulse: 0.0,
            flow_sphere_palette: flow_sphere::FlowSpherePalette::default(),
            pulse_matrix_palette: PulseMatrixPalette::default(),
            last_frame: Instant::now(),
            flow_sphere_renderer: None,
            pulse_matrix_renderer: None,
            companion_preview_renderer: None,
            writing_slate: WritingSlateAnimation::default(),
        }
    }

    pub(crate) fn set_style(&mut self, style: VisualizerStyle) {
        let style = style.released_or_default();
        self.style = style;
        if style == VisualizerStyle::FlowSphere && self.flow_sphere_renderer.is_none() {
            self.flow_sphere_renderer = Some(flow_sphere::FlowSphereRenderer::start());
        } else if style != VisualizerStyle::FlowSphere {
            self.flow_sphere_renderer = None;
        }
        if style == VisualizerStyle::PulseMatrix && self.pulse_matrix_renderer.is_none() {
            self.pulse_matrix_renderer = Some(crate::pulse_matrix::PulseMatrixRenderer::start());
        } else if style != VisualizerStyle::PulseMatrix {
            self.pulse_matrix_renderer = None;
        }
        if style == VisualizerStyle::CompanionOrb && self.companion_preview_renderer.is_none() {
            self.companion_preview_renderer = CompanionPreviewRenderer::new().ok();
        } else if style != VisualizerStyle::CompanionOrb {
            self.companion_preview_renderer = None;
        }
    }

    pub(crate) fn set_flow_sphere_palette(&mut self, palette: flow_sphere::FlowSpherePalette) {
        self.flow_sphere_palette = palette;
    }

    pub(crate) fn set_pulse_matrix_palette(&mut self, palette: PulseMatrixPalette) {
        self.pulse_matrix_palette = palette;
    }

    pub(crate) fn begin_listening(&mut self) {
        self.mode = IndicatorMode::Listening;
        self.target_level = 0.08;
        self.display_level = self.display_level.max(0.08);
        self.target_spectrum.fill(0.0);
        self.display_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        if self.style == VisualizerStyle::WritingSlate {
            self.writing_slate.reset();
            self.slate_lines.set_vec(Vec::new());
        }
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    pub(crate) fn set_microphone_signal(&mut self, microphone_level: f32, spectrum: &[f32]) {
        if self.mode == IndicatorMode::Listening {
            let next_level = (microphone_level.clamp(0.0, 1.0).sqrt() * 2.35).clamp(0.0, 1.0);
            if self.style == VisualizerStyle::FlowSphere {
                let onset = (next_level - self.target_level).max(0.0);
                self.flow_voice_impulse =
                    self.flow_voice_impulse.max((onset * 1.65).clamp(0.0, 1.0));
            }
            self.target_level = next_level;
            for (index, target) in self.target_spectrum.iter_mut().enumerate() {
                *target = spectrum.get(index).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }
    }

    pub(crate) fn begin_processing(&mut self) {
        self.mode = IndicatorMode::Processing;
        self.target_level = 0.18;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        if self.style == VisualizerStyle::WritingSlate {
            self.writing_slate.stop_input();
        }
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    pub(crate) fn begin_success(&mut self) {
        self.mode = IndicatorMode::Success;
        self.target_level = 0.0;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
        self.last_frame = Instant::now() - INDICATOR_FRAME_INTERVAL;
    }

    pub(crate) fn stop(&mut self) {
        self.mode = IndicatorMode::Hidden;
        self.target_level = 0.08;
        self.target_spectrum.fill(0.0);
        self.mode_progress = 0.0;
        self.flow_voice_impulse = 0.0;
    }

    pub(crate) fn tick(&mut self, indicators: &[RecordingIndicator]) {
        let Some(frame) = self.next_frame() else {
            return;
        };

        let flow_frame = if self.style == VisualizerStyle::FlowSphere {
            self.flow_sphere_renderer.as_ref().and_then(|renderer| {
                renderer.render_latest(
                    self.phase,
                    self.display_level,
                    self.flow_voice_impulse,
                    self.flow_sphere_palette,
                    &self.display_spectrum,
                    frame.visual_opacity,
                )
            })
        } else {
            None
        };
        let pulse_matrix_frame = if self.style == VisualizerStyle::PulseMatrix {
            self.pulse_matrix_renderer.as_ref().and_then(|renderer| {
                renderer.render_latest(
                    self.phase,
                    self.display_level,
                    self.pulse_matrix_palette,
                    frame.visual_opacity,
                )
            })
        } else {
            None
        };

        for indicator in indicators {
            indicator.set_visualizer_style(self.style.index());
            indicator.set_processing(self.mode == IndicatorMode::Processing);
            indicator.set_signal_opacity(frame.signal_opacity);
            indicator.set_disc_opacity(frame.disc_opacity);
            indicator.set_orb_size(frame.orb_size);
            indicator.set_orb_glow(frame.orb_glow);
            indicator.set_ribbon_y1(frame.ribbon_y1);
            indicator.set_ribbon_y2(frame.ribbon_y2);
            indicator.set_ribbon_y3(frame.ribbon_y3);
            indicator.set_ribbon_y4(frame.ribbon_y4);
            indicator.set_mic_edge_height(frame.mic_edge_height);
            indicator.set_mic_outer_height(frame.mic_outer_height);
            indicator.set_mic_middle_height(frame.mic_middle_height);
            indicator.set_mic_inner_height(frame.mic_inner_height);
            indicator.set_mic_near_height(frame.mic_near_height);
            indicator.set_visual_scale(frame.visual_scale);
            indicator.set_visual_opacity(frame.visual_opacity);
            if let Some(flow_frame) = &flow_frame {
                indicator.set_flow_sphere_frame(flow_frame.clone());
            }
            if let Some(pulse_matrix_frame) = &pulse_matrix_frame {
                indicator.set_pulse_matrix_frame(pulse_matrix_frame.clone());
            }
        }
    }

    fn next_frame(&mut self) -> Option<IndicatorFrame> {
        if self.mode == IndicatorMode::Hidden
            || self.last_frame.elapsed() < INDICATOR_FRAME_INTERVAL
        {
            return None;
        }

        let elapsed = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let frame_scale =
            (elapsed.as_secs_f32() / INDICATOR_FRAME_INTERVAL.as_secs_f32()).clamp(0.5, 2.5);
        let frame = self.advance_frame(frame_scale);
        self.bars.set_vec(frame.heights.clone());
        self.spectrum.set_vec(frame.spectrum_heights.clone());
        if self.style == VisualizerStyle::WritingSlate {
            let slate_energy = self.target_level.max(self.display_level);
            self.slate_lines.set_vec(self.writing_slate.advance(
                frame_scale,
                self.mode,
                slate_energy,
            ));
        }
        Some(frame)
    }

    /// Drives the settings preview through the same animation and render path
    /// as the recording overlay.  The microphone signal is synthetic so the
    /// choices remain comparable without starting a recording.
    pub(crate) fn tick_settings_preview(&mut self, settings: &SettingsWindow) {
        let selected = VisualizerStyle::from_selectable_index(settings.get_selected_visualizer());
        if !settings.window().is_visible()
            || settings.get_active_page() != 5
            || selected.is_none_or(|style| !style.has_live_settings_preview())
        {
            if self.mode != IndicatorMode::Hidden {
                self.stop();
            }
            if matches!(
                self.style,
                VisualizerStyle::FlowSphere | VisualizerStyle::PulseMatrix
            ) {
                self.set_style(VisualizerStyle::Off);
            }
            return;
        }

        let style = selected.expect("preview style was checked above");
        if self.style != style || self.mode == IndicatorMode::Hidden {
            self.set_style(style);
            self.begin_listening();
        }
        self.set_flow_sphere_palette(
            flow_sphere::FlowSpherePalette::from_index(settings.get_selected_flow_sphere_palette())
                .unwrap_or_default(),
        );
        self.set_pulse_matrix_palette(
            PulseMatrixPalette::from_index(settings.get_selected_pulse_matrix_palette())
                .unwrap_or_default(),
        );

        let pulse = ((self.phase * 0.71).sin() * 0.5 + 0.5).powf(1.6);
        let peak = 0.16 + pulse * 0.48;
        let mut spectrum = [0.0; AUDIO_SPECTRUM_BANDS];
        for (index, band) in spectrum.iter_mut().enumerate() {
            let wave = ((self.phase * 0.83 + index as f32 * 0.57).sin() * 0.5 + 0.5).powf(2.1);
            *band = (0.08 + wave * peak).clamp(0.0, 1.0);
        }
        self.set_microphone_signal(peak, &spectrum);

        let Some(frame) = self.next_frame() else {
            return;
        };
        settings.set_visual_preview_processing(false);
        settings.set_visual_preview_signal_opacity(frame.signal_opacity);
        settings.set_visual_preview_disc_opacity(frame.disc_opacity);
        settings.set_visual_preview_orb_size(frame.orb_size);
        settings.set_visual_preview_orb_glow(frame.orb_glow);
        settings.set_visual_preview_ribbon_y1(frame.ribbon_y1);
        settings.set_visual_preview_ribbon_y2(frame.ribbon_y2);
        settings.set_visual_preview_ribbon_y3(frame.ribbon_y3);
        settings.set_visual_preview_ribbon_y4(frame.ribbon_y4);
        settings.set_visual_preview_mic_edge_height(frame.mic_edge_height);
        settings.set_visual_preview_mic_outer_height(frame.mic_outer_height);
        settings.set_visual_preview_mic_middle_height(frame.mic_middle_height);
        settings.set_visual_preview_mic_inner_height(frame.mic_inner_height);
        settings.set_visual_preview_mic_near_height(frame.mic_near_height);

        if style == VisualizerStyle::CompanionOrb
            && let Some(renderer) = self.companion_preview_renderer.as_mut()
        {
            settings.set_visual_preview_companion_frame(renderer.render(self.display_level));
        }

        if style == VisualizerStyle::FlowSphere
            && let Some(renderer) = &self.flow_sphere_renderer
            && let Some(flow_frame) = renderer.render_latest(
                self.phase,
                self.display_level,
                self.flow_voice_impulse,
                self.flow_sphere_palette,
                &self.display_spectrum,
                1.0,
            )
        {
            settings.set_visual_preview_flow_sphere_frame(flow_frame);
        }

        if style == VisualizerStyle::PulseMatrix
            && let Some(renderer) = &self.pulse_matrix_renderer
            && let Some(pulse_matrix_frame) = renderer.render_latest(
                self.phase,
                self.display_level,
                self.pulse_matrix_palette,
                1.0,
            )
        {
            settings.set_visual_preview_pulse_matrix_frame(pulse_matrix_frame);
        }
    }

    pub(crate) fn advance_frame(&mut self, frame_scale: f32) -> IndicatorFrame {
        if self.mode == IndicatorMode::Success {
            self.mode_progress = (self.mode_progress + 0.045 * frame_scale).min(1.0);
        }
        let level_response: f32 = if self.style == VisualizerStyle::FlowSphere {
            if self.target_level > self.display_level {
                0.34
            } else {
                0.085
            }
        } else if self.style == VisualizerStyle::PulseMatrix {
            if self.target_level > self.display_level {
                0.2
            } else {
                0.075
            }
        } else {
            0.18
        };
        let smoothing = 1.0 - (1.0 - level_response).powf(frame_scale);
        self.display_level += (self.target_level - self.display_level) * smoothing;
        for (display, target) in self.display_spectrum.iter_mut().zip(self.target_spectrum) {
            let response = if target > *display {
                if self.style == VisualizerStyle::FlowSphere {
                    0.52_f32
                } else {
                    0.46_f32
                }
            } else if self.style == VisualizerStyle::FlowSphere {
                0.095_f32
            } else {
                0.16_f32
            };
            let spectrum_smoothing = 1.0 - (1.0 - response).powf(frame_scale);
            *display += (target - *display) * spectrum_smoothing;
        }
        let flow_activity = ((self.display_level - 0.14) / 0.48).clamp(0.0, 1.0);
        let phase_step = match self.mode {
            IndicatorMode::Listening if self.style == VisualizerStyle::FlowSphere => {
                0.045 + flow_activity * 0.245 + self.flow_voice_impulse * 0.075
            }
            IndicatorMode::Listening if self.style == VisualizerStyle::PulseMatrix => {
                0.035 + self.display_level * 0.075
            }
            IndicatorMode::Listening => 0.29,
            IndicatorMode::Processing => 0.18,
            IndicatorMode::Success => 0.24,
            IndicatorMode::Hidden => 0.0,
        };
        self.phase += phase_step * frame_scale;
        self.flow_voice_impulse *= 0.78_f32.powf(frame_scale);

        let processing_pulse = ((self.phase * 0.72).sin() + 1.0) / 2.0;
        let energy = match self.mode {
            IndicatorMode::Listening => self.display_level.max(0.07),
            IndicatorMode::Processing => self.target_level + (processing_pulse * 0.08),
            IndicatorMode::Success => 0.0,
            IndicatorMode::Hidden => 0.0,
        };
        let heights: Vec<f32> = INDICATOR_BAR_PROFILE
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                let travelling_wave = ((self.phase + index as f32 * 0.62).sin() + 1.0) / 2.0;
                (3.0 + (profile * 5.0) + (travelling_wave * energy * 12.0)).clamp(3.0, 19.0)
            })
            .collect();
        let spectrum_heights = self
            .display_spectrum
            .iter()
            .enumerate()
            .map(|(index, band)| match self.mode {
                IndicatorMode::Listening => {
                    let voice_activation = ((self.display_level - 0.24) / 0.76).clamp(0.0, 1.0);
                    (2.8 + band * 16.2 + band.sqrt() * voice_activation * 2.5).clamp(2.8, 21.5)
                }
                IndicatorMode::Processing => {
                    let angular_position =
                        index as f32 / AUDIO_SPECTRUM_BANDS as f32 * std::f32::consts::TAU;
                    let travelling = ((angular_position - self.phase * 0.72).cos() + 1.0) * 0.5;
                    2.8 + travelling.powi(5) * 9.4
                }
                IndicatorMode::Success => 2.8,
                IndicatorMode::Hidden => 2.8,
            })
            .collect();
        let mic_edge_height = (3.0
            + energy * (4.0 + (((self.phase * 0.67 + 0.45).sin() + 1.0) / 2.0) * 5.0))
            .clamp(3.0, 12.0);
        let mic_outer_height = (4.0
            + energy * (5.4 + (((self.phase * 0.78).sin() + 1.0) / 2.0) * 7.6))
            .clamp(4.0, 17.0);
        let mic_middle_height = (5.0
            + energy * (6.2 + (((self.phase * 1.06 + 1.4).sin() + 1.0) / 2.0) * 6.8))
            .clamp(5.0, 18.0);
        let mic_inner_height = (3.0
            + energy * (4.8 + (((self.phase * 1.34 + 2.6).sin() + 1.0) / 2.0) * 5.6))
            .clamp(3.0, 14.0);
        let mic_near_height = (4.0
            + energy * (5.0 + (((self.phase * 1.52 + 3.35).sin() + 1.0) / 2.0) * 6.0))
            .clamp(4.0, 15.0);
        IndicatorFrame {
            heights,
            spectrum_heights,
            signal_opacity: (0.5 + energy.min(1.0) * 0.5).clamp(0.5, 1.0),
            disc_opacity: match self.mode {
                IndicatorMode::Processing => 0.72 + processing_pulse * 0.28,
                IndicatorMode::Listening => 1.0,
                IndicatorMode::Success => 1.0 - self.mode_progress,
                IndicatorMode::Hidden => 0.0,
            },
            orb_size: (9.0 + energy * 10.0 + processing_pulse * 1.5).clamp(8.0, 20.0),
            orb_glow: (0.12 + energy * 0.38 + processing_pulse * 0.12).clamp(0.12, 0.62),
            ribbon_y1: 9.5 + (self.phase * 0.93).sin() * energy * 6.5,
            ribbon_y2: 9.5 + (self.phase * 0.93 + 1.35).sin() * energy * 7.4,
            ribbon_y3: 9.5 + (self.phase * 0.93 + 2.7).sin() * energy * 6.8,
            ribbon_y4: 9.5 + (self.phase * 0.93 + 4.05).sin() * energy * 5.8,
            mic_edge_height,
            mic_outer_height,
            mic_middle_height,
            mic_inner_height,
            mic_near_height,
            visual_scale: match self.mode {
                IndicatorMode::Success => {
                    let eased =
                        self.mode_progress * self.mode_progress * (3.0 - 2.0 * self.mode_progress);
                    1.0 - eased * 0.55
                }
                IndicatorMode::Processing => 1.0 + processing_pulse * 0.025,
                IndicatorMode::Listening | IndicatorMode::Hidden => 1.0,
            },
            visual_opacity: match self.mode {
                IndicatorMode::Success => 1.0 - self.mode_progress,
                IndicatorMode::Listening | IndicatorMode::Processing => 1.0,
                IndicatorMode::Hidden => 0.0,
            },
        }
    }
}

#[derive(Clone)]
pub(crate) struct IndicatorModels {
    pub(crate) bars: Rc<VecModel<f32>>,
    pub(crate) spectrum: Rc<VecModel<f32>>,
    pub(crate) slate_lines: Rc<VecModel<SlateLine>>,
}

pub(crate) fn new_indicator_models() -> IndicatorModels {
    IndicatorModels {
        bars: Rc::new(VecModel::from(vec![
            4.0, 6.0, 9.0, 12.0, 15.0, 15.0, 12.0, 9.0, 6.0, 4.0,
        ])),
        spectrum: Rc::new(VecModel::from(vec![2.8; AUDIO_SPECTRUM_BANDS])),
        slate_lines: Rc::new(VecModel::from(Vec::<SlateLine>::new())),
    }
}

pub(crate) fn new_settings_preview(settings: &SettingsWindow) -> Rc<RefCell<IndicatorAnimation>> {
    let models = new_indicator_models();
    settings.set_visual_preview_bars(ModelRc::from(Rc::clone(&models.bars)));
    settings.set_visual_preview_spectrum(ModelRc::from(Rc::clone(&models.spectrum)));
    settings.set_visual_preview_slate_lines(ModelRc::from(Rc::clone(&models.slate_lines)));
    Rc::new(RefCell::new(IndicatorAnimation::new(
        models.bars,
        models.spectrum,
        models.slate_lines,
    )))
}

pub(crate) fn build_indicator(
    models: &IndicatorModels,
) -> Result<RecordingIndicator, slint::PlatformError> {
    let indicator = RecordingIndicator::new()?;
    indicator.set_bar_heights(ModelRc::from(Rc::clone(&models.bars)));
    indicator.set_spectrum_heights(ModelRc::from(Rc::clone(&models.spectrum)));
    indicator.set_slate_lines(ModelRc::from(Rc::clone(&models.slate_lines)));
    Ok(indicator)
}

pub(crate) fn new_indicator()
-> Result<(RecordingIndicator, Rc<RefCell<IndicatorAnimation>>), slint::PlatformError> {
    let models = new_indicator_models();
    let indicator = build_indicator(&models)?;
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(
        models.bars,
        models.spectrum,
        models.slate_lines,
    )));
    Ok((indicator, animation))
}

pub(crate) fn new_recording_indicators() -> Result<
    (
        RecordingIndicator,
        RecordingIndicator,
        Rc<RefCell<IndicatorAnimation>>,
    ),
    slint::PlatformError,
> {
    let models = new_indicator_models();
    let panel = build_indicator(&models)?;
    let transparent = build_indicator(&models)?;
    let animation = Rc::new(RefCell::new(IndicatorAnimation::new(
        models.bars,
        models.spectrum,
        models.slate_lines,
    )));
    Ok((panel, transparent, animation))
}

pub(crate) fn indicator_preview_style_for_argument(argument: &str) -> Option<VisualizerStyle> {
    match argument {
        "--indicator-preview" => Some(VisualizerStyle::BroadcastMic),
        "--indicator-preview=mic" | "--indicator-preview=microphone" => {
            Some(VisualizerStyle::BroadcastMic)
        }
        "--indicator-preview=circle" | "--indicator-preview=circular-spectrum" => {
            Some(VisualizerStyle::CircularSpectrum)
        }
        "--indicator-preview=flow-sphere" | "--indicator-preview=sphere" => {
            Some(VisualizerStyle::FlowSphere)
        }
        "--indicator-preview=pulse-matrix" => Some(VisualizerStyle::PulseMatrix),
        "--indicator-preview=slate" | "--indicator-preview=writing-slate" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview=slate-transparent"
        | "--indicator-preview=writing-slate-transparent" => Some(VisualizerStyle::WritingSlate),
        "--indicator-preview=slate-frosted" | "--indicator-preview=writing-slate-frosted" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview=slate-blue-glass" | "--indicator-preview=writing-slate-blue-glass" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=circle" | "--indicator-preview-live=circular-spectrum" => {
            Some(VisualizerStyle::CircularSpectrum)
        }
        "--indicator-preview-live=flow-sphere" | "--indicator-preview-live=sphere" => {
            Some(VisualizerStyle::FlowSphere)
        }
        "--indicator-preview-live=pulse-matrix" => Some(VisualizerStyle::PulseMatrix),
        "--indicator-preview-live=slate" | "--indicator-preview-live=writing-slate" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=slate-transparent"
        | "--indicator-preview-live=writing-slate-transparent" => {
            Some(VisualizerStyle::WritingSlate)
        }
        "--indicator-preview-live=slate-frosted"
        | "--indicator-preview-live=writing-slate-frosted" => Some(VisualizerStyle::WritingSlate),
        "--indicator-preview-live=slate-blue-glass"
        | "--indicator-preview-live=writing-slate-blue-glass" => {
            Some(VisualizerStyle::WritingSlate)
        }
        _ => None,
    }
}

pub(crate) fn indicator_preview_pulse_matrix_palette_for_argument(
    argument: &str,
) -> Option<PulseMatrixPalette> {
    match argument {
        "--pulse-matrix-palette=cyan" => Some(PulseMatrixPalette::Cyan),
        "--pulse-matrix-palette=graphite-silver" | "--pulse-matrix-palette=graphite" => {
            Some(PulseMatrixPalette::GraphiteSilver)
        }
        "--pulse-matrix-palette=red-amber" | "--pulse-matrix-palette=amber" => {
            Some(PulseMatrixPalette::RedAmber)
        }
        _ => None,
    }
}

pub(crate) fn indicator_preview_slate_surface_for_argument(
    argument: &str,
) -> Option<WritingSlateSurface> {
    match argument {
        "--indicator-preview=slate-transparent"
        | "--indicator-preview=writing-slate-transparent"
        | "--indicator-preview-live=slate-transparent"
        | "--indicator-preview-live=writing-slate-transparent"
        | "--indicator-preview=slate-frosted"
        | "--indicator-preview=writing-slate-frosted"
        | "--indicator-preview-live=slate-frosted"
        | "--indicator-preview-live=writing-slate-frosted" => Some(WritingSlateSurface::MatteGlass),
        "--indicator-preview=slate-blue-glass"
        | "--indicator-preview=writing-slate-blue-glass"
        | "--indicator-preview-live=slate-blue-glass"
        | "--indicator-preview-live=writing-slate-blue-glass" => {
            Some(WritingSlateSurface::BlueGlass)
        }
        _ => None,
    }
}

#[cfg(test)]
mod indicator_animation_tests {
    use std::rc::Rc;

    use rust_protocol::AUDIO_SPECTRUM_BANDS;
    use slint::VecModel;

    use super::{
        CircularSpectrumPalette, CompanionPreviewRenderer, IndicatorAnimation, IndicatorMode,
        RecordingSurface, SlateLine, VisualizerStyle, WritingSlateAnimation, WritingSlateSurface,
        flow_sphere, indicator_preview_pulse_matrix_palette_for_argument,
        indicator_preview_slate_surface_for_argument, indicator_preview_style_for_argument,
    };
    use crate::pulse_matrix::PulseMatrixPalette;

    fn animation() -> IndicatorAnimation {
        IndicatorAnimation::new(
            Rc::new(VecModel::from(vec![0.0; 10])),
            Rc::new(VecModel::from(vec![0.0; AUDIO_SPECTRUM_BANDS])),
            Rc::new(VecModel::from(Vec::<SlateLine>::new())),
        )
    }

    #[test]
    fn visualizer_styles_are_partitioned_by_immutable_native_surface() {
        for style in [
            VisualizerStyle::VoiceOrb,
            VisualizerStyle::SymmetricSpectrum,
            VisualizerStyle::FluidRibbon,
        ] {
            assert_eq!(style.recording_surface(), Some(RecordingSurface::Panel));
            assert!(!style.uses_transparent_window());
        }
        for style in [
            VisualizerStyle::BroadcastMic,
            VisualizerStyle::CircularSpectrum,
            VisualizerStyle::FlowSphere,
            VisualizerStyle::WritingSlate,
            VisualizerStyle::CompanionOrb,
            VisualizerStyle::PulseMatrix,
        ] {
            assert_eq!(
                style.recording_surface(),
                Some(RecordingSurface::Transparent)
            );
            assert!(style.uses_transparent_window());
        }
        assert_eq!(VisualizerStyle::Off.recording_surface(), None);
        assert!(!VisualizerStyle::Off.uses_transparent_window());
    }

    #[test]
    fn every_released_visualizer_has_a_live_settings_preview() {
        for index in [3, 4, 5, 6, 7, 9] {
            let style =
                VisualizerStyle::from_selectable_index(index).expect("released visualizer index");
            assert!(style.has_live_settings_preview(), "{style:?}");
        }
        assert!(!VisualizerStyle::Off.has_live_settings_preview());

        let mut companion = CompanionPreviewRenderer::new().expect("embedded companion asset");
        let frame = companion.render(0.5);
        assert_eq!(frame.size().width, super::COMPANION_PREVIEW_WIDTH);
        assert_eq!(frame.size().height, super::COMPANION_PREVIEW_HEIGHT);
    }

    #[test]
    fn broadcast_mic_uses_the_transparent_recording_surface_contract() {
        i_slint_backend_testing::init_no_event_loop();
        let (indicator, _) = super::new_indicator().expect("recording indicator");
        indicator.set_visualizer_style(VisualizerStyle::BroadcastMic.index());
        indicator.set_broadcast_mic_palette(super::BroadcastMicPalette::Red.index());
        indicator.set_prepared(true);
        assert!(indicator.get_transparent_surface());
        assert_eq!(indicator.get_broadcast_mic_palette(), 2);

        indicator.set_visualizer_style(VisualizerStyle::VoiceOrb.index());
        assert!(!indicator.get_transparent_surface());
    }

    #[test]
    fn listening_frame_has_independent_bar_motion() {
        let mut animation = animation();
        animation.begin_listening();
        animation.set_microphone_signal(0.55, &[]);
        let frame = animation.advance_frame(1.0);

        assert_eq!(frame.heights.len(), 10);
        assert!(
            frame
                .heights
                .windows(2)
                .any(|pair| (pair[0] - pair[1]).abs() > 0.05)
        );
        assert!(frame.signal_opacity > 0.5);
        assert_eq!(frame.disc_opacity, 1.0);
    }

    #[test]
    fn processing_frames_keep_moving_until_completion() {
        let mut animation = animation();
        animation.begin_processing();
        let first = animation.advance_frame(1.0);
        let second = animation.advance_frame(1.0);

        assert_ne!(first.heights, second.heights);
        assert_ne!(first.disc_opacity, second.disc_opacity);
    }

    #[test]
    fn success_frames_collapse_and_fade_without_blocking() {
        let mut animation = animation();
        animation.begin_success();
        let first = animation.advance_frame(1.0);
        for _ in 0..12 {
            let _ = animation.advance_frame(1.0);
        }
        let later = animation.advance_frame(1.0);

        assert!(later.visual_scale < first.visual_scale);
        assert!(later.visual_opacity < first.visual_opacity);
        assert!(later.visual_scale >= 0.45);
        assert!(later.visual_opacity >= 0.0);
    }

    #[test]
    fn every_visible_visualizer_choice_maps_to_a_style() {
        assert_eq!(
            VisualizerStyle::from_selectable_index(3),
            Some(VisualizerStyle::BroadcastMic)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(4),
            Some(VisualizerStyle::CircularSpectrum)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(5),
            Some(VisualizerStyle::FlowSphere)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(6),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(7),
            Some(VisualizerStyle::CompanionOrb)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(8),
            Some(VisualizerStyle::Off)
        );
        assert_eq!(
            VisualizerStyle::from_selectable_index(9),
            Some(VisualizerStyle::PulseMatrix)
        );
        assert_eq!(
            serde_json::to_string(&VisualizerStyle::FlowSphere).unwrap(),
            "\"flow_sphere\""
        );
        assert_eq!(
            serde_json::from_str::<VisualizerStyle>("\"flow_sphere_preview\"").unwrap(),
            VisualizerStyle::FlowSphere
        );
        assert_eq!(
            serde_json::to_string(&VisualizerStyle::WritingSlate).unwrap(),
            "\"writing_slate\""
        );
        assert_eq!(
            serde_json::from_str::<VisualizerStyle>("\"writing_slate_preview\"").unwrap(),
            VisualizerStyle::WritingSlate
        );
    }

    #[test]
    fn retired_visualizers_cannot_be_selected() {
        for index in 0..=2 {
            assert_eq!(VisualizerStyle::from_selectable_index(index), None);
        }
        for index in 3..=9 {
            assert!(
                VisualizerStyle::from_selectable_index(index).is_some(),
                "released visualizer index {index}"
            );
        }
    }

    #[test]
    fn new_indicator_surfaces_default_to_a_released_visualizer() {
        i_slint_backend_testing::init_no_event_loop();
        let (indicator, _) = super::new_indicator().expect("recording indicator");

        assert_eq!(
            indicator.get_visualizer_style(),
            VisualizerStyle::BroadcastMic.index()
        );
        assert!(indicator.get_transparent_surface());
    }

    #[test]
    fn retired_visualizer_preview_arguments_are_not_addressable() {
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview"),
            Some(VisualizerStyle::BroadcastMic)
        );
        for argument in [
            "--indicator-preview=orb",
            "--indicator-preview=spectrum",
            "--indicator-preview=ribbon",
        ] {
            assert_eq!(indicator_preview_style_for_argument(argument), None);
        }
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview=pulse-matrix"),
            Some(VisualizerStyle::PulseMatrix)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=pulse-matrix"),
            Some(VisualizerStyle::PulseMatrix)
        );
    }

    #[test]
    fn pulse_matrix_renderer_lives_only_while_that_style_is_active() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::PulseMatrix);
        assert!(animation.pulse_matrix_renderer.is_some());

        animation.set_style(VisualizerStyle::BroadcastMic);
        assert!(animation.pulse_matrix_renderer.is_none());
    }

    #[test]
    fn pulse_matrix_preview_palette_arguments_are_explicit() {
        assert_eq!(
            indicator_preview_pulse_matrix_palette_for_argument(
                "--pulse-matrix-palette=graphite-silver"
            ),
            Some(PulseMatrixPalette::GraphiteSilver)
        );
        assert_eq!(
            indicator_preview_pulse_matrix_palette_for_argument("--pulse-matrix-palette=red-amber"),
            Some(PulseMatrixPalette::RedAmber)
        );
        assert_eq!(
            indicator_preview_pulse_matrix_palette_for_argument("--pulse-matrix-palette=green"),
            None
        );
    }

    #[test]
    fn pulse_matrix_keeps_the_approved_calm_motion_timing() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::PulseMatrix);
        animation.begin_listening();
        animation.set_microphone_signal(0.64, &[]);
        animation.advance_frame(1.0);

        assert!(animation.phase >= 0.035);
        assert!(
            animation.phase <= 0.11,
            "phase advanced too quickly: {}",
            animation.phase
        );
    }

    #[test]
    fn writing_slate_builds_short_segments_and_scrolls_after_two_thirds() {
        let mut slate = WritingSlateAnimation::default();
        for _ in 0..80 {
            slate.start_line();
            slate.commit_active();
        }
        let mut lines = Vec::new();
        for _ in 0..12 {
            lines = slate.advance(1.0, IndicatorMode::Listening, 0.0);
        }
        let colors = lines
            .iter()
            .map(|line| line.color_index)
            .collect::<std::collections::BTreeSet<_>>();

        assert!(colors.len() >= 3);
        assert!(lines.len() < 80);
        assert!(lines.iter().all(|line| line.width <= 30.0));
        assert!(
            lines
                .windows(2)
                .any(|pair| (pair[0].y - pair[1].y).abs() < 0.2)
        );
        assert!(lines.iter().all(|line| line.y <= 91.0));
    }

    #[test]
    fn writing_slate_is_available_in_settings_and_preview_modes() {
        assert_eq!(
            VisualizerStyle::from_index(6),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview=writing-slate"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate-transparent"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument(
                "--indicator-preview-live=slate-transparent"
            ),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview-live=slate"),
            None
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=slate-frosted"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview-live=slate-frosted"),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview=slate-blue-glass"),
            Some(VisualizerStyle::WritingSlate)
        );
        assert_eq!(
            indicator_preview_slate_surface_for_argument("--indicator-preview=slate-blue-glass"),
            Some(WritingSlateSurface::BlueGlass)
        );
        assert_eq!(
            WritingSlateSurface::from_index(0),
            Some(WritingSlateSurface::DarkPanel)
        );
        assert_eq!(
            WritingSlateSurface::from_index(1),
            Some(WritingSlateSurface::MatteGlass)
        );
        assert_eq!(
            WritingSlateSurface::from_index(2),
            Some(WritingSlateSurface::BlueGlass)
        );
        assert_eq!(
            serde_json::to_string(&WritingSlateSurface::BlueGlass).unwrap(),
            "\"blue_glass\""
        );
    }

    #[test]
    fn flow_sphere_live_preview_is_explicitly_addressable() {
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=flow-sphere"),
            Some(VisualizerStyle::FlowSphere)
        );
        assert_eq!(
            indicator_preview_style_for_argument("--indicator-preview-live=sphere"),
            Some(VisualizerStyle::FlowSphere)
        );
    }

    #[test]
    fn flow_sphere_is_calm_in_silence_and_accelerates_on_voice_onset() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::FlowSphere);
        animation.begin_listening();

        let quiet_start = animation.phase;
        let _ = animation.advance_frame(1.0);
        let quiet_step = animation.phase - quiet_start;

        animation.set_microphone_signal(0.62, &[0.55; AUDIO_SPECTRUM_BANDS]);
        let active_start = animation.phase;
        let _ = animation.advance_frame(1.0);
        let active_step = animation.phase - active_start;

        assert!(quiet_step < 0.06);
        assert!(active_step > quiet_step * 3.0);
        assert!(animation.flow_voice_impulse > 0.2);
    }

    #[test]
    fn every_circular_palette_choice_maps_to_a_persisted_value() {
        assert_eq!(
            CircularSpectrumPalette::from_index(0),
            Some(CircularSpectrumPalette::Sunset)
        );
        assert_eq!(
            CircularSpectrumPalette::from_index(1),
            Some(CircularSpectrumPalette::Arctic)
        );
        assert_eq!(
            CircularSpectrumPalette::from_index(2),
            Some(CircularSpectrumPalette::Monochrome)
        );
        assert_eq!(CircularSpectrumPalette::from_index(3), None);
        assert_eq!(CircularSpectrumPalette::Sunset.index(), 0);
        assert_eq!(CircularSpectrumPalette::Arctic.index(), 1);
        assert_eq!(CircularSpectrumPalette::Monochrome.index(), 2);
        assert_eq!(
            serde_json::to_string(&CircularSpectrumPalette::Arctic).unwrap(),
            "\"arctic\""
        );
        assert_eq!(
            serde_json::from_str::<CircularSpectrumPalette>("\"monochrome\"").unwrap(),
            CircularSpectrumPalette::Monochrome
        );
    }

    #[test]
    fn every_broadcast_mic_palette_choice_maps_to_a_persisted_value() {
        use super::BroadcastMicPalette;

        for (index, palette, serialized) in [
            (0, BroadcastMicPalette::White, "\"white\""),
            (1, BroadcastMicPalette::Blue, "\"blue\""),
            (2, BroadcastMicPalette::Red, "\"red\""),
        ] {
            assert_eq!(BroadcastMicPalette::from_index(index), Some(palette));
            assert_eq!(palette.index(), index);
            assert_eq!(serde_json::to_string(&palette).unwrap(), serialized);
        }
        assert_eq!(BroadcastMicPalette::from_index(3), None);
    }

    #[test]
    fn every_flow_sphere_palette_maps_to_a_persisted_value() {
        assert_eq!(
            flow_sphere::FlowSpherePalette::from_index(0),
            Some(flow_sphere::FlowSpherePalette::Reference)
        );
        assert_eq!(
            flow_sphere::FlowSpherePalette::from_index(1),
            Some(flow_sphere::FlowSpherePalette::BlackYellow)
        );
        assert_eq!(flow_sphere::FlowSpherePalette::from_index(2), None);
        assert_eq!(flow_sphere::FlowSpherePalette::Reference.index(), 0);
        assert_eq!(flow_sphere::FlowSpherePalette::BlackYellow.index(), 1);
        assert_eq!(
            serde_json::to_string(&flow_sphere::FlowSpherePalette::BlackYellow).unwrap(),
            "\"black_yellow\""
        );
    }

    #[test]
    fn broadcast_microphone_lines_react_independently() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::BroadcastMic);
        animation.begin_listening();
        animation.set_microphone_signal(0.64, &[]);
        let first = animation.advance_frame(1.0);
        let second = animation.advance_frame(1.0);

        assert!((3.0..=12.0).contains(&first.mic_edge_height));
        assert!((4.0..=17.0).contains(&first.mic_outer_height));
        assert!((5.0..=18.0).contains(&first.mic_middle_height));
        assert!((3.0..=14.0).contains(&first.mic_inner_height));
        assert!((4.0..=15.0).contains(&first.mic_near_height));
        assert!(
            (first.mic_edge_height - second.mic_edge_height).abs() > 0.01
                || (first.mic_outer_height - second.mic_outer_height).abs() > 0.01
                || (first.mic_middle_height - second.mic_middle_height).abs() > 0.01
                || (first.mic_inner_height - second.mic_inner_height).abs() > 0.01
                || (first.mic_near_height - second.mic_near_height).abs() > 0.01
        );
    }

    #[test]
    fn circular_spectrum_preserves_real_band_differences() {
        let mut animation = animation();
        animation.set_style(VisualizerStyle::CircularSpectrum);
        animation.begin_listening();
        let mut bands = vec![0.0; AUDIO_SPECTRUM_BANDS];
        bands[5] = 1.0;
        bands[14] = 0.52;
        animation.set_microphone_signal(0.72, &bands);

        let frame = animation.advance_frame(1.0);
        assert_eq!(frame.spectrum_heights.len(), AUDIO_SPECTRUM_BANDS);
        assert!(frame.spectrum_heights[5] > frame.spectrum_heights[14]);
        assert!(frame.spectrum_heights[14] > frame.spectrum_heights[25]);
        assert!(
            frame
                .spectrum_heights
                .iter()
                .all(|height| (2.8..=21.5).contains(height))
        );
    }
}
