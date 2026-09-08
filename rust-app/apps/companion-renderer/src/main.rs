#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    io::{BufRead, BufReader, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

use pet_orb::OrbActor;
use pet_particles::{PALETTE, Vec2};
use pet_platform::{ModifierDragEvent, ModifierDragMonitor, MonitorArea, Overlay};
use pet_render::{Canvas, SpriteSource};
use rust_protocol::{
    CompanionRendererCommand, CompanionRendererEvent, PROTOCOL_VERSION, SessionId,
};

// These are the accepted desktop-pet scene dimensions at its default 0.16
// scale. The large transparent surface is intentional: the burst particles
// need the same travel room they have beside the character.
const CANVAS_WIDTH: u32 = 640;
const CANVAS_HEIGHT: u32 = 560;
const ORB_SCALE: f32 = 0.26;
const ORB_GROW_START: f32 = 0.08;
const ORB_GROW_FRAMES: usize = 24;
const ORB_MOTION_RADIUS: f32 = 85.5;
const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const HOME_FRAME_INTERVAL: Duration = Duration::from_micros(33_333);
const PARK_COORDINATE: i32 = -32_000;
const AUDIO_BANDS: usize = 16;
const WAVE_SQUARES_PER_SIDE: usize = 8;
const WAVE_SQUARE_SPACING: f32 = 8.5;
const WAVE_SQUARE_GAP: f32 = 12.0;
// Match the existing orbit particles: their source sizes are 8..24 px and
// the accepted orb is rendered at ORB_SCALE, so the visible squares are only
// about 2..6 px rather than a second, oversized visualizer.
const WAVE_SQUARE_MIN_SIZE: f32 = 8.0 * ORB_SCALE;
const WAVE_SQUARE_MAX_SIZE: f32 = 24.0 * ORB_SCALE;
const WAVE_BASE_AMPLITUDE: f32 = 2.5;
const WAVE_PEAK_AMPLITUDE: f32 = 20.0;
// The regular recording indicator is 42 px high and sits 22 px above the
// work-area bottom. Aligning the orb's centre to that indicator's centre
// keeps both visualizer styles on exactly the same baseline.
const RECORDING_CENTER_FROM_BOTTOM: i32 = 22 + 42 / 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Disabled,
    Home,
    Listening,
    Processing,
    Success,
}

struct Runtime {
    enabled: bool,
    stage: Stage,
    session_id: Option<SessionId>,
    peak: f32,
    display_peak: f32,
    spectrum: [f32; AUDIO_BANDS],
    display_spectrum: [f32; AUDIO_BANDS],
    success_until: Option<Instant>,
    home_position: Option<(i32, i32)>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            enabled: false,
            stage: Stage::Disabled,
            session_id: None,
            peak: 0.0,
            display_peak: 0.0,
            spectrum: [0.0; AUDIO_BANDS],
            display_spectrum: [0.0; AUDIO_BANDS],
            success_until: None,
            home_position: None,
        }
    }
}

impl Runtime {
    fn accepts(&self, session_id: SessionId) -> bool {
        self.session_id == Some(session_id)
    }

    fn brightness(&self) -> f32 {
        match self.stage {
            Stage::Listening => 1.0 + self.display_peak * 0.5,
            Stage::Processing => 1.18,
            Stage::Success => 1.55,
            Stage::Home | Stage::Disabled => 1.0,
        }
    }

    fn settle_peak(&mut self, dt: f32) {
        let response = if self.peak > self.display_peak {
            13.0
        } else {
            5.0
        };
        self.display_peak += (self.peak - self.display_peak) * (dt * response).min(1.0);
    }

    fn settle_spectrum(&mut self, dt: f32) {
        for (target, display) in self.spectrum.iter().zip(&mut self.display_spectrum) {
            let response = if *target > *display { 16.0 } else { 7.0 };
            *display += (*target - *display) * (dt * response).min(1.0);
        }
    }
}

fn main() {
    // Asked by the release check, which otherwise cannot find out what a
    // packaged process speaks short of driving a whole handshake.
    if std::env::args().any(|argument| argument == "--protocol-version") {
        println!("{PROTOCOL_VERSION}");
        return;
    }
    if let Err(error) = run() {
        rust_app_core::diagnostics::log_line(&format!(
            "companion_renderer lifecycle=start_failed error={error}"
        ));
        write_event(&CompanionRendererEvent::Failed {
            session_id: None,
            code: "renderer_failed".to_owned(),
            message: error,
        });
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let _ = rust_app_core::product::migrate_legacy_data_root();
    let _ = rust_app_core::diagnostics::initialize(
        &rust_app_core::product::data_root().join("logs"),
        "companion-renderer",
    );
    rust_app_core::diagnostics::log_line("companion_renderer lifecycle=starting engine=pet_orb");

    let source =
        SpriteSource::load_bytes(include_bytes!("../../../../desktop-pet/assets/orb.png"), 10)
            .map_err(|error| format!("could not decode the embedded pet orb: {error}"))?;
    // Cache the growing sphere once. Runtime frames only select a prepared
    // sprite and never resample image data on the animation thread.
    let growth_frames = (0..ORB_GROW_FRAMES)
        .map(|index| {
            let progress = index as f32 / (ORB_GROW_FRAMES - 1) as f32;
            let scale = ORB_SCALE * (ORB_GROW_START + (1.0 - ORB_GROW_START) * progress);
            source.scaled(scale)
        })
        .collect::<Vec<_>>();
    let sprite = growth_frames
        .last()
        .expect("the orb growth frame count is non-zero");
    let centre = Vec2::new(CANVAS_WIDTH as f32 / 2.0, CANVAS_HEIGHT as f32 / 2.0);
    let body_radius = sprite.width() as f32 / 2.0;
    let rest = centre + Vec2::new(ORB_MOTION_RADIUS, 0.0);
    let mut actor = OrbActor::new(rest, ORB_MOTION_RADIUS, body_radius, ORB_SCALE);
    let mut canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let mut overlay = Overlay::new(
        CANVAS_WIDTH,
        CANVAS_HEIGHT,
        PARK_COORDINATE,
        PARK_COORDINATE,
    )?;
    canvas.clear();
    overlay.present(canvas.pixels())?;

    let commands = command_stream()?;
    let drag_monitor = ModifierDragMonitor::start()
        .map_err(|error| format!("could not start companion drag monitor: {error}"))?;
    write_event(&CompanionRendererEvent::Ready {
        protocol_version: PROTOCOL_VERSION,
    });

    let mut runtime = Runtime::default();
    let mut drag_offset: Option<(i32, i32)> = None;
    let mut last_frame = Instant::now();
    let mut animation_elapsed = 0.0_f32;
    let mut running = true;
    while running && overlay.pump() {
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    running = apply_command(
                        command,
                        &mut runtime,
                        &mut overlay,
                        &mut canvas,
                        &mut actor,
                        centre,
                        rest,
                    )?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if !running {
            break;
        }

        if runtime.stage != Stage::Home && drag_offset.take().is_some() {
            actor.set_motion_paused(false);
            drag_monitor.cancel_drag();
        }
        update_drag_hit_rect(&drag_monitor, &runtime, &overlay, &actor);
        if let Some(event) = drag_monitor.poll() {
            handle_drag_event(
                event,
                &mut drag_offset,
                &mut runtime,
                &mut overlay,
                &mut actor,
            )?;
        }

        if runtime.stage == Stage::Disabled {
            overlay.wait(Duration::from_millis(100))?;
            last_frame = Instant::now();
            continue;
        }

        let now = Instant::now();
        if runtime.stage == Stage::Success
            && runtime
                .success_until
                .is_some_and(|deadline| now >= deadline)
        {
            runtime.session_id = None;
            runtime.success_until = None;
            runtime.peak = 0.0;
            runtime.spectrum.fill(0.0);
            runtime.display_spectrum.fill(0.0);
            actor.resume_home_pose(rest);
            runtime.stage = if runtime.enabled {
                place_home(&mut overlay, runtime.home_position)?;
                Stage::Home
            } else {
                park(&mut overlay, &mut canvas)?;
                Stage::Disabled
            };
        }

        let dt = now
            .duration_since(last_frame)
            .as_secs_f32()
            .clamp(0.0, 0.05);
        last_frame = now;
        animation_elapsed += dt;
        runtime.settle_peak(dt);
        runtime.settle_spectrum(dt);
        update_gaze(&runtime, &overlay, &mut actor);
        actor.update(dt, centre, runtime.brightness());
        canvas.clear();
        actor.draw_behind(&mut canvas);
        if matches!(runtime.stage, Stage::Listening | Stage::Processing) {
            draw_recording_wave(
                &mut canvas,
                centre,
                body_radius,
                animation_elapsed,
                runtime.display_peak,
                &runtime.display_spectrum,
            );
        }
        actor.draw_sprite(&mut canvas, &growth_frames);
        actor.draw_front(&mut canvas);
        actor.draw_flash(&mut canvas);
        overlay.present(canvas.pixels())?;
        let frame_interval = if runtime.stage == Stage::Home {
            HOME_FRAME_INTERVAL
        } else {
            ACTIVE_FRAME_INTERVAL
        };
        overlay.wait(frame_interval.saturating_sub(now.elapsed()))?;
    }

    rust_app_core::diagnostics::log_line("companion_renderer lifecycle=exiting");
    Ok(())
}

fn apply_command(
    command: CompanionRendererCommand,
    runtime: &mut Runtime,
    overlay: &mut Overlay,
    canvas: &mut Canvas,
    actor: &mut OrbActor,
    centre: Vec2,
    home_position: Vec2,
) -> Result<bool, String> {
    match command {
        CompanionRendererCommand::SetEnabled { enabled } => {
            runtime.enabled = enabled;
            if runtime.session_id.is_none() {
                runtime.stage = if enabled {
                    place_home(overlay, runtime.home_position)?;
                    Stage::Home
                } else {
                    park(overlay, canvas)?;
                    Stage::Disabled
                };
            }
            rust_app_core::diagnostics::log_line(&format!(
                "companion_renderer selection enabled={enabled} stage={:?}",
                runtime.stage
            ));
        }
        CompanionRendererCommand::SetHomePosition { x, y } => {
            let position = visible_home_position(x, y);
            runtime.home_position = Some(position);
            if runtime.stage == Stage::Home {
                overlay.set_position(position.0, position.1)?;
            }
            rust_app_core::diagnostics::log_line(&format!(
                "companion_renderer home_position=restored x={} y={}",
                position.0, position.1
            ));
        }
        CompanionRendererCommand::Activate {
            session_id,
            target_window_handle,
        } => {
            // A transparent frame makes relocation an intentional disappear /
            // reappear transition instead of dragging the previous pixels.
            canvas.clear();
            overlay.present(canvas.pixels())?;
            place_recording(overlay, target_window_handle)?;
            actor.enter_recording_pose(centre);
            runtime.session_id = Some(session_id);
            runtime.stage = Stage::Listening;
            runtime.peak = 0.0;
            runtime.display_peak = 0.0;
            runtime.spectrum.fill(0.0);
            runtime.display_spectrum.fill(0.0);
            runtime.success_until = None;
            rust_app_core::diagnostics::log_line(&format!(
                "companion_renderer transition=listening session={session_id}"
            ));
        }
        CompanionRendererCommand::AudioLevel {
            session_id,
            peak,
            spectrum,
        } if runtime.accepts(session_id) => {
            runtime.peak = (peak.clamp(0.0, 1.0).sqrt() * 2.8).clamp(0.0, 1.0);
            for (index, target) in runtime.spectrum.iter_mut().enumerate() {
                let fallback = peak * (0.72 - index as f32 * 0.035).max(0.18);
                let sample = spectrum
                    .get(index)
                    .copied()
                    .unwrap_or(fallback)
                    .clamp(0.0, 1.0);
                *target = (sample.sqrt() * 1.7).clamp(0.0, 1.0);
            }
        }
        CompanionRendererCommand::Processing { session_id } if runtime.accepts(session_id) => {
            runtime.stage = Stage::Processing;
            runtime.peak = 0.12;
        }
        CompanionRendererCommand::Complete { session_id } if runtime.accepts(session_id) => {
            runtime.stage = Stage::Success;
            runtime.peak = 0.0;
            runtime.success_until = Some(Instant::now() + Duration::from_millis(520));
        }
        CompanionRendererCommand::Dismiss { session_id }
            if session_id.is_none() || session_id == runtime.session_id =>
        {
            runtime.session_id = None;
            runtime.peak = 0.0;
            runtime.display_peak = 0.0;
            runtime.spectrum.fill(0.0);
            runtime.display_spectrum.fill(0.0);
            runtime.success_until = None;
            actor.resume_home_pose(home_position);
            runtime.stage = if runtime.enabled {
                place_home(overlay, runtime.home_position)?;
                Stage::Home
            } else {
                park(overlay, canvas)?;
                Stage::Disabled
            };
        }
        CompanionRendererCommand::Shutdown => return Ok(false),
        _ => {}
    }
    Ok(true)
}

fn place_home(overlay: &mut Overlay, saved: Option<(i32, i32)>) -> Result<(), String> {
    if let Some((x, y)) = saved {
        let (x, y) = visible_home_position(x, y);
        return overlay.set_position(x, y);
    }
    let (x, y) = pet_platform::cursor_position();
    let monitor = pet_platform::monitor_area_at(x, y);
    // Keep the same roomy transparent scene as the pet, but align its visible
    // orbit rather than the transparent canvas itself to the lower-right.
    let x = monitor.work.right - CANVAS_WIDTH as i32 + 152;
    let y = monitor.work.bottom - CANVAS_HEIGHT as i32 + 137;
    overlay.set_position(x, y)
}

fn visible_home_position(x: i32, y: i32) -> (i32, i32) {
    let screen = pet_platform::virtual_screen();
    // The surface is intentionally much larger than the orb. Preserve broad
    // multi-monitor freedom while guaranteeing that at least a usable part of
    // the scene remains reachable after a monitor-layout change.
    let minimum_visible = 96;
    let min_x = screen.left - CANVAS_WIDTH as i32 + minimum_visible;
    let max_x = screen.right - minimum_visible;
    let min_y = screen.top - CANVAS_HEIGHT as i32 + minimum_visible;
    let max_y = screen.bottom - minimum_visible;
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

fn update_drag_hit_rect(
    monitor: &ModifierDragMonitor,
    runtime: &Runtime,
    overlay: &Overlay,
    actor: &OrbActor,
) {
    if runtime.stage != Stage::Home || !actor.is_grabbable() {
        monitor.clear_hit_rect();
        return;
    }
    let (window_x, window_y) = overlay.position();
    let position = actor.position();
    let radius = actor.body_radius() + 14.0;
    monitor.set_hit_rect(
        window_x + (position.x - radius).floor() as i32,
        window_y + (position.y - radius).floor() as i32,
        window_x + (position.x + radius).ceil() as i32,
        window_y + (position.y + radius).ceil() as i32,
    );
}

fn handle_drag_event(
    event: ModifierDragEvent,
    drag_offset: &mut Option<(i32, i32)>,
    runtime: &mut Runtime,
    overlay: &mut Overlay,
    actor: &mut OrbActor,
) -> Result<(), String> {
    match event {
        ModifierDragEvent::Started { x, y } => {
            let (window_x, window_y) = overlay.position();
            *drag_offset = Some((x - window_x, y - window_y));
            actor.begin_drag();
            rust_app_core::diagnostics::log_line(
                "companion_renderer drag=started modifier=ctrl_alt",
            );
        }
        ModifierDragEvent::Moving { x, y } => {
            if let Some((offset_x, offset_y)) = *drag_offset {
                let position = visible_home_position(x - offset_x, y - offset_y);
                let previous = overlay.position();
                actor.drag_window(Vec2::new(
                    (position.0 - previous.0) as f32,
                    (position.1 - previous.1) as f32,
                ));
                overlay.set_position(position.0, position.1)?;
                runtime.home_position = Some(position);
            }
        }
        ModifierDragEvent::Ended { x, y } => {
            if let Some((offset_x, offset_y)) = drag_offset.take() {
                let position = visible_home_position(x - offset_x, y - offset_y);
                let previous = overlay.position();
                actor.drag_window(Vec2::new(
                    (position.0 - previous.0) as f32,
                    (position.1 - previous.1) as f32,
                ));
                overlay.set_position(position.0, position.1)?;
                runtime.home_position = Some(position);
                write_event(&CompanionRendererEvent::HomePositionChanged {
                    x: position.0,
                    y: position.1,
                });
                rust_app_core::diagnostics::log_line(&format!(
                    "companion_renderer drag=completed x={} y={}",
                    position.0, position.1
                ));
            }
            actor.end_drag();
        }
        ModifierDragEvent::RejectedOutsideHitRect { x, y } => {
            rust_app_core::diagnostics::log_line(&format!(
                "companion_renderer drag=rejected reason=outside_hit_rect x={x} y={y}"
            ));
        }
    }
    Ok(())
}

fn update_gaze(runtime: &Runtime, overlay: &Overlay, actor: &mut OrbActor) {
    if runtime.stage != Stage::Home {
        actor.look_towards(Vec2::default());
        return;
    }
    let (cursor_x, cursor_y) = pet_platform::cursor_position();
    let (window_x, window_y) = overlay.position();
    let position = actor.position();
    let delta = Vec2::new(
        (cursor_x as f32 - (window_x as f32 + position.x)) / 180.0,
        (cursor_y as f32 - (window_y as f32 + position.y)) / 135.0,
    );
    actor.look_towards(delta);
}

fn place_recording(overlay: &mut Overlay, target_window_handle: u64) -> Result<(), String> {
    let monitor = monitor_for_target(target_window_handle);
    let x = monitor.work.left + (monitor.work.right - monitor.work.left - CANVAS_WIDTH as i32) / 2;
    let y = recording_overlay_y(monitor.work.bottom);
    overlay.set_position(x, y)
}

fn recording_overlay_y(work_bottom: i32) -> i32 {
    work_bottom - CANVAS_HEIGHT as i32 / 2 - RECORDING_CENTER_FROM_BOTTOM
}

fn draw_recording_wave(
    canvas: &mut Canvas,
    centre: Vec2,
    body_radius: f32,
    elapsed: f32,
    display_peak: f32,
    spectrum: &[f32; AUDIO_BANDS],
) {
    for side in [-1.0_f32, 1.0_f32] {
        for index in 0..WAVE_SQUARES_PER_SIDE {
            // A deliberately uneven silhouette: the squares are satellites
            // from the accepted orb, not equal-height bars of a new
            // equalizer. Each side samples a different part of the incoming
            // spectrum, so the two rows do not mirror a single repeating
            // sine-wave pattern.
            const PEAK_PROFILE: [f32; WAVE_SQUARES_PER_SIDE] =
                [0.36, 0.64, 0.46, 0.88, 0.57, 0.32, 0.74, 0.48];
            let profile = PEAK_PROFILE[index];
            let side_index = if side < 0.0 { 0 } else { 1 };
            let band_index = (index * 2 + side_index * 5) % AUDIO_BANDS;
            let neighbor_index = (band_index + 3 + index % 3) % AUDIO_BANDS;
            let band = spectrum[band_index];
            let neighbor = spectrum[neighbor_index];
            let level = (band * 0.95 + neighbor * 0.25 + display_peak * (0.12 + profile * 0.24))
                .clamp(0.0, 1.0);
            // Smooth, decorrelated motion keeps each square alive without
            // making the whole row march in lockstep. Audio controls the
            // travel distance; these two incommensurate curves only provide
            // a lightweight organic direction change between audio frames.
            let motion = decorrelated_motion(elapsed, index, side_index);
            let travel = WAVE_BASE_AMPLITUDE
                + level * WAVE_PEAK_AMPLITUDE * (0.62 + profile * 0.38)
                + motion * level * 3.0;
            let y = centre.y + (motion * 2.0 - 1.0) * travel;
            let seed_variation =
                ((index * 37 + if side > 0.0 { 11 } else { 3 }) % 17) as f32 / 16.0;
            let size = WAVE_SQUARE_MIN_SIZE
                + (WAVE_SQUARE_MAX_SIZE - WAVE_SQUARE_MIN_SIZE) * seed_variation
                + level * ORB_SCALE * 2.0;
            let x = centre.x
                + side * (body_radius + WAVE_SQUARE_GAP + index as f32 * WAVE_SQUARE_SPACING);
            let side_offset = if side > 0.0 { 1 } else { 0 };
            let colour = PALETTE[(index + side_offset) % PALETTE.len()];
            let alpha = 0.62 + level * 0.38;
            let half = (size / 2.0).round() as i32;
            canvas.fill_rect(
                x.round() as i32 - half,
                y.round() as i32 - half,
                size.round().max(1.0) as i32,
                size.round().max(1.0) as i32,
                premultiplied(colour.0, colour.1, colour.2, alpha),
            );
        }
    }
}

fn decorrelated_motion(elapsed: f32, index: usize, side: usize) -> f32 {
    let seed = (index as f32 * 1.73 + side as f32 * 4.91) + 0.37;
    let slow = (elapsed * (1.21 + index as f32 * 0.087) + seed * 1.91).sin();
    let quick = (elapsed * (2.07 + side as f32 * 0.19 + index as f32 * 0.043) + seed * 0.73).sin();
    (0.5 + slow * 0.31 + quick * 0.19).clamp(0.0, 1.0)
}

fn premultiplied(red: u8, green: u8, blue: u8, alpha: f32) -> [u8; 4] {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
    [scale(blue), scale(green), scale(red), alpha as u8]
}

fn monitor_for_target(target_window_handle: u64) -> MonitorArea {
    if target_window_handle != 0 {
        pet_platform::monitor_area_for_window(target_window_handle)
    } else {
        let (x, y) = pet_platform::cursor_position();
        pet_platform::monitor_area_at(x, y)
    }
}

fn park(overlay: &mut Overlay, canvas: &mut Canvas) -> Result<(), String> {
    canvas.clear();
    overlay.present(canvas.pixels())?;
    overlay.set_position(PARK_COORDINATE, PARK_COORDINATE)
}

fn command_stream() -> Result<Receiver<CompanionRendererCommand>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("companion-renderer-commands".to_owned())
        .spawn(move || {
            for line in BufReader::new(std::io::stdin()).lines() {
                let command = match line {
                    Ok(line) => match serde_json::from_str::<CompanionRendererCommand>(&line) {
                        Ok(command) => command,
                        Err(error) => {
                            write_event(&CompanionRendererEvent::Failed {
                                session_id: None,
                                code: "invalid_command".to_owned(),
                                message: error.to_string(),
                            });
                            continue;
                        }
                    },
                    Err(error) => {
                        write_event(&CompanionRendererEvent::Failed {
                            session_id: None,
                            code: "command_stream_failed".to_owned(),
                            message: error.to_string(),
                        });
                        break;
                    }
                };
                if sender.send(command).is_err() {
                    return;
                }
            }
            let _ = sender.send(CompanionRendererCommand::Shutdown);
        })
        .map_err(|error| format!("could not start the companion command reader: {error}"))?;
    Ok(receiver)
}

fn write_event(event: &CompanionRendererEvent) {
    if let Ok(json) = serde_json::to_string(event) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{json}");
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_commands_remain_session_scoped() {
        let session_id = SessionId::new();
        let runtime = Runtime {
            session_id: Some(session_id),
            ..Runtime::default()
        };
        assert!(runtime.accepts(session_id));
        assert!(!runtime.accepts(SessionId::new()));
    }

    #[test]
    fn recording_pose_centres_on_the_regular_indicator_baseline() {
        let work_bottom = 1080;
        let y = recording_overlay_y(work_bottom);
        let orb_centre = y + CANVAS_HEIGHT as i32 / 2;
        let regular_indicator_centre = work_bottom - RECORDING_CENTER_FROM_BOTTOM;
        assert_eq!(orb_centre, regular_indicator_centre);
    }

    #[test]
    fn spectrum_smoothing_rises_without_jumping() {
        let mut runtime = Runtime::default();
        runtime.spectrum[3] = 1.0;
        runtime.settle_spectrum(1.0 / 60.0);
        assert!(runtime.display_spectrum[3] > 0.0);
        assert!(runtime.display_spectrum[3] < 1.0);
    }

    #[test]
    fn recording_motion_is_decorrelated_between_squares_and_sides() {
        let left = decorrelated_motion(1.0, 2, 0);
        let right = decorrelated_motion(1.0, 2, 1);
        let next = decorrelated_motion(1.0, 3, 0);
        assert!((left - right).abs() > 0.01);
        assert!((left - next).abs() > 0.01);
        assert!((0.0..=1.0).contains(&left));
    }
}
