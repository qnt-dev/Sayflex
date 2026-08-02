#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rust_protocol::{
    CompanionRendererCommand, CompanionRendererEvent, PROTOCOL_VERSION, SessionId,
};
use slint::winit_030::{
    WinitWindowAccessor,
    winit::raw_window_handle::{HasWindowHandle, RawWindowHandle},
};
use slint::{ComponentHandle, PhysicalPosition, RenderingState, Timer, TimerMode};

slint::include_modules!();

// Thirty animation updates per second are visually continuous at this overlay
// size while avoiding a permanent 60 Hz wake-up/render cost during dictation.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

type CommandHandler = Box<dyn FnMut(CompanionRendererCommand)>;

thread_local! {
    static COMMAND_HANDLER: RefCell<Option<CommandHandler>> = RefCell::new(None);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Hidden,
    Assembling,
    Listening,
    Processing,
    Success,
    Dissolving,
}

struct RendererState {
    session_id: Option<SessionId>,
    target_window_handle: Option<u64>,
    stage: Stage,
    stage_elapsed: f32,
    target_level: f32,
    display_level: f32,
    phase: f32,
    last_frame: Instant,
}

impl Default for RendererState {
    fn default() -> Self {
        Self {
            session_id: None,
            target_window_handle: None,
            stage: Stage::Hidden,
            stage_elapsed: 0.0,
            target_level: 0.0,
            display_level: 0.0,
            phase: 0.0,
            last_frame: Instant::now(),
        }
    }
}

impl RendererState {
    fn accepts(&self, session_id: SessionId) -> bool {
        self.session_id == Some(session_id)
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_elapsed = 0.0;
    }

    fn tick(&mut self, overlay: &CompanionOverlay, window_handle: u64) {
        let now = Instant::now();
        let delta = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .clamp(0.0, 0.05);
        self.last_frame = now;
        self.stage_elapsed += delta;

        let response = if self.target_level > self.display_level {
            1.0 - 0.25_f32.powf(delta * 60.0)
        } else {
            1.0 - 0.82_f32.powf(delta * 60.0)
        };
        self.display_level += (self.target_level - self.display_level) * response;
        self.phase = (self.phase + delta * (1.35 + self.display_level * 3.6)) % 1.0;

        match self.stage {
            Stage::Hidden => {
                overlay.set_active(false);
                overlay.set_visual_opacity(0.0);
                return;
            }
            Stage::Assembling if self.stage_elapsed >= 0.24 => self.enter(Stage::Listening),
            Stage::Success if self.stage_elapsed >= 0.28 => self.enter(Stage::Dissolving),
            Stage::Dissolving if self.stage_elapsed >= 0.22 => {
                self.enter(Stage::Hidden);
                self.session_id = None;
                self.target_window_handle = None;
                overlay.set_active(false);
                overlay.set_visual_opacity(0.0);
                if window_handle != 0 {
                    let _ = rust_platform_windows::conceal_overlay_window(window_handle);
                }
                return;
            }
            _ => {}
        }

        let assembly = match self.stage {
            Stage::Assembling => (self.stage_elapsed / 0.24).clamp(0.0, 1.0),
            Stage::Dissolving => 1.0 - (self.stage_elapsed / 0.22).clamp(0.0, 1.0),
            _ => 1.0,
        };
        let opacity = match self.stage {
            Stage::Assembling => assembly,
            Stage::Dissolving => assembly,
            _ => 1.0,
        };
        let success_burst = if self.stage == Stage::Success {
            (self.stage_elapsed / 0.28).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let processing_pulse = if self.stage == Stage::Processing {
            ((self.phase * std::f32::consts::TAU).sin() + 1.0) * 0.5
        } else {
            0.0
        };

        overlay.set_active(true);
        overlay.set_visual_opacity(opacity);
        overlay.set_assembly(assembly);
        overlay.set_level(
            (self.display_level + processing_pulse * 0.12 + success_burst * 0.22).clamp(0.0, 1.0),
        );
        overlay.set_phase(self.phase);
        overlay.set_orb_scale(
            (0.58 + assembly * 0.42 + self.display_level * 0.12 + success_burst * 0.16)
                .clamp(0.5, 1.28),
        );
        overlay.set_wave_one(self.phase);
        overlay.set_wave_two((self.phase + 0.34) % 1.0);
        overlay.set_wave_three((self.phase + 0.67) % 1.0);
        overlay.set_processing(self.stage == Stage::Processing);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rust_app_core::product::migrate_legacy_data_root();
    let _ = rust_app_core::diagnostics::initialize(
        &rust_app_core::product::data_root().join("logs"),
        "companion-renderer",
    );
    rust_app_core::diagnostics::log_line("companion_renderer lifecycle=starting");
    let overlay = CompanionOverlay::new()?;
    let state = std::rc::Rc::new(RefCell::new(RendererState::default()));
    let animation_timer = std::rc::Rc::new(Timer::default());
    let window_handle = Arc::new(AtomicU64::new(0));
    let reveal_after_frame = Arc::new(AtomicBool::new(false));

    {
        let window_handle = Arc::clone(&window_handle);
        let reveal_after_frame = Arc::clone(&reveal_after_frame);
        overlay
            .window()
            .set_rendering_notifier(move |rendering_state, _| {
                if !matches!(rendering_state, RenderingState::AfterRendering)
                    || !reveal_after_frame.swap(false, Ordering::AcqRel)
                {
                    return;
                }
                let handle = window_handle.load(Ordering::Acquire);
                if handle != 0 {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Err(error) = rust_platform_windows::reveal_overlay_window(handle) {
                            rust_app_core::diagnostics::log_line(&format!(
                                "companion_renderer transition=failed stage=reveal error={error}"
                            ));
                        }
                    });
                }
            })
            .map_err(|error| format!("could not install render acknowledgement: {error:?}"))?;
    }

    overlay.set_active(false);
    overlay
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    overlay.show()?;

    {
        let weak = overlay.as_weak();
        let state = std::rc::Rc::clone(&state);
        let window_handle = Arc::clone(&window_handle);
        let reveal_after_frame = Arc::clone(&reveal_after_frame);
        slint::spawn_local(async move {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            let result = match overlay.window().winit_window().await {
                Ok(window) => native_window_handle(&window)
                    .ok_or_else(|| "native HWND is unavailable".to_owned())
                    .and_then(|handle| {
                        rust_platform_windows::prepare_transparent_overlay_window(handle)?;
                        window_handle.store(handle, Ordering::Release);
                        if let Some(target) = state.borrow().target_window_handle {
                            rust_platform_windows::configure_transparent_overlay_window(
                                handle,
                                Some(target),
                            )?;
                            reveal_after_frame.store(true, Ordering::Release);
                            overlay.window().request_redraw();
                        }
                        Ok(())
                    }),
                Err(error) => Err(format!("native window creation failed: {error}")),
            };
            match result {
                Ok(()) => write_event(&CompanionRendererEvent::Ready {
                    protocol_version: PROTOCOL_VERSION,
                }),
                Err(error) => write_event(&CompanionRendererEvent::Failed {
                    session_id: None,
                    code: "overlay_initialization_failed".to_owned(),
                    message: error,
                }),
            }
        })?;
    }

    {
        let weak = overlay.as_weak();
        let state = std::rc::Rc::clone(&state);
        let window_handle = Arc::clone(&window_handle);
        let reveal_after_frame = Arc::clone(&reveal_after_frame);
        let animation_timer = std::rc::Rc::clone(&animation_timer);
        COMMAND_HANDLER.with(|handler| {
            *handler.borrow_mut() = Some(Box::new(move |command| {
                let Some(overlay) = weak.upgrade() else {
                    return;
                };
                let state_owner = std::rc::Rc::clone(&state);
                let mut state = state.borrow_mut();
                match command {
                    CompanionRendererCommand::Activate {
                        session_id,
                        target_window_handle: target,
                    } => {
                        state.session_id = Some(session_id);
                        state.target_window_handle = Some(target);
                        state.target_level = 0.0;
                        state.display_level = 0.0;
                        state.enter(Stage::Assembling);
                        overlay.set_active(true);
                        overlay.set_visual_opacity(0.0);
                        start_animation_timer(
                            &animation_timer,
                            overlay.as_weak(),
                            state_owner,
                            Arc::clone(&window_handle),
                        );
                        let handle = window_handle.load(Ordering::Acquire);
                        if handle != 0 {
                            match rust_platform_windows::configure_transparent_overlay_window(
                                handle,
                                Some(target),
                            ) {
                                Ok(()) => {
                                    reveal_after_frame.store(true, Ordering::Release);
                                    overlay.window().request_redraw();
                                }
                                Err(error) => write_event(&CompanionRendererEvent::Failed {
                                    session_id: Some(session_id),
                                    code: "overlay_configuration_failed".to_owned(),
                                    message: error,
                                }),
                            }
                        }
                    }
                    CompanionRendererCommand::AudioLevel {
                        session_id, peak, ..
                    } if state.accepts(session_id) => {
                        state.target_level = (peak.clamp(0.0, 1.0).sqrt() * 1.75).clamp(0.0, 1.0);
                    }
                    CompanionRendererCommand::Processing { session_id }
                        if state.accepts(session_id) =>
                    {
                        state.target_level = 0.12;
                        state.enter(Stage::Processing);
                    }
                    CompanionRendererCommand::Complete { session_id }
                        if state.accepts(session_id) =>
                    {
                        state.target_level = 0.0;
                        state.enter(Stage::Success);
                    }
                    CompanionRendererCommand::Dismiss { session_id }
                        if session_id.is_none() || session_id == state.session_id =>
                    {
                        state.target_level = 0.0;
                        if state.stage != Stage::Hidden {
                            state.enter(Stage::Dissolving);
                        }
                    }
                    CompanionRendererCommand::Shutdown => {
                        rust_app_core::diagnostics::log_line(
                            "companion_renderer lifecycle=shutdown_requested",
                        );
                        animation_timer.stop();
                        let _ = slint::quit_event_loop();
                    }
                    _ => {}
                }
            }));
        });
    }

    std::thread::Builder::new()
        .name("companion-renderer-commands".to_owned())
        .spawn(|| {
            for line in BufReader::new(std::io::stdin()).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        write_event(&CompanionRendererEvent::Failed {
                            session_id: None,
                            code: "command_stream_failed".to_owned(),
                            message: error.to_string(),
                        });
                        break;
                    }
                };
                match serde_json::from_str::<CompanionRendererCommand>(&line) {
                    Ok(command) => dispatch_command(command),
                    Err(error) => write_event(&CompanionRendererEvent::Failed {
                        session_id: None,
                        code: "invalid_command".to_owned(),
                        message: error.to_string(),
                    }),
                }
            }
            dispatch_command(CompanionRendererCommand::Shutdown);
        })?;

    slint::run_event_loop()?;
    COMMAND_HANDLER.with(|handler| handler.borrow_mut().take());
    rust_app_core::diagnostics::log_line("companion_renderer lifecycle=exiting");
    Ok(())
}

fn start_animation_timer(
    timer: &std::rc::Rc<Timer>,
    overlay: slint::Weak<CompanionOverlay>,
    state: std::rc::Rc<RefCell<RendererState>>,
    window_handle: Arc<AtomicU64>,
) {
    let timer_weak = std::rc::Rc::downgrade(timer);
    timer.start(TimerMode::Repeated, FRAME_INTERVAL, move || {
        let Some(overlay) = overlay.upgrade() else {
            return;
        };
        let hidden = {
            let mut state = state.borrow_mut();
            state.tick(&overlay, window_handle.load(Ordering::Acquire));
            state.stage == Stage::Hidden
        };
        if hidden && let Some(timer) = timer_weak.upgrade() {
            timer.stop();
        }
    });
}

fn dispatch_command(command: CompanionRendererCommand) {
    if let Err(error) = slint::invoke_from_event_loop(move || {
        COMMAND_HANDLER.with(|handler| {
            if let Some(handler) = handler.borrow_mut().as_mut() {
                handler(command);
            }
        });
    }) {
        rust_app_core::diagnostics::log_line(&format!(
            "companion_renderer command_dispatch=failed error={error}"
        ));
    }
}

fn write_event(event: &CompanionRendererEvent) {
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, event).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}

fn native_window_handle(window: &slint::winit_030::winit::window::Window) -> Option<u64> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as u64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{RendererState, Stage};
    use rust_protocol::SessionId;

    #[test]
    fn commands_are_scoped_to_the_active_dictation() {
        let active = SessionId::new();
        let stale = SessionId::new();
        let mut state = RendererState {
            session_id: Some(active),
            ..RendererState::default()
        };
        state.enter(Stage::Listening);

        assert!(state.accepts(active));
        assert!(!state.accepts(stale));
    }
}
