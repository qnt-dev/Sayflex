//! Direct whole-detent scrolling through Slint's native hit-testing.

use std::time::{Duration, Instant};

use slint::{
    ComponentHandle, LogicalPosition,
    winit_030::{
        EventResult,
        winit::{
            dpi::PhysicalPosition,
            event::{MouseScrollDelta, TouchPhase, WindowEvent},
        },
    },
};

use crate::SettingsWindow;

// Slint 1.17's unphased wheel animation lasts 180 ms. Leave a little margin
// before dispatching Cancelled, which would discard any remaining distance.
// This is an input-time gate, not a timer or a touchpad-fling duration estimate.
const NATIVE_WHEEL_SETTLE_TIME: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(crate) struct WheelScroll {
    cursor: Option<PhysicalPosition<f64>>,
    gesture_active: bool,
    last_native_wheel_at: Option<Instant>,
}

impl WheelScroll {
    pub(crate) fn cursor_position(&self) -> Option<PhysicalPosition<f64>> {
        self.cursor
    }

    pub(crate) fn handle_event(
        &mut self,
        settings: &SettingsWindow,
        event: &WindowEvent,
    ) -> EventResult {
        self.handle_event_at(settings, event, Instant::now())
    }

    /// Observe the native A path too, so switching to B cannot cut a live tail.
    pub(crate) fn observe_native_event_at(&mut self, event: &WindowEvent, now: Instant) {
        self.track_input(event);
        if matches!(event, WindowEvent::MouseWheel { .. }) {
            self.last_native_wheel_at = Some(now);
        }
    }

    fn track_input(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => self.cursor = Some(*position),
            WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false) => {
                self.cursor = None;
                self.gesture_active = false;
                // A cursor/focus transition does not finish a native animation.
            }
            WindowEvent::MouseWheel { phase, .. } => match phase {
                TouchPhase::Started => self.gesture_active = true,
                TouchPhase::Ended | TouchPhase::Cancelled => self.gesture_active = false,
                TouchPhase::Moved => {}
            },
            _ => {}
        }
    }

    pub(crate) fn handle_event_at(
        &mut self,
        settings: &SettingsWindow,
        event: &WindowEvent,
        now: Instant,
    ) -> EventResult {
        self.track_input(event);
        let WindowEvent::MouseWheel { delta, phase, .. } = event else {
            return EventResult::Propagate;
        };
        // Windows can report touchpad input as fractional LineDelta too.
        // Never round that input or take over horizontal/gesture scrolling.
        let whole_vertical = match delta {
            MouseScrollDelta::LineDelta(x, y)
                if *x == 0.0 && y.is_finite() && *y != 0.0 && y.fract() == 0.0 =>
            {
                Some(*y)
            }
            _ => None,
        };
        let native_tail = self.last_native_wheel_at.is_some_and(|last| {
            now.checked_duration_since(last)
                .is_none_or(|elapsed| elapsed <= NATIVE_WHEEL_SETTLE_TIME)
        });
        let direct = self
            .cursor
            .zip(whole_vertical)
            .filter(|_| !self.gesture_active && *phase == TouchPhase::Moved && !native_tail);
        let Some((cursor, lines)) = direct else {
            // Every passed-through wheel may start/extend native motion, even
            // a whole detent received while the previous tail is protected.
            self.last_native_wheel_at = Some(now);
            return EventResult::Propagate;
        };
        let cursor = cursor.to_logical::<f64>(f64::from(settings.window().scale_factor()));
        let position = LogicalPosition::new(cursor.x as f32, cursor.y as f32);
        // Winit flushes its pending move after this filter. Supply the latest
        // position first; Slint then hit-tests and clamps the scroll itself.
        settings
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        // Slint 1.17 maps this public event to Cancelled: immediate scrolling.
        settings
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: lines * 60.0,
            });
        EventResult::PreventDefault
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_backend_testing::ElementHandle;
    use i_slint_core::{
        input::{MouseEvent, TouchPhase as SlintTouchPhase},
        lengths::LogicalPoint,
        window::WindowInner,
    };
    use slint::winit_030::winit::event::DeviceId;
    use slint::{LogicalSize, ModelRc, VecModel};
    use std::time::Duration;

    fn fixture() -> (SettingsWindow, WheelScroll, LogicalPosition, Instant) {
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
        settings.set_active_page(6);
        settings.set_history_rows(ModelRc::new(VecModel::from(
            (0..100)
                .map(|index| crate::HistoryRow {
                    text: format!("fixture row {index}").into(),
                    detail: "fixture detail".into(),
                    origin: "local".into(),
                })
                .collect::<Vec<_>>(),
        )));
        settings.show().unwrap();
        settings.window().set_size(LogicalSize::new(1000.0, 600.0));
        advance(&settings, 250);
        let item = ElementHandle::find_by_element_id(&settings, "HistoryPage::list")
            .next()
            .unwrap();
        let origin = item.absolute_position();
        let size = item.size();
        let at = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let now = Instant::now();
        let mut router = WheelScroll::default();
        router.handle_event_at(&settings, &cursor(at), now);
        settings
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at });
        (settings, router, at, now)
    }

    fn cursor(at: LogicalPosition) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: PhysicalPosition::new(f64::from(at.x), f64::from(at.y)),
        }
    }

    fn event(y: f32) -> WindowEvent {
        WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, y),
            phase: TouchPhase::Moved,
        }
    }

    fn advance(settings: &SettingsWindow, milliseconds: u64) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
        slint::platform::update_timers_and_animations();
        settings.window().take_snapshot().unwrap();
    }

    // Match Winit's pass-through path; public PointerScrolled would incorrectly
    // turn the native side into the immediate Cancelled path too.
    fn wheel(
        settings: &SettingsWindow,
        router: &mut WheelScroll,
        at: LogicalPosition,
        now: Instant,
        y: f32,
    ) -> bool {
        let intercepted = matches!(
            router.handle_event_at(settings, &event(y), now),
            EventResult::PreventDefault
        );
        if !intercepted {
            WindowInner::from_pub(settings.window()).process_mouse_input(MouseEvent::Wheel {
                position: LogicalPoint::new(at.x, at.y),
                delta_x: 0.0,
                delta_y: y * 60.0,
                phase: SlintTouchPhase::Moved,
            });
        }
        intercepted
    }

    #[test]
    fn whole_detents_move_before_any_animation_frame() {
        let (settings, mut router, at, now) = fixture();
        assert!(wheel(&settings, &mut router, at, now, -1.0));
        assert!((settings.get_history_scroll_y() + 60.0).abs() < 0.1);
        assert!(wheel(&settings, &mut router, at, now, -2.0));
        assert!((settings.get_history_scroll_y() + 180.0).abs() < 0.1);
    }

    #[test]
    fn fractional_animation_tail_survives_a_following_whole_detent() {
        let (settings, mut router, at, now) = fixture();
        assert!(!wheel(&settings, &mut router, at, now, -0.25));
        advance(&settings, 90);
        let partial = settings.get_history_scroll_y();
        assert!(partial < 0.0 && partial > -15.0);
        assert!(!wheel(
            &settings,
            &mut router,
            at,
            now + Duration::from_millis(90),
            -1.0
        ));
        assert!((settings.get_history_scroll_y() - partial).abs() < 0.1);
        advance(&settings, 250);
        assert!(
            (settings.get_history_scroll_y() + 75.0).abs() < 0.1,
            "both native deltas must complete without losing the first tail"
        );
        assert!(wheel(
            &settings,
            &mut router,
            at,
            now + Duration::from_millis(340),
            -1.0
        ));
        assert!((settings.get_history_scroll_y() + 135.0).abs() < 0.1);
    }

    #[test]
    fn each_native_wheel_extends_the_quiet_period_before_direct_routing() {
        let (settings, mut router, at, now) = fixture();
        // A preview uses this same observation path before switching to B.
        router.observe_native_event_at(&event(-1.0), now);
        for milliseconds in [180, 380] {
            assert!(
                matches!(
                    router.handle_event_at(
                        &settings,
                        &event(-1.0),
                        now + Duration::from_millis(milliseconds)
                    ),
                    EventResult::Propagate
                ),
                "native input must retain the animation path at {milliseconds} ms"
            );
        }
        assert_eq!(
            settings.get_history_scroll_y(),
            0.0,
            "propagation must not dispatch an extra synthetic scroll"
        );
        assert!(wheel(
            &settings,
            &mut router,
            at,
            now + Duration::from_millis(581),
            -1.0
        ));
        assert!((settings.get_history_scroll_y() + 60.0).abs() < 0.1);
    }

    #[test]
    fn cursor_loss_requires_a_fresh_position_and_keeps_the_native_tail_guard() {
        let (settings, mut router, at, now) = fixture();
        router.observe_native_event_at(&event(-0.25), now);
        for (index, loss) in [
            WindowEvent::Focused(false),
            WindowEvent::CursorLeft {
                device_id: DeviceId::dummy(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let base = index as u64 * 100;
            router.handle_event_at(&settings, &loss, now + Duration::from_millis(base + 20));
            assert!(matches!(
                router.handle_event_at(
                    &settings,
                    &event(-1.0),
                    now + Duration::from_millis(base + 30)
                ),
                EventResult::Propagate
            ));
            router.handle_event_at(
                &settings,
                &cursor(at),
                now + Duration::from_millis(base + 40),
            );
            assert!(matches!(
                router.handle_event_at(
                    &settings,
                    &event(-1.0),
                    now + Duration::from_millis(base + 50)
                ),
                EventResult::Propagate
            ));
        }
        assert_eq!(settings.get_history_scroll_y(), 0.0);
        assert!(wheel(
            &settings,
            &mut router,
            at,
            now + Duration::from_millis(351),
            -1.0
        ));
        assert!((settings.get_history_scroll_y() + 60.0).abs() < 0.1);
    }
}
