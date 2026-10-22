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
    let _transaction =
        clipboard::begin_clipboard_transaction().map_err(|error| DeliveryFailure {
            method: InsertionMethod::ClipboardTransaction,
            state: DeliveryFailureState::NotDelivered,
            message: error.to_string(),
        })?;
    // Protection checks are mandatory for every route, including test routes
    // that disable UIA acknowledgement. Unknown UIA remains compatible.
    let baseline = ensure_insertion_allowed(target_snapshot)
        .map_err(|error| injection_failure(InsertionMethod::NativeEditMessage, error))?;
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
            return Err(injection_failure(InsertionMethod::NativeEditMessage, error));
        }
    }

    let shortcut = target::paste_shortcut(target_snapshot);
    // UIA text is an optional acknowledgement channel. A positive protected
    // result is a terminal denial independently of that optional text.
    let verification_disabled_for_smoke =
        std::env::var("SAYFLEX_INSERTION_SMOKE_DISABLE_UIA_VERIFY").as_deref() == Ok("1");
    let baseline = if verification_disabled_for_smoke {
        None
    } else {
        baseline
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
                .ok()
                .flatten()
                .is_some_and(|current| payload_was_inserted(baseline, &current, payload))
        },
        || {
            // Snapshotting/publishing can take time; protection and focus
            // must be checked again at the actual fallback dispatch.
            ensure_insertion_allowed(target_snapshot)
                .map(|_| ())
                .map_err(|error| match error {
                    inject::InjectionError::Protected => clipboard::ClipboardError::Protected,
                    inject::InjectionError::TargetChanged => {
                        clipboard::ClipboardError::TargetChanged
                    }
                    _ => clipboard::ClipboardError::Paste,
                })
        },
        || {
            inject::paste_shortcut(shortcut).map_err(|error| match error {
                inject::InjectionError::Partial { .. } => {
                    clipboard::ClipboardError::DeliveryUnconfirmed
                }
                _ => clipboard::ClipboardError::Paste,
            })
        },
        target_snapshot.process_id,
        payload,
    )
    .map(|receipt| {
        let delivery = match receipt.disposition {
            clipboard::ClipboardPasteDisposition::VerifiedAndRestored
            | clipboard::ClipboardPasteDisposition::VerifiedAndSuperseded
            | clipboard::ClipboardPasteDisposition::VerifiedRestoreFailed => {
                InsertionDelivery::Verified
            }
            clipboard::ClipboardPasteDisposition::PayloadReadAndRestored
            | clipboard::ClipboardPasteDisposition::PayloadReadAndSuperseded
            | clipboard::ClipboardPasteDisposition::PayloadReadRestoreFailed => {
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
            clipboard::ClipboardError::DeliveryUnconfirmed => DeliveryFailureState::Ambiguous,
            clipboard::ClipboardError::TargetChanged | clipboard::ClipboardError::Superseded => {
                DeliveryFailureState::Superseded
            }
            clipboard::ClipboardError::NotDelivered
            | clipboard::ClipboardError::Protected
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

fn observed_focused_text(target: TargetSnapshot) -> Result<Option<String>, inject::InjectionError> {
    observed_focused_text_result(uia_selection::focused_text(target))
}

fn observed_focused_text_result(
    observation: Result<uia_selection::FocusedTextOutcome, uia_selection::SelectionError>,
) -> Result<Option<String>, inject::InjectionError> {
    match observation {
        Ok(uia_selection::FocusedTextOutcome::Text(text)) => Ok(Some(text)),
        Ok(uia_selection::FocusedTextOutcome::Protected) => Err(inject::InjectionError::Protected),
        Ok(uia_selection::FocusedTextOutcome::TargetChanged) => {
            Err(inject::InjectionError::TargetChanged)
        }
        Ok(uia_selection::FocusedTextOutcome::Unavailable) | Err(_) => Ok(None),
    }
}

pub(crate) fn ensure_insertion_allowed(
    target: TargetSnapshot,
) -> Result<Option<String>, inject::InjectionError> {
    ensure_insertion_allowed_using(
        || inject::ensure_target_not_protected(target),
        || uia_selection::focused_text(target),
    )
}

fn ensure_insertion_allowed_using(
    mut native_focus: impl FnMut() -> Result<u64, inject::InjectionError>,
    observe: impl FnOnce() -> Result<uia_selection::FocusedTextOutcome, uia_selection::SelectionError>,
) -> Result<Option<String>, inject::InjectionError> {
    let before = native_focus()?;
    let observed = observed_focused_text_result(observe())?;
    let after = native_focus()?;
    if before != after {
        return Err(inject::InjectionError::TargetChanged);
    }
    Ok(observed)
}

fn injection_failure(method: InsertionMethod, error: inject::InjectionError) -> DeliveryFailure {
    DeliveryFailure {
        method,
        state: match error {
            inject::InjectionError::Protected | inject::InjectionError::PhysicalModifiersHeld => {
                DeliveryFailureState::NotDelivered
            }
            inject::InjectionError::TargetChanged => DeliveryFailureState::Superseded,
            inject::InjectionError::Partial { .. } => DeliveryFailureState::Ambiguous,
        },
        message: error.to_string(),
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
    use super::{
        ensure_insertion_allowed_using, observed_focused_text_result, payload_was_inserted,
    };
    use crate::inject::InjectionError;
    use crate::uia_selection::{FocusedTextOutcome, SelectionError};

    #[test]
    fn protected_uia_cannot_become_an_unverified_paste() {
        let mut delivered = false;
        let result = observed_focused_text_result(Ok(FocusedTextOutcome::Protected)).map(|_| {
            delivered = true;
        });
        assert!(result.is_err());
        assert!(!delivered);
    }

    #[test]
    fn unavailable_uia_and_provider_errors_keep_compatibility_fallback() {
        for observation in [
            Ok(FocusedTextOutcome::Unavailable),
            Err(SelectionError::Timeout),
        ] {
            assert!(matches!(
                observed_focused_text_result(observation),
                Ok(None)
            ));
        }
    }

    #[test]
    fn a_native_password_blocks_before_uia_or_any_delivery_adapter() {
        let mut delivered = false;
        let result = ensure_insertion_allowed_using(
            || Err(InjectionError::Protected),
            || panic!("positive native protection must deny immediately"),
        )
        .map(|_| delivered = true);
        assert!(matches!(result, Err(InjectionError::Protected)));
        assert!(!delivered);
    }

    #[test]
    fn a_password_discovered_at_dispatch_recheck_cannot_reach_paste() {
        let mut checks = 0;
        let mut delivered = false;
        let result = ensure_insertion_allowed_using(
            || {
                checks += 1;
                if checks == 1 {
                    Ok(10)
                } else {
                    Err(InjectionError::Protected)
                }
            },
            || Ok(FocusedTextOutcome::Unavailable),
        )
        .map(|_| delivered = true);
        assert!(matches!(result, Err(InjectionError::Protected)));
        assert!(!delivered);
    }

    #[test]
    fn changing_native_focus_during_the_provider_read_cancels_delivery() {
        let mut focus = 10;
        let result = ensure_insertion_allowed_using(
            || {
                focus += 1;
                Ok(focus)
            },
            || Ok(FocusedTextOutcome::Text("old editor".to_owned())),
        );
        assert!(matches!(result, Err(InjectionError::TargetChanged)));
    }

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
