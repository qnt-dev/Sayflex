//! Decisions about a completed dictation, separated from how they are carried
//! out.
//!
//! This is the first piece of the desktop's orchestration to move out of the
//! UI event loop. It takes what is known when a transcript arrives and returns
//! what should happen, in the order it must happen. Nothing here touches a
//! window, a pipe or the clock, so the rules can be tested directly instead of
//! being inferred from a handler that needs a running Slint application.

use rust_protocol::TargetSnapshot;

/// Where a completed transcript should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionDelivery {
    /// Send the text to a window.
    ///
    /// `add_trailing_space` is true only when the text goes to the window that
    /// was captured when recording started: continuing to dictate into it reads
    /// as one flowing sentence. Text handed to a waiting "insert last" request
    /// is placed exactly where the caret is now, so it gets no extra space.
    Insert {
        target: TargetSnapshot,
        add_trailing_space: bool,
    },
    /// Keep the transcript, but deliver it nowhere: the target is gone.
    HistoryOnly,
}

/// What the visible history should do about this transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryUpdate {
    /// Add one row at the top; the rest of the list is untouched.
    PrependLatest,
    /// Rebuild, because an existing row for this session was replaced.
    Rebuild,
    /// Nothing changed: the completion was a duplicate.
    Unchanged,
}

/// The plan for one completed transcript.
///
/// Delivery is named first because it must happen first: presentation work
/// belongs after the text has been handed over, never before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionPlan {
    pub delivery: CompletionDelivery,
    pub history: HistoryUpdate,
}

/// Decides what a completed transcript should do.
///
/// * `stored` — the session registry accepted this completion. A duplicate is
///   not stored and must not be delivered again.
/// * `replaced_existing` — an entry for the same session was already in the
///   history and was removed to make room for this one.
/// * `awaiting_target` — a request to insert the newest dictation is waiting
///   for exactly this session.
/// * `captured_target` — the window that was focused when recording started,
///   if it is still known.
#[must_use]
pub fn plan_completion(
    stored: bool,
    replaced_existing: bool,
    awaiting_target: Option<TargetSnapshot>,
    captured_target: Option<TargetSnapshot>,
) -> CompletionPlan {
    if !stored {
        return CompletionPlan {
            delivery: CompletionDelivery::HistoryOnly,
            history: HistoryUpdate::Unchanged,
        };
    }

    let history = if replaced_existing {
        HistoryUpdate::Rebuild
    } else {
        HistoryUpdate::PrependLatest
    };

    // A waiting request wins: the user asked for this text after the recording
    // had already started, and knows where they want it.
    let delivery = awaiting_target
        .map(|target| CompletionDelivery::Insert {
            target,
            add_trailing_space: false,
        })
        .or_else(|| {
            captured_target.map(|target| CompletionDelivery::Insert {
                target,
                add_trailing_space: true,
            })
        })
        .unwrap_or(CompletionDelivery::HistoryOnly);

    CompletionPlan { delivery, history }
}

#[cfg(test)]
mod tests {
    use rust_protocol::TargetSnapshot;

    use super::{CompletionDelivery, HistoryUpdate, plan_completion};

    fn target(process_id: u32) -> TargetSnapshot {
        TargetSnapshot {
            window_handle: u64::from(process_id) * 10,
            process_id,
            thread_id: 1,
            keyboard_layout: 0,
            ..TargetSnapshot::default()
        }
    }

    #[test]
    fn the_recorded_window_receives_the_text_with_a_trailing_space() {
        let plan = plan_completion(true, false, None, Some(target(42)));

        assert_eq!(
            plan.delivery,
            CompletionDelivery::Insert {
                target: target(42),
                add_trailing_space: true,
            }
        );
        assert_eq!(plan.history, HistoryUpdate::PrependLatest);
    }

    #[test]
    fn a_waiting_insert_request_wins_and_adds_no_space() {
        let plan = plan_completion(true, false, Some(target(7)), Some(target(42)));

        assert_eq!(
            plan.delivery,
            CompletionDelivery::Insert {
                target: target(7),
                add_trailing_space: false,
            }
        );
    }

    #[test]
    fn a_lost_target_keeps_the_transcript_without_delivering_it() {
        let plan = plan_completion(true, false, None, None);

        assert_eq!(plan.delivery, CompletionDelivery::HistoryOnly);
        assert_eq!(plan.history, HistoryUpdate::PrependLatest);
    }

    #[test]
    fn a_duplicate_completion_delivers_nothing_and_changes_nothing() {
        let plan = plan_completion(false, false, Some(target(7)), Some(target(42)));

        assert_eq!(plan.delivery, CompletionDelivery::HistoryOnly);
        assert_eq!(plan.history, HistoryUpdate::Unchanged);
    }

    #[test]
    fn replacing_an_existing_entry_asks_for_a_rebuild() {
        let plan = plan_completion(true, true, None, Some(target(42)));

        assert_eq!(plan.history, HistoryUpdate::Rebuild);
    }
}
