use rust_protocol::{InsertionDelivery, InsertionMethod, TargetSnapshot};

use std::time::{Duration, Instant};

use crate::{clipboard, inject, target, uia_selection, wezterm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub method: InsertionMethod,
    pub delivery: InsertionDelivery,
    pub clipboard_receipt: Option<clipboard::ClipboardPasteReceipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryFailureState {
    NotDelivered,
    Superseded,
    Ambiguous,
}

#[derive(Debug)]
pub struct DeliveryFailure {
    pub method: InsertionMethod,
    pub state: DeliveryFailureState,
    pub message: String,
}

/// Routes one immutable payload by target capability. A route is attempted at
/// most once. In particular, an ambiguous specialized-adapter result is never
/// followed by a second transport, because doing so can duplicate text.
pub fn deliver(
    clipboard_host: &mut clipboard::ClipboardPasteHost,
    target_snapshot: TargetSnapshot,
    payload: &str,
) -> Result<DeliveryReceipt, DeliveryFailure> {
    let force_clipboard_transaction =
        std::env::var("SAYFLEX_INSERTION_SMOKE_FORCE_CLIPBOARD_TRANSACTION").as_deref() == Ok("1");

    if !force_clipboard_transaction && target::is_wezterm_target(target_snapshot) {
        return wezterm::send_text(target_snapshot, payload)
            .map(|()| DeliveryReceipt {
                method: InsertionMethod::WezTermCli,
                delivery: InsertionDelivery::Committed,
                clipboard_receipt: None,
            })
            .map_err(|error| DeliveryFailure {
                method: InsertionMethod::WezTermCli,
                state: if error.delivery_is_ambiguous() {
                    DeliveryFailureState::Ambiguous
                } else if error.target_was_superseded() {
                    DeliveryFailureState::Superseded
                } else {
                    DeliveryFailureState::NotDelivered
                },
                message: error.to_string(),
            });
    }

    match if force_clipboard_transaction {
        Ok(false)
    } else {
        inject::insert_native_edit(target_snapshot, payload, false)
    } {
        Ok(true) => {
            return Ok(DeliveryReceipt {
                method: InsertionMethod::NativeEditMessage,
                delivery: InsertionDelivery::Committed,
                clipboard_receipt: None,
            });
        }
        Ok(false) => {}
        Err(error) => {
            return Err(DeliveryFailure {
                method: InsertionMethod::NativeEditMessage,
                state: DeliveryFailureState::Ambiguous,
                message: format!("native edit adapter failed: {error}"),
            });
        }
    }

    let shortcut = target::paste_shortcut(target_snapshot);
    // UI Automation is an optional acknowledgement channel, not permission to
    // attempt a paste. Electron/Chromium providers can temporarily expose no
    // readable text even though their focused editor accepts Ctrl+V normally.
    let verification_disabled_for_smoke =
        std::env::var("SAYFLEX_INSERTION_SMOKE_DISABLE_UIA_VERIFY").as_deref() == Ok("1");
    let baseline = if verification_disabled_for_smoke {
        None
    } else {
        observed_focused_text(target_snapshot)
    };
    let verification_available = baseline.is_some();
    let mut last_observation = Instant::now() - Duration::from_millis(75);
    clipboard::dispatch_paste(
        clipboard_host,
        || inject::is_foreground(target_snapshot),
        verification_available,
        || {
            let Some(baseline) = baseline.as_deref() else {
                return false;
            };
            if last_observation.elapsed() < Duration::from_millis(60) {
                return false;
            }
            last_observation = Instant::now();
            observed_focused_text(target_snapshot)
                .is_some_and(|current| payload_was_inserted(baseline, &current, payload))
        },
        || inject::paste_shortcut(shortcut).map_err(|_| clipboard::ClipboardError::Paste),
        target_snapshot.process_id,
        payload,
    )
    .map(|receipt| {
        let delivery = match receipt.disposition {
            clipboard::ClipboardPasteDisposition::VerifiedAndRestored
            | clipboard::ClipboardPasteDisposition::VerifiedAndSuperseded => {
                InsertionDelivery::Verified
            }
            clipboard::ClipboardPasteDisposition::PayloadReadAndRestored
            | clipboard::ClipboardPasteDisposition::PayloadReadAndSuperseded => {
                InsertionDelivery::PayloadRead
            }
            clipboard::ClipboardPasteDisposition::DispatchedUnverifiedAndRestored => {
                InsertionDelivery::Dispatched
            }
        };
        DeliveryReceipt {
            method: InsertionMethod::ClipboardTransaction,
            delivery,
            clipboard_receipt: Some(receipt),
        }
    })
    .map_err(|error| DeliveryFailure {
        method: InsertionMethod::ClipboardTransaction,
        state: match error {
            clipboard::ClipboardError::TargetChanged | clipboard::ClipboardError::Superseded => {
                DeliveryFailureState::Superseded
            }
            clipboard::ClipboardError::NotDelivered
            | clipboard::ClipboardError::Paste
            | clipboard::ClipboardError::Busy
            | clipboard::ClipboardError::Allocation
            | clipboard::ClipboardError::NoText
            | clipboard::ClipboardError::CopyTimeout
            | clipboard::ClipboardError::InvalidData
            | clipboard::ClipboardError::Ole(_) => DeliveryFailureState::NotDelivered,
        },
        message: error.to_string(),
    })
}

fn observed_focused_text(target: TargetSnapshot) -> Option<String> {
    match uia_selection::focused_text(target) {
        Ok(uia_selection::FocusedTextOutcome::Text(text)) => Some(text),
        Ok(uia_selection::FocusedTextOutcome::Unavailable)
        | Ok(uia_selection::FocusedTextOutcome::Protected)
        | Err(_) => None,
    }
}

fn payload_was_inserted(baseline: &str, current: &str, payload: &str) -> bool {
    let normalize = |value: &str| value.replace("\r\n", "\n");
    let current = normalize(current);
    // SayFlex may append one convenience space after dictation. Accessibility
    // providers (notably browser editors) are allowed to trim that terminal
    // whitespace, so it cannot be part of the delivery proof. The immutable
    // transcript body still must appear as a new exact occurrence.
    let payload = normalize(payload).trim_end_matches(' ').to_owned();
    if payload.is_empty() {
        return false;
    }
    let before_count = normalize(baseline).matches(&payload).count();
    current.matches(&payload).count() > before_count
}

#[cfg(test)]
mod tests {
    use super::payload_was_inserted;

    #[test]
    fn verification_requires_one_new_exact_payload_occurrence() {
        assert!(payload_was_inserted("before", "before hello", "hello"));
        assert!(!payload_was_inserted(
            "hello before",
            "hello before",
            "hello"
        ));
        assert!(payload_was_inserted(
            "hello before",
            "hello before hello",
            "hello"
        ));
    }

    #[test]
    fn verification_normalizes_windows_line_endings() {
        assert!(payload_was_inserted(
            "prefix",
            "prefix\r\nline one\r\nline two",
            "line one\nline two"
        ));
    }

    #[test]
    fn verification_does_not_depend_on_provider_visible_trailing_space() {
        assert!(payload_was_inserted(
            "prefix",
            "prefix dictated text",
            "dictated text "
        ));
    }
}
