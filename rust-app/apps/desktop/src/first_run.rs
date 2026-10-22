//! First-run consent is resolved before any worker is started.
//!
//! The model catalog owns the name and byte count. This module only presents
//! that information and returns the user's explicit choice; it does not start
//! a download itself.

use std::{cell::Cell, error::Error, rc::Rc};

use rust_model_manager::BuiltInCatalog;
use slint::ComponentHandle;

use crate::{FirstRunWindow, configure_standard_app_window, settings_document::FirstRunChoice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecommendedSpeechModel {
    pub(crate) id: &'static str,
    pub(crate) name: String,
    pub(crate) size: String,
}

/// The model that suits this computer. On the processor, Large v3 Turbo took
/// 11-15 s per phrase and Small about 2 s (audit 2026-10-21, M4), so a computer
/// without a dedicated graphics card starts with Small. A user can switch.
const CPU_SPEECH_MODEL: &str = "whisper-small";
const GPU_SPEECH_MODEL: &str = "whisper-large-v3-turbo";

pub(crate) fn recommended_speech_model_id(has_dedicated_gpu: bool) -> &'static str {
    if has_dedicated_gpu {
        GPU_SPEECH_MODEL
    } else {
        CPU_SPEECH_MODEL
    }
}

pub(crate) fn recommended_speech_model() -> Result<RecommendedSpeechModel, Box<dyn Error>> {
    recommended_speech_model_for(rust_platform_windows::has_dedicated_gpu())
}

fn recommended_speech_model_for(
    has_dedicated_gpu: bool,
) -> Result<RecommendedSpeechModel, Box<dyn Error>> {
    let catalog = BuiltInCatalog::load()?;
    let id = recommended_speech_model_id(has_dedicated_gpu);
    let model = catalog.find(id).unwrap_or_else(|| catalog.recommended());
    Ok(RecommendedSpeechModel {
        id,
        name: model.display_name.clone(),
        size: decimal_gigabytes(model.artifact.bytes),
    })
}

fn decimal_gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
}

pub(crate) fn configure_first_run_window(
    window: &FirstRunWindow,
    download_recommended: bool,
) -> Result<&'static str, Box<dyn Error>> {
    let model = recommended_speech_model()?;
    window.set_model_name(model.name.into());
    window.set_model_size(model.size.into());
    window.set_download_recommended(download_recommended);
    Ok(model.id)
}

pub(crate) fn request_first_run_choice() -> Result<Option<FirstRunChoice>, Box<dyn Error>> {
    let window = FirstRunWindow::new()?;
    let speech_model_id = configure_first_run_window(&window, true)?;
    let decision = Rc::new(Cell::new(None));
    let decision_from_continue = Rc::clone(&decision);
    let weak = window.as_weak();
    window.on_continue_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        decision_from_continue.set(Some(FirstRunChoice {
            download_recommended_speech_model: window.get_download_recommended(),
            speech_model_id,
        }));
        let _ = slint::quit_event_loop();
    });

    window.show()?;
    configure_standard_app_window(&window, "First run");
    slint::run_event_loop()?;
    window.hide()?;
    Ok(decision.get())
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use i_slint_backend_testing::ElementHandle;
    use slint::ComponentHandle;

    use crate::FirstRunWindow;

    use super::{configure_first_run_window, decimal_gigabytes, recommended_speech_model_for};

    #[test]
    fn first_run_uses_the_catalog_name_and_human_download_size() {
        let model = recommended_speech_model_for(true).unwrap();

        assert_eq!(model.id, "whisper-large-v3-turbo");
        assert_eq!(model.name, "Whisper Large v3 Turbo");
        assert_eq!(model.size, "1.6 GB");
        assert_eq!(decimal_gigabytes(1_000_000_000), "1.0 GB");
    }

    #[test]
    fn a_computer_without_a_dedicated_gpu_starts_with_the_small_model() {
        let model = recommended_speech_model_for(false).unwrap();

        assert_eq!(model.id, "whisper-small");
        assert_eq!(model.name, "Whisper Small");
        assert_eq!(model.size, "0.5 GB");
    }

    #[test]
    fn first_run_choice_is_accessible_and_continue_is_explicit() {
        i_slint_backend_testing::init_no_event_loop();
        let window = FirstRunWindow::new().expect("first-run surface");
        configure_first_run_window(&window, true).expect("catalog model");
        window.show().expect("show first-run surface");

        let choice = ElementHandle::find_by_accessible_label(
            &window,
            "Download the recommended local model",
        )
        .next()
        .expect("download choice");
        assert_eq!(choice.accessible_checked(), Some(true));
        choice.invoke_accessible_default_action();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        assert_eq!(choice.accessible_checked(), Some(false));

        let confirmations = Rc::new(Cell::new(0));
        let confirmations_from_callback = Rc::clone(&confirmations);
        window.on_continue_requested(move || {
            confirmations_from_callback.set(confirmations_from_callback.get() + 1);
        });
        let continue_action = ElementHandle::find_by_accessible_label(&window, "Continue")
            .next()
            .expect("explicit continue action");
        continue_action.invoke_accessible_default_action();
        assert_eq!(confirmations.get(), 1);
    }
}
