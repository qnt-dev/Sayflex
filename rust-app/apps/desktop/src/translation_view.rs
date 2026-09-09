//! The translation preview window.
//!
//! A small non-activating surface that appears next to the caret, sizes itself
//! to the text it is given, reads the wallpaper behind it to stay legible, and
//! can be pinned, dragged and resized without ever taking focus from the window
//! the user is typing in. All of that is presentation; what to translate and
//! what to do with the result is decided elsewhere.

use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rust_protocol::RequestId;
use slint::{
    ComponentHandle, Image, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer,
    winit_030::WinitWindowAccessor,
};

use crate::{
    MAX_TRANSLATION_BLUR_RADIUS, RuntimeEvent, TranslationPair, TranslationPreview,
    TranslatorWindow, dispatch_runtime_event, native_window_handle,
    overlay_presenter::{ContentPolicy, OverlayContract, OverlayPresenter},
    present_translation_preview, slint_window_handle,
};

#[derive(Clone)]
pub(crate) struct TranslationPreviewSlot {
    pub(crate) preview: Rc<TranslationPreview>,
    pub(crate) presenter: OverlayPresenter,
    pub(crate) attached: Rc<Cell<bool>>,
}

pub(crate) struct TranslationPreviewRegistry {
    pub(crate) current: TranslationPreviewSlot,
    pub(crate) current_request: Option<RequestId>,
    pub(crate) pinned: Vec<TranslationPreviewSlot>,
    pub(crate) blur_radius: u32,
}

impl TranslationPreviewRegistry {
    pub(crate) fn current(&self) -> TranslationPreviewSlot {
        self.current.clone()
    }

    pub(crate) fn slot_by_handle(&self, window_handle: u64) -> Option<TranslationPreviewSlot> {
        std::iter::once(&self.current)
            .chain(self.pinned.iter())
            .find(|slot| slint_window_handle(slot.preview.window()) == Some(window_handle))
            .cloned()
    }

    pub(crate) fn slot_for_request(&self, request_id: RequestId) -> Option<TranslationPreviewSlot> {
        let request_id = request_id.to_string();
        self.pinned
            .iter()
            .find(|slot| slot.preview.get_request_id().as_str() == request_id)
            .cloned()
            .or_else(|| {
                self.current_request
                    .is_some_and(|current| current.to_string() == request_id)
                    .then(|| self.current.clone())
            })
    }

    pub(crate) fn reserve_current(&mut self, request_id: RequestId) {
        self.current_request = Some(request_id);
    }

    pub(crate) fn clear_current_request(&mut self, request_id: RequestId) {
        if self.current_request == Some(request_id) {
            self.current_request = None;
        }
    }

    pub(crate) fn set_blur_radius(&mut self, radius: u32) {
        self.blur_radius = radius;
        for slot in std::iter::once(&self.current).chain(self.pinned.iter()) {
            slot.preview.set_blur_radius(radius as i32);
            if slot.preview.get_prepared()
                && !slot.preview.get_live_glass()
                && let Some(handle) = slint_window_handle(slot.preview.window())
            {
                refresh_cloaked_translation_backdrop(&slot.preview, handle);
            }
        }
    }
}

pub(crate) fn translation_pairs(source: &str, translation: &str) -> Vec<TranslationPair> {
    let source_units = translation_units(source);
    let translation_units = translation_units(translation);
    if source_units.len() > 1 && source_units.len() == translation_units.len() {
        return source_units
            .into_iter()
            .zip(translation_units)
            .map(|(source, translation)| TranslationPair {
                source: source.into(),
                translation: translation.into(),
            })
            .collect();
    }

    vec![TranslationPair {
        source: source.trim().into(),
        translation: translation.trim().into(),
    }]
}

pub(crate) fn translation_units(text: &str) -> Vec<String> {
    let characters = text.chars().collect::<Vec<_>>();
    let mut units = Vec::new();
    let mut current = String::new();
    let mut token = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        current.push(character);
        if character.is_alphabetic() {
            token.push(character);
        }
        let punctuation = matches!(character, '.' | '!' | '?' | '。' | '！' | '？');
        if !punctuation {
            if !character.is_alphabetic() && character != '\'' && character != '’' {
                token.clear();
            }
            continue;
        }

        let next = characters.get(index + 1).copied();
        let numeric_period = character == '.'
            && index > 0
            && characters[index - 1].is_ascii_digit()
            && next.is_some_and(|next| next.is_ascii_digit());
        let abbreviation = character == '.' && is_sentence_abbreviation(&token);
        let boundary = !numeric_period
            && !abbreviation
            && next
                .is_none_or(|next| next.is_whitespace() || matches!(next, '"' | '”' | ')' | ']'));
        token.clear();
        if boundary {
            let unit = current.trim();
            if !unit.is_empty() {
                units.push(unit.to_owned());
            }
            current.clear();
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        units.push(tail.to_owned());
    }
    if units.is_empty() && !text.trim().is_empty() {
        units.push(text.trim().to_owned());
    }
    units
}

pub(crate) fn is_sentence_abbreviation(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "mr" | "mrs" | "ms" | "dr" | "prof" | "sr" | "jr" | "st" | "vs" | "etc" | "e" | "i"
    )
}

pub(crate) fn wire_translation_preview(preview: &TranslationPreview, presenter: &OverlayPresenter) {
    let drag_active = Arc::new(AtomicBool::new(false));
    {
        let preview_weak = preview.as_weak();
        preview.on_pin_state_changed(move |pinned| {
            if let Some(preview) = preview_weak.upgrade()
                && let Some(window_handle) = slint_window_handle(preview.window())
            {
                dispatch_runtime_event(RuntimeEvent::TranslationPreviewPinChanged {
                    window_handle,
                    pinned,
                });
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        preview.on_begin_resize(move |edge| {
            let Some(preview) = preview_weak.upgrade() else {
                return;
            };
            if !preview.get_pinned() {
                return;
            }
            if let Err(error) = begin_slint_window_resize(preview.window(), edge) {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview resize=failed stage=native_resize error={error}"
                ));
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        preview.on_view_mode_changed(move |_| {
            let Some(preview) = preview_weak.upgrade() else {
                return;
            };
            // Showing the original alongside is more content, and the window
            // has to make room for it. This only reconfigured the resize edges
            // of a pinned window, so switching to ORIG or DUAL with a long
            // original left it cut off.
            apply_translation_preview_layout(
                &preview,
                &preview.get_source_text(),
                &preview.get_translated_text(),
            );
            if preview.get_pinned()
                && let Err(error) = configure_slint_translation_resize(&preview, true)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview resize_state=failed pinned=true reason=view_mode error={error}"
                ));
            }
        });
    }
    {
        let preview_weak = preview.as_weak();
        let presenter = presenter.clone();
        let drag_active = Arc::clone(&drag_active);
        preview.on_begin_drag(move || {
            if let Some(preview) = preview_weak.upgrade()
                && preview.get_pinned()
            {
                if let Err(error) = begin_slint_window_drag(preview.window()) {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=native_drag error={error}"
                    ));
                }
                return;
            }
            if drag_active.swap(true, Ordering::AcqRel) {
                return;
            }
            if let Some(preview) = preview_weak.upgrade()
                && let Some(handle) = slint_window_handle(preview.window())
            {
                preview.set_live_glass(true);
                if let Err(error) = rust_platform_windows::enable_translation_live_backdrop(handle)
                {
                    drag_active.store(false, Ordering::Release);
                    preview.set_live_glass(false);
                    let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=enable_live_glass error={error}"
                    ));
                    return;
                }

                let preview_after_frame = preview_weak.clone();
                let presenter_after_frame = presenter.clone();
                let drag_after_frame = Arc::clone(&drag_active);
                let queue_result = presenter.after_next_frame(preview.window(), move || {
                    let Some(preview) = preview_after_frame.upgrade() else {
                        drag_after_frame.store(false, Ordering::Release);
                        return;
                    };
                    if !preview.get_prepared()
                        || !preview.get_live_glass()
                        || slint_window_handle(preview.window()) != Some(handle)
                    {
                        drag_after_frame.store(false, Ordering::Release);
                        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                        return;
                    }

                    if let Err(error) = begin_slint_window_drag(preview.window()) {
                        drag_after_frame.store(false, Ordering::Release);
                        preview.set_live_glass(false);
                        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                        rust_app_core::diagnostics::log_line(&format!(
                            "translation_preview drag=failed stage=native_drag error={error}"
                        ));
                        return;
                    }

                    start_translation_drag_watcher(
                        preview_after_frame,
                        presenter_after_frame,
                        drag_after_frame,
                        handle,
                    );
                });
                if let Err(error) = queue_result {
                    drag_active.store(false, Ordering::Release);
                    preview.set_live_glass(false);
                    let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview drag=failed stage=queue_rendered_frame error={error}"
                    ));
                }
                return;
            }
            drag_active.store(false, Ordering::Release);
        });
    }
}

pub(crate) fn create_translation_preview_slot(
    blur_radius: u32,
) -> Result<TranslationPreviewSlot, String> {
    let preview = Rc::new(
        TranslationPreview::new()
            .map_err(|error| format!("could not create translation window: {error}"))?,
    );
    preview.set_blur_radius(blur_radius as i32);
    let presenter = OverlayPresenter::install(
        preview.window(),
        "translation_preview_overlay",
        OverlayContract::Interactive,
        ContentPolicy::RequiresContent,
        Some(Arc::new(rust_platform_windows::activate_popup_dismiss)),
    )?;
    wire_translation_preview(&preview, &presenter);
    Ok(TranslationPreviewSlot {
        preview,
        presenter,
        attached: Rc::new(Cell::new(false)),
    })
}

pub(crate) fn retire_translation_preview(slot: &TranslationPreviewSlot, reason: &str) {
    slot.preview.set_pinned(false);
    if let Err(error) = configure_slint_translation_resize(&slot.preview, false) {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview resize_state=failed pinned=false reason={reason} error={error}"
        ));
    }
    slot.preview.set_live_glass(false);
    if let Some(handle) = slint_window_handle(slot.preview.window()) {
        let _ = rust_platform_windows::disable_translation_live_backdrop(handle);
        rust_platform_windows::deactivate_popup_dismiss(handle);
    }
    slot.preview.set_prepared(false);
    slot.preview.set_request_id("".into());
    slot.presenter.retire(slot.preview.window(), reason);
}

pub(crate) fn apply_translation_preview_pin_state(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    pinned: bool,
) -> Result<(), String> {
    preview.set_pinned(pinned);
    configure_slint_translation_resize(preview, pinned)?;

    let Some(window_handle) = slint_window_handle(preview.window()) else {
        return Err("translation window handle is not available".to_owned());
    };
    if pinned {
        // A resizable surface must have one compositor owner for its entire
        // pinned lifetime. DWM continuously recomposes this material as the
        // HWND changes size; no captured bitmap participates in resize.
        // The native material is explicitly dark, so its typography must not
        // inherit a contrast decision made from the previous captured frame.
        preview.set_dark_text(false);
        preview.set_live_glass(true);
        rust_platform_windows::enable_translation_live_backdrop(window_handle)?;
        preview.window().request_redraw();
        return Ok(());
    }

    if preview.get_prepared() && preview.get_live_glass() {
        settle_translation_preview_backdrop(preview, presenter, window_handle)
    } else {
        preview.set_live_glass(false);
        rust_platform_windows::disable_translation_live_backdrop(window_handle)
    }
}

pub(crate) fn configure_slint_translation_resize(
    preview: &TranslationPreview,
    resizable: bool,
) -> Result<(), String> {
    use slint::winit_030::winit::dpi::LogicalSize;

    let minimum_height = if preview.get_view_mode() == 1 {
        f64::from(preview.get_original_card_height()) + 98.0
    } else {
        80.0
    };
    preview
        .window()
        .with_winit_window(|window| {
            window.set_min_inner_size(
                resizable.then_some(LogicalSize::new(252.0_f64, minimum_height)),
            );
            window.set_resizable(resizable);
        })
        .ok_or_else(|| "translation window is not backed by winit".to_owned())?;

    // winit rewrites the whole extended style when resizability changes, which
    // takes WS_EX_TOOLWINDOW and WS_EX_NOACTIVATE with it and hands the window
    // WS_EX_APPWINDOW instead — a popup that would appear in the taskbar and in
    // Alt+Tab, and that takes focus from what the user is reading. A parked slot
    // was found in exactly that state; a presented one was only ever saved by
    // the presentation re-applying the contract afterwards.
    let Some(handle) = slint_window_handle(preview.window()) else {
        return Err("translation window handle is not available".to_owned());
    };
    // The styles only. `prepare_translation_window` would also cloak and park
    // the window, which is right for a slot being made ready and wrong here —
    // this runs on pin, and pinning a preview made it vanish.
    rust_platform_windows::configure_action_window(handle)
}

pub(crate) fn begin_slint_window_resize(window: &slint::Window, edge: i32) -> Result<(), String> {
    use slint::winit_030::winit::window::ResizeDirection;

    let direction = match edge {
        0 => ResizeDirection::West,
        1 => ResizeDirection::East,
        2 => ResizeDirection::North,
        3 => ResizeDirection::South,
        4 => ResizeDirection::NorthWest,
        5 => ResizeDirection::NorthEast,
        6 => ResizeDirection::SouthWest,
        7 => ResizeDirection::SouthEast,
        _ => return Err(format!("invalid translation resize edge {edge}")),
    };
    window
        .with_winit_window(|window| {
            window
                .drag_resize_window(direction)
                .map_err(|error| error.to_string())
        })
        .ok_or_else(|| "translation window is not backed by winit".to_owned())?
}

pub(crate) fn start_translation_drag_watcher(
    preview_after_drag: slint::Weak<TranslationPreview>,
    presenter_after_drag: OverlayPresenter,
    drag_active: Arc<AtomicBool>,
    window_handle: u64,
) {
    let watcher_active = Arc::clone(&drag_active);
    let preview_on_spawn_failure = preview_after_drag.clone();
    let presenter_on_spawn_failure = presenter_after_drag.clone();
    let spawn_result = std::thread::Builder::new()
        .name("translation-preview-drag-watch".to_owned())
        .spawn(move || {
            wait_for_primary_pointer_release();
            watcher_active.store(false, Ordering::Release);
            let _ = preview_after_drag.upgrade_in_event_loop(move |preview| {
                if preview.get_prepared()
                    && preview.get_live_glass()
                    && slint_window_handle(preview.window()) == Some(window_handle)
                    && let Err(error) = settle_translation_preview_backdrop(
                        &preview,
                        &presenter_after_drag,
                        window_handle,
                    )
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview backdrop_refresh=failed error={error}"
                    ));
                }
            });
        });
    if let Err(error) = spawn_result {
        drag_active.store(false, Ordering::Release);
        if let Some(preview) = preview_on_spawn_failure.upgrade()
            && let Err(refresh_error) = settle_translation_preview_backdrop(
                &preview,
                &presenter_on_spawn_failure,
                window_handle,
            )
        {
            preview.set_live_glass(false);
            let _ = rust_platform_windows::disable_translation_live_backdrop(window_handle);
            rust_app_core::diagnostics::log_line(&format!(
                "translation_preview backdrop_refresh=failed error={refresh_error}"
            ));
        }
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview drag=failed stage=watcher error={error}"
        ));
    }
}

pub(crate) fn begin_slint_window_drag(window: &slint::Window) -> Result<(), String> {
    window
        .with_winit_window(|window| window.drag_window().map_err(|error| error.to_string()))
        .ok_or_else(|| "translation window is not backed by winit".to_owned())?
}

pub(crate) fn wait_for_primary_pointer_release() {
    let pressed_deadline = Instant::now() + Duration::from_millis(120);
    while !rust_platform_windows::primary_pointer_button_is_down()
        && Instant::now() < pressed_deadline
    {
        std::thread::sleep(Duration::from_millis(2));
    }
    while rust_platform_windows::primary_pointer_button_is_down() {
        std::thread::sleep(Duration::from_millis(8));
    }
}

pub(crate) fn adaptive_text_panel_height(
    text: &str,
    characters_per_line: usize,
    minimum_lines: usize,
    maximum_lines: usize,
    line_height: f32,
    vertical_padding: f32,
) -> f32 {
    let estimated_lines =
        estimated_wrapped_lines(text, characters_per_line).clamp(minimum_lines, maximum_lines);
    estimated_lines as f32 * line_height + vertical_padding
}

/// How wide the card is drawn, from how much there is to read.
///
/// A policy, not a measurement: three widths, so a word does not get a
/// paragraph's window and a paragraph does not get a word's.
#[must_use]
pub(crate) fn translation_preview_card_width(result_text: &str) -> f32 {
    match result_text.chars().count() {
        0..=28 => 252.0,
        29..=140 => 352.0,
        _ => 480.0,
    }
}

/// The window height for a card Slint has already measured.
///
/// This mirrors the `preferred-height` expression on `TranslationPreview` and
/// nothing else. The height used to be estimated here — characters per line,
/// pixels per glyph, a guess at the chrome — which was wrong for DUAL, where
/// every line of the result is paired with a line of the original and the real
/// stack is two or three times what a line count suggests, and wrong again for
/// scripts whose glyphs take two columns.
///
/// Slint has laid the card out and knows both numbers. Asking it is the only
/// way this can be right for a mode nobody thought to special-case.
#[must_use]
pub(crate) fn translation_window_height(
    main_card_height: f32,
    original_card_height: f32,
    view_mode: i32,
) -> f32 {
    let alongside = if view_mode == 1 {
        original_card_height + 18.0
    } else {
        0.0
    };
    (main_card_height + alongside).clamp(96.0, 720.0)
}

/// Sizes the window to the text and tells the card how wide it is.
///
/// A pinned window is left alone: the user has put it where they want it and
/// may have resized it themselves.
pub(crate) fn apply_translation_preview_layout(
    preview: &TranslationPreview,
    _source_text: &str,
    result_text: &str,
) {
    preview.set_card_width(translation_preview_card_width(result_text));
    if preview.get_pinned() {
        return;
    }
    // The original's card, measured at the width it is actually drawn at. It
    // was estimated from a fixed forty-eight characters a line against a card
    // that is usually wider than that, and the last line came out clipped.
    let measured = preview.get_original_content_height();
    if measured > 0.0 {
        preview.set_original_card_height(measured.clamp(48.0, 240.0));
    }
    // Read after the content and the width are set, which is why every caller
    // sets them first. `main-card-height` is Slint's own measurement of the
    // card it has laid out — the interleaved stack included — and it measures
    // whatever text is on it at the moment it is asked.
    let height = translation_window_height(
        preview.get_main_card_height(),
        preview.get_original_card_height(),
        preview.get_view_mode(),
    );
    preview
        .window()
        .set_size(slint::LogicalSize::new(preview.get_card_width(), height));
}

pub(crate) fn refresh_cloaked_translation_backdrop(
    preview: &TranslationPreview,
    window_handle: u64,
) {
    let blur_radius = preview
        .get_blur_radius()
        .clamp(0, MAX_TRANSLATION_BLUR_RADIUS as i32) as u32;
    let result =
        rust_platform_windows::capture_blurred_cloaked_overlay_backdrop(window_handle, blur_radius);
    match result.and_then(|backdrop| apply_translation_backdrop(preview, backdrop)) {
        Ok(()) => preview.set_live_glass(false),
        Err(error) => {
            preview.set_live_glass(true);
            if let Err(live_error) =
                rust_platform_windows::enable_translation_live_backdrop(window_handle)
            {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview backdrop=failed error={error}; live_fallback={live_error}"
                ));
            } else {
                rust_app_core::diagnostics::log_line(&format!(
                    "translation_preview backdrop=captured_failed fallback=live error={error}"
                ));
            }
        }
    }
}

pub(crate) fn settle_translation_preview_backdrop(
    preview: &TranslationPreview,
    presenter: &OverlayPresenter,
    window_handle: u64,
) -> Result<(), String> {
    let ticket = presenter.begin_refresh()?;
    if let Err(error) = rust_platform_windows::disable_translation_live_backdrop(window_handle) {
        presenter.abort_prepare(preview.window(), ticket, "disable_live_glass", &error);
        return Err(error);
    }
    refresh_cloaked_translation_backdrop(preview, window_handle);
    presenter
        .await_frame(preview.window(), ticket)
        .inspect_err(|error| {
            preview.set_prepared(false);
            presenter.abort_prepare(preview.window(), ticket, "drag_refresh", error);
        })
}

pub(crate) fn apply_translation_backdrop(
    preview: &TranslationPreview,
    backdrop: rust_platform_windows::BlurredBackdrop,
) -> Result<(), String> {
    if backdrop.rgba.len() != backdrop.width as usize * backdrop.height as usize * 4 {
        return Err("invalid pixel buffer".to_owned());
    }
    preview.set_dark_text(backdrop_prefers_dark_text(
        &backdrop.rgba,
        preview.get_dark_text(),
    ));
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &backdrop.rgba,
        backdrop.width,
        backdrop.height,
    );
    preview.set_glass_backdrop(Image::from_rgba8(pixels));
    preview.window().request_redraw();
    Ok(())
}

pub(crate) fn backdrop_prefers_dark_text(rgba: &[u8], currently_dark: bool) -> bool {
    const SAMPLE_BUDGET: usize = 4_096;
    const SWITCH_TO_DARK: u8 = 156;
    const SWITCH_TO_LIGHT: u8 = 116;

    let pixels = rgba.len() / 4;
    if pixels == 0 {
        return currently_dark;
    }
    let stride = pixels.div_ceil(SAMPLE_BUDGET).max(1);
    let mut luminances = rgba
        .chunks_exact(4)
        .step_by(stride)
        .map(|pixel| {
            let red = u32::from(pixel[0]);
            let green = u32::from(pixel[1]);
            let blue = u32::from(pixel[2]);
            ((54 * red + 183 * green + 19 * blue) >> 8) as u8
        })
        .collect::<Vec<_>>();
    let midpoint = luminances.len() / 2;
    let (_, median, _) = luminances.select_nth_unstable(midpoint);
    if currently_dark {
        *median >= SWITCH_TO_LIGHT
    } else {
        *median >= SWITCH_TO_DARK
    }
}

pub(crate) fn estimated_wrapped_lines(text: &str, characters_per_line: usize) -> usize {
    text.split('\n')
        .map(|line| {
            let characters = line.chars().count().max(1);
            characters.div_ceil(characters_per_line)
        })
        .sum()
}

pub(crate) fn set_translation_status(translator: &slint::Weak<TranslatorWindow>, message: &str) {
    if let Some(translator) = translator.upgrade() {
        translator.set_status_text(message.into());
    }
}

pub(crate) fn precreate_translation_preview(slot: &TranslationPreviewSlot) {
    slot.preview.set_prepared(false);
    slot.preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    if let Err(error) = slot.preview.show() {
        rust_app_core::diagnostics::log_line(&format!(
            "translation_preview_overlay transition=failed stage=create error={error}"
        ));
        return;
    }

    let preview = slot.preview.as_weak();
    let presenter = slot.presenter.clone();
    let attached = Rc::clone(&slot.attached);
    let _ = slint::spawn_local(async move {
        let Some(preview) = preview.upgrade() else {
            return;
        };
        let result = preview
            .window()
            .winit_window()
            .await
            .ok()
            .as_deref()
            .and_then(native_window_handle)
            .ok_or_else(|| "native translation-preview HWND is unavailable".to_owned())
            .and_then(|handle| {
                rust_platform_windows::prepare_translation_window(handle)?;
                presenter.attach(handle)?;
                configure_slint_translation_resize(&preview, false)
            });
        match result {
            Ok(()) => {
                attached.set(true);
                if !preview.get_request_id().is_empty()
                    && !preview.get_prepared()
                    && let Err(error) = present_translation_preview(&preview, &presenter, None)
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_overlay transition=failed stage=deferred_present error={error}"
                    ));
                }
            }
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "translation_preview_overlay transition=failed stage=attach error={error}"
            )),
        }
    });
}

#[cfg(test)]
mod adaptive_popup_tests {
    use super::{translation_preview_card_width, translation_window_height};

    #[test]
    fn the_card_is_as_wide_as_there_is_to_read() {
        assert_eq!(translation_preview_card_width("Привет"), 252.0);
        assert_eq!(translation_preview_card_width(&"a".repeat(60)), 352.0);
        assert_eq!(translation_preview_card_width(&"a".repeat(300)), 480.0);
    }

    #[test]
    fn the_window_is_the_height_the_card_measured() {
        // Whatever Slint laid out, plus the original's card when it is shown
        // alongside. The height was estimated here before — characters per
        // line, pixels per glyph, a guess at the chrome — and was wrong for
        // DUAL, where every line is paired with its original, and wrong again
        // for scripts whose glyphs take two columns.
        assert_eq!(translation_window_height(140.0, 96.0, 0), 140.0);
        assert_eq!(translation_window_height(140.0, 96.0, 2), 140.0);
        assert_eq!(translation_window_height(140.0, 96.0, 1), 254.0);
    }

    #[test]
    fn a_card_that_measured_absurdly_is_still_a_usable_window() {
        // Slint reports zero before the first layout, and a very long
        // translation can measure taller than a screen.
        assert_eq!(translation_window_height(0.0, 0.0, 0), 96.0);
        assert_eq!(translation_window_height(4000.0, 0.0, 0), 720.0);
    }

    use slint::ComponentHandle;

    use super::{
        TranslationPreview, adaptive_text_panel_height, backdrop_prefers_dark_text,
        translation_pairs, translation_units,
    };

    fn uniform_backdrop(value: u8) -> Vec<u8> {
        [value, value, value, u8::MAX].repeat(5_000)
    }

    #[test]
    fn short_text_uses_the_compact_minimum() {
        assert_eq!(
            adaptive_text_panel_height("Short", 70, 2, 4, 15.0, 16.0),
            46.0
        );
    }

    #[test]
    fn wrapped_text_grows_until_the_defined_cap() {
        let medium = adaptive_text_panel_height(&"word ".repeat(50), 70, 2, 4, 15.0, 16.0);
        let very_long = adaptive_text_panel_height(&"word ".repeat(500), 70, 2, 4, 15.0, 16.0);

        assert_eq!(medium, 76.0);
        assert_eq!(very_long, 76.0);
    }

    #[test]
    fn explicit_lines_are_included_in_the_estimate() {
        assert_eq!(
            adaptive_text_panel_height("one\ntwo\nthree", 70, 1, 6, 15.0, 16.0),
            61.0
        );
    }

    #[test]
    fn backdrop_contrast_uses_dark_text_on_light_material() {
        assert!(backdrop_prefers_dark_text(&uniform_backdrop(230), false));
    }

    #[test]
    fn backdrop_contrast_uses_light_text_on_dark_material() {
        assert!(!backdrop_prefers_dark_text(&uniform_backdrop(40), true));
    }

    #[test]
    fn backdrop_contrast_has_hysteresis_around_midtones() {
        let backdrop = uniform_backdrop(135);
        assert!(!backdrop_prefers_dark_text(&backdrop, false));
        assert!(backdrop_prefers_dark_text(&backdrop, true));
    }

    #[test]
    fn translation_blur_setting_is_bounded_to_the_supported_range() {
        assert_eq!(
            crate::settings_document::parse_settings(br#"{"translation_blur_radius": 0}"#)
                .translation_blur_radius,
            0
        );
        assert_eq!(
            crate::settings_document::parse_settings(br#"{"translation_blur_radius": 3}"#)
                .translation_blur_radius,
            3
        );
        assert_eq!(
            crate::settings_document::parse_settings(br#"{"translation_blur_radius": 200}"#)
                .translation_blur_radius,
            super::MAX_TRANSLATION_BLUR_RADIUS
        );
    }

    #[test]
    fn dual_mode_pairs_matching_sentences_in_order() {
        let pairs = translation_pairs(
            "First sentence. Second sentence! Third sentence?",
            "Первое предложение. Второе предложение! Третье предложение?",
        );
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1].source.as_str(), "Second sentence!");
        assert_eq!(pairs[1].translation.as_str(), "Второе предложение!");
    }

    #[test]
    fn dual_mode_keeps_an_unpunctuated_phrase_as_one_pair() {
        let pairs = translation_pairs("order number", "номер заказа");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].source.as_str(), "order number");
        assert_eq!(pairs[0].translation.as_str(), "номер заказа");
    }

    #[test]
    fn dual_mode_does_not_claim_alignment_when_sentence_counts_differ() {
        let pairs = translation_pairs(
            "One sentence. A second sentence.",
            "Один объединённый перевод без точки",
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].source.as_str(), "One sentence. A second sentence.");
    }

    #[test]
    fn sentence_splitter_preserves_common_titles_and_decimal_numbers() {
        assert_eq!(
            translation_units("Mr. Ratchett paid 3.5 dollars. Then he left."),
            vec!["Mr. Ratchett paid 3.5 dollars.", "Then he left."]
        );
    }

    #[test]
    fn wrapped_preview_uses_intrinsic_text_height_and_exposes_scroll_range() {
        let preview = TranslationPreview::new().unwrap();
        preview
            .window()
            .set_size(slint::LogicalSize::new(480.0, 302.0));
        preview.set_card_width(480.0);
        preview.set_translated_text(
            ("Длинный переведённый текст с пропорциональным шрифтом и переносами. ".repeat(120))
                .into(),
        );

        assert_eq!(preview.get_result_panel_height(), 252.0);
        assert!(
            preview.get_result_content_height() > preview.get_result_panel_height(),
            "wrapped text must remain reachable below the capped viewport"
        );
        assert!(preview.get_result_scrollable());

        preview.invoke_reset_scroll();
        assert_eq!(preview.get_result_scroll_offset(), 0.0);

        preview.invoke_scroll_by(-120.0);
        assert_eq!(preview.get_result_scroll_offset(), 48.0);

        for _ in 0..1_000 {
            preview.invoke_scroll_by(-120.0);
        }
        let maximum = preview.get_result_content_height() - preview.get_result_viewport_height();
        assert!(
            (preview.get_result_scroll_offset() - maximum).abs() < 0.01,
            "offset={} maximum={} content={} viewport={}",
            preview.get_result_scroll_offset(),
            maximum,
            preview.get_result_content_height(),
            preview.get_result_viewport_height(),
        );

        for _ in 0..1_000 {
            preview.invoke_scroll_by(120.0);
        }
        assert_eq!(preview.get_result_scroll_offset(), 0.0);
    }
}
