#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    io::{BufRead, BufReader, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

use pet_orb::OrbActor;
use pet_particles::Vec2;
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
}

fn main() {
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
    let mut running = true;
    while running && overlay.pump() {
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    running = apply_command(command, &mut runtime, &mut overlay, &mut canvas)?;
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
        runtime.settle_peak(dt);
        update_gaze(&runtime, &overlay, &mut actor);
        actor.update(dt, centre, runtime.brightness());
        canvas.clear();
        actor.draw_behind(&mut canvas);
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
            runtime.session_id = Some(session_id);
            runtime.stage = Stage::Listening;
            runtime.peak = 0.0;
            runtime.display_peak = 0.0;
            runtime.success_until = None;
            rust_app_core::diagnostics::log_line(&format!(
                "companion_renderer transition=listening session={session_id}"
            ));
        }
        CompanionRendererCommand::AudioLevel {
            session_id, peak, ..
        } if runtime.accepts(session_id) => {
            runtime.peak = (peak.clamp(0.0, 1.0).sqrt() * 1.45).clamp(0.0, 1.0);
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
            runtime.success_until = None;
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
        actor.look_towards(Vec2::new(0.0, -0.18));
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
    let y = monitor.work.bottom - CANVAS_HEIGHT as i32 - 22;
    overlay.set_position(x, y)
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
}
