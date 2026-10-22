//! Opt-in settings-preview experiment. Never installed on the real app window.

use std::{cell::RefCell, fmt::Write, path::PathBuf, rc::Rc, time::Instant};

use slint::{
    ComponentHandle, RenderingState,
    winit_030::{
        EventResult, WinitWindowAccessor,
        winit::{
            dpi::PhysicalPosition,
            event::{ElementState, MouseScrollDelta, TouchPhase, WindowEvent},
            keyboard::{KeyCode, PhysicalKey},
        },
    },
};

use crate::{SettingsWindow, wheel_scroll::WheelScroll};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Mode {
    #[default]
    Native,
    Direct,
}

impl Mode {
    fn code(self) -> &'static str {
        match self {
            Self::Native => "A",
            Self::Direct => "B",
        }
    }

    fn show(self, settings: &SettingsWindow) {
        let description = match self {
            Self::Native => "A — штатная анимация, 180 мс",
            Self::Direct => "B — мгновенные шаги колеса",
        };
        settings.set_runtime_status(
            match self {
                Self::Native => "A · текущая",
                Self::Direct => "B · без анимации",
            }
            .into(),
        );
        settings.set_today(format!("Режим {} · F6: переключить", self.code()).into());
        settings.set_indicator_status("Тестовый список · без загрузок".into());
        // Slint owns the native title and reapplies its binding on updates.
        settings.set_window_title(format!("SayFlex Scroll A/B — {description}").into());
    }
}

#[derive(Default)]
struct Comparison {
    mode: Mode,
    wheel: WheelScroll,
}

impl Comparison {
    fn handle_key(
        &mut self,
        settings: &SettingsWindow,
        key: KeyCode,
        state: ElementState,
        repeat: bool,
    ) -> EventResult {
        if key != KeyCode::F6 {
            return EventResult::Propagate;
        }
        if state == ElementState::Pressed && !repeat {
            self.mode = match self.mode {
                Mode::Native => Mode::Direct,
                Mode::Direct => Mode::Native,
            };
            self.mode.show(settings);
        }
        EventResult::PreventDefault
    }

    fn handle_event(&mut self, settings: &SettingsWindow, event: &WindowEvent) -> EventResult {
        self.handle_event_at(settings, event, Instant::now())
    }

    fn handle_event_at(
        &mut self,
        settings: &SettingsWindow,
        event: &WindowEvent,
        now: Instant,
    ) -> EventResult {
        if let WindowEvent::KeyboardInput { event, .. } = event
            && let PhysicalKey::Code(key) = event.physical_key
        {
            return self.handle_key(settings, key, event.state, event.repeat);
        }
        match self.mode {
            Mode::Native => {
                self.wheel.observe_native_event_at(event, now);
                EventResult::Propagate
            }
            Mode::Direct => self.wheel.handle_event_at(settings, event, now),
        }
    }
}

const MAX_TRACE_SAMPLES: usize = 20_000;

struct TraceSample {
    elapsed_us: u128,
    event: &'static str,
    mode: Mode,
    delta_kind: &'static str,
    delta_x: f64,
    delta_y: f64,
    models_y: f32,
    workspace_y: f32,
    active_page: i32,
    cursor_physical: Option<PhysicalPosition<f64>>,
    scale_factor: f32,
    phase: &'static str,
    intercepted: Option<bool>,
}

struct WheelTrace<'a> {
    arrived_us: u128,
    delta: &'a MouseScrollDelta,
    phase: TouchPhase,
    intercepted: bool,
}

struct Trace {
    started: Instant,
    samples: Vec<TraceSample>,
    dropped: usize,
}

impl Trace {
    fn record(
        &mut self,
        event: &'static str,
        comparison: &Comparison,
        wheel: Option<WheelTrace<'_>>,
        settings: &SettingsWindow,
    ) {
        if self.samples.len() >= MAX_TRACE_SAMPLES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let (delta_kind, delta_x, delta_y) = match wheel.as_ref().map(|wheel| wheel.delta) {
            Some(MouseScrollDelta::LineDelta(x, y)) => ("line", f64::from(*x), f64::from(*y)),
            Some(MouseScrollDelta::PixelDelta(at)) => ("pixel", at.x, at.y),
            None => ("none", 0.0, 0.0),
        };
        let phase = wheel.as_ref().map_or("", |wheel| match wheel.phase {
            TouchPhase::Started => "started",
            TouchPhase::Moved => "moved",
            TouchPhase::Ended => "ended",
            TouchPhase::Cancelled => "cancelled",
        });
        self.samples.push(TraceSample {
            // Wheel timestamps mark arrival; positions are read after routing.
            elapsed_us: wheel.as_ref().map_or_else(
                || self.started.elapsed().as_micros(),
                |wheel| wheel.arrived_us,
            ),
            event,
            mode: comparison.mode,
            delta_kind,
            delta_x,
            delta_y,
            models_y: settings.get_models_scroll_y(),
            // On list pages the sidebar has a separate, private offset. This
            // field is explicitly the outer workspace, not that sidebar.
            workspace_y: settings.get_workspace_scroll_y(),
            active_page: settings.get_active_page(),
            cursor_physical: comparison.wheel.cursor_position(),
            scale_factor: settings.window().scale_factor(),
            phase,
            intercepted: wheel.map(|wheel| wheel.intercepted),
        });
    }
}

/// Writes only at preview shutdown. No file operations occur in input or render
/// callbacks. AfterRendering is a toolkit callback, not display-present timing.
pub(super) struct TraceGuard {
    trace: Option<Rc<RefCell<Trace>>>,
    path: Option<PathBuf>,
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        let (Some(trace), Some(path)) = (&self.trace, &self.path) else {
            return;
        };
        let trace = trace.borrow();
        let mut csv = String::from(
            "elapsed_us,event,mode,delta_kind,delta_x,delta_y,models_scroll_y,workspace_scroll_y,active_page,cursor_physical_x,cursor_physical_y,scale_factor,dropped_samples,phase,intercepted\n",
        );
        for sample in &trace.samples {
            let x = sample
                .cursor_physical
                .map(|at| at.x.to_string())
                .unwrap_or_default();
            let y = sample
                .cursor_physical
                .map(|at| at.y.to_string())
                .unwrap_or_default();
            let intercepted = match sample.intercepted {
                Some(true) => "true",
                Some(false) => "false",
                None => "",
            };
            let _ = writeln!(
                csv,
                "{},{},{},{},{},{},{},{},{},{},{},{},0,{},{}",
                sample.elapsed_us,
                sample.event,
                sample.mode.code(),
                sample.delta_kind,
                sample.delta_x,
                sample.delta_y,
                sample.models_y,
                sample.workspace_y,
                sample.active_page,
                x,
                y,
                sample.scale_factor,
                sample.phase,
                intercepted,
            );
        }
        let mut end = vec![String::new(); 15];
        end[0] = trace.started.elapsed().as_micros().to_string();
        end[1] = "trace_end".into();
        end[12] = trace.dropped.to_string();
        let _ = writeln!(csv, "{}", end.join(","));
        if let Err(error) = std::fs::write(path, csv) {
            eprintln!(
                "scroll comparison trace could not be saved to {}: {error}",
                path.display()
            );
        }
    }
}

pub(super) fn install(settings: &SettingsWindow, path: Option<PathBuf>) -> TraceGuard {
    let comparison = Rc::new(RefCell::new(Comparison::default()));
    comparison.borrow().mode.show(settings);
    let trace = path.as_ref().map(|_| {
        Rc::new(RefCell::new(Trace {
            started: Instant::now(),
            samples: Vec::with_capacity(MAX_TRACE_SAMPLES),
            dropped: 0,
        }))
    });
    if let Some(trace) = trace.as_ref() {
        let trace = Rc::clone(trace);
        let comparison = Rc::clone(&comparison);
        let weak = settings.as_weak();
        if let Err(error) = settings.window().set_rendering_notifier(move |state, _| {
            if matches!(state, RenderingState::AfterRendering)
                && let Some(settings) = weak.upgrade()
            {
                trace
                    .borrow_mut()
                    .record("after_rendering", &comparison.borrow(), None, &settings);
            }
        }) {
            eprintln!("scroll comparison render timestamps unavailable: {error}");
        }
    }
    let event_trace = trace.clone();
    let weak = settings.as_weak();
    settings.window().on_winit_window_event(move |_, event| {
        let Some(settings) = weak.upgrade() else {
            return EventResult::Propagate;
        };
        let mut comparison = comparison.borrow_mut();
        let before = comparison.mode;
        let arrived_us = event_trace
            .as_ref()
            .map(|trace| trace.borrow().started.elapsed().as_micros());
        let result = comparison.handle_event(&settings, event);
        if let (Some(trace), Some(arrived_us), WindowEvent::MouseWheel { delta, phase, .. }) =
            (&event_trace, arrived_us, event)
        {
            trace.borrow_mut().record(
                "wheel_after_handling",
                &comparison,
                Some(WheelTrace {
                    arrived_us,
                    delta,
                    phase: *phase,
                    intercepted: matches!(result, EventResult::PreventDefault),
                }),
                &settings,
            );
        }
        if before != comparison.mode
            && let Some(trace) = &event_trace
        {
            trace
                .borrow_mut()
                .record("mode_change", &comparison, None, &settings);
        }
        result
    });
    TraceGuard { trace, path }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_backend_testing::ElementHandle;
    use slint::winit_030::winit::event::DeviceId;
    use slint::{LogicalPosition, LogicalSize};
    use std::time::Duration;

    fn fixture() -> (SettingsWindow, Comparison) {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
        let settings = SettingsWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        super::super::scroll_comparison_models::fill(
            &settings,
            crate::localization::UiLocale::English,
        )
        .unwrap();
        settings.set_active_page(3);
        settings.show().unwrap();
        settings.window().set_size(LogicalSize::new(1000.0, 420.0));
        settle(&settings);
        (settings, Comparison::default())
    }

    fn settle(settings: &SettingsWindow) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        slint::platform::update_timers_and_animations();
        settings.window().take_snapshot().unwrap();
    }

    fn bound_title(settings: &SettingsWindow) -> slint::SharedString {
        i_slint_core::window::WindowInner::from_pub(settings.window())
            .window_item()
            .unwrap()
            .as_pin_ref()
            .title()
    }

    fn center(item: ElementHandle) -> LogicalPosition {
        let at = item.absolute_position();
        let size = item.size();
        LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
    }

    fn move_to(settings: &SettingsWindow, state: &mut Comparison, at: LogicalPosition) {
        let scale = f64::from(settings.window().scale_factor());
        state.handle_event(
            settings,
            &WindowEvent::CursorMoved {
                device_id: DeviceId::dummy(),
                position: PhysicalPosition::new(f64::from(at.x) * scale, f64::from(at.y) * scale),
            },
        );
    }

    fn wheel(
        settings: &SettingsWindow,
        state: &mut Comparison,
        delta: MouseScrollDelta,
        phase: TouchPhase,
    ) -> EventResult {
        wheel_at(settings, state, delta, phase, Instant::now())
    }

    fn wheel_at(
        settings: &SettingsWindow,
        state: &mut Comparison,
        delta: MouseScrollDelta,
        phase: TouchPhase,
        now: Instant,
    ) -> EventResult {
        state.handle_event_at(
            settings,
            &WindowEvent::MouseWheel {
                device_id: DeviceId::dummy(),
                delta,
                phase,
            },
            now,
        )
    }

    fn toggle(settings: &SettingsWindow, state: &mut Comparison) {
        assert!(matches!(
            state.handle_key(settings, KeyCode::F6, ElementState::Pressed, false),
            EventResult::PreventDefault
        ));
    }

    #[test]
    fn f6_changes_visible_mode_and_input_without_resetting_position() {
        let (settings, mut state) = fixture();
        assert_eq!(
            bound_title(&settings),
            "SayFlex",
            "ordinary title keeps its default"
        );
        state.mode.show(&settings);
        settle(&settings);
        assert!(bound_title(&settings).starts_with("SayFlex Scroll A/B — A"));
        let at = center(
            ElementHandle::find_by_element_id(&settings, "ModelsPage::list")
                .next()
                .unwrap(),
        );
        move_to(&settings, &mut state, at);
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved
            ),
            EventResult::Propagate
        ));
        assert_eq!(settings.get_models_scroll_y(), 0.0);
        toggle(&settings, &mut state);
        assert_eq!(state.mode, Mode::Direct);
        assert!(settings.get_runtime_status().contains('B'));
        assert!(settings.get_today().contains("F6"));
        // Switching modes must not drop an animation started on A's path.
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved
            ),
            EventResult::Propagate
        ));
        assert_eq!(settings.get_models_scroll_y(), 0.0);
        assert!(matches!(
            wheel_at(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved,
                Instant::now() + Duration::from_secs(1)
            ),
            EventResult::PreventDefault
        ));
        assert!((settings.get_models_scroll_y() + 60.0).abs() < 0.1);
        settle(&settings);
        assert!(bound_title(&settings).starts_with("SayFlex Scroll A/B — B"));
        let at = settings.get_models_scroll_y();
        state.handle_key(&settings, KeyCode::F6, ElementState::Pressed, true);
        assert_eq!(
            state.mode,
            Mode::Direct,
            "key repeat must not switch modes repeatedly"
        );
        toggle(&settings, &mut state);
        assert_eq!(state.mode, Mode::Native);
        assert!(settings.get_runtime_status().contains('A'));
        assert_eq!(settings.get_models_scroll_y(), at);
        settle(&settings);
        assert!(bound_title(&settings).starts_with("SayFlex Scroll A/B — A"));
    }

    #[test]
    fn direct_wheel_uses_native_hit_testing_for_left_right_header_and_footer() {
        let (settings, mut state) = fixture();
        toggle(&settings, &mut state);
        settings.set_models_scroll_y(-60.0);
        settle(&settings);
        let left = center(
            ElementHandle::find_by_accessible_label(&settings, "Overview")
                .next()
                .unwrap(),
        );
        move_to(&settings, &mut state, left);
        for _ in 0..12 {
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved,
            );
        }
        settle(&settings);
        assert_eq!(settings.get_models_scroll_y(), -60.0);
        let about = ElementHandle::find_by_accessible_label(&settings, "About")
            .next()
            .expect("left menu must reach About")
            .absolute_position()
            .y;
        let right = center(
            ElementHandle::find_by_element_id(&settings, "ModelsPage::list")
                .next()
                .unwrap(),
        );
        move_to(&settings, &mut state, right);
        wheel(
            &settings,
            &mut state,
            MouseScrollDelta::LineDelta(0.0, -1.0),
            TouchPhase::Moved,
        );
        settle(&settings);
        assert!(settings.get_models_scroll_y() < -60.0);
        assert_eq!(
            ElementHandle::find_by_accessible_label(&settings, "About")
                .next()
                .unwrap()
                .absolute_position()
                .y,
            about
        );
        let offset = settings.get_models_scroll_y();
        let fixed_actions: Vec<_> =
            ElementHandle::find_by_accessible_label(&settings, "Quick translation").collect();
        assert_eq!(fixed_actions.len(), 2);
        for action in fixed_actions {
            move_to(&settings, &mut state, center(action));
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved,
            );
            settle(&settings);
            assert_eq!(settings.get_models_scroll_y(), offset);
        }
    }

    #[test]
    fn fractional_pixels_horizontal_and_gesture_streams_remain_native() {
        let (settings, mut state) = fixture();
        toggle(&settings, &mut state);
        let at = center(
            ElementHandle::find_by_element_id(&settings, "ModelsPage::list")
                .next()
                .unwrap(),
        );
        move_to(&settings, &mut state, at);
        for delta in [
            MouseScrollDelta::LineDelta(0.0, -0.25),
            MouseScrollDelta::LineDelta(1.0, -1.0),
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -17.5)),
        ] {
            assert!(matches!(
                wheel(&settings, &mut state, delta, TouchPhase::Moved),
                EventResult::Propagate
            ));
        }
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, 0.0),
                TouchPhase::Started
            ),
            EventResult::Propagate
        ));
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved
            ),
            EventResult::Propagate
        ));
        wheel(
            &settings,
            &mut state,
            MouseScrollDelta::LineDelta(0.0, 0.0),
            TouchPhase::Ended,
        );
        assert_eq!(
            settings.get_models_scroll_y(),
            0.0,
            "pass-through must not dispatch a second copy"
        );
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved
            ),
            EventResult::Propagate
        ));
        assert_eq!(settings.get_models_scroll_y(), 0.0);
        assert!(matches!(
            wheel_at(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved,
                Instant::now() + Duration::from_secs(1)
            ),
            EventResult::PreventDefault
        ));
        assert!((settings.get_models_scroll_y() + 60.0).abs() < 0.1);
    }

    #[test]
    fn current_scale_and_boundaries_are_owned_by_slint() {
        let (settings, mut state) = fixture();
        toggle(&settings, &mut state);
        settings
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
        // TestingWindow::set_size uses scale1 for both its backing buffer and
        // Resized dispatch. Resize the physical buffer, then deliver the actual
        // logical size as a native DPI transition would, before rendering.
        settings
            .window()
            .set_size(slint::PhysicalSize::new(2000, 840));
        settings
            .window()
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: LogicalSize::new(1000.0, 420.0),
            });
        assert_eq!(
            settings.window().size(),
            slint::PhysicalSize::new(2000, 840)
        );
        assert_eq!(settings.window().scale_factor(), 2.0);
        settle(&settings);
        let at = center(
            ElementHandle::find_by_element_id(&settings, "ModelsPage::list")
                .next()
                .unwrap(),
        );
        move_to(&settings, &mut state, at);
        wheel(
            &settings,
            &mut state,
            MouseScrollDelta::LineDelta(0.0, 1.0),
            TouchPhase::Moved,
        );
        assert_eq!(settings.get_models_scroll_y(), 0.0);
        wheel(
            &settings,
            &mut state,
            MouseScrollDelta::LineDelta(0.0, -1.0),
            TouchPhase::Moved,
        );
        assert!((settings.get_models_scroll_y() + 60.0).abs() < 0.1);
        for _ in 0..40 {
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, -1.0),
                TouchPhase::Moved,
            );
            settings.window().take_snapshot().unwrap();
        }
        let lower =
            settings.get_models_scroll_shown_height() - settings.get_models_scroll_content_height();
        assert!(settings.get_models_scroll_y() >= lower - 1.0);
        state.handle_event(
            &settings,
            &WindowEvent::CursorLeft {
                device_id: DeviceId::dummy(),
            },
        );
        assert!(matches!(
            wheel(
                &settings,
                &mut state,
                MouseScrollDelta::LineDelta(0.0, 1.0),
                TouchPhase::Moved
            ),
            EventResult::Propagate
        ));
    }

    #[test]
    fn trace_stops_at_its_bound_without_discarding_the_first_mode() {
        let (settings, _) = fixture();
        let mut trace = Trace {
            started: Instant::now(),
            samples: Vec::new(),
            dropped: 0,
        };
        let mut comparison = Comparison::default();
        trace.record("mode_change", &comparison, None, &settings);
        comparison.mode = Mode::Direct;
        for _ in 0..MAX_TRACE_SAMPLES + 1 {
            trace.record("after_rendering", &comparison, None, &settings);
        }
        assert_eq!(trace.samples.len(), MAX_TRACE_SAMPLES);
        assert_eq!(trace.samples[0].mode, Mode::Native);
        assert_eq!(trace.dropped, 2);
    }
}
