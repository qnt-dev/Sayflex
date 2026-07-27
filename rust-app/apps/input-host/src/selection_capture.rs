use rust_protocol::TargetSnapshot;
use thiserror::Error;

use crate::{
    clipboard::{self, ClipboardCaptureMethod},
    inject, target,
    uia_selection::{self, SelectionOutcome},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMethod {
    UiAutomation,
    WindowMessage,
    Shortcut(target::SelectionCopyShortcut),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CapturedSelection {
    pub text: String,
    pub method: CaptureMethod,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("selected text is protected")]
    Protected,
    #[error(transparent)]
    Clipboard(#[from] clipboard::ClipboardError),
}

pub fn capture(target: TargetSnapshot) -> Result<CapturedSelection, CaptureError> {
    if !inject::is_foreground(target) {
        return Err(clipboard::ClipboardError::TargetChanged.into());
    }

    match uia_selection::selected_text(target) {
        Ok(SelectionOutcome::Text(text)) if !text.is_empty() => {
            return Ok(CapturedSelection {
                text,
                method: CaptureMethod::UiAutomation,
            });
        }
        Ok(SelectionOutcome::Protected) => return Err(CaptureError::Protected),
        Ok(SelectionOutcome::Text(_)) | Ok(SelectionOutcome::Unavailable) | Err(_) => {}
    }

    let shortcut = target::selection_copy_shortcut(target);
    let captured = clipboard::capture_selection(
        || inject::is_foreground(target),
        || inject::copy_message(target).map_err(|_| clipboard::ClipboardError::InvalidData),
        || inject::copy_shortcut(shortcut).map_err(|_| clipboard::ClipboardError::InvalidData),
    )?;
    let method = match captured.method {
        ClipboardCaptureMethod::WindowMessage => CaptureMethod::WindowMessage,
        ClipboardCaptureMethod::Shortcut => CaptureMethod::Shortcut(shortcut),
    };
    Ok(CapturedSelection {
        text: captured.text,
        method,
    })
}
