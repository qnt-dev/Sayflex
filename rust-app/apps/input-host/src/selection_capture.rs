use rust_protocol::TargetSnapshot;
use std::time::Instant;
use thiserror::Error;

use crate::{
    clipboard::{self, ClipboardCaptureMethod},
    inject, target,
    uia_selection::{self, PendingSelection, SelectionError, SelectionOutcome},
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
    pub uia_ms: Option<u64>,
    pub clipboard_ms: Option<u64>,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("selected text is protected")]
    Protected,
    #[error("the translation hotkey was not fully released before capture")]
    HotkeyReleaseTimeout,
    #[error(transparent)]
    Clipboard(#[from] clipboard::ClipboardError),
}

pub fn capture(target: TargetSnapshot) -> Result<CapturedSelection, CaptureError> {
    let pending_uia = uia_selection::begin_selected_text(target);
    capture_after_release(target, pending_uia)
}

pub fn begin_uia_capture(target: TargetSnapshot) -> Result<PendingSelection, SelectionError> {
    uia_selection::begin_selected_text(target)
}

pub fn capture_after_release(
    target: TargetSnapshot,
    pending_uia: Result<PendingSelection, SelectionError>,
) -> Result<CapturedSelection, CaptureError> {
    if !inject::is_foreground(target) {
        return Err(clipboard::ClipboardError::TargetChanged.into());
    }

    let uia_attempt = match pending_uia {
        Ok(pending) => pending.wait(),
        Err(error) => uia_selection::TimedSelectionAttempt {
            elapsed_ms: 0,
            result: Err(error),
        },
    };
    let uia_ms = Some(uia_attempt.elapsed_ms);
    match uia_attempt.result {
        Ok(SelectionOutcome::Text(text)) if !text.is_empty() => {
            crate::diagnostic_line(&format!(
                "uia_selection target_pid={} result=success chars={} elapsed_ms={}",
                target.process_id,
                text.chars().count(),
                uia_attempt.elapsed_ms,
            ));
            return Ok(CapturedSelection {
                text,
                method: CaptureMethod::UiAutomation,
                uia_ms,
                clipboard_ms: None,
            });
        }
        Ok(SelectionOutcome::Protected) => {
            crate::diagnostic_line(&format!(
                "uia_selection target_pid={} result=protected",
                target.process_id
            ));
            return Err(CaptureError::Protected);
        }
        Ok(SelectionOutcome::Text(_)) | Ok(SelectionOutcome::Unavailable) => {
            crate::diagnostic_line(&format!(
                "uia_selection target_pid={} result=unavailable fallback=clipboard",
                target.process_id
            ));
        }
        Err(error) => {
            crate::diagnostic_line(&format!(
                "uia_selection target_pid={} result=failed error={error} fallback=clipboard",
                target.process_id
            ));
        }
    }

    let shortcut = target::selection_copy_shortcut(target);
    let clipboard_started = Instant::now();
    let captured = clipboard::capture_selection(
        || inject::is_foreground(target),
        || inject::copy_message(target).map_err(|_| clipboard::ClipboardError::InvalidData),
        || inject::copy_shortcut(shortcut).map_err(|_| clipboard::ClipboardError::InvalidData),
    )?;
    let clipboard_ms = clipboard_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let method = match captured.method {
        ClipboardCaptureMethod::WindowMessage => CaptureMethod::WindowMessage,
        ClipboardCaptureMethod::Shortcut => CaptureMethod::Shortcut(shortcut),
    };
    Ok(CapturedSelection {
        text: captured.text,
        method,
        uia_ms,
        clipboard_ms: Some(clipboard_ms),
    })
}
