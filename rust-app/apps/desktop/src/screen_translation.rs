use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};
#[cfg(feature = "screen-translation")]
use std::{collections::HashMap, sync::Arc};

use rust_platform_windows::{
    region_selection::{MonitorRegion, PointerEvent, RegionSelectionMonitor},
    screen_capture::{CaptureError, DesktopCapture},
};
use rust_protocol::{InputHostCommand, PhysicalRect, RequestId};
#[cfg(feature = "screen-translation")]
use rust_protocol::{
    ScreenBlockId, ScreenTextBlock, SourceContext, TranslatedScreenBlock,
    TranslationBackendCapabilities,
};
use slint::winit_030::WinitWindowAccessor;
use slint::{
    ComponentHandle, Image, PhysicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, Timer,
    TimerMode,
};

#[cfg(feature = "screen-translation")]
use crate::screen_result::{
    DocumentError, ScreenResultSnapshot, ScreenTranslationDocument, UserTranslationEdit,
};
use crate::{
    RuntimeEvent, ScreenRegionOverlay, dispatch_runtime_event,
    overlay_presenter::{OverlayContract, OverlayPresenter},
    worker_supervisor::Workers,
};
#[cfg(feature = "screen-translation")]
use rust_platform_windows::{WindowInventory, screen_capture::RgbaFrame};

const SELECTION_WATCHDOG_SECONDS: u64 = 15;
const SURFACE_READY_DEADLINE: Duration = Duration::from_millis(2_500);
const SURFACE_READY_RETRY: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "screen-translation"), allow(dead_code))]
pub(crate) enum ScreenTranslationStage {
    Selecting,
    Recognizing,
    Translating,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveScreenTranslation {
    request_id: RequestId,
    anchor: Option<PhysicalRect>,
    stage: ScreenTranslationStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenTranslationCancellation {
    Ocr(RequestId),
    Translation(RequestId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenTranslationBegin {
    Started(Option<ScreenTranslationCancellation>),
    AlreadySelecting { active_request_id: RequestId },
}

/// Owns only request identity and geometry. Captured pixels deliberately never
/// enter this state machine, so replacing, completing, pinning, or dismissing a
/// request cannot retain a screenshot by accident.
#[derive(Debug, Default)]
pub(crate) struct ScreenTranslationFlow {
    active: Option<ActiveScreenTranslation>,
}

/// Canonical owner of the screen-translation request after the native
/// selection surface has identified it.  Pixel buffers, captured-window
/// evidence, OCR context, the editable document and partial translations all
/// share the same request identity here, instead of being mirrored across
/// `main.rs` maps.
#[derive(Default)]
pub(crate) struct ScreenTranslationCoordinator {
    flow: ScreenTranslationFlow,
    #[cfg(feature = "screen-translation")]
    pending_crops: HashMap<RequestId, Arc<RgbaFrame>>,
    #[cfg(feature = "screen-translation")]
    pending_inventories: HashMap<RequestId, WindowInventory>,
    #[cfg(feature = "screen-translation")]
    pending_contexts: HashMap<RequestId, SourceContext>,
    #[cfg(feature = "screen-translation")]
    documents: HashMap<RequestId, ScreenTranslationDocument>,
    #[cfg(feature = "screen-translation")]
    partials: HashMap<RequestId, (u64, HashMap<ScreenBlockId, String>)>,
    #[cfg(feature = "screen-translation")]
    capabilities: HashMap<RequestId, TranslationBackendCapabilities>,
    #[cfg(feature = "screen-translation")]
    languages: HashMap<
        RequestId,
        (
            rust_protocol::TranslationLanguage,
            rust_protocol::TranslationLanguage,
        ),
    >,
}

#[cfg_attr(not(feature = "screen-translation"), allow(dead_code))]
impl ScreenTranslationCoordinator {
    pub(crate) fn begin(&mut self, request_id: RequestId) -> ScreenTranslationBegin {
        let result = self.flow.begin(request_id);
        if let ScreenTranslationBegin::Started(Some(cancelled)) = result {
            self.discard_request(cancelled.request_id());
        }
        result
    }

    pub(crate) fn selected(&mut self, request_id: RequestId, anchor: PhysicalRect) -> bool {
        self.flow.selected(request_id, anchor)
    }

    pub(crate) fn recognized(&mut self, request_id: RequestId) -> Option<PhysicalRect> {
        self.flow.recognized(request_id)
    }

    pub(crate) fn complete(&mut self, request_id: RequestId) -> Option<PhysicalRect> {
        self.flow.complete(request_id)
    }

    pub(crate) fn anchor(&self, request_id: RequestId) -> Option<PhysicalRect> {
        self.flow.anchor(request_id)
    }

    pub(crate) fn is_recognizing(&self, request_id: RequestId) -> bool {
        self.flow.is_recognizing(request_id)
    }

    pub(crate) fn is_selecting(&self, request_id: RequestId) -> bool {
        self.flow.is_selecting(request_id)
    }

    pub(crate) fn has_active_work(&self) -> bool {
        self.flow
            .active
            .is_some_and(|active| active.stage != ScreenTranslationStage::Complete)
    }

    fn active_translation(&self) -> Option<(RequestId, PhysicalRect)> {
        let active = self
            .flow
            .active
            .filter(|active| active.stage == ScreenTranslationStage::Translating)?;
        Some((active.request_id, active.anchor?))
    }

    pub(crate) fn close(&mut self, request_id: RequestId) -> Option<ScreenTranslationCancellation> {
        let cancellation = self.flow.close(request_id);
        self.discard_request(request_id);
        cancellation
    }

    pub(crate) fn pin_completed(&mut self, request_id: RequestId) -> bool {
        self.flow.pin_completed(request_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn store_inventory(&mut self, request_id: RequestId, inventory: WindowInventory) {
        self.pending_inventories.insert(request_id, inventory);
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn take_inventory(&mut self, request_id: RequestId) -> Option<WindowInventory> {
        self.pending_inventories.remove(&request_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn store_crop(&mut self, request_id: RequestId, crop: Arc<RgbaFrame>) {
        self.pending_crops.insert(request_id, crop);
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn take_crop(&mut self, request_id: RequestId) -> Option<Arc<RgbaFrame>> {
        self.pending_crops.remove(&request_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn store_context(&mut self, request_id: RequestId, context: SourceContext) {
        self.pending_contexts.insert(request_id, context);
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn take_context(&mut self, request_id: RequestId) -> SourceContext {
        self.pending_contexts
            .remove(&request_id)
            .unwrap_or_default()
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn insert_document(&mut self, document: ScreenTranslationDocument) {
        self.documents.insert(document.request_id(), document);
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn source_text(&self, request_id: RequestId) -> Option<String> {
        self.documents
            .get(&request_id)
            .map(ScreenTranslationDocument::source_text)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn active_translation_source(&self) -> Option<(RequestId, PhysicalRect, String)> {
        let (request_id, anchor) = self.active_translation()?;
        Some((request_id, anchor, self.source_text(request_id)?))
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn translation_input(
        &self,
        request_id: RequestId,
    ) -> Option<(u64, Vec<ScreenTextBlock>, SourceContext)> {
        let document = self.documents.get(&request_id)?;
        Some((
            document.revision(),
            document.source_blocks(),
            document.context.clone(),
        ))
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn apply_partial(
        &mut self,
        request_id: RequestId,
        base_revision: u64,
        block_id: ScreenBlockId,
        translated_text: String,
    ) -> Result<String, DocumentError> {
        let document = self
            .documents
            .get(&request_id)
            .ok_or(DocumentError::RequestMismatch)?;
        if document.revision() != base_revision {
            return Err(DocumentError::RevisionMismatch);
        }
        if document.block(&block_id).is_none() {
            return Err(DocumentError::UnknownBlockId);
        }
        let (_, partials) = self
            .partials
            .entry(request_id)
            .or_insert_with(|| (base_revision, HashMap::new()));
        partials.insert(block_id, translated_text);
        Ok(document
            .source_blocks()
            .iter()
            .filter_map(|block| partials.get(&block.block_id).map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn apply_completed(
        &mut self,
        request_id: RequestId,
        base_revision: u64,
        blocks: Vec<TranslatedScreenBlock>,
        source_language: rust_protocol::TranslationLanguage,
        target_language: rust_protocol::TranslationLanguage,
        capabilities: TranslationBackendCapabilities,
    ) -> Result<ScreenTranslationProjection, DocumentError> {
        let document = self
            .documents
            .get_mut(&request_id)
            .ok_or(DocumentError::RequestMismatch)?;
        document.apply_machine_blocks(request_id, base_revision, blocks)?;
        let projection = ScreenTranslationProjection {
            source_text: document.source_text(),
            translated_text: document.copy_text()?,
            revision: document.revision(),
            capabilities,
        };
        self.partials.remove(&request_id);
        self.capabilities.insert(request_id, capabilities);
        self.languages
            .insert(request_id, (source_language, target_language));
        Ok(projection)
    }

    #[cfg(feature = "screen-translation")]
    #[allow(dead_code)] // Production correction callbacks are wired after visual approval in Task 14.
    pub(crate) fn save_user_edit(
        &mut self,
        request_id: RequestId,
        block_id: &ScreenBlockId,
        expected_revision: u64,
        text: String,
    ) -> Result<u64, DocumentError> {
        let document = self
            .documents
            .get_mut(&request_id)
            .ok_or(DocumentError::RequestMismatch)?;
        document.save_user_edit(block_id, expected_revision, UserTranslationEdit::new(text))?;
        Ok(document.revision())
    }

    #[cfg(feature = "screen-translation")]
    #[allow(dead_code)] // Production copy callbacks are wired after visual approval in Task 14.
    pub(crate) fn copy_text(&self, request_id: RequestId) -> Result<String, DocumentError> {
        self.documents
            .get(&request_id)
            .ok_or(DocumentError::RequestMismatch)?
            .copy_text()
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn result_snapshot(
        &self,
        request_id: RequestId,
    ) -> Result<
        (
            ScreenResultSnapshot,
            rust_protocol::TranslationLanguage,
            rust_protocol::TranslationLanguage,
        ),
        DocumentError,
    > {
        let document = self
            .documents
            .get(&request_id)
            .ok_or(DocumentError::RequestMismatch)?;
        let (source, target) = self
            .languages
            .get(&request_id)
            .copied()
            .ok_or(DocumentError::IncompleteBlock)?;
        Ok((document.result_snapshot()?, source, target))
    }

    #[cfg(all(test, feature = "screen-translation"))]
    fn document_has_pixels(&self, request_id: RequestId) -> bool {
        self.documents
            .get(&request_id)
            .is_some_and(ScreenTranslationDocument::has_pixels)
    }

    #[cfg(all(test, feature = "screen-translation"))]
    fn has_pending_crop(&self, request_id: RequestId) -> bool {
        self.pending_crops.contains_key(&request_id)
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn expire_pixels(&mut self, now_unix_ms: u64) {
        for document in self.documents.values_mut() {
            let _ = document.expire_pixels(now_unix_ms);
        }
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn expire_request_pixels(&mut self, request_id: RequestId) -> bool {
        self.documents
            .get_mut(&request_id)
            .and_then(|document| document.expire_pixels(u64::MAX))
            .is_some()
    }

    #[cfg(feature = "screen-translation")]
    pub(crate) fn discard_pending(&mut self, request_id: RequestId) {
        self.pending_crops.remove(&request_id);
        self.pending_inventories.remove(&request_id);
        self.pending_contexts.remove(&request_id);
        self.partials.remove(&request_id);
    }

    fn discard_request(&mut self, request_id: RequestId) {
        #[cfg(feature = "screen-translation")]
        {
            self.discard_pending(request_id);
            self.documents.remove(&request_id);
            self.capabilities.remove(&request_id);
            self.languages.remove(&request_id);
        }
        #[cfg(not(feature = "screen-translation"))]
        let _ = request_id;
    }
}

#[cfg(feature = "screen-translation")]
pub(crate) struct ScreenTranslationProjection {
    pub(crate) source_text: String,
    pub(crate) translated_text: String,
    pub(crate) revision: u64,
    pub(crate) capabilities: TranslationBackendCapabilities,
}

#[cfg_attr(not(feature = "screen-translation"), allow(dead_code))]
impl ScreenTranslationCancellation {
    fn request_id(self) -> RequestId {
        match self {
            Self::Ocr(request_id) | Self::Translation(request_id) => request_id,
        }
    }
}

#[cfg_attr(not(feature = "screen-translation"), allow(dead_code))]
impl ScreenTranslationFlow {
    pub(crate) fn begin(&mut self, request_id: RequestId) -> ScreenTranslationBegin {
        if let Some(active) = self
            .active
            .filter(|active| active.stage == ScreenTranslationStage::Selecting)
        {
            return ScreenTranslationBegin::AlreadySelecting {
                active_request_id: active.request_id,
            };
        }
        let cancellation = self.cancel_active_work();
        self.active = Some(ActiveScreenTranslation {
            request_id,
            anchor: None,
            stage: ScreenTranslationStage::Selecting,
        });
        ScreenTranslationBegin::Started(cancellation)
    }

    pub(crate) fn selected(&mut self, request_id: RequestId, anchor: PhysicalRect) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id)
        else {
            return false;
        };
        active.anchor = Some(anchor);
        active.stage = ScreenTranslationStage::Recognizing;
        true
    }

    pub(crate) fn recognized(&mut self, request_id: RequestId) -> Option<PhysicalRect> {
        let active = self.active.as_mut().filter(|active| {
            active.request_id == request_id && active.stage == ScreenTranslationStage::Recognizing
        })?;
        active.stage = ScreenTranslationStage::Translating;
        active.anchor
    }

    pub(crate) fn complete(&mut self, request_id: RequestId) -> Option<PhysicalRect> {
        let active = self
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id)?;
        active.stage = ScreenTranslationStage::Complete;
        active.anchor
    }

    pub(crate) fn anchor(&self, request_id: RequestId) -> Option<PhysicalRect> {
        self.active
            .filter(|active| active.request_id == request_id)
            .and_then(|active| active.anchor)
    }

    pub(crate) fn is_recognizing(&self, request_id: RequestId) -> bool {
        self.active.is_some_and(|active| {
            active.request_id == request_id && active.stage == ScreenTranslationStage::Recognizing
        })
    }

    pub(crate) fn is_selecting(&self, request_id: RequestId) -> bool {
        self.active.is_some_and(|active| {
            active.request_id == request_id && active.stage == ScreenTranslationStage::Selecting
        })
    }

    pub(crate) fn close(&mut self, request_id: RequestId) -> Option<ScreenTranslationCancellation> {
        if self
            .active
            .is_none_or(|active| active.request_id != request_id)
        {
            return None;
        }
        let cancellation = self.cancel_active_work();
        self.active = None;
        cancellation
    }

    pub(crate) fn pin_completed(&mut self, request_id: RequestId) -> bool {
        if self.active.is_some_and(|active| {
            active.request_id == request_id && active.stage == ScreenTranslationStage::Complete
        }) {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn cancel_active_work(&self) -> Option<ScreenTranslationCancellation> {
        let active = self.active?;
        match active.stage {
            ScreenTranslationStage::Recognizing => {
                Some(ScreenTranslationCancellation::Ocr(active.request_id))
            }
            ScreenTranslationStage::Translating => Some(
                ScreenTranslationCancellation::Translation(active.request_id),
            ),
            ScreenTranslationStage::Selecting | ScreenTranslationStage::Complete => None,
        }
    }
}

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
        error: SelectionFailure,
    },
}

#[derive(Debug)]
pub(crate) enum SelectionFailure {
    Capture(CaptureError),
    EmptySelection,
    Runtime(String),
}

impl std::fmt::Display for SelectionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capture(error) => error.fmt(formatter),
            Self::EmptySelection => formatter.write_str("the selected screen region is empty"),
            Self::Runtime(error) => formatter.write_str(error),
        }
    }
}

pub(crate) trait SelectionEnvironment {
    #[cfg(test)]
    fn capture(&mut self) -> Result<DesktopCapture, CaptureError>;
    fn set_capture_mode(&mut self, request_id: RequestId, active: bool) -> Result<(), String>;
    fn show_surfaces(&mut self, capture: &DesktopCapture) -> Result<SurfacePresentation, String>;
    fn schedule_surface_retry(&mut self, request_id: RequestId);
    fn update_selection(&mut self, selection: Option<PhysicalRect>);
    fn teardown_surfaces(&mut self);
    fn arm_watchdog(&mut self, request_id: RequestId);
    fn disarm_watchdog(&mut self);
    fn desktop_layout_current(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfacePresentation {
    Ready,
    Preparing,
}

struct ActiveSelection {
    request_id: RequestId,
    capture: DesktopCapture,
    drag_origin: Option<(i32, i32)>,
    capture_mode_active: bool,
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

    #[cfg(test)]
    pub(crate) fn begin(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        if self.active.is_some() {
            return Some(SelectionTerminal::Failed {
                request_id,
                error: SelectionFailure::Runtime(
                    "another screen selection is already active".into(),
                ),
            });
        }
        let capture = match self.environment.capture() {
            Ok(capture) => capture,
            Err(error) => {
                return Some(SelectionTerminal::Failed {
                    request_id,
                    error: SelectionFailure::Capture(error),
                });
            }
        };
        self.begin_with_capture(request_id, capture)
    }

    pub(crate) fn begin_with_capture(
        &mut self,
        request_id: RequestId,
        capture: DesktopCapture,
    ) -> Option<SelectionTerminal> {
        if self.active.is_some() {
            return Some(SelectionTerminal::Failed {
                request_id,
                error: SelectionFailure::Runtime(
                    "another screen selection is already active".into(),
                ),
            });
        }
        self.active = Some(ActiveSelection {
            request_id,
            capture,
            drag_origin: None,
            capture_mode_active: false,
        });
        self.present_active_surfaces()
    }

    pub(crate) fn surfaces_ready(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.request_id != request_id || active.capture_mode_active)
        {
            return None;
        }
        self.present_active_surfaces()
    }

    fn present_active_surfaces(&mut self) -> Option<SelectionTerminal> {
        let request_id = self.active.as_ref()?.request_id;
        let show_result = {
            let active = self.active.as_ref().expect("selection was just installed");
            self.environment.show_surfaces(&active.capture)
        };
        match show_result {
            Ok(SurfacePresentation::Preparing) => {
                self.environment.schedule_surface_retry(request_id);
                return None;
            }
            Ok(SurfacePresentation::Ready) => {}
            Err(error) => {
                return self.terminate(SelectionOutcome::Failed(SelectionFailure::Runtime(error)));
            }
        }
        if let Err(error) = self.environment.set_capture_mode(request_id, true) {
            return self.terminate(SelectionOutcome::Failed(SelectionFailure::Runtime(error)));
        }
        self.active
            .as_mut()
            .expect("selection was just installed")
            .capture_mode_active = true;
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
                    None => {
                        self.terminate(SelectionOutcome::Failed(SelectionFailure::EmptySelection))
                    }
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
        self.terminate(SelectionOutcome::Failed(SelectionFailure::Runtime(
            "the desktop layout changed during selection".into(),
        )))
    }

    pub(crate) fn watchdog_expired(&mut self, request_id: RequestId) -> Option<SelectionTerminal> {
        (self.active.as_ref()?.request_id == request_id).then(|| {
            self.terminate(SelectionOutcome::Failed(SelectionFailure::Runtime(
                format!(
                    "screen selection exceeded the {SELECTION_WATCHDOG_SECONDS}-second safety limit"
                ),
            )))
        })?
    }

    fn terminate(&mut self, outcome: SelectionOutcome) -> Option<SelectionTerminal> {
        let active = self.active.take()?;
        self.environment.disarm_watchdog();
        self.environment.teardown_surfaces();
        let release_error = active
            .capture_mode_active
            .then(|| {
                self.environment
                    .set_capture_mode(active.request_id, false)
                    .err()
            })
            .flatten();
        Some(match (outcome, release_error) {
            (_, Some(error)) => SelectionTerminal::Failed {
                request_id: active.request_id,
                error: SelectionFailure::Runtime(format!(
                    "could not release screen-selection keyboard capture: {error}"
                )),
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
    Failed(SelectionFailure),
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
    surfaces: Option<ScreenSelectionSurfaces>,
    pointer_monitor: Option<RegionSelectionMonitor>,
    watchdog: Timer,
    surface_ready_retry: Timer,
    surface_ready_deadline: Option<Instant>,
}

impl DesktopSelectionEnvironment {
    pub(crate) fn new(workers: Rc<Workers>) -> Self {
        let surfaces = match rust_platform_windows::region_selection::attached_monitor_regions()
            .and_then(ScreenSelectionSurfaces::new)
        {
            Ok(surfaces) => Some(surfaces),
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "screen_translation stage=surface_precreate result=failed error={error}"
                ));
                None
            }
        };
        Self {
            workers,
            surfaces,
            pointer_monitor: None,
            watchdog: Timer::default(),
            surface_ready_retry: Timer::default(),
            surface_ready_deadline: None,
        }
    }
}

impl SelectionEnvironment for DesktopSelectionEnvironment {
    #[cfg(test)]
    fn capture(&mut self) -> Result<DesktopCapture, CaptureError> {
        use rust_platform_windows::screen_capture::DesktopCaptureBackend as _;

        rust_platform_windows::screen_capture::WindowsDesktopCaptureBackend::default().capture()
    }

    fn set_capture_mode(&mut self, request_id: RequestId, active: bool) -> Result<(), String> {
        self.workers
            .input
            .send(&InputHostCommand::SetScreenCaptureMode { request_id, active })
            .map_err(|error| error.to_string())
    }

    fn show_surfaces(&mut self, capture: &DesktopCapture) -> Result<SurfacePresentation, String> {
        let current_regions = rust_platform_windows::region_selection::attached_monitor_regions()?;
        let needs_rebuild = self
            .surfaces
            .as_ref()
            .is_none_or(|surfaces| !surfaces.regions_match(&current_regions));
        if needs_rebuild {
            if let Some(surfaces) = self.surfaces.take() {
                surfaces.retire();
            }
            self.surfaces = Some(ScreenSelectionSurfaces::new(current_regions.clone())?);
            rust_app_core::diagnostics::log_line(
                "screen_translation stage=surface_precreate result=recreated_for_desktop_layout",
            );
            self.surface_ready_deadline = Some(Instant::now() + SURFACE_READY_DEADLINE);
        }
        let surfaces = self
            .surfaces
            .as_ref()
            .ok_or_else(|| "screen-selection surfaces are unavailable".to_owned())?;
        let deadline = *self
            .surface_ready_deadline
            .get_or_insert_with(|| Instant::now() + SURFACE_READY_DEADLINE);
        if surfaces.await_ready(deadline)? == SurfacePresentation::Preparing {
            return Ok(SurfacePresentation::Preparing);
        }
        surfaces.verify_regions(&current_regions)?;
        surfaces.present(capture)?;
        self.surface_ready_deadline = None;
        self.surface_ready_retry.stop();
        self.pointer_monitor = Some(RegionSelectionMonitor::start(|event| {
            dispatch_runtime_event(RuntimeEvent::ScreenSelectionPointer(event));
        })?);
        Ok(SurfacePresentation::Ready)
    }

    fn schedule_surface_retry(&mut self, request_id: RequestId) {
        self.surface_ready_retry
            .start(TimerMode::SingleShot, SURFACE_READY_RETRY, move || {
                dispatch_runtime_event(RuntimeEvent::ScreenSelectionSurfacesReady { request_id });
            });
    }

    fn update_selection(&mut self, selection: Option<PhysicalRect>) {
        if let Some(surfaces) = &self.surfaces {
            surfaces.update_selection(selection);
        }
    }

    fn teardown_surfaces(&mut self) {
        self.surface_ready_retry.stop();
        self.surface_ready_deadline = None;
        self.pointer_monitor.take();
        if let Some(surfaces) = &self.surfaces {
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
            .is_some_and(|surfaces| surfaces.regions_match(&current))
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

    fn await_ready(&self, deadline: Instant) -> Result<SurfacePresentation, String> {
        if self.surfaces.iter().all(|surface| surface.attached.get()) {
            return Ok(SurfacePresentation::Ready);
        }
        if Instant::now() >= deadline {
            return Err("screen-selection surfaces did not attach before the deadline".into());
        }
        Ok(SurfacePresentation::Preparing)
    }

    fn regions_match(&self, current: &[MonitorRegion]) -> bool {
        let prepared = self
            .surfaces
            .iter()
            .map(|surface| surface.region)
            .collect::<Vec<_>>();
        !surface_regions_need_rebuild(&prepared, current)
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

fn surface_regions_need_rebuild(prepared: &[MonitorRegion], current: &[MonitorRegion]) -> bool {
    prepared != current
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
    use rust_platform_windows::region_selection::{MonitorRegion, PointerEvent};
    #[cfg(feature = "screen-translation")]
    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_platform_windows::screen_capture::{
        CaptureError, CapturedOutput, DesktopCapture, Rotation,
    };
    #[cfg(feature = "screen-translation")]
    use rust_protocol::{
        BlockCompletion, ScreenBlockId, ScreenTextBlock, SourceContext, TranslatedScreenBlock,
        TranslationBackendCapabilities, TranslationLanguage,
    };
    use rust_protocol::{PhysicalRect, RequestId};
    #[cfg(feature = "screen-translation")]
    use std::sync::Arc;

    use super::{
        ScreenSelectionCoordinator, ScreenTranslationBegin, ScreenTranslationCancellation,
        ScreenTranslationCoordinator, ScreenTranslationFlow, SelectionEnvironment,
        SelectionTerminal, SurfacePresentation, surface_regions_need_rebuild,
    };
    #[cfg(feature = "screen-translation")]
    use crate::screen_result::ScreenTranslationDocument;

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

    #[cfg(feature = "screen-translation")]
    fn document(request_id: RequestId) -> ScreenTranslationDocument {
        let frame = Arc::new(
            RgbaFrame::try_new(rect(0, 0, 2, 1), 8, vec![0, 0, 0, 255, 1, 1, 1, 255]).unwrap(),
        );
        ScreenTranslationDocument::new(
            request_id,
            frame,
            vec![
                ScreenTextBlock {
                    block_id: ScreenBlockId::from("a"),
                    source_text: "Damage 25".to_owned(),
                    bounds: rect(0, 0, 100, 20),
                    confidence_milli: 900,
                },
                ScreenTextBlock {
                    block_id: ScreenBlockId::from("b"),
                    source_text: "Cooldown".to_owned(),
                    bounds: rect(0, 20, 100, 20),
                    confidence_milli: 900,
                },
            ],
            SourceContext {
                domain: Some("dota2".to_owned()),
                ..SourceContext::default()
            },
            100,
            200,
        )
        .unwrap()
    }

    #[cfg(feature = "screen-translation")]
    fn translated(id: &str, text: &str) -> TranslatedScreenBlock {
        TranslatedScreenBlock {
            block_id: ScreenBlockId::from(id),
            translated_text: text.to_owned(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        }
    }

    #[test]
    fn monitor_geometry_or_dpi_change_requires_fresh_selection_surfaces() {
        let prepared = [MonitorRegion {
            bounds: rect(0, 0, 1_920, 1_080),
            dpi_scale_milli: 1_000,
        }];
        assert!(!surface_regions_need_rebuild(&prepared, &prepared));

        let resized = [MonitorRegion {
            bounds: rect(0, 0, 2_560, 1_440),
            dpi_scale_milli: 1_000,
        }];
        assert!(surface_regions_need_rebuild(&prepared, &resized));

        let scaled = [MonitorRegion {
            bounds: prepared[0].bounds,
            dpi_scale_milli: 1_250,
        }];
        assert!(surface_regions_need_rebuild(&prepared, &scaled));
    }

    #[test]
    fn selection_overlay_dims_without_turning_dark_content_black() {
        let source = include_str!("../ui/app.slint");

        assert!(source.contains("in property <float> dim-opacity: 0.28;"));
    }

    struct FakeEnvironment {
        events: Vec<String>,
        capture_result: Result<DesktopCapture, CaptureError>,
        show_error: Option<String>,
        visible_surfaces: usize,
        layout_current: bool,
        surfaces_ready: bool,
        readiness_retries: usize,
    }

    impl FakeEnvironment {
        fn successful() -> Self {
            Self {
                events: Vec::new(),
                capture_result: Ok(capture()),
                show_error: None,
                visible_surfaces: 0,
                layout_current: true,
                surfaces_ready: true,
                readiness_retries: 0,
            }
        }
    }

    impl SelectionEnvironment for FakeEnvironment {
        fn capture(&mut self) -> Result<DesktopCapture, CaptureError> {
            self.events.push("capture".into());
            self.capture_result.clone()
        }

        fn set_capture_mode(&mut self, _request_id: RequestId, active: bool) -> Result<(), String> {
            self.events
                .push(if active { "esc:on" } else { "esc:off" }.into());
            Ok(())
        }

        fn show_surfaces(
            &mut self,
            capture: &DesktopCapture,
        ) -> Result<SurfacePresentation, String> {
            if !self.surfaces_ready {
                self.events.push("surfaces:preparing".into());
                return Ok(SurfacePresentation::Preparing);
            }
            for bounds in capture.output_bounds() {
                self.events.push(format!("show:{}", bounds.x));
                self.visible_surfaces += 1;
            }
            if let Some(error) = self.show_error.take() {
                return Err(error);
            }
            Ok(SurfacePresentation::Ready)
        }

        fn schedule_surface_retry(&mut self, _request_id: RequestId) {
            self.events.push("surfaces:retry".into());
            self.readiness_retries += 1;
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
                "show:-1920",
                "show:0",
                "esc:on",
                "teardown",
                "esc:off",
                "selected:-10,10,30,20",
            ]
        );
    }

    #[test]
    fn a_background_capture_can_enter_selection_without_recapturing_on_the_ui_thread() {
        let request_id = RequestId::new();
        let mut environment = FakeEnvironment::successful();
        environment.capture_result = Err(CaptureError::NoOutputs);
        let mut coordinator = ScreenSelectionCoordinator::new(environment);

        assert!(
            coordinator
                .begin_with_capture(request_id, capture())
                .is_none()
        );
        assert_eq!(
            coordinator.environment().events,
            ["show:-1920", "show:0", "esc:on"]
        );
        assert!(coordinator.cancel(request_id).is_some());
    }

    #[test]
    fn rebuilt_surfaces_continue_the_same_request_after_native_attachment() {
        let request_id = RequestId::new();
        let mut environment = FakeEnvironment::successful();
        environment.surfaces_ready = false;
        let mut coordinator = ScreenSelectionCoordinator::new(environment);

        assert!(
            coordinator
                .begin_with_capture(request_id, capture())
                .is_none()
        );
        assert!(coordinator.is_active());
        assert_eq!(
            coordinator.environment().events,
            ["surfaces:preparing", "surfaces:retry"]
        );

        coordinator.environment.surfaces_ready = true;
        assert!(coordinator.surfaces_ready(request_id).is_none());
        assert_eq!(
            coordinator.environment().events,
            [
                "surfaces:preparing",
                "surfaces:retry",
                "show:-1920",
                "show:0",
                "esc:on",
            ]
        );
        assert_eq!(coordinator.environment().readiness_retries, 1);
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
            let captures = coordinator
                .environment()
                .events
                .iter()
                .filter(|event| event.as_str() == "esc:on")
                .count();
            let releases = coordinator
                .environment()
                .events
                .iter()
                .filter(|event| event.as_str() == "esc:off")
                .count();
            assert_eq!(captures, if case == "presenter" { 0 } else { 1 }, "{case}");
            assert_eq!(releases, captures, "{case}");
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

    #[test]
    fn newest_screen_request_supersedes_work_and_ignores_stale_ocr() {
        let first = RequestId::new();
        let second = RequestId::new();
        let mut flow = ScreenTranslationFlow::default();

        assert_eq!(flow.begin(first), ScreenTranslationBegin::Started(None));
        assert!(flow.selected(first, rect(10, 20, 300, 120)));
        assert_eq!(
            flow.begin(second),
            ScreenTranslationBegin::Started(Some(ScreenTranslationCancellation::Ocr(first)))
        );
        assert!(flow.selected(second, rect(800, 40, 240, 90)));
        assert_eq!(flow.recognized(first), None);
        assert_eq!(flow.recognized(second), Some(rect(800, 40, 240, 90)));
    }

    #[test]
    fn repeated_hotkey_during_selection_keeps_the_visible_selection_alive() {
        let first = RequestId::new();
        let repeated = RequestId::new();
        let mut flow = ScreenTranslationFlow::default();

        assert_eq!(flow.begin(first), ScreenTranslationBegin::Started(None));
        assert_eq!(
            flow.begin(repeated),
            ScreenTranslationBegin::AlreadySelecting {
                active_request_id: first,
            }
        );
        assert!(flow.is_selecting(first));
        assert!(!flow.is_selecting(repeated));
    }

    #[test]
    fn resource_policy_sees_screen_work_until_completion() {
        let request_id = RequestId::new();
        let mut coordinator = ScreenTranslationCoordinator::default();

        assert!(!coordinator.has_active_work());
        coordinator.begin(request_id);
        assert!(coordinator.has_active_work());
        coordinator.selected(request_id, rect(0, 0, 100, 80));
        assert!(coordinator.has_active_work());
        coordinator.recognized(request_id);
        assert!(coordinator.has_active_work());
        coordinator.complete(request_id);
        assert!(!coordinator.has_active_work());
    }

    #[test]
    fn closing_processing_card_cancels_the_owning_worker() {
        let ocr = RequestId::new();
        let translation = RequestId::new();
        let mut flow = ScreenTranslationFlow::default();

        flow.begin(ocr);
        flow.selected(ocr, rect(0, 0, 100, 80));
        assert_eq!(
            flow.close(ocr),
            Some(ScreenTranslationCancellation::Ocr(ocr))
        );

        flow.begin(translation);
        flow.selected(translation, rect(0, 0, 100, 80));
        flow.recognized(translation);
        assert_eq!(
            flow.close(translation),
            Some(ScreenTranslationCancellation::Translation(translation))
        );
    }

    #[test]
    fn only_completed_content_can_be_pinned_and_flow_owns_no_pixels() {
        let request_id = RequestId::new();
        let mut flow = ScreenTranslationFlow::default();
        flow.begin(request_id);
        flow.selected(request_id, rect(2, 3, 50, 60));
        assert!(!flow.pin_completed(request_id));
        flow.recognized(request_id);
        flow.complete(request_id);
        assert!(flow.pin_completed(request_id));
        assert_eq!(flow.anchor(request_id), None);
        let debug = format!("{flow:?}");
        for pixel_canary in ["rgba", "base64", "pixels", "SECRET_SCREEN_CANARY"] {
            assert!(
                !debug
                    .to_ascii_lowercase()
                    .contains(&pixel_canary.to_ascii_lowercase())
            );
        }
    }

    #[cfg(feature = "screen-translation")]
    #[test]
    fn canonical_coordinator_projects_blocks_and_preserves_a_user_edit_from_late_machine_work() {
        let request_id = RequestId::new();
        let mut coordinator = ScreenTranslationCoordinator::default();
        assert_eq!(
            coordinator.begin(request_id),
            ScreenTranslationBegin::Started(None)
        );
        assert!(coordinator.selected(request_id, rect(0, 0, 200, 80)));
        assert_eq!(
            coordinator.recognized(request_id),
            Some(rect(0, 0, 200, 80))
        );
        coordinator.insert_document(document(request_id));

        let (base_revision, blocks, context) = coordinator.translation_input(request_id).unwrap();
        assert_eq!(base_revision, 0);
        assert_eq!(blocks.len(), 2);
        assert_eq!(context.domain.as_deref(), Some("dota2"));
        assert_eq!(
            coordinator
                .apply_partial(
                    request_id,
                    0,
                    ScreenBlockId::from("a"),
                    "Урон 25".to_owned(),
                )
                .unwrap(),
            "Урон 25"
        );

        assert_eq!(
            coordinator
                .save_user_edit(
                    request_id,
                    &ScreenBlockId::from("a"),
                    0,
                    "Исправленный урон 25".to_owned(),
                )
                .unwrap(),
            1
        );
        assert!(
            coordinator
                .apply_partial(
                    request_id,
                    0,
                    ScreenBlockId::from("b"),
                    "Поздний фрагмент".to_owned(),
                )
                .is_err()
        );
        let projection = coordinator
            .apply_completed(
                request_id,
                0,
                vec![
                    translated("b", "Перезарядка"),
                    translated("a", "Машинный урон 25"),
                ],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        assert_eq!(
            projection.translated_text,
            "Исправленный урон 25\n\nПерезарядка"
        );
        assert_eq!(
            coordinator.copy_text(request_id).unwrap(),
            projection.translated_text
        );
    }

    #[cfg(feature = "screen-translation")]
    #[test]
    fn supersede_and_pixel_expiry_release_only_request_owned_buffers() {
        let first = RequestId::new();
        let second = RequestId::new();
        let mut coordinator = ScreenTranslationCoordinator::default();
        coordinator.begin(first);
        coordinator.selected(first, rect(0, 0, 20, 20));
        let crop = Arc::new(RgbaFrame::try_new(rect(0, 0, 1, 1), 4, vec![0, 0, 0, 255]).unwrap());
        coordinator.store_crop(first, crop);
        assert!(coordinator.has_pending_crop(first));
        assert_eq!(
            coordinator.begin(second),
            ScreenTranslationBegin::Started(Some(ScreenTranslationCancellation::Ocr(first)))
        );
        assert!(!coordinator.has_pending_crop(first));

        coordinator.selected(second, rect(0, 0, 20, 20));
        coordinator.recognized(second);
        coordinator.insert_document(document(second));
        coordinator
            .apply_completed(
                second,
                0,
                vec![translated("a", "Урон 25"), translated("b", "Перезарядка")],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        coordinator.complete(second);
        assert!(coordinator.pin_completed(second));
        assert!(coordinator.document_has_pixels(second));
        coordinator.expire_pixels(200);
        assert!(!coordinator.document_has_pixels(second));
        assert_eq!(
            coordinator.copy_text(second).unwrap(),
            "Урон 25\n\nПерезарядка"
        );
    }

    #[cfg(feature = "screen-translation")]
    #[test]
    fn closing_a_pinned_completed_result_discards_its_document_and_pixels() {
        let request_id = RequestId::new();
        let mut coordinator = ScreenTranslationCoordinator::default();
        coordinator.begin(request_id);
        coordinator.selected(request_id, rect(0, 0, 20, 20));
        coordinator.recognized(request_id);
        coordinator.insert_document(document(request_id));
        coordinator
            .apply_completed(
                request_id,
                0,
                vec![translated("a", "Урон 25"), translated("b", "Перезарядка")],
                TranslationLanguage::English,
                TranslationLanguage::Russian,
                TranslationBackendCapabilities::structured(),
            )
            .unwrap();
        coordinator.complete(request_id);
        assert!(coordinator.pin_completed(request_id));
        assert!(coordinator.document_has_pixels(request_id));

        assert_eq!(coordinator.close(request_id), None);
        assert!(!coordinator.document_has_pixels(request_id));
        assert!(coordinator.copy_text(request_id).is_err());
    }

    #[test]
    fn structured_failure_keeps_anchor_until_user_dismisses_it() {
        let request_id = RequestId::new();
        let anchor = rect(-300, 50, 200, 40);
        let mut flow = ScreenTranslationFlow::default();
        flow.begin(request_id);
        flow.selected(request_id, anchor);
        assert_eq!(flow.complete(request_id), Some(anchor));
        assert_eq!(flow.anchor(request_id), Some(anchor));
        assert_eq!(flow.close(request_id), None);
        assert_eq!(flow.anchor(request_id), None);
    }
}
