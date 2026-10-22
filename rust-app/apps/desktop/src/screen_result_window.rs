//! Dedicated ownership and native lifetime for translated screenshots.
//!
//! A screen result is intentionally never a mode of `TranslationPreview`.
//! Each request keeps one Slint component and one HWND from first reveal until
//! retirement. Pinning changes only z-order; dragging and resizing retain the
//! same window and renderer.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use rust_protocol::{PhysicalRect, RequestId};
use slint::{ComponentHandle, PhysicalPosition, winit_030::WinitWindowAccessor};

use crate::{
    RuntimeEvent, ScreenTranslationWindow, dispatch_runtime_event, native_window_handle,
    overlay_presenter::{
        CaptureExcludedOverlay, OverlayContract, OverlayPresenter, PresentationTicket,
    },
    slint_window_handle,
};

#[derive(Clone)]
pub(crate) struct ScreenResultWindowSlot {
    pub(crate) window: Rc<ScreenTranslationWindow>,
    presenter: Option<OverlayPresenter>,
    request_id: RequestId,
    native_handle: Rc<Cell<u64>>,
    attached: Rc<Cell<bool>>,
    shown: Rc<Cell<bool>>,
    pinned: Rc<Cell<bool>>,
    retired: Arc<AtomicBool>,
    retire_count: Rc<Cell<u32>>,
    presentation_anchor: Arc<Mutex<Option<PhysicalRect>>>,
    attachment_failure: Rc<Cell<Option<u64>>>,
}

impl ScreenResultWindowSlot {
    pub(crate) fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub(crate) fn handle(&self) -> u64 {
        let remembered = self.native_handle.get();
        if remembered != 0 {
            remembered
        } else {
            slint_window_handle(self.window.window()).unwrap_or(0)
        }
    }

    pub(crate) fn is_pinned(&self) -> bool {
        self.pinned.get()
    }

    pub(crate) fn is_current_presentation_failure(&self, ticket: PresentationTicket) -> bool {
        !self.retired.load(Ordering::Acquire)
            && self
                .presenter
                .as_ref()
                .is_some_and(|presenter| presenter.is_current_failure(ticket))
    }

    fn record_attachment_failure(&self) -> u64 {
        static NEXT_FAILURE: AtomicU64 = AtomicU64::new(1);
        let attempt = NEXT_FAILURE.fetch_add(1, Ordering::Relaxed);
        self.attachment_failure.set(Some(attempt));
        attempt
    }

    pub(crate) fn is_current_attachment_failure(&self, attempt: u64) -> bool {
        !self.retired.load(Ordering::Acquire) && self.attachment_failure.get() == Some(attempt)
    }

    #[cfg(test)]
    fn component_id(&self) -> usize {
        Rc::as_ptr(&self.window) as usize
    }

    #[cfg(test)]
    fn test_slot(request_id: RequestId, handle: u64) -> Self {
        Self {
            window: Rc::new(ScreenTranslationWindow::new().expect("test screen window")),
            presenter: None,
            request_id,
            native_handle: Rc::new(Cell::new(handle)),
            attached: Rc::new(Cell::new(true)),
            shown: Rc::new(Cell::new(true)),
            pinned: Rc::new(Cell::new(false)),
            retired: Arc::new(AtomicBool::new(false)),
            retire_count: Rc::new(Cell::new(0)),
            presentation_anchor: Arc::new(Mutex::new(None)),
            attachment_failure: Rc::new(Cell::new(None)),
        }
    }

    #[cfg(test)]
    fn set_test_pinned(&self, pinned: bool) {
        self.pinned.set(pinned);
    }

    #[cfg(test)]
    fn test_retire_count(&self) -> u32 {
        self.retire_count.get()
    }
}

#[derive(Default)]
pub(crate) struct ScreenResultWindowRegistry {
    slots: HashMap<RequestId, ScreenResultWindowSlot>,
}

impl ScreenResultWindowRegistry {
    pub(crate) fn insert(
        &mut self,
        slot: ScreenResultWindowSlot,
    ) -> Option<ScreenResultWindowSlot> {
        self.slots.insert(slot.request_id(), slot)
    }

    pub(crate) fn slot_for_request(&self, request_id: RequestId) -> Option<ScreenResultWindowSlot> {
        self.slots.get(&request_id).cloned()
    }

    pub(crate) fn slot_by_handle(&self, window_handle: u64) -> Option<ScreenResultWindowSlot> {
        (window_handle != 0)
            .then(|| {
                self.slots
                    .values()
                    .find(|slot| slot.handle() == window_handle)
                    .cloned()
            })
            .flatten()
    }

    pub(crate) fn retire(
        &mut self,
        request_id: RequestId,
        reason: &str,
    ) -> Option<ScreenResultWindowSlot> {
        let slot = self.slots.remove(&request_id)?;
        retire_screen_result(&slot, reason);
        Some(slot)
    }

    pub(crate) fn retire_all(&mut self, reason: &str) {
        for (_, slot) in self.slots.drain() {
            retire_screen_result(&slot, reason);
        }
    }

    pub(crate) fn retire_unpinned(&mut self, reason: &str) -> Vec<RequestId> {
        let requests = self
            .slots
            .iter()
            .filter_map(|(request_id, slot)| (!slot.is_pinned()).then_some(*request_id))
            .collect::<Vec<_>>();
        for request_id in &requests {
            self.retire(*request_id, reason);
        }
        requests
    }

    pub(crate) fn cloak_presented_for_capture(
        &self,
    ) -> Result<Vec<CaptureExcludedOverlay>, String> {
        cloak_screen_results_for_capture(self)
    }
}

pub(crate) fn get_or_create_screen_result_slot(
    registry: &RefCell<ScreenResultWindowRegistry>,
    request_id: RequestId,
    create: impl FnOnce() -> Result<ScreenResultWindowSlot, String>,
) -> Result<ScreenResultWindowSlot, String> {
    let existing = registry.borrow().slot_for_request(request_id);
    match existing {
        Some(slot) => Ok(slot),
        None => {
            let slot = create()?;
            registry.borrow_mut().insert(slot.clone());
            Ok(slot)
        }
    }
}

pub(crate) fn retire_screen_result_slot_by_handle(
    registry: &RefCell<ScreenResultWindowRegistry>,
    window_handle: u64,
    before_retire: impl FnOnce(),
) -> Option<RequestId> {
    let slot = registry.borrow().slot_by_handle(window_handle)?;
    let request_id = slot.request_id();
    before_retire();
    registry.borrow_mut().retire(request_id, "close");
    Some(request_id)
}

pub(crate) fn create_screen_result_window(
    request_id: RequestId,
) -> Result<ScreenResultWindowSlot, String> {
    let window = Rc::new(
        ScreenTranslationWindow::new()
            .map_err(|error| format!("could not create screen-result window: {error}"))?,
    );
    window.set_request_id(request_id.to_string().into());
    let weak = window.as_weak();
    let presenter = OverlayPresenter::install_content_gated(
        window.window(),
        "screen_result_window",
        OverlayContract::Interactive,
        Some(Arc::new(move |_| {
            if let Some(window) = weak.upgrade() {
                window.set_prepared(true);
            }
        })),
    )?;
    let presentation_anchor = Arc::new(Mutex::new(None));
    let failure_anchor = Arc::clone(&presentation_anchor);
    presenter.set_failure_callback(move |ticket, error| {
        let anchor = *failure_anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(anchor) = anchor {
            dispatch_runtime_event(RuntimeEvent::ScreenResultPresentationFailed {
                request_id,
                anchor,
                error: error.to_owned(),
                presentation: Some(ticket),
                attachment_attempt: None,
            });
        }
    });
    wire_screen_result_window(&window);
    Ok(ScreenResultWindowSlot {
        window,
        presenter: Some(presenter),
        request_id,
        native_handle: Rc::new(Cell::new(0)),
        attached: Rc::new(Cell::new(false)),
        shown: Rc::new(Cell::new(false)),
        pinned: Rc::new(Cell::new(false)),
        retired: Arc::new(AtomicBool::new(false)),
        retire_count: Rc::new(Cell::new(0)),
        presentation_anchor,
        attachment_failure: Rc::new(Cell::new(None)),
    })
}

pub(crate) fn present_screen_result(
    slot: &ScreenResultWindowSlot,
    anchor: PhysicalRect,
) -> Result<(), String> {
    if slot.retired.load(Ordering::Acquire) {
        return Err("screen-result window is already retired".to_owned());
    }
    // A queued attachment error belongs only to the attempt that produced it.
    // Starting another presentation invalidates it even before the next frame.
    slot.attachment_failure.set(None);
    *slot
        .presentation_anchor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(anchor);
    slot.window.window().set_size(slint::LogicalSize::new(
        slot.window.get_screen_content_width(),
        slot.window.get_screen_content_height() + 38.0 + slot.window.get_screen_notice_height(),
    ));
    if slot.attached.get() {
        return present_attached(slot, anchor);
    }
    if slot.shown.replace(true) {
        return Ok(());
    }

    slot.window.set_prepared(false);
    slot.window
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    slot.window
        .show()
        .map_err(|error| format!("could not create screen-result surface: {error}"))?;

    let pending = slot.clone();
    let weak = slot.window.as_weak();
    slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if pending.retired.load(Ordering::Acquire) {
            let _ = window.hide();
            return;
        }
        let result = window
            .window()
            .winit_window()
            .await
            .ok()
            .as_deref()
            .and_then(native_window_handle)
            .ok_or_else(|| "native screen-result HWND is unavailable".to_owned())
            .and_then(|handle| {
                if pending.retired.load(Ordering::Acquire) {
                    let _ = window.hide();
                    return Ok(());
                }
                pending.native_handle.set(handle);
                rust_platform_windows::configure_screen_result_window(
                    handle,
                    pending.pinned.get(),
                )?;
                rust_platform_windows::conceal_overlay_window(handle)?;
                pending
                    .presenter
                    .as_ref()
                    .ok_or_else(|| "screen-result presenter is unavailable".to_owned())?
                    .attach(handle)?;
                pending.attached.set(true);
                pending.attachment_failure.set(None);
                present_attached(&pending, anchor)
            });
        if let Err(error) = result {
            pending.shown.set(false);
            dispatch_runtime_event(attachment_failure_event(&pending, anchor, error));
        }
    })
    .map_err(|error| format!("could not schedule screen-result attachment: {error}"))?;
    Ok(())
}

fn attachment_failure_event(
    slot: &ScreenResultWindowSlot,
    anchor: PhysicalRect,
    error: String,
) -> RuntimeEvent {
    // abort_prepare already reports its ticket. Keep that identity when the
    // enclosing attachment future also receives the error, so this duplicate
    // cannot later retire a newer presentation of the same request.
    let presentation = slot
        .presenter
        .as_ref()
        .and_then(OverlayPresenter::current_failure_ticket);
    RuntimeEvent::ScreenResultPresentationFailed {
        request_id: slot.request_id,
        anchor,
        error,
        presentation,
        attachment_attempt: presentation
            .is_none()
            .then(|| slot.record_attachment_failure()),
    }
}

fn present_attached(slot: &ScreenResultWindowSlot, anchor: PhysicalRect) -> Result<(), String> {
    let presenter = slot
        .presenter
        .as_ref()
        .ok_or_else(|| "screen-result presenter is unavailable".to_owned())?;
    let handle = slot.handle();
    if handle == 0 {
        return Err("native screen-result HWND is unavailable".to_owned());
    }
    rust_platform_windows::configure_screen_result_window(handle, slot.pinned.get())?;
    let right = i64::from(anchor.x)
        .saturating_add(i64::from(anchor.width))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let bottom = i64::from(anchor.y)
        .saturating_add(i64::from(anchor.height))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    presenter.content_changed(slot.window.window(), true);
    // A completion or view change can land before the previous update's first
    // frame. content_changed has already invalidated its queued reveal; keep
    // that ticket and its watchdog instead of trying to prepare a second one.
    let pending = presenter.pending_ticket();
    let ticket = if let Some(ticket) = pending {
        ticket
    } else if slot.window.get_prepared() && presenter.has_active_presentation() {
        presenter.begin_refresh()?
    } else {
        presenter.begin_prepare()?
    };
    // begin_prepare parks the HWND while waiting for its first rendered frame.
    // Placement must happen after that parking step, or reveal succeeds while
    // the result is still at the off-screen prewarm coordinate.
    if let Err(error) = rust_platform_windows::position_screen_result_window_for_region(
        handle,
        (anchor.x, anchor.y, right, bottom),
    ) {
        presenter.abort_prepare(slot.window.window(), ticket, "position", &error);
        return Err(error);
    }
    if pending.is_none()
        && let Err(error) = presenter.await_frame(slot.window.window(), ticket)
    {
        presenter.abort_prepare(slot.window.window(), ticket, "await_frame", &error);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn pin_screen_result(slot: &ScreenResultWindowSlot, pinned: bool) -> Result<(), String> {
    let handle = slot.handle();
    if handle == 0 {
        // A retry can inherit pinning before Slint has created its HWND.
        slot.pinned.set(pinned);
        slot.window.set_pinned(pinned);
        return Ok(());
    }
    rust_platform_windows::set_screen_result_window_pinned(handle, pinned)?;
    slot.pinned.set(pinned);
    slot.window.set_pinned(pinned);
    Ok(())
}

pub(crate) fn retire_screen_result(slot: &ScreenResultWindowSlot, reason: &str) {
    if slot.retired.swap(true, Ordering::AcqRel) {
        return;
    }
    slot.retire_count
        .set(slot.retire_count.get().saturating_add(1));
    slot.window.set_screen_editor_visible(false);
    slot.window.set_screen_crop_image(slint::Image::default());
    slot.window.set_screen_use_repaired_image(false);
    slot.window
        .set_screen_repaired_image(slint::Image::default());
    slot.window.set_prepared(false);
    if let Some(presenter) = &slot.presenter {
        presenter.retire(slot.window.window(), reason);
    } else {
        let _ = slot.window.hide();
    }
    let handle = slot.handle();
    if handle != 0 {
        rust_platform_windows::clear_screen_result_window_mode(handle);
    }
}

pub(crate) fn cloak_screen_results_for_capture(
    registry: &ScreenResultWindowRegistry,
) -> Result<Vec<CaptureExcludedOverlay>, String> {
    let mut excluded = Vec::new();
    for slot in registry.slots.values() {
        let Some(presenter) = &slot.presenter else {
            continue;
        };
        match presenter.cloak_for_capture() {
            Ok(Some(surface)) => excluded.push(surface),
            Ok(None) => {}
            Err(error) => {
                for surface in &excluded {
                    let _ = surface.restore();
                }
                return Err(error);
            }
        }
    }
    Ok(excluded)
}

fn wire_screen_result_window(window: &ScreenTranslationWindow) {
    {
        let weak = window.as_weak();
        window.on_screen_close_requested(move || {
            if let Some(window) = weak.upgrade()
                && let Some(handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::DismissTranslationPreview(handle));
            }
        });
    }
    window.on_screen_editor_save_requested(|| {
        dispatch_runtime_event(RuntimeEvent::ScreenEditorCommit);
    });
    window.on_screen_editor_confirm_requested(|| {
        dispatch_runtime_event(RuntimeEvent::ScreenEditorCommit);
    });
    window.on_screen_editor_cancel_requested(|| {
        dispatch_runtime_event(RuntimeEvent::ScreenEditorCancel);
    });
    window.on_screen_editor_retry_requested(|| {
        dispatch_runtime_event(RuntimeEvent::ScreenEditorRetry);
    });
    {
        let weak = window.as_weak();
        window.on_screen_translation_retry_requested(move || {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenTranslationRetryRequested {
                    window_handle,
                });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_screen_view_state_changed(move |mode| {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenViewModeChanged {
                    window_handle,
                    mode,
                });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_screen_block_edit_requested(move |block_index| {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenBlockEditRequested {
                    window_handle,
                    block_index,
                });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_screen_copy_text_requested(move || {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenCopyTextRequested { window_handle });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_screen_save_file_requested(move || {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenSaveFileRequested { window_handle });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_screen_delete_image_requested(move || {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::ScreenDeleteImageRequested { window_handle });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_pin_state_changed(move |pinned| {
            if let Some(window) = weak.upgrade()
                && let Some(window_handle) = slint_window_handle(window.window())
            {
                dispatch_runtime_event(RuntimeEvent::TranslationPreviewPinChanged {
                    window_handle,
                    pinned,
                });
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_begin_drag(move || {
            if let Some(window) = weak.upgrade()
                && let Some(handle) = slint_window_handle(window.window())
                && let Err(error) = rust_platform_windows::begin_borderless_window_drag(handle)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_result drag=failed hwnd={handle} error={error}"
                ));
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_begin_resize(move || {
            if let Some(window) = weak.upgrade()
                && let Err(error) =
                    crate::translation_view::begin_slint_window_resize(window.window(), 7)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_result resize=failed edge=southeast error={error}"
                ));
            }
        });
    }
}

#[cfg(test)]
mod screen_result_window_tests {
    use std::cell::{Cell, RefCell};

    use rust_protocol::RequestId;

    use super::{
        ScreenResultWindowRegistry, ScreenResultWindowSlot, get_or_create_screen_result_slot,
        retire_screen_result_slot_by_handle,
    };

    #[test]
    fn first_result_and_restored_result_can_create_after_registry_lookup() {
        i_slint_backend_testing::init_no_event_loop();
        let registry = RefCell::new(ScreenResultWindowRegistry::default());
        let first_request = RequestId::new();
        let first = ScreenResultWindowSlot::test_slot(first_request, 41);
        let slot = get_or_create_screen_result_slot(&registry, first_request, || Ok(first.clone()))
            .expect("first completed result opens");
        assert_eq!(slot.handle(), 41);

        let reused = get_or_create_screen_result_slot(&registry, first_request, || {
            panic!("an existing result must reuse its window")
        })
        .expect("existing result reuses its window");
        assert_eq!(reused.component_id(), slot.component_id());

        assert_eq!(
            registry
                .borrow_mut()
                .retire(first_request, "test")
                .unwrap()
                .handle(),
            41
        );
        let restored = ScreenResultWindowSlot::test_slot(first_request, 43);
        let slot = get_or_create_screen_result_slot(&registry, first_request, || Ok(restored))
            .expect("restored result opens after the earlier window closed");
        assert_eq!(slot.handle(), 43);
    }

    #[test]
    fn close_result_releases_lookup_before_editor_cleanup_and_retirement() {
        i_slint_backend_testing::init_no_event_loop();
        let registry = RefCell::new(ScreenResultWindowRegistry::default());
        let first_request = RequestId::new();
        let first = ScreenResultWindowSlot::test_slot(first_request, 51);
        registry.borrow_mut().insert(first.clone());
        let editor_closed = Cell::new(false);

        let closed = retire_screen_result_slot_by_handle(&registry, 51, || {
            assert!(registry.try_borrow_mut().is_ok());
            editor_closed.set(true);
        });
        assert_eq!(closed, Some(first_request));
        assert!(editor_closed.get());
        assert!(registry.borrow().slot_for_request(first_request).is_none());
        assert_eq!(first.test_retire_count(), 1);

        let second_request = RequestId::new();
        registry
            .borrow_mut()
            .insert(ScreenResultWindowSlot::test_slot(second_request, 52));
        assert_eq!(
            retire_screen_result_slot_by_handle(&registry, 52, || {}),
            Some(second_request)
        );
        assert!(registry.borrow().slot_for_request(second_request).is_none());
    }

    #[test]
    fn registry_owns_identity_pin_and_idempotent_retirement() {
        i_slint_backend_testing::init_no_event_loop();
        let compact_source = include_str!("../ui/app.slint");
        assert!(
            !compact_source.contains("screen-image-active"),
            "compact TranslationPreview must not own screenshot state"
        );
        let request_id = RequestId::new();
        let slot = ScreenResultWindowSlot::test_slot(request_id, 41);
        let mut registry = ScreenResultWindowRegistry::default();

        registry.insert(slot.clone());

        assert_eq!(registry.slot_for_request(request_id).unwrap().handle(), 41);
        assert_eq!(
            registry.slot_by_handle(41).unwrap().request_id(),
            request_id
        );
        let component_id = slot.component_id();

        slot.set_test_pinned(true);
        assert_eq!(slot.handle(), 41);
        assert_eq!(slot.component_id(), component_id);
        assert!(slot.is_pinned());

        slot.set_test_pinned(false);
        assert_eq!(slot.handle(), 41);
        assert_eq!(slot.component_id(), component_id);
        assert!(!slot.is_pinned());

        assert!(registry.retire(request_id, "close").is_some());
        for reason in ["expiry", "delete_all", "supersede", "shutdown"] {
            assert!(registry.retire(request_id, reason).is_none());
        }
        assert_eq!(slot.test_retire_count(), 1);
    }

    #[cfg(windows)]
    fn native_test_slot() -> (ScreenResultWindowSlot, windows_sys::Win32::Foundation::HWND) {
        use std::{
            ptr::null_mut,
            rc::Rc,
            sync::{Arc, Mutex, atomic::AtomicBool},
        };

        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_OVERLAPPEDWINDOW};

        use super::ScreenTranslationWindow;

        i_slint_backend_testing::init_no_event_loop();
        let class = "STATIC\0".encode_utf16().collect::<Vec<_>>();
        let native = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                320,
                180,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        assert!(!native.is_null());
        let handle = native as usize as u64;
        let window = Rc::new(ScreenTranslationWindow::new().unwrap());
        let presenter = crate::overlay_presenter::test_utils::attached_presenter(handle);
        rust_platform_windows::configure_screen_result_window(handle, false).unwrap();
        rust_platform_windows::conceal_overlay_window(handle).unwrap();
        let slot = ScreenResultWindowSlot {
            window,
            presenter: Some(presenter),
            request_id: RequestId::new(),
            native_handle: Rc::new(Cell::new(handle)),
            attached: Rc::new(Cell::new(true)),
            shown: Rc::new(Cell::new(true)),
            pinned: Rc::new(Cell::new(false)),
            retired: Arc::new(AtomicBool::new(false)),
            retire_count: Rc::new(Cell::new(0)),
            presentation_anchor: Arc::new(Mutex::new(None)),
            attachment_failure: Rc::new(Cell::new(None)),
        };
        (slot, native)
    }

    #[cfg(windows)]
    fn clean_up_native_test_slot(
        slot: &ScreenResultWindowSlot,
        native: windows_sys::Win32::Foundation::HWND,
    ) {
        use slint::ComponentHandle;
        slot.presenter
            .as_ref()
            .unwrap()
            .retire(slot.window.window(), "test_cleanup");
        rust_platform_windows::clear_screen_result_window_mode(slot.handle());
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(native) };
    }

    #[cfg(windows)]
    #[test]
    fn delayed_attachment_failure_cannot_retire_a_successful_later_presentation() {
        let (slot, native) = native_test_slot();
        let failed_attempt = slot.record_attachment_failure();
        assert!(slot.is_current_attachment_failure(failed_attempt));

        super::present_screen_result(
            &slot,
            rust_protocol::PhysicalRect {
                x: 100,
                y: 100,
                width: 200,
                height: 100,
            },
        )
        .expect("a later attached presentation succeeds");

        assert!(
            !slot.is_current_attachment_failure(failed_attempt),
            "the delayed pre-attachment error must not retire the successful newer presentation"
        );
        clean_up_native_test_slot(&slot, native);
    }

    #[cfg(windows)]
    #[test]
    fn attachment_error_after_prepare_preserves_the_failed_presentation_ticket() {
        use slint::ComponentHandle;
        let (slot, native) = native_test_slot();
        let anchor = rust_protocol::PhysicalRect {
            x: 100,
            y: 100,
            width: 200,
            height: 100,
        };
        super::present_screen_result(&slot, anchor).unwrap();
        let presenter = slot.presenter.as_ref().unwrap();
        let ticket = presenter.pending_ticket().unwrap();
        presenter.abort_prepare(
            slot.window.window(),
            ticket,
            "test",
            "failed after attachment",
        );

        let super::RuntimeEvent::ScreenResultPresentationFailed {
            presentation,
            attachment_attempt,
            ..
        } = super::attachment_failure_event(&slot, anchor, "failed after attachment".into())
        else {
            panic!("expected a presentation failure");
        };
        assert_eq!(
            presentation,
            Some(ticket),
            "the post-attachment failure must retain its presenter generation"
        );
        assert_eq!(attachment_attempt, None);
        clean_up_native_test_slot(&slot, native);
    }

    #[cfg(windows)]
    #[test]
    fn first_result_is_positioned_on_screen_after_prepare_parks_the_window() {
        use super::present_attached;
        use rust_protocol::PhysicalRect;
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let (slot, native) = native_test_slot();

        present_attached(
            &slot,
            PhysicalRect {
                x: 100,
                y: 100,
                width: 200,
                height: 100,
            },
        )
        .unwrap();
        slot.window
            .set_screen_translation_notice("Translation completed before the first frame".into());
        assert!(
            present_attached(
                &slot,
                PhysicalRect {
                    x: 100,
                    y: 100,
                    width: 200,
                    height: 100
                },
            )
            .is_ok(),
            "a content update while the first frame is pending must reuse that presentation"
        );
        let mut rect = RECT::default();
        assert_ne!(unsafe { GetWindowRect(native, &mut rect) }, 0);
        assert!(
            rect.left >= 0 && rect.top >= 0,
            "result remained parked at ({}, {})",
            rect.left,
            rect.top
        );

        clean_up_native_test_slot(&slot, native);
    }
}
