//! The visible answer to a dictation that could not be inserted.
//!
//! The reason used to reach only the status line of the hidden settings
//! window: the user saw nothing, and the text seemed lost. The notice is an
//! action window like the layout chooser. It never takes the keyboard focus,
//! so the user can click into the field they want and then press Insert. The
//! input host inserts into the window that is in front at that moment, the
//! same as the insert-last shortcut: SayFlex never picks another window itself.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use rust_protocol::{InputHostCommand, Transcript};
use slint::{ComponentHandle, PhysicalPosition, Timer, TimerMode, winit_030::WinitWindowAccessor};

use crate::{
    InsertionNotice,
    localization::{UiLocale, UiText, text},
    native_window_handle,
    overlay_presenter::{OverlayContract, OverlayPresenter},
    slint_window_handle,
    worker_supervisor::Workers,
};

/// Long enough to read two lines and move to another field; the text stays in
/// History afterwards.
const VISIBLE_FOR: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(500);
const PREVIEW_CHARACTERS: usize = 90;

pub(crate) struct InsertionNoticeController {
    window: InsertionNotice,
    presenter: OverlayPresenter,
    transcript: Rc<RefCell<Option<Transcript>>>,
    timer: Rc<Timer>,
}

/// The explanation for one input-host failure code, or `None` when the code
/// is not about inserting a dictation.
pub(crate) fn failure_reason(code: &str) -> Option<UiText> {
    Some(match code {
        "insert_target_changed" => UiText::InsertFailedTargetChanged,
        "insert_unconfirmed" => UiText::InsertFailedUnconfirmed,
        "insert_not_delivered" | "insert_transcript" => UiText::InsertFailedNotDelivered,
        "insert_modifiers_busy" => UiText::InsertFailedModifiersBusy,
        "insert_protected_field" => UiText::InsertFailedProtectedField,
        _ => return None,
    })
}

impl InsertionNoticeController {
    pub(crate) fn new(workers: &Rc<Workers>) -> Result<Rc<Self>, String> {
        let window = InsertionNotice::new().map_err(|error| error.to_string())?;
        let presenter = OverlayPresenter::install_always_renderable(
            window.window(),
            "insertion_notice_overlay",
            OverlayContract::Interactive,
            None,
        )
        .map_err(|error| format!("insertion notice initialization failed: {error}"))?;
        let controller = Rc::new(Self {
            window,
            presenter,
            transcript: Rc::default(),
            timer: Rc::new(Timer::default()),
        });
        controller.wire(workers);
        controller.precreate();
        Ok(controller)
    }

    fn hide_action(&self) -> impl Fn(&str) + Clone + 'static {
        let window = self.window.as_weak();
        let presenter = self.presenter.clone();
        // Weak: the timer's own callback holds this action.
        let timer = Rc::downgrade(&self.timer);
        move |reason: &str| {
            if let Some(timer) = timer.upgrade() {
                timer.stop();
            }
            if let Some(window) = window.upgrade() {
                window.set_prepared(false);
                presenter.retire(window.window(), reason);
            }
        }
    }

    fn wire(&self, workers: &Rc<Workers>) {
        {
            let hide = self.hide_action();
            let transcript = Rc::clone(&self.transcript);
            let workers = Rc::clone(workers);
            self.window.on_insert(move || {
                let Some(transcript) = transcript.borrow_mut().take() else {
                    return;
                };
                hide("insert");
                rust_app_core::diagnostics::log_line(&format!(
                    "insertion_notice action=insert_into_foreground session={}",
                    transcript.session_id
                ));
                let _ = workers
                    .input
                    .send(&InputHostCommand::InsertIntoForeground { transcript });
            });
        }
        {
            let hide = self.hide_action();
            let transcript = Rc::clone(&self.transcript);
            let workers = Rc::clone(workers);
            self.window.on_copy(move || {
                let Some(transcript) = transcript.borrow_mut().take() else {
                    return;
                };
                hide("copy");
                rust_app_core::diagnostics::log_line(&format!(
                    "insertion_notice action=copy session={}",
                    transcript.session_id
                ));
                let _ = workers
                    .input
                    .send(&InputHostCommand::CopyTranscript { transcript });
            });
        }
        {
            let hide = self.hide_action();
            let transcript = Rc::clone(&self.transcript);
            self.window.on_dismiss(move || {
                transcript.borrow_mut().take();
                hide("dismissed");
            });
        }
    }

    /// Creates the native window off screen once, like the other action
    /// windows, so the first failure does not wait for window creation.
    fn precreate(&self) {
        self.window.set_prepared(false);
        self.window
            .window()
            .set_position(PhysicalPosition::new(-32_000, -32_000));
        if self.window.show().is_err() {
            return;
        }
        let window = self.window.as_weak();
        let presenter = self.presenter.clone();
        let _ = slint::spawn_local(async move {
            let Some(window) = window.upgrade() else {
                return;
            };
            let attached = window
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle)
                .is_some_and(|handle| {
                    rust_platform_windows::prepare_action_window(handle)
                        .and_then(|()| presenter.attach(handle))
                        .is_ok()
                });
            if !attached {
                rust_app_core::diagnostics::log_line(
                    "insertion_notice_overlay transition=failed stage=attach",
                );
            }
        });
    }

    pub(crate) fn show(&self, transcript: Transcript, reason: UiText, locale: UiLocale) {
        if self.window.get_prepared() {
            self.hide_action()("replaced");
        }
        let preview = preview_line(&transcript.text);
        rust_app_core::diagnostics::log_line(&format!(
            "insertion_notice action=show session={} reason={reason:?}",
            transcript.session_id
        ));
        *self.transcript.borrow_mut() = Some(transcript);
        let ticket = match self.presenter.begin_prepare() {
            Ok(ticket) => ticket,
            Err(error) => {
                rust_app_core::diagnostics::log_line(&format!(
                    "insertion_notice_overlay transition=failed stage=begin_prepare error={error}"
                ));
                return;
            }
        };
        self.window.set_reason(text(locale, reason).into());
        self.window.set_preview(preview.into());
        let configured = slint_window_handle(self.window.window())
            .ok_or_else(|| "native insertion-notice HWND is unavailable".to_owned())
            .and_then(rust_platform_windows::configure_notice_window);
        if let Err(error) = configured {
            self.presenter
                .abort_prepare(self.window.window(), ticket, "configure", &error);
            return;
        }
        self.window.set_prepared(true);
        if let Err(error) = self.presenter.await_frame(self.window.window(), ticket) {
            self.window.set_prepared(false);
            self.presenter
                .abort_prepare(self.window.window(), ticket, "await_frame", &error);
            return;
        }
        self.arm_timer();
    }

    /// Hides the notice for this session, if it is the one shown: a later
    /// successful insertion of the same text makes it obsolete.
    pub(crate) fn resolve(&self, session_id: rust_protocol::SessionId) {
        let shown = self
            .transcript
            .borrow()
            .as_ref()
            .is_some_and(|transcript| transcript.session_id == session_id);
        if shown {
            self.transcript.borrow_mut().take();
            self.hide_action()("resolved");
        }
    }

    fn arm_timer(&self) {
        let window = self.window.as_weak();
        let transcript = Rc::clone(&self.transcript);
        let hide = self.hide_action();
        let remaining = Rc::new(Cell::new(VISIBLE_FOR));
        self.timer.start(TimerMode::Repeated, TICK, move || {
            // The notice waits while the pointer rests on it.
            if window.upgrade().is_some_and(|window| window.get_hovered()) {
                return;
            }
            let left = remaining.get().saturating_sub(TICK);
            remaining.set(left);
            if left.is_zero() {
                transcript.borrow_mut().take();
                hide("timeout");
            }
        });
    }
}

/// The first words of the transcript on one line, so the user knows which
/// dictation the notice is about.
fn preview_line(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= PREVIEW_CHARACTERS {
        return single_line;
    }
    let mut preview = single_line
        .chars()
        .take(PREVIEW_CHARACTERS)
        .collect::<String>();
    preview.push('…');
    preview
}

#[cfg(test)]
mod tests {
    use super::{failure_reason, preview_line};
    use crate::localization::UiText;

    #[test]
    fn every_insertion_failure_code_has_an_explanation() {
        for code in [
            "insert_target_changed",
            "insert_unconfirmed",
            "insert_not_delivered",
            "insert_transcript",
            "insert_modifiers_busy",
            "insert_protected_field",
        ] {
            assert!(failure_reason(code).is_some(), "{code}");
        }
        assert_eq!(
            failure_reason("insert_target_changed"),
            Some(UiText::InsertFailedTargetChanged)
        );
        assert_eq!(failure_reason("copy_text"), None);
        assert_eq!(failure_reason("invalid_command"), None);
    }

    #[test]
    fn preview_is_one_short_line() {
        assert_eq!(preview_line("Hello\n  world "), "Hello world");
        let long = "word ".repeat(40);
        let preview = preview_line(&long);
        assert!(preview.ends_with('…'));
        assert_eq!(preview.chars().count(), 91);
    }
}
