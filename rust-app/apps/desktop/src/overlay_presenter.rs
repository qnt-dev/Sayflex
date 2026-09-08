use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use slint::RenderingState;

/// How long a presentation may wait for the frame that opens the gate.
///
/// The gate exists so the first frame is drawn before the window is uncloaked,
/// which is what keeps a new overlay from flashing empty. It is an appearance
/// optimisation, not a correctness requirement — and it cannot be guaranteed:
/// a cloaked window is one the compositor may treat as occluded, and an
/// occluded window is not asked to draw. That is a race, and it was observed
/// losing: a translation finished in 364 ms and its window stayed cloaked for
/// the rest of the session, because nothing in this file could leave
/// `AwaitingFrame`.
///
/// So the gate has a deadline. A frame normally lands within one or two
/// refreshes; past this the overlay is shown regardless. The worst case is one
/// frame of flash, which is a great deal better than silence.
const FRAME_GATE: Duration = Duration::from_millis(150);

/// How soon a dropped redraw request is asked for again.
///
/// A request issued while the window is still settling after a move can be
/// coalesced away. Asking once more costs nothing when the frame already
/// arrived — the ticket is no longer waiting — and saves the flash when it did
/// not.
const REDRAW_RETRY: Duration = Duration::from_millis(40);

thread_local! {
    /// The early second request for a frame, kept alive alongside its watchdog
    /// and dropped with it.
    static REDRAW_RETRIES: RefCell<HashMap<u64, slint::Timer>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Watchdogs live on the event-loop thread, because a `slint::Timer` is
    /// neither `Send` nor able to outlive being dropped.
    ///
    /// Keyed by the window, not by the presenter's name. Every translation
    /// preview is called `translation_preview_overlay`, so with two of them
    /// pinned at once the second one's timer replaced the first one's and the
    /// first was left waiting for a frame nobody would time out.
    static FRAME_WATCHDOGS: RefCell<HashMap<u64, slint::Timer>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayContract {
    ClickThrough,
    Interactive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PresentationTicket {
    generation: u64,
    window_handle: u64,
}

impl PresentationTicket {
    pub(super) fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentationPhase {
    Detached,
    Parked { window_handle: u64 },
    Preparing(PresentationTicket),
    AwaitingFrame(PresentationTicket),
    RevealQueued(PresentationTicket),
    Presented(PresentationTicket),
}

#[derive(Debug)]
struct PresentationState {
    phase: PresentationPhase,
    next_generation: u64,
}

impl Default for PresentationState {
    fn default() -> Self {
        Self {
            phase: PresentationPhase::Detached,
            next_generation: 1,
        }
    }
}

impl PresentationState {
    fn attach(&mut self, window_handle: u64) -> Result<(), String> {
        if window_handle == 0 {
            return Err("overlay has no native window handle".to_owned());
        }
        match self.phase {
            PresentationPhase::Detached => {
                self.phase = PresentationPhase::Parked { window_handle };
                Ok(())
            }
            PresentationPhase::Parked {
                window_handle: current,
            } if current == window_handle => Ok(()),
            _ => Err(format!("overlay cannot attach in phase {:?}", self.phase)),
        }
    }

    fn next_ticket(&mut self, window_handle: u64) -> PresentationTicket {
        let ticket = PresentationTicket {
            generation: self.next_generation,
            window_handle,
        };
        self.next_generation = self.next_generation.saturating_add(1);
        self.phase = PresentationPhase::Preparing(ticket);
        ticket
    }

    fn begin_prepare(&mut self) -> Result<PresentationTicket, String> {
        let PresentationPhase::Parked { window_handle } = self.phase else {
            return Err(format!("overlay cannot prepare in phase {:?}", self.phase));
        };
        Ok(self.next_ticket(window_handle))
    }

    fn begin_refresh(&mut self) -> Result<PresentationTicket, String> {
        let PresentationPhase::Presented(current) = self.phase else {
            return Err(format!("overlay cannot refresh in phase {:?}", self.phase));
        };
        Ok(self.next_ticket(current.window_handle))
    }

    fn await_frame(&mut self, ticket: PresentationTicket) -> Result<(), String> {
        if self.phase != PresentationPhase::Preparing(ticket) {
            return Err(format!(
                "overlay rejected prepared frame generation={} phase={:?}",
                ticket.generation, self.phase
            ));
        }
        self.phase = PresentationPhase::AwaitingFrame(ticket);
        Ok(())
    }

    fn frame_rendered(&mut self) -> Option<PresentationTicket> {
        let PresentationPhase::AwaitingFrame(ticket) = self.phase else {
            return None;
        };
        self.phase = PresentationPhase::RevealQueued(ticket);
        Some(ticket)
    }

    /// The watchdog's counterpart to `frame_rendered`: promotes the ticket
    /// only if it is still the one waiting, so a frame that landed first wins.
    fn frame_rendered_by_deadline(&mut self, ticket: PresentationTicket) -> bool {
        if self.phase != PresentationPhase::AwaitingFrame(ticket) {
            return false;
        }
        self.phase = PresentationPhase::RevealQueued(ticket);
        true
    }

    fn can_reveal(&self, ticket: PresentationTicket) -> bool {
        self.phase == PresentationPhase::RevealQueued(ticket)
    }

    fn complete_reveal(&mut self, ticket: PresentationTicket) -> bool {
        if !self.can_reveal(ticket) {
            return false;
        }
        self.phase = PresentationPhase::Presented(ticket);
        true
    }

    fn cancel(&mut self, ticket: PresentationTicket) -> Option<u64> {
        let matches = matches!(
            self.phase,
            PresentationPhase::Preparing(current)
                | PresentationPhase::AwaitingFrame(current)
                | PresentationPhase::RevealQueued(current)
                | PresentationPhase::Presented(current)
                if current == ticket
        );
        if !matches {
            return None;
        }
        self.phase = PresentationPhase::Parked {
            window_handle: ticket.window_handle,
        };
        Some(ticket.window_handle)
    }

    fn retire(&mut self) -> Option<(u64, Option<u64>)> {
        let (window_handle, generation) = match self.phase {
            PresentationPhase::Detached | PresentationPhase::Parked { .. } => return None,
            PresentationPhase::Preparing(ticket)
            | PresentationPhase::AwaitingFrame(ticket)
            | PresentationPhase::RevealQueued(ticket)
            | PresentationPhase::Presented(ticket) => {
                (ticket.window_handle, Some(ticket.generation))
            }
        };
        self.phase = PresentationPhase::Parked { window_handle };
        Some((window_handle, generation))
    }
}

type PresentedCallback = Arc<dyn Fn(u64) + Send + Sync>;
type RenderedFrameCallback = Box<dyn FnOnce() + Send>;

#[derive(Clone)]
pub(crate) struct OverlayPresenter {
    state: Arc<Mutex<PresentationState>>,
    rendered_frame_callback: Arc<Mutex<Option<RenderedFrameCallback>>>,
    /// Held here as well as in the rendering notifier, because the watchdog
    /// completes the same transition and owes the same notification.
    on_presented: Option<PresentedCallback>,
    name: &'static str,
    contract: OverlayContract,
}

impl OverlayPresenter {
    pub(crate) fn install(
        window: &slint::Window,
        name: &'static str,
        contract: OverlayContract,
        on_presented: Option<PresentedCallback>,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(PresentationState::default()));
        let render_state = Arc::clone(&state);
        let rendered_frame_callback = Arc::new(Mutex::new(None::<RenderedFrameCallback>));
        let render_callback = Arc::clone(&rendered_frame_callback);
        let callback = on_presented.clone();
        window
            .set_rendering_notifier(move |rendering_state, _graphics_api| {
                if !matches!(rendering_state, RenderingState::AfterRendering) {
                    return;
                }
                if let Some(callback) = render_callback
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    && let Err(error) = slint::invoke_from_event_loop(callback)
                {
                    rust_app_core::diagnostics::log_line(&format!(
                        "{name} transition=failed stage=queue_frame_callback error={error}"
                    ));
                }
                let ticket = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .frame_rendered();
                let Some(ticket) = ticket else {
                    return;
                };

                let queued_state = Arc::clone(&render_state);
                let queued_callback = callback.clone();
                let queue_result = slint::invoke_from_event_loop(move || {
                    disarm_watchdog(ticket.window_handle);
                    reveal_revealable(
                        &queued_state,
                        name,
                        contract,
                        ticket,
                        &queued_callback,
                        None,
                    );
                });

                if let Err(error) = queue_result {
                    render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cancel(ticket);
                    let _ = rust_platform_windows::conceal_overlay_window(ticket.window_handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "{name} transition=failed generation={} stage=queue_reveal error={error}",
                        ticket.generation
                    ));
                }
            })
            .map_err(|error| format!("{name} renderer cannot acknowledge frames: {error:?}"))?;
        Ok(Self {
            state,
            rendered_frame_callback,
            on_presented,
            name,
            contract,
        })
    }

    /// The native window this presenter is attached to, if it has one yet.
    fn window_handle(&self) -> Option<u64> {
        match self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase
        {
            PresentationPhase::Detached => None,
            PresentationPhase::Parked { window_handle } => Some(window_handle),
            PresentationPhase::Preparing(ticket)
            | PresentationPhase::AwaitingFrame(ticket)
            | PresentationPhase::RevealQueued(ticket)
            | PresentationPhase::Presented(ticket) => Some(ticket.window_handle),
        }
    }

    pub(crate) fn attach(&self, window_handle: u64) -> Result<(), String> {
        reconcile_and_verify_contract(self.contract, window_handle, true)?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attach(window_handle)?;
        rust_app_core::diagnostics::log_line(&format!(
            "{} transition=parked hwnd={window_handle}",
            self.name
        ));
        Ok(())
    }

    pub(crate) fn begin_prepare(&self) -> Result<PresentationTicket, String> {
        let ticket = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_prepare()?;
        if let Err(error) = reconcile_contract(self.contract, ticket.window_handle)
            .and_then(|()| rust_platform_windows::conceal_overlay_window(ticket.window_handle))
            .and_then(|()| reconcile_and_verify_contract(self.contract, ticket.window_handle, true))
        {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(ticket);
            return Err(error);
        }
        self.log_preparing(ticket, "present");
        Ok(ticket)
    }

    pub(crate) fn begin_refresh(&self) -> Result<PresentationTicket, String> {
        let ticket = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_refresh()?;
        if let Err(error) = reconcile_contract(self.contract, ticket.window_handle)
            .and_then(|()| rust_platform_windows::cloak_overlay_window(ticket.window_handle))
            .and_then(|()| reconcile_and_verify_contract(self.contract, ticket.window_handle, true))
        {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(ticket);
            return Err(error);
        }
        self.log_preparing(ticket, "refresh");
        Ok(ticket)
    }

    fn log_preparing(&self, ticket: PresentationTicket, operation: &str) {
        rust_app_core::diagnostics::log_line(&format!(
            "{} transition=preparing generation={} hwnd={} operation={operation}",
            self.name, ticket.generation, ticket.window_handle
        ));
    }

    pub(crate) fn await_frame(
        &self,
        window: &slint::Window,
        ticket: PresentationTicket,
    ) -> Result<(), String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .await_frame(ticket)?;
        window.request_redraw();
        self.arm_watchdog(ticket);
        Ok(())
    }

    /// Show the overlay anyway if the frame the gate waits for never lands.
    ///
    /// Without this the overlay has a state it cannot leave: cloaked until a
    /// frame arrives, and not drawn because it is cloaked.
    fn arm_watchdog(&self, ticket: PresentationTicket) {
        let state = Arc::clone(&self.state);
        let callback = self.on_presented.clone();
        let name = self.name;
        let contract = self.contract;

        // One more request for the frame, early, before the deadline that
        // shows the window without one. Measured over a session, thirty-five
        // presentations in thirty-seven got their frame from the first
        // request; this is for the other two.
        let retry = slint::Timer::default();
        let retry_state = Arc::clone(&state);
        retry.start(slint::TimerMode::SingleShot, REDRAW_RETRY, move || {
            let still_waiting = matches!(
                retry_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .phase,
                PresentationPhase::AwaitingFrame(waiting) if waiting == ticket
            );
            if still_waiting {
                // Asked through the native window: a timer cannot hold a
                // `slint::Window`, and this is where `request_redraw` ends up.
                rust_platform_windows::request_repaint(ticket.window_handle);
            }
        });
        REDRAW_RETRIES.with_borrow_mut(|timers| {
            timers.insert(ticket.window_handle, retry);
        });

        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::SingleShot, FRAME_GATE, move || {
            // The frame may have arrived while this was queued; only a ticket
            // still waiting is overdue.
            let overdue = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .frame_rendered_by_deadline(ticket);
            if !overdue {
                return;
            }
            rust_app_core::diagnostics::log_line(&format!(
                "{name} transition=frame_timeout generation={} hwnd={} after_ms={}",
                ticket.generation,
                ticket.window_handle,
                FRAME_GATE.as_millis()
            ));
            reveal_revealable(
                &state,
                name,
                contract,
                ticket,
                &callback,
                Some("frame_timeout"),
            );
        });
        FRAME_WATCHDOGS.with_borrow_mut(|timers| {
            timers.insert(ticket.window_handle, timer);
        });
    }

    pub(crate) fn after_next_frame(
        &self,
        window: &slint::Window,
        callback: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        let presented = matches!(
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .phase,
            PresentationPhase::Presented(_)
        );
        if !presented {
            return Err("overlay is not presented for a frame-gated interaction".to_owned());
        }
        let mut pending = self
            .rendered_frame_callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_some() {
            return Err("overlay already has a frame-gated interaction pending".to_owned());
        }
        *pending = Some(Box::new(callback));
        drop(pending);
        window.request_redraw();
        Ok(())
    }

    pub(crate) fn abort_prepare(
        &self,
        window: &slint::Window,
        ticket: PresentationTicket,
        stage: &str,
        error: &str,
    ) {
        disarm_watchdog(ticket.window_handle);
        let window_handle = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel(ticket);
        if let Some(window_handle) = window_handle {
            let _ = rust_platform_windows::conceal_overlay_window(window_handle);
        }
        window.request_redraw();
        rust_app_core::diagnostics::log_line(&format!(
            "{} transition=failed generation={} stage={stage} error={error}",
            self.name, ticket.generation
        ));
    }

    pub(crate) fn retire(&self, window: &slint::Window, reason: &str) {
        if let Some(handle) = self.window_handle() {
            disarm_watchdog(handle);
        }
        let retirement = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retire();
        let Some((window_handle, generation)) = retirement else {
            return;
        };
        let conceal_result = reconcile_contract(self.contract, window_handle)
            .and_then(|()| rust_platform_windows::conceal_overlay_window(window_handle))
            .and_then(|()| reconcile_and_verify_contract(self.contract, window_handle, true));
        window.request_redraw();
        match conceal_result {
            Ok(()) => rust_app_core::diagnostics::log_line(&format!(
                "{} transition=parked generation={} reason={reason}",
                self.name,
                generation.unwrap_or_default()
            )),
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "{} transition=failed generation={} stage=retire reason={reason} error={error}",
                self.name,
                generation.unwrap_or_default()
            )),
        }
    }
}

/// The one place a queued reveal is carried out, so the watchdog and the
/// renderer cannot drift apart in what they do or what they log.
fn reveal_revealable(
    state: &Arc<Mutex<PresentationState>>,
    name: &'static str,
    contract: OverlayContract,
    ticket: PresentationTicket,
    callback: &Option<PresentedCallback>,
    recovered: Option<&str>,
) {
    let can_reveal = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .can_reveal(ticket);
    if !can_reveal {
        return;
    }
    let result = reconcile_and_verify_contract(contract, ticket.window_handle, true)
        .and_then(|()| rust_platform_windows::reveal_overlay_window(ticket.window_handle))
        .and_then(|()| reconcile_and_verify_contract(contract, ticket.window_handle, false));

    match result {
        Ok(()) => {
            let completed = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .complete_reveal(ticket);
            if completed {
                if let Some(callback) = callback {
                    callback(ticket.window_handle);
                }
                let note = recovered.map_or_else(String::new, |why| format!(" recovered={why}"));
                rust_app_core::diagnostics::log_line(&format!(
                    "{name} transition=presented generation={} hwnd={}{note}",
                    ticket.generation, ticket.window_handle
                ));
            }
        }
        Err(error) => {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(ticket);
            let _ = rust_platform_windows::conceal_overlay_window(ticket.window_handle);
            rust_app_core::diagnostics::log_line(&format!(
                "{name} transition=failed generation={} stage=reveal error={error}",
                ticket.generation
            ));
        }
    }
}

/// Dropping the timer cancels it. A watchdog that fires after its frame landed
/// would find the ticket already revealed and do nothing, but leaving live
/// timers behind for every presentation is untidy and eventually expensive.
fn disarm_watchdog(window_handle: u64) {
    FRAME_WATCHDOGS.with_borrow_mut(|timers| {
        timers.remove(&window_handle);
    });
    REDRAW_RETRIES.with_borrow_mut(|timers| {
        timers.remove(&window_handle);
    });
}

fn verify_contract(
    contract: OverlayContract,
    window_handle: u64,
    expected_cloaked: bool,
) -> Result<(), String> {
    match contract {
        OverlayContract::ClickThrough => {
            rust_platform_windows::verify_overlay_window_contract(window_handle, expected_cloaked)
        }
        OverlayContract::Interactive => {
            rust_platform_windows::verify_interactive_overlay_window_contract(
                window_handle,
                expected_cloaked,
            )
        }
    }
}

fn reconcile_contract(contract: OverlayContract, window_handle: u64) -> Result<(), String> {
    match contract {
        OverlayContract::ClickThrough => {
            rust_platform_windows::reconcile_overlay_window_contract(window_handle)
        }
        OverlayContract::Interactive => {
            rust_platform_windows::reconcile_interactive_overlay_window_contract(window_handle)
        }
    }
}

fn reconcile_and_verify_contract(
    contract: OverlayContract,
    window_handle: u64,
    expected_cloaked: bool,
) -> Result<(), String> {
    reconcile_contract(contract, window_handle)?;
    verify_contract(contract, window_handle, expected_cloaked)
}

#[cfg(test)]
mod tests {
    use super::{PresentationPhase, PresentationState};

    #[test]
    fn frame_acknowledgement_is_required_before_reveal() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();

        assert!(!state.can_reveal(ticket));
        state.await_frame(ticket).unwrap();
        assert_eq!(state.frame_rendered(), Some(ticket));
        assert!(state.can_reveal(ticket));
        assert!(state.complete_reveal(ticket));
        assert_eq!(state.phase, PresentationPhase::Presented(ticket));
    }

    #[test]
    fn retirement_invalidates_a_queued_reveal() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();
        assert_eq!(state.frame_rendered(), Some(ticket));

        assert_eq!(state.retire(), Some((42, Some(ticket.generation))));
        assert!(!state.can_reveal(ticket));
        assert!(!state.complete_reveal(ticket));
        assert_eq!(state.phase, PresentationPhase::Parked { window_handle: 42 });
    }

    #[test]
    fn visible_overlay_can_enter_a_frame_gated_refresh() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let first = state.begin_prepare().unwrap();
        state.await_frame(first).unwrap();
        assert_eq!(state.frame_rendered(), Some(first));
        assert!(state.complete_reveal(first));

        let refresh = state.begin_refresh().unwrap();
        assert_ne!(refresh.generation, first.generation);
        assert_eq!(refresh.window_handle, first.window_handle);
        assert_eq!(state.phase, PresentationPhase::Preparing(refresh));
    }

    #[test]
    fn a_frame_that_never_arrives_still_reveals_the_overlay() {
        // The failure this replaces: a translation finished in 364 ms, its
        // window was cloaked to await the first frame, the frame never came
        // because a cloaked window may be treated as occluded, and the overlay
        // stayed hidden for the rest of the session.
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        assert!(state.frame_rendered_by_deadline(ticket));
        assert!(state.can_reveal(ticket));
        assert!(state.complete_reveal(ticket));
        assert_eq!(state.phase, PresentationPhase::Presented(ticket));
    }

    #[test]
    fn a_deadline_that_loses_the_race_to_the_frame_changes_nothing() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        assert_eq!(state.frame_rendered(), Some(ticket));
        // The watchdog fires a moment later and must not disturb the reveal
        // the frame already queued.
        assert!(!state.frame_rendered_by_deadline(ticket));
        assert!(state.can_reveal(ticket));
    }

    #[test]
    fn a_deadline_from_a_superseded_presentation_is_ignored() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let first = state.begin_prepare().unwrap();
        state.await_frame(first).unwrap();
        assert!(state.frame_rendered_by_deadline(first));
        assert!(state.complete_reveal(first));

        let second = state.begin_refresh().unwrap();
        state.await_frame(second).unwrap();
        // The first presentation's watchdog is still in flight; it must not
        // reveal the second one on the first one's behalf.
        assert!(!state.frame_rendered_by_deadline(first));
        assert_eq!(state.phase, PresentationPhase::AwaitingFrame(second));
    }

    #[test]
    fn two_windows_waiting_at_once_keep_their_own_deadlines() {
        // The watchdogs were keyed by the presenter's name, and every
        // translation preview is called `translation_preview_overlay`. Pinning
        // two at once meant the second one's timer replaced the first one's,
        // leaving the first waiting for a frame nobody would ever time out.
        //
        // The state machines are separate, so what this checks is that two
        // tickets on two windows do not share an identity: the key is the
        // window handle, and these differ.
        let mut first = PresentationState::default();
        first.attach(4242).unwrap();
        let first_ticket = first.begin_prepare().unwrap();
        first.await_frame(first_ticket).unwrap();

        let mut second = PresentationState::default();
        second.attach(9797).unwrap();
        let second_ticket = second.begin_prepare().unwrap();
        second.await_frame(second_ticket).unwrap();

        assert_ne!(
            first_ticket.window_handle, second_ticket.window_handle,
            "two previews must not share the slot their watchdog is filed under"
        );

        // Timing one out leaves the other exactly where it was.
        assert!(first.frame_rendered_by_deadline(first_ticket));
        assert_eq!(
            second.phase,
            PresentationPhase::AwaitingFrame(second_ticket),
            "the second window is still waiting for its own frame"
        );
        assert!(second.frame_rendered_by_deadline(second_ticket));
    }

    #[test]
    fn no_phase_waits_for_a_frame_without_a_way_out() {
        // Every phase that holds a ticket can be left: by a frame, by the
        // deadline, by cancellation or by retirement. `AwaitingFrame` used to
        // be reachable with only the first of those, which is what made the
        // overlay's hidden state permanent.
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        let mut by_deadline = PresentationState {
            phase: state.phase,
            next_generation: state.next_generation,
        };
        assert!(by_deadline.frame_rendered_by_deadline(ticket));

        let mut by_cancel = PresentationState {
            phase: PresentationPhase::AwaitingFrame(ticket),
            next_generation: state.next_generation,
        };
        assert_eq!(by_cancel.cancel(ticket), Some(42));

        let mut by_retire = PresentationState {
            phase: PresentationPhase::AwaitingFrame(ticket),
            next_generation: state.next_generation,
        };
        assert!(by_retire.retire().is_some());
    }

    #[test]
    fn duplicate_render_callbacks_queue_only_one_reveal() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        assert_eq!(state.frame_rendered(), Some(ticket));
        assert_eq!(state.frame_rendered(), None);
    }

    #[test]
    fn detached_or_busy_overlay_cannot_prepare() {
        let mut state = PresentationState::default();
        assert!(state.begin_prepare().is_err());

        state.attach(42).unwrap();
        let _ticket = state.begin_prepare().unwrap();
        assert!(state.begin_prepare().is_err());
        assert!(state.begin_refresh().is_err());
    }
}
