//! Deterministic, worker-free fixtures for the screen-translation visual review.
//!
//! This module is intentionally kept separate from the production coordinator:
//! opening a visual fixture must never register a hotkey, start OCR, or retain a
//! screenshot selected by the user. The bundled GPL image is only a deterministic
//! visual-review fixture.

use std::error::Error;

use slint::{ComponentHandle, LogicalSize};

use crate::ScreenTranslationPreviewWindow;

const PREVIEW_WIDTH: f32 = 960.0;
const PREVIEW_HEIGHT: f32 = 614.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenTranslationPreviewState {
    Result,
    Editing,
    Saved,
    SaveError,
    Expired,
}

impl ScreenTranslationPreviewState {
    #[cfg(test)]
    const ALL: [Self; 5] = [
        Self::Result,
        Self::Editing,
        Self::Saved,
        Self::SaveError,
        Self::Expired,
    ];

    fn slint_value(self) -> i32 {
        match self {
            Self::Result => 0,
            Self::Editing => 1,
            Self::Saved => 2,
            Self::SaveError => 3,
            Self::Expired => 4,
        }
    }

    #[cfg(test)]
    fn slug(self) -> &'static str {
        match self {
            Self::Result => "image-result",
            Self::Editing => "editing",
            Self::Saved => "saved",
            Self::SaveError => "save-error",
            Self::Expired => "expired-image",
        }
    }
}

pub(crate) fn requested_state() -> Option<ScreenTranslationPreviewState> {
    std::env::args().find_map(|argument| {
        let value = argument.strip_prefix("--screen-translation-preview=")?;
        match value {
            "result" | "image-result" => Some(ScreenTranslationPreviewState::Result),
            "editing" => Some(ScreenTranslationPreviewState::Editing),
            "saved" => Some(ScreenTranslationPreviewState::Saved),
            "save-error" | "error" => Some(ScreenTranslationPreviewState::SaveError),
            "expired" | "expired-image" => Some(ScreenTranslationPreviewState::Expired),
            _ => None,
        }
    })
}

pub(crate) fn run_preview() -> Result<(), Box<dyn Error>> {
    let state = requested_state().unwrap_or(ScreenTranslationPreviewState::Result);
    let window = configured_window(state)?;
    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}

fn configured_window(
    state: ScreenTranslationPreviewState,
) -> Result<ScreenTranslationPreviewWindow, slint::PlatformError> {
    let window = ScreenTranslationPreviewWindow::new()?;
    window.set_preview_state(state.slint_value());
    window.set_editor_visible(matches!(
        state,
        ScreenTranslationPreviewState::Editing | ScreenTranslationPreviewState::SaveError
    ));
    window.set_view_mode(3);
    window
        .window()
        .set_size(LogicalSize::new(PREVIEW_WIDTH, PREVIEW_HEIGHT));
    Ok(window)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::BufWriter, path::Path};

    use slint::{ComponentHandle, LogicalSize};

    use super::{PREVIEW_HEIGHT, PREVIEW_WIDTH, ScreenTranslationPreviewState, configured_window};

    const SOURCE: &str = include_str!("../ui/screen_translation.slint");
    const APP_SOURCE: &str = include_str!("../ui/app.slint");
    const DRIVER: &str = include_str!("screen_translation_preview.rs");

    #[test]
    fn preview_window_is_frameless_but_does_not_stay_above_other_apps() {
        for contract in [
            "no-frame: true",
            "always-on-top: false",
            "background: transparent",
            "in-out property <int> view-mode",
            "in-out property <bool> pinned",
            "in-out property <bool> editor-visible",
            "callback choose-view(int)",
            "callback toggle-pin()",
            "callback edit-block(int)",
            "callback save-edit()",
            "callback confirm-edit()",
            "callback cancel-edit()",
            "callback retry-save()",
        ] {
            assert!(
                SOURCE.contains(contract),
                "missing live preview contract: {contract}"
            );
        }
        let run_preview = DRIVER
            .split("pub(crate) fn run_preview")
            .nth(1)
            .expect("run_preview should exist")
            .split("fn configured_window")
            .next()
            .expect("configured_window should follow run_preview");
        assert!(
            !run_preview.contains("configure_standard_app_window"),
            "the visual preview must not be wrapped in the old standard window chrome"
        );
        assert!(
            !SOURCE.contains("preferred-width: 1120px"),
            "the preview must use the compact overlay proportions"
        );
    }

    #[test]
    fn preview_actions_change_the_visible_state() {
        let window = configured_window(ScreenTranslationPreviewState::Result)
            .expect("preview should instantiate");

        let surface_size = window.window().size();
        assert_eq!(surface_size.width, PREVIEW_WIDTH as u32);
        assert_eq!(surface_size.height, PREVIEW_HEIGHT as u32);
        assert!(!window.get_dim_other_blocks());
        assert!(!window.get_save_confirmation_visible());

        assert_eq!(window.get_view_mode(), 3);
        window.invoke_choose_view(1);
        assert_eq!(window.get_view_mode(), 1);
        window.invoke_choose_view(2);
        assert_eq!(window.get_view_mode(), 2);
        window.invoke_choose_view(3);
        assert_eq!(window.get_view_mode(), 3);

        assert!(!window.get_pinned());
        window.invoke_toggle_pin();
        assert!(window.get_pinned());
        window.invoke_toggle_pin();
        assert!(!window.get_pinned());

        window.invoke_select_block(3);
        assert_eq!(window.get_selected_block(), 3);
        assert!(!window.get_copied());
        window.invoke_copy_text();
        assert!(window.get_copied());

        assert_eq!(window.get_unit_translation(), "Кочевник · быстрый, стойкий");
        assert_eq!(window.get_end_turn_translation(), "Завершить ход");
        assert!(!window.get_editor_visible());
        window.invoke_keyboard_edit_selected();
        assert!(window.get_editor_visible());
        assert!(window.get_dim_other_blocks());
        assert_eq!(window.get_editor_panel_x(), 488.0);
        assert_eq!(window.get_editor_panel_y(), 292.0);
        assert_eq!(
            window.get_editor_original(),
            "Khalifate Human · quick, resilient"
        );
        assert_eq!(
            window.get_editor_translation(),
            "Кочевник · быстрый, стойкий"
        );
        assert_eq!(
            window.window().size(),
            surface_size,
            "opening correction must not create or reveal a second side window"
        );
        window.set_editor_translation("Воин-кочевник · быстрый, стойкий".into());
        window.invoke_commit_edit();
        assert_eq!(
            window.get_unit_translation(),
            "Воин-кочевник · быстрый, стойкий"
        );
        assert_eq!(
            window.get_end_turn_translation(),
            "Завершить ход",
            "saving one image block must not overwrite another block"
        );
        assert_eq!(window.get_preview_state(), 2);
        assert!(window.get_save_confirmation_visible());
        assert!(!window.get_editor_visible());
        assert!(!window.get_dim_other_blocks());

        window.invoke_select_block(4);
        assert!(!window.get_editor_visible());
        window.invoke_edit_block(4);
        assert_eq!(window.get_editor_original(), "End Turn");
        assert_eq!(window.get_editor_translation(), "Завершить ход");
        window.set_editor_translation("Закончить ход".into());
        window.invoke_save_edit();
        assert_eq!(window.get_end_turn_translation(), "Закончить ход");

        window.invoke_edit_block(4);
        window.invoke_cancel_edit();
        assert!(!window.get_editor_visible());
        assert!(!window.get_save_confirmation_visible());

        window.invoke_edit_block(4);
        window.invoke_save_edit();
        assert_eq!(window.get_preview_state(), 2);
        assert!(!window.get_editor_visible());

        window.set_preview_state(3);
        window.set_editor_visible(true);
        assert!(!window.get_save_confirmation_visible());
        window.invoke_retry_save();
        assert_eq!(window.get_preview_state(), 2);
        assert!(window.get_save_confirmation_visible());
        assert!(!window.get_editor_visible());

        let editing = configured_window(ScreenTranslationPreviewState::Editing)
            .expect("editing fixture should instantiate");
        assert!(editing.get_editor_visible());
        assert_eq!(editing.get_preview_state(), 1);

        let save_error = configured_window(ScreenTranslationPreviewState::SaveError)
            .expect("save-error fixture should instantiate");
        assert!(save_error.get_editor_visible());
        assert_eq!(save_error.get_preview_state(), 3);

        let expired = configured_window(ScreenTranslationPreviewState::Expired)
            .expect("expired fixture should instantiate");
        assert!(!expired.get_editor_visible());
        assert_eq!(expired.get_preview_state(), 4);

        window.invoke_select_block(0);
        assert!(!window.get_editor_visible());
        window.invoke_edit_block(0);
        assert_eq!(window.get_editor_original(), "Menu");
        assert_eq!(window.get_editor_translation(), "Меню");
        assert_eq!(window.get_editor_panel_x(), 164.0);
        assert_eq!(window.get_editor_panel_y(), 30.0);

        assert_eq!(window.get_drag_request_count(), 0);
        assert_eq!(window.get_close_request_count(), 0);
        assert_eq!(window.get_delete_request_count(), 0);

        window.invoke_request_drag();
        window.invoke_request_close();
        window.invoke_request_delete_image();

        assert_eq!(window.get_drag_request_count(), 1);
        assert_eq!(window.get_close_request_count(), 1);
        assert_eq!(window.get_delete_request_count(), 1);
    }

    #[test]
    fn real_preview_asset_is_tracked_and_attributed() {
        let asset = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/battle-for-wesnoth-1.12-screen-translation-preview.jpg");
        let bytes = fs::read(&asset).expect("the real preview screenshot should be tracked");
        assert_eq!(bytes.len(), 551_908, "the approved source image changed");
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "preview asset must remain JPEG");

        let attribution = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../docs/evidence/screen-translation-preview/ATTRIBUTION.md"),
        )
        .expect("the GPL screenshot attribution should be tracked");
        assert!(attribution.contains("Battle for Wesnoth"));
        assert!(attribution.contains("GNU General Public License"));
        assert!(attribution.contains("Wikimedia Commons"));
    }

    #[test]
    fn visible_controls_are_wired_and_share_the_current_overlay_theme() {
        assert!(
            APP_SOURCE.contains("import { Theme } from \"overlay_theme.slint\"")
                && SOURCE.contains("import { Theme } from \"overlay_theme.slint\""),
            "the existing translation card and screen preview must consume one Theme owner"
        );
        for wiring in [
            "root.view-requested(1)",
            "root.view-requested(2)",
            "root.view-requested(3)",
            "root.pin-requested()",
            "view-requested(mode) => { root.choose-view(mode); }",
            "pin-requested => { root.toggle-pin(); }",
            "block-edit-requested(block) => { root.edit-block(block); }",
            "save-requested => { root.save-edit(); }",
            "confirm-requested => { root.confirm-edit(); }",
            "cancel-requested => { root.cancel-edit(); }",
            "retry-requested => { root.retry-save(); }",
            "copy-text-requested => { root.copy-text(); }",
            "save-file-requested => { root.save-file(); }",
            "begin-drag-requested => { root.request-drag(); }",
            "close-requested => { root.request-close(); }",
            "delete-image-requested => { root.request-delete-image(); }",
        ] {
            assert!(
                SOURCE.contains(wiring),
                "visible control is not wired: {wiring}"
            );
        }
    }

    #[test]
    fn preview_surface_keeps_the_approved_actions_and_states() {
        for label in [
            "ORIG",
            "DUAL",
            "ON IMAGE",
            "PIN",
            "Save file",
            "Delete image",
            "Save",
            "Confirm",
            "Retry",
            "Saved to glossary",
            "Image expired",
        ] {
            assert!(SOURCE.contains(label), "missing preview contract: {label}");
        }
    }

    #[test]
    fn preview_surface_declares_accessible_controls_and_readable_overlay_text() {
        assert!(SOURCE.contains("accessible-role: button"));
        assert!(SOURCE.contains("accessible-label"));
        assert!(SOURCE.contains("minimum-overlay-font-size: 12px"));
        assert!(SOURCE.contains("measured-content-height"));
        assert!(SOURCE.contains("inline-editor := TranslationCorrectionPanel"));
        assert!(!SOURCE.contains("overflow-panel"));
        assert!(!SOURCE.contains("dense-panel"));
        assert!(SOURCE.contains("for mask in root.masks"));
        assert!(
            SOURCE.contains("background: #120e09ff"),
            "replacement masks must fully hide the original glyphs"
        );
        assert!(SOURCE.contains("double-clicked => { root.edit-requested(); }"));
        assert!(
            !SOURCE.contains("Double-click a translation to edit"),
            "the screen-result header must not carry a permanent editing hint"
        );
        assert!(
            SOURCE.contains("init => { translation-input.focus(); }"),
            "the in-image editor must accept typing immediately after double-click"
        );
        assert!(
            !SOURCE.contains("label: \"Correct\""),
            "editing must be opened by double-clicking the translated text, not a floating button"
        );
    }

    #[test]
    fn editor_is_embedded_in_the_screen_result_without_a_parallel_window() {
        assert!(SOURCE.contains("export component ScreenImageResult"));
        assert!(SOURCE.contains("export component TranslationCorrectionPanel"));
        assert!(SOURCE.contains("editor-focusable"));
        assert!(SOURCE.contains("inline-editor := TranslationCorrectionPanel"));
        assert!(!SOURCE.contains("TranslationCorrectionWindow"));
        assert!(!SOURCE.contains("external-editor"));
    }

    #[test]
    fn render_screen_translation_artifacts() {
        let Some(output_dir) = std::env::var_os("SAYFLEX_SCREEN_PREVIEW_DIR") else {
            return;
        };
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .expect("testing platform should initialize once");

        let output_dir = Path::new(&output_dir);
        fs::create_dir_all(output_dir).expect("preview directory should be creatable");
        for state in ScreenTranslationPreviewState::ALL {
            let window = configured_window(state).expect("preview should instantiate");
            window
                .show()
                .expect("preview should show on the headless backend");
            window
                .window()
                .set_size(LogicalSize::new(PREVIEW_WIDTH, PREVIEW_HEIGHT));
            let frame = window
                .window()
                .take_snapshot()
                .expect("preview should render");
            for scale_percent in [100_u32, 125, 200] {
                let target_width = frame.width() * scale_percent / 100;
                let target_height = frame.height() * scale_percent / 100;
                let pixels = resize_rgba(
                    frame.as_bytes(),
                    frame.width(),
                    frame.height(),
                    target_width,
                    target_height,
                );
                let path = output_dir.join(format!(
                    "screen-translation-{}-{}pct.png",
                    state.slug(),
                    scale_percent
                ));
                let file = fs::File::create(&path).expect("preview file should be creatable");
                let mut encoder =
                    png::Encoder::new(BufWriter::new(file), target_width, target_height);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .expect("PNG header should be valid")
                    .write_image_data(&pixels)
                    .expect("PNG pixels should be written");
            }
            drop(window);
        }
    }

    fn resize_rgba(
        source: &[u8],
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> Vec<u8> {
        if source_width == target_width && source_height == target_height {
            return source.to_vec();
        }
        let mut target = vec![0_u8; (target_width * target_height * 4) as usize];
        let x_ratio = source_width as f32 / target_width as f32;
        let y_ratio = source_height as f32 / target_height as f32;
        for target_y in 0..target_height {
            let source_y = ((target_y as f32 + 0.5) * y_ratio - 0.5)
                .clamp(0.0, source_height.saturating_sub(1) as f32);
            let y0 = source_y.floor() as u32;
            let y1 = (y0 + 1).min(source_height - 1);
            let fy = source_y - y0 as f32;
            for target_x in 0..target_width {
                let source_x = ((target_x as f32 + 0.5) * x_ratio - 0.5)
                    .clamp(0.0, source_width.saturating_sub(1) as f32);
                let x0 = source_x.floor() as u32;
                let x1 = (x0 + 1).min(source_width - 1);
                let fx = source_x - x0 as f32;
                let target_offset = ((target_y * target_width + target_x) * 4) as usize;
                for channel in 0..4 {
                    let top_left = source[((y0 * source_width + x0) * 4) as usize + channel] as f32;
                    let top_right =
                        source[((y0 * source_width + x1) * 4) as usize + channel] as f32;
                    let bottom_left =
                        source[((y1 * source_width + x0) * 4) as usize + channel] as f32;
                    let bottom_right =
                        source[((y1 * source_width + x1) * 4) as usize + channel] as f32;
                    let top = top_left + (top_right - top_left) * fx;
                    let bottom = bottom_left + (bottom_right - bottom_left) * fx;
                    target[target_offset + channel] =
                        (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        target
    }
}
