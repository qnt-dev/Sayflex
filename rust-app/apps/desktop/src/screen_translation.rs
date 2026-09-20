use std::{cell::Cell, rc::Rc, time::Duration};

use rust_platform_windows::{
    region_selection::{MonitorRegion, PointerEvent, RegionSelectionMonitor},
    screen_capture::{DesktopCapture, DesktopCaptureBackend, WindowsDesktopCaptureBackend},
};
use rust_protocol::{InputHostCommand, PhysicalRect, RequestId};
use slint::winit_030::WinitWindowAccessor;
use slint::{
    ComponentHandle, Image, PhysicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, Timer,
    TimerMode,
};

use crate::{
    RuntimeEvent, ScreenRegionOverlay, dispatch_runtime_event,
    overlay_presenter::{OverlayContract, OverlayPresenter},
    worker_supervisor::Workers,
};

const SELECTION_WATCHDOG_SECONDS: u64 = 15;

pub(crate) enum SelectionTerminal {
    Selected {
        request_id: RequestId,
        rect: PhysicalRect,
        capture: DesktopCapture,
    },
    Cancelled {
        request_id: RequestId,
    },
    Failed {
        request_id: RequestId,
        error: String,
    },
}

pub(crate) trait SelectionEnvironment {
    fn capture(&mut self) -> Result<DesktopCapture, String>;
    fn set_capture_mode(&mut self, request_id: RequestId, active: bool) -> Result<(), String>;
    fn show_surfaces(&mut self, capture: &DesktopCapture) -> Result<(), String>;
    fn update_selection(&mut self, selection: Option<PhysicalRect>);
    fn teardown_surfaces(&mut self);
    fn arm_watchdog(&mut self, request_id: RequestId);
    fn disarm_watchdog(&mut self);
    fn desktop_layout_current(&self) -> bool;
}

struct ActiveSelection {
    request_id: RequestId,
    capture: DesktopCapture,
    drag_origin: Option<(i32, i32)>,
}

pub(crate) struct ScreenSelectionCoordinator<E: SelectionEnvironment> {
    environment: E,
    active: Option<ActiveSelection>,
}

impl<E: SelectionEnvironment> ScreenSelectionCoordinator<E> {
    pub(crate) fn new(environment: E) -> Self {
        Self {
            environment,
            active: None,
        }
    }

    pub(crate) fn begin(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        if self.active.is_some() {
            return Some(SelectionTerminal::Failed {
                request_id,
                error: "another screen selection is already active".into(),
            });
        }
        let capture = match self.environment.capture() {
            Ok(capture) => capture,
            Err(error) => {
                return Some(SelectionTerminal::Failed { request_id, error });
            }
        };
        if let Err(error) = self.environment.set_capture_mode(request_id, true) {
            return Some(SelectionTerminal::Failed { request_id, error });
        }
        self.active = Some(ActiveSelection {
            request_id,
            capture,
            drag_origin: None,
        });
        let show_result = {
            let active = self.active.as_ref().expect("selection was just installed");
            self.environment.show_surfaces(&active.capture)
        };
        if let Err(error) = show_result {
            return self.terminate(SelectionOutcome::Failed(error));
        }
        self.environment.arm_watchdog(request_id);
        None
    }

    pub(crate) fn handle_pointer(&mut self, event: PointerEvent) -> Option<SelectionTerminal> {
        if !matches!(event, PointerEvent::Move { .. }) && !self.environment.desktop_layout_current()
        {
            return self.desktop_disconnected();
        }
        let active = self.active.as_mut()?;
        match event {
            PointerEvent::Down { x, y } => {
                active.drag_origin = Some((x, y));
                self.environment.update_selection(None);
                None
            }
            PointerEvent::Move { x, y } => {
                let (origin_x, origin_y) = active.drag_origin?;
                self.environment
                    .update_selection(selection_rect(origin_x, origin_y, x, y));
                None
            }
            PointerEvent::Up { x, y } => {
                let (origin_x, origin_y) = active.drag_origin?;
                match selection_rect(origin_x, origin_y, x, y) {
                    Some(rect) => self.terminate(SelectionOutcome::Selected(rect)),
                    None => self.terminate(SelectionOutcome::Cancelled),
                }
            }
            PointerEvent::RightDown { .. } => None,
            PointerEvent::RightUp { .. } => self.terminate(SelectionOutcome::Cancelled),
        }
    }

    pub(crate) fn cancel(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        (self.active.as_ref()?.request_id == request_id)
            .then(|| self.terminate(SelectionOutcome::Cancelled))?
    }

    pub(crate) fn desktop_disconnected(&mut self) -> Option<SelectionTerminal> {
        self.active.as_ref()?;
        self.terminate(SelectionOutcome::Failed(
            "the desktop layout changed during selection".into(),
        ))
    }

    pub(crate) fn watchdog_expired(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        (self.active.as_ref()?.request_id == request_id).then(|| {
            self.terminate(SelectionOutcome::Failed(format!(
                "screen selection exceeded the {SELECTION_WATCHDOG_SECONDS}-second safety limit"
            )))
        })?
    }

    fn terminate(&mut self, outcome: SelectionOutcome) -> Option<SelectionTerminal> {
        let active = self.active.take()?;
        self.environment.disarm_watchdog();
        self.environment.teardown_surfaces();
        let release_error = self
            .environment
            .set_capture_mode(active.request_id, false)
            .err();
        Some(match (outcome, release_error) {
            (_, Some(error)) => SelectionTerminal::Failed {
                request_id: active.request_id,
                error: format!("could not release screen-selection keyboard capture: {error}"),
            },
            (SelectionOutcome::Selected(rect), None) => SelectionTerminal::Selected {
                request_id: active.request_id,
                rect,
                capture: active.capture,
            },
            (SelectionOutcome::Cancelled, None) => SelectionTerminal::Cancelled {
                request_id: active.request_id,
            },
            (SelectionOutcome::Failed(error), None) => SelectionTerminal::Failed {
                request_id: active.request_id,
                error,
            },
        })
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        self.active.is_some()
    }

    #[cfg(test)]
    fn environment(&self) -> &E {
        &self.environment
    }
}

enum SelectionOutcome {
    Selected(PhysicalRect),
    Cancelled,
    Failed(String),
}

fn selection_rect(start_x: i32, start_y: i32, end_x: i32, end_y: i32) -> Option<PhysicalRect> {
    let left = start_x.min(end_x);
    let top = start_y.min(end_y);
    let width = (i64::from(start_x) - i64::from(end_x)).unsigned_abs();
    let height = (i64::from(start_y) - i64::from(end_y)).unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }
    Some(PhysicalRect {
        x: left,
        y: top,
        width: width as u32,
        height: height as u32,
    })
}

pub(crate) type DesktopScreenSelectionCoordinator =
    ScreenSelectionCoordinator<DesktopSelectionEnvironment>;

pub(crate) struct DesktopSelectionEnvironment {
    workers: Rc<Workers>,
    capture_backend: WindowsDesktopCaptureBackend,
    surfaces: Result<ScreenSelectionSurfaces, String>,
    pointer_monitor: Option<RegionSelectionMonitor>,
    watchdog: Timer,
}

impl DesktopSelectionEnvironment {
    pub(crate) fn new(workers: Rc<Workers>) -> Self {
        let surfaces = rust_platform_windows::region_selection::attached_monitor_regions()
            .and_then(ScreenSelectionSurfaces::new);
        if let Err(error) = &surfaces {
            rust_app_core::diagnostics::log_line(&format!(
                "screen_translation stage=surface_precreate result=failed error={error}"
            ));
        }
        Self {
            workers,
            capture_backend: WindowsDesktopCaptureBackend::default(),
            surfaces,
            pointer_monitor: None,
            watchdog: Timer::default(),
        }
    }
}

impl SelectionEnvironment for DesktopSelectionEnvironment {
    fn capture(&mut self) -> Result<DesktopCapture, String> {
        self.capture_backend
            .capture()
            .map_err(|error| error.to_string())
    }

    fn set_capture_mode(&mut self, request_id: RequestId, active: bool) -> Result<(), String> {
        self.workers
            .input
            .send(&InputHostCommand::SetScreenCaptureMode { request_id, active })
            .map_err(|error| error.to_string())
    }

    fn show_surfaces(&mut self, capture: &DesktopCapture) -> Result<(), String> {
        self.pointer_monitor = Some(RegionSelectionMonitor::start(|event| {
            dispatch_runtime_event(RuntimeEvent::ScreenSelectionPointer(event));
        })?);
        let current_regions = rust_platform_windows::region_selection::attached_monitor_regions()?;
        let surfaces = self.surfaces.as_ref().map_err(Clone::clone)?;
        surfaces.verify_regions(&current_regions)?;
        surfaces.present(capture)
    }

    fn update_selection(&mut self, selection: Option<PhysicalRect>) {
        if let Ok(surfaces) = &self.surfaces {
            surfaces.update_selection(selection);
        }
    }

    fn teardown_surfaces(&mut self) {
        self.pointer_monitor.take();
        if let Ok(surfaces) = &self.surfaces {
            surfaces.retire();
        }
    }

    fn arm_watchdog(&mut self, request_id: RequestId) {
        self.watchdog.start(
            TimerMode::SingleShot,
            Duration::from_secs(SELECTION_WATCHDOG_SECONDS),
            move || {
                dispatch_runtime_event(RuntimeEvent::ScreenSelectionWatchdog { request_id });
            },
        );
    }

    fn disarm_watchdog(&mut self) {
        self.watchdog.stop();
    }

    fn desktop_layout_current(&self) -> bool {
        let Ok(current) = rust_platform_windows::region_selection::attached_monitor_regions()
        else {
            return false;
        };
        self.surfaces
            .as_ref()
            .is_ok_and(|surfaces| surfaces.regions_match(&current))
    }
}

struct ScreenSelectionSurface {
    region: MonitorRegion,
    overlay: Rc<ScreenRegionOverlay>,
    presenter: OverlayPresenter,
    attached: Rc<Cell<bool>>,
    window_handle: Rc<Cell<u64>>,
}

struct ScreenSelectionSurfaces {
    surfaces: Vec<ScreenSelectionSurface>,
}

impl ScreenSelectionSurfaces {
    fn new(regions: Vec<MonitorRegion>) -> Result<Self, String> {
        let mut surfaces = Vec::with_capacity(regions.len());
        for region in regions {
            let overlay = Rc::new(
                ScreenRegionOverlay::new()
                    .map_err(|error| format!("could not create selection overlay: {error}"))?,
            );
            let presenter = OverlayPresenter::install_always_renderable(
                overlay.window(),
                "screen_region_overlay",
                OverlayContract::ClickThrough,
                None,
            )?;
            overlay.set_selection_active(false);
            overlay
                .window()
                .set_position(PhysicalPosition::new(-32_000, -32_000));
            overlay.window().set_size(PhysicalSize::new(1, 1));
            overlay
                .show()
                .map_err(|error| format!("could not precreate selection overlay: {error}"))?;

            let attached = Rc::new(Cell::new(false));
            let window_handle = Rc::new(Cell::new(0));
            precreate_surface(
                &overlay,
                &presenter,
                Rc::clone(&attached),
                Rc::clone(&window_handle),
            );
            surfaces.push(ScreenSelectionSurface {
                region,
                overlay,
                presenter,
                attached,
                window_handle,
            });
        }
        Ok(Self { surfaces })
    }

    fn verify_regions(&self, current: &[MonitorRegion]) -> Result<(), String> {
        if !self.regions_match(current) {
            return Err("the desktop layout changed after selection surfaces were prepared".into());
        }
        if self.surfaces.iter().any(|surface| !surface.attached.get()) {
            return Err("screen-selection surfaces are still preparing".into());
        }
        Ok(())
    }

    fn regions_match(&self, current: &[MonitorRegion]) -> bool {
        self.surfaces
            .iter()
            .map(|surface| surface.region)
            .eq(current.iter().copied())
    }

    fn present(&self, capture: &DesktopCapture) -> Result<(), String> {
        for surface in &self.surfaces {
            if let Err(error) = surface.present(capture) {
                self.retire();
                return Err(error);
            }
        }
        Ok(())
    }

    fn update_selection(&self, selection: Option<PhysicalRect>) {
        for surface in &self.surfaces {
            surface.update_selection(selection);
        }
    }

    fn retire(&self) {
        for surface in &self.surfaces {
            surface.overlay.set_selection_active(false);
            surface
                .presenter
                .retire(surface.overlay.window(), "screen_selection_terminal");
            surface.overlay.set_frozen_image(Image::default());
            surface.overlay.window().set_size(PhysicalSize::new(1, 1));
        }
    }
}

impl ScreenSelectionSurface {
    fn present(&self, capture: &DesktopCapture) -> Result<(), String> {
        let ticket = self.presenter.begin_prepare()?;
        let result = (|| {
            let frame = capture
                .crop(self.region.bounds)
                .map_err(|error| error.to_string())?;
            if frame.stride != frame.rect.width * 4 {
                return Err("screen-selection crop is not tightly packed RGBA".into());
            }
            let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                &frame.pixels,
                frame.rect.width,
                frame.rect.height,
            );
            self.overlay.set_frozen_image(Image::from_rgba8(pixels));
            self.overlay.set_selection_active(false);
            self.overlay.window().set_size(PhysicalSize::new(
                self.region.bounds.width,
                self.region.bounds.height,
            ));
            rust_platform_windows::region_selection::place_region_selection_window(
                self.window_handle.get(),
                self.region.bounds,
            )?;
            self.presenter.await_frame(self.overlay.window(), ticket)
        })();
        if let Err(error) = &result {
            self.presenter.abort_prepare(
                self.overlay.window(),
                ticket,
                "screen_selection_present",
                error,
            );
        }
        result
    }

    fn update_selection(&self, selection: Option<PhysicalRect>) {
        let Some(intersection) =
            selection.and_then(|selection| intersect(selection, self.region.bounds))
        else {
            self.overlay.set_selection_active(false);
            self.overlay.window().request_redraw();
            return;
        };
        let scale = self.region.dpi_scale_milli.max(1) as f32 / 1_000.0;
        self.overlay.set_selection_x(
            (i64::from(intersection.x) - i64::from(self.region.bounds.x)) as f32 / scale,
        );
        self.overlay.set_selection_y(
            (i64::from(intersection.y) - i64::from(self.region.bounds.y)) as f32 / scale,
        );
        self.overlay
            .set_selection_width(intersection.width as f32 / scale);
        self.overlay
            .set_selection_height(intersection.height as f32 / scale);
        self.overlay.set_selection_active(true);
        self.overlay.window().request_redraw();
    }
}

fn precreate_surface(
    overlay: &Rc<ScreenRegionOverlay>,
    presenter: &OverlayPresenter,
    attached: Rc<Cell<bool>>,
    window_handle: Rc<Cell<u64>>,
) {
    let weak = overlay.as_weak();
    let presenter = presenter.clone();
    let _ = slint::spawn_local(async move {
        let Some(overlay) = weak.upgrade() else {
            return;
        };
        let result = overlay
            .window()
            .winit_window()
            .await
            .ok()
            .as_deref()
            .and_then(crate::native_window_handle)
            .ok_or_else(|| "native selection-overlay HWND is unavailable".to_owned())
            .and_then(|handle| {
                rust_platform_windows::prepare_transparent_overlay_window(handle)?;
                presenter.attach(handle)?;
                window_handle.set(handle);
                Ok(())
            });
        match result {
            Ok(()) => attached.set(true),
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "screen_translation stage=surface_attach result=failed error={error}"
            )),
        }
    });
}

fn intersect(first: PhysicalRect, second: PhysicalRect) -> Option<PhysicalRect> {
    let left = first.x.max(second.x);
    let top = first.y.max(second.y);
    let right = (i64::from(first.x) + i64::from(first.width))
        .min(i64::from(second.x) + i64::from(second.width));
    let bottom = (i64::from(first.y) + i64::from(first.height))
        .min(i64::from(second.y) + i64::from(second.height));
    (right > i64::from(left) && bottom > i64::from(top)).then_some(PhysicalRect {
        x: left,
        y: top,
        width: (right - i64::from(left)) as u32,
        height: (bottom - i64::from(top)) as u32,
    })
}

#[cfg(test)]
mod tests {
    use rust_platform_windows::region_selection::PointerEvent;
    use rust_platform_windows::screen_capture::{
        CaptureError, CapturedOutput, DesktopCapture, Rotation,
    };
    use rust_protocol::{PhysicalRect, RequestId};

    use super::{ScreenSelectionCoordinator, SelectionEnvironment, SelectionTerminal};

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn capture() -> DesktopCapture {
        let output = |bounds: PhysicalRect, color: [u8; 4]| {
            let stride = bounds.width * 4;
            CapturedOutput::try_new(
                bounds,
                Rotation::Identity,
                1_000,
                stride,
                color
                    .into_iter()
                    .cycle()
                    .take(stride as usize * bounds.height as usize)
                    .collect(),
            )
            .unwrap()
        };
        DesktopCapture::try_new(
            1,
            vec![
                output(rect(-1_920, 0, 1_920, 1_080), [1, 2, 3, 255]),
                output(rect(0, 0, 1_920, 1_080), [4, 5, 6, 255]),
            ],
        )
        .unwrap()
    }

    struct FakeEnvironment {
        events: Vec<String>,
        capture_result: Result<DesktopCapture, CaptureError>,
        show_error: Option<String>,
        visible_surfaces: usize,
        layout_current: bool,
    }

    impl FakeEnvironment {
        fn successful() -> Self {
            Self {
                events: Vec::new(),
                capture_result: Ok(capture()),
                show_error: None,
                visible_surfaces: 0,
                layout_current: true,
            }
        }
    }

    impl SelectionEnvironment for FakeEnvironment {
        fn capture(&mut self) -> Result<DesktopCapture, String> {
            self.events.push("capture".into());
            self.capture_result
                .clone()
                .map_err(|error| error.to_string())
        }

        fn set_capture_mode(&mut self, _request_id: RequestId, active: bool) -> Result<(), String> {
            self.events
                .push(if active { "esc:on" } else { "esc:off" }.into());
            Ok(())
        }

        fn show_surfaces(&mut self, capture: &DesktopCapture) -> Result<(), String> {
            for bounds in capture.output_bounds() {
                self.events.push(format!("show:{}", bounds.x));
                self.visible_surfaces += 1;
            }
            if let Some(error) = self.show_error.take() {
                return Err(error);
            }
            Ok(())
        }

        fn update_selection(&mut self, _selection: Option<PhysicalRect>) {}

        fn teardown_surfaces(&mut self) {
            self.events.push("teardown".into());
            self.visible_surfaces = 0;
        }

        fn arm_watchdog(&mut self, _request_id: RequestId) {}

        fn disarm_watchdog(&mut self) {}

        fn desktop_layout_current(&self) -> bool {
            self.layout_current
        }
    }

    fn selected_event(terminal: SelectionTerminal) -> String {
        match terminal {
            SelectionTerminal::Selected { rect, .. } => format!(
                "selected:{},{},{},{}",
                rect.x, rect.y, rect.width, rect.height
            ),
            SelectionTerminal::Cancelled { .. } => "cancelled".into(),
            SelectionTerminal::Failed { error, .. } => format!("failed:{error}"),
        }
    }

    #[test]
    fn capture_precedes_escape_capture_and_every_surface() {
        let request_id = RequestId::new();
        let mut coordinator = ScreenSelectionCoordinator::new(FakeEnvironment::successful());

        assert!(coordinator.begin(request_id).is_none());
        assert!(
            coordinator
                .handle_pointer(PointerEvent::Down { x: -10, y: 10 })
                .is_none()
        );
        assert!(
            coordinator
                .handle_pointer(PointerEvent::Move { x: 20, y: 30 })
                .is_none()
        );
        let terminal = coordinator
            .handle_pointer(PointerEvent::Up { x: 20, y: 30 })
            .unwrap();
        let terminal = selected_event(terminal);

        let mut events = coordinator.environment().events.clone();
        events.push(terminal);
        assert_eq!(
            events,
            [
                "capture",
                "esc:on",
                "show:-1920",
                "show:0",
                "teardown",
                "esc:off",
                "selected:-10,10,30,20",
            ]
        );
    }

    #[test]
    fn every_terminal_path_tears_down_once_and_releases_escape_once() {
        let cases = [
            "escape",
            "right",
            "zero",
            "presenter",
            "disconnect",
            "watchdog",
        ];
        for case in cases {
            let request_id = RequestId::new();
            let mut environment = FakeEnvironment::successful();
            if case == "presenter" {
                environment.show_error = Some("renderer unavailable".into());
            }
            let mut coordinator = ScreenSelectionCoordinator::new(environment);
            let begin_terminal = coordinator.begin(request_id);
            if case == "disconnect" {
                coordinator.environment.layout_current = false;
            }
            let terminal = match case {
                "escape" => coordinator.cancel(request_id),
                "right" => {
                    assert!(
                        coordinator
                            .handle_pointer(PointerEvent::RightDown { x: 4, y: 5 })
                            .is_none()
                    );
                    coordinator.handle_pointer(PointerEvent::RightUp { x: 4, y: 5 })
                }
                "zero" => {
                    assert!(
                        coordinator
                            .handle_pointer(PointerEvent::Down { x: 8, y: 9 })
                            .is_none()
                    );
                    coordinator.handle_pointer(PointerEvent::Up { x: 8, y: 9 })
                }
                "presenter" => begin_terminal,
                "disconnect" => coordinator.handle_pointer(PointerEvent::Down { x: 1, y: 1 }),
                "watchdog" => coordinator.watchdog_expired(request_id),
                _ => unreachable!(),
            };

            assert!(terminal.is_some(), "{case} must terminate");
            assert_eq!(
                coordinator
                    .environment()
                    .events
                    .iter()
                    .filter(|event| event.as_str() == "esc:off")
                    .count(),
                1,
                "{case}"
            );
            assert_eq!(coordinator.environment().visible_surfaces, 0, "{case}");
            assert!(!coordinator.is_active(), "{case}");
        }
    }

    #[test]
    fn stale_escape_and_watchdog_events_do_not_cancel_the_current_request() {
        let request_id = RequestId::new();
        let stale = RequestId::new();
        let mut coordinator = ScreenSelectionCoordinator::new(FakeEnvironment::successful());
        assert!(coordinator.begin(request_id).is_none());

        assert!(coordinator.cancel(stale).is_none());
        assert!(coordinator.watchdog_expired(stale).is_none());
        assert!(coordinator.is_active());
        assert_eq!(
            coordinator
                .environment()
                .events
                .iter()
                .filter(|event| event.as_str() == "esc:off")
                .count(),
            0
        );
    }
}
