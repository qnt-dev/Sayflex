use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use slint::RenderingState;

/// How long a presentation waits for its frame before giving up on it.
///
/// This is a safety net for a renderer that has genuinely stopped, not a
/// deadline the normal path is expected to reach. It used to be 150 ms, and it
/// used to *reveal the window anyway* — which is where the empty popup came
/// from. Under load every one of four consecutive live translations passed
/// 150 ms, was uncloaked by the watchdog, and showed the surface last rendered
/// while the card was still fully transparent: the content was correct, saved
/// to history, and never drawn.
///
/// So the deadline no longer reveals. Nothing is shown that has not been drawn,
/// and if the frame never comes the presentation fails where it can be seen and
/// logged instead of succeeding into an empty rectangle.
const FRAME_DEADLINE: Duration = Duration::from_millis(2_500);

/// How often a frame that has not arrived is asked for again.
///
/// A frame normally lands within one or two refreshes — seven milliseconds on
/// an idle machine. Under load it can take much longer, and a request issued
/// while the window is still settling after a move can be coalesced away
/// entirely, so it is asked for again on this cadence until it comes.
const REDRAW_RETRY: Duration = Duration::from_millis(40);

/// Which show a timer belongs to: the window, and the generation on it.
///
/// Keyed by the window alone, a callback from an abandoned show could cancel
/// the timers of the show that replaced it — same window, different
/// presentation — and leave the new one waiting for a frame nobody would time
/// out.
type TimerKey = (u64, u64);

thread_local! {
    /// The repeated request for a frame, kept alive alongside its watchdog and
    /// dropped with it.
    static REDRAW_RETRIES: RefCell<HashMap<TimerKey, slint::Timer>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Watchdogs live on the event-loop thread, because a `slint::Timer` is
    /// neither `Send` nor able to outlive being dropped.
    ///
    /// Keyed by the window and the generation, not by the presenter's name.
    /// Every translation preview is called `translation_preview_overlay`, so
    /// with two of them pinned at once the second one's timer replaced the
    /// first one's and the first was left waiting for a frame nobody would time
    /// out.
    static FRAME_WATCHDOGS: RefCell<HashMap<TimerKey, slint::Timer>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayContract {
    ClickThrough,
    Interactive,
}

/// Whether this overlay has content that has to arrive before it is worth
/// showing.
///
/// The frame gate refuses to reveal a window nothing has been drawn on, which
/// is what stopped the translation preview appearing empty. That rule needs to
/// know what "nothing" means, and it is not the same for every window: a
/// translation is empty until its text arrives, while the recording visualiser
/// draws itself from the moment it exists and has no separate content at all.
///
/// It was a method the owner had to remember to call, and three of the five
/// owners did not — the visualiser, the action palette and the selection
/// chooser each ran their frame gate against a flag nobody ever set, had every
/// frame refused, and died at the deadline. Nothing showed. Making it a
/// property of the presenter means there is nothing left to forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentPolicy {
    /// Renderable as soon as it exists. Any frame reveals it.
    AlwaysRenderable,
    /// Empty until the owner says otherwise, through `content_changed`.
    RequiresContent,
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
    policy: ContentPolicy,
    phase: PresentationPhase,
    next_generation: u64,
    /// Whether what the window has been told to show is worth showing.
    ///
    /// The presenter cannot read the surface, but it can know whether the owner
    /// has put anything on it. A frame drawn while this is false is a frame of
    /// nothing, and revealing on it is what produced the empty popup.
    content_ready: bool,
    /// Bumped every time the content changes.
    content_revision: u64,
    /// The revision the frame that queued the reveal was drawn with.
    ///
    /// A reveal is queued onto the event loop and carried out later. In that
    /// gap the content can change — a `Completed` arriving just after the
    /// `Partial`'s frame — and revealing then would put the partial on screen
    /// with no one left to ask for a frame of the final text.
    accepted_revision: Option<u64>,
}

impl PresentationState {
    fn new(policy: ContentPolicy) -> Self {
        Self {
            policy,
            phase: PresentationPhase::Detached,
            next_generation: 1,
            content_ready: matches!(policy, ContentPolicy::AlwaysRenderable),
            content_revision: 0,
            accepted_revision: None,
        }
    }
}

impl Default for PresentationState {
    fn default() -> Self {
        Self::new(ContentPolicy::RequiresContent)
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
        // A frame drawn before the owner had anything to draw is a frame of
        // nothing. Promoting it is exactly how an empty window got revealed.
        if !self.content_ready {
            return None;
        }
        self.phase = PresentationPhase::RevealQueued(ticket);
        self.accepted_revision = Some(self.content_revision);
        Some(ticket)
    }

    /// Records what the window has been told to show.
    ///
    /// Returns the ticket that is waiting for a frame, if one is: content that
    /// changes mid-wait is a normal thing — a `Completed` arriving while the
    /// `Partial` is still being drawn — and it means the frame that eventually
    /// lands must be a new one.
    fn content_changed(&mut self, ready: bool) -> (u64, Option<PresentationTicket>) {
        // Always-renderable overlays do not have an empty state.  Reporting a
        // content change must never be able to turn one into a gated overlay.
        self.content_ready = matches!(self.policy, ContentPolicy::AlwaysRenderable) || ready;
        self.content_revision = self.content_revision.saturating_add(1);
        let waiting = match self.phase {
            PresentationPhase::AwaitingFrame(ticket) => Some(ticket),
            // A reveal already queued for the previous content is taken back.
            // The reveal runs later, off the event loop, and carrying it out
            // now would show the partial with nothing left to ask for a frame
            // of the final text — which is the same defect as the deadline
            // revealing an undrawn window, one step further along.
            PresentationPhase::RevealQueued(ticket) => {
                self.phase = PresentationPhase::AwaitingFrame(ticket);
                self.accepted_revision = None;
                Some(ticket)
            }
            _ => None,
        };
        (self.content_revision, waiting)
    }

    /// Whether this queued reveal may still be carried out.
    ///
    /// Both the ticket and the content have to be the ones that were accepted:
    /// a reveal is queued and runs later, and in that gap either can change.
    fn can_reveal(&self, ticket: PresentationTicket) -> bool {
        self.phase == PresentationPhase::RevealQueued(ticket)
            && self.accepted_revision == Some(self.content_revision)
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
        self.forget_content();
        Some(ticket.window_handle)
    }

    /// Back to however empty this overlay starts.
    ///
    /// A window that has been put away has nothing on it as far as the next
    /// show is concerned: leaving the flag set would let a frame drawn before
    /// the new content arrived reveal the previous translation.
    fn forget_content(&mut self) {
        self.content_ready = matches!(self.policy, ContentPolicy::AlwaysRenderable);
        self.accepted_revision = None;
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
    name: &'static str,
    contract: OverlayContract,
}

impl OverlayPresenter {
    pub(crate) fn install_always_renderable(
        window: &slint::Window,
        name: &'static str,
        contract: OverlayContract,
        on_presented: Option<PresentedCallback>,
    ) -> Result<Self, String> {
        Self::install(
            window,
            name,
            contract,
            ContentPolicy::AlwaysRenderable,
            on_presented,
        )
    }

    pub(crate) fn install_content_gated(
        window: &slint::Window,
        name: &'static str,
        contract: OverlayContract,
        on_presented: Option<PresentedCallback>,
    ) -> Result<Self, String> {
        Self::install(
            window,
            name,
            contract,
            ContentPolicy::RequiresContent,
            on_presented,
        )
    }

    fn install(
        window: &slint::Window,
        name: &'static str,
        contract: OverlayContract,
        policy: ContentPolicy,
        on_presented: Option<PresentedCallback>,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(PresentationState::new(policy)));
        let render_state = Arc::clone(&state);
        // Read when the renderer starts a frame, reported when it finishes it.
        // Reading the revision only at `AfterRendering` says what was current
        // when the callback ran, not what the frame contains — and the two
        // differ exactly when it matters, because content set during a render
        // is in the next frame, not this one.
        let drawing_revision = Arc::new(AtomicU64::new(0));
        let started_revision = Arc::clone(&drawing_revision);
        let rendered_frame_callback = Arc::new(Mutex::new(None::<RenderedFrameCallback>));
        let render_callback = Arc::clone(&rendered_frame_callback);
        let callback = on_presented;
        window
            .set_rendering_notifier(move |rendering_state, _graphics_api| {
                if matches!(rendering_state, RenderingState::BeforeRendering) {
                    let revision = render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .content_revision;
                    started_revision.store(revision, Ordering::Release);
                    return;
                }
                if !matches!(rendering_state, RenderingState::AfterRendering) {
                    return;
                }
                let drawn = started_revision.load(Ordering::Acquire);
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
                let ticket = {
                    let mut state = render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Every frame is recorded with the phase it arrived in.
                    // Without this there was no way to tell a frame that came
                    // and was ignored from a frame that never came, and the two
                    // want opposite fixes.
                    let phase = state.phase;
                    let current = state.content_revision;
                    let ready = state.content_ready;
                    // A frame that began before the latest content change does
                    // not carry it. Accepting it would reveal the window on a
                    // drawing of the previous text.
                    let ticket = if drawn == current {
                        state.frame_rendered()
                    } else {
                        None
                    };
                    // Only frames that had a decision to make. A presented
                    // window draws sixty times a second, and each of those was
                    // formatted and queued from inside the render callback:
                    // four thousand lines in one session, three quarters of
                    // them about windows that were parked.
                    if ticket.is_some() || matches!(phase, PresentationPhase::AwaitingFrame(_)) {
                        rust_app_core::diagnostics::log_line(&format!(
                            "{name} frame=rendered phase={phase:?} drew={drawn} \
                             current={current} ready={ready} accepted={}",
                            ticket.is_some()
                        ));
                    }
                    ticket
                };
                let Some(ticket) = ticket else {
                    return;
                };

                let queued_state = Arc::clone(&render_state);
                let queued_callback = callback.clone();
                let queue_result = slint::invoke_from_event_loop(move || {
                    // Deliberately not disarmed first. This runs after the
                    // event loop gets to it, and by then the content may have
                    // changed and put the show back to waiting for a new frame.
                    // Cancelling the timers here left that show with no frame,
                    // no watchdog and no way out — a window hidden for good.
                    // `reveal_revealable` disarms them when it actually
                    // reveals.
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
        rust_app_core::diagnostics::log_line(&format!(
            "{} transition=awaiting_frame generation={} hwnd={}",
            self.name, ticket.generation, ticket.window_handle
        ));
        self.arm_watchdog(ticket);
        Ok(())
    }

    /// Keeps asking for the frame, and gives up rather than showing nothing.
    ///
    /// This used to reveal the window when the frame was late, which is where
    /// the empty popup came from: the surface it uncloaked was the one rendered
    /// while the card was still transparent. It now nudges the renderer until
    /// the frame comes, and on the hard deadline fails the presentation instead
    /// of succeeding into an empty rectangle.
    fn arm_watchdog(&self, ticket: PresentationTicket) {
        let name = self.name;

        // Ask again, on a cadence, for as long as the frame has not arrived. A
        // request issued while the window is still settling after a move can be
        // coalesced away, and a busy event loop can simply be late.
        let nudge = slint::Timer::default();
        let nudge_state = Arc::clone(&self.state);
        nudge.start(slint::TimerMode::Repeated, REDRAW_RETRY, move || {
            let still_waiting = matches!(
                nudge_state
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
            timers.insert(key_of(ticket), nudge);
        });

        let state = Arc::clone(&self.state);
        let contract = self.contract;
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::SingleShot, FRAME_DEADLINE, move || {
            let overdue = matches!(
                state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .phase,
                PresentationPhase::AwaitingFrame(waiting) if waiting == ticket
            );
            if !overdue {
                return;
            }
            // Nothing was ever drawn for this presentation. Showing the window
            // now would show whatever was on it before, which is nothing.
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(ticket);
            // Both timers go, including the repeating one asking for the frame.
            // Without this it kept waking every forty milliseconds to check a
            // ticket that had already been given up on, until the next show
            // replaced it.
            disarm_watchdog(ticket);
            let _ = reconcile_contract(contract, ticket.window_handle);
            let _ = rust_platform_windows::conceal_overlay_window(ticket.window_handle);
            rust_app_core::diagnostics::log_line(&format!(
                "{name} transition=failed generation={} hwnd={} stage=frame_deadline after_ms={}",
                ticket.generation,
                ticket.window_handle,
                FRAME_DEADLINE.as_millis()
            ));
        });
        FRAME_WATCHDOGS.with_borrow_mut(|timers| {
            timers.insert(key_of(ticket), timer);
        });
    }

    /// Records what the window has been told to show, and re-arms if a frame
    /// for the previous content was already being waited on.
    ///
    /// The owner calls this every time it changes the overlay's content. A
    /// `Completed` that lands while the `Partial` is still being drawn is the
    /// ordinary case, and it must produce a new frame rather than let the old
    /// wait be satisfied by a drawing of the old text.
    pub(crate) fn content_changed(&self, window: &slint::Window, ready: bool) {
        let (revision, waiting) = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .content_changed(ready);
        rust_app_core::diagnostics::log_line(&format!(
            "{} content=changed revision={revision} ready={ready} rearmed={}",
            self.name,
            waiting.is_some()
        ));
        if waiting.is_some() {
            window.request_redraw();
        }
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
        disarm_watchdog(ticket);
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
            disarm_all_watchdogs(handle);
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
        // The content moved while this was queued, so the show is waiting for a
        // new frame. Its timers stay armed: they are the only thing that will
        // get it out if that frame never comes.
        return;
    }
    disarm_watchdog(ticket);
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
/// The slot a show's timers are filed under.
const fn key_of(ticket: PresentationTicket) -> TimerKey {
    (ticket.window_handle, ticket.generation)
}

/// Stops the timers of one show, and only that one.
fn disarm_watchdog(ticket: PresentationTicket) {
    let key = key_of(ticket);
    FRAME_WATCHDOGS.with_borrow_mut(|timers| {
        timers.remove(&key);
    });
    REDRAW_RETRIES.with_borrow_mut(|timers| {
        timers.remove(&key);
    });
}

/// Stops every timer on a window, whichever show they belong to.
///
/// For retirement, where the window itself is going away.
fn disarm_all_watchdogs(window_handle: u64) {
    FRAME_WATCHDOGS.with_borrow_mut(|timers| {
        timers.retain(|(window, _), _| *window != window_handle);
    });
    REDRAW_RETRIES.with_borrow_mut(|timers| {
        timers.retain(|(window, _), _| *window != window_handle);
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
    use super::{ContentPolicy, PresentationPhase, PresentationState};

    #[test]
    fn frame_acknowledgement_is_required_before_reveal() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();

        assert!(!state.can_reveal(ticket));
        state.content_changed(true);
        state.content_changed(true);
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
        state.content_changed(true);
        state.content_changed(true);
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
        state.content_changed(true);
        state.content_changed(true);
        state.await_frame(first).unwrap();
        assert_eq!(state.frame_rendered(), Some(first));
        assert!(state.complete_reveal(first));

        let refresh = state.begin_refresh().unwrap();
        assert_ne!(refresh.generation, first.generation);
        assert_eq!(refresh.window_handle, first.window_handle);
        assert_eq!(state.phase, PresentationPhase::Preparing(refresh));
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

        // Giving up on one leaves the other exactly where it was.
        first.content_changed(true);
        assert_eq!(first.frame_rendered(), Some(first_ticket));
        assert_eq!(
            second.phase,
            PresentationPhase::AwaitingFrame(second_ticket),
            "the second window is still waiting for its own frame"
        );
        second.content_changed(true);
        assert_eq!(second.frame_rendered(), Some(second_ticket));
    }

    #[test]
    fn no_phase_waits_for_a_frame_without_a_way_out() {
        // Every phase that holds a ticket can be left: by a frame with content
        // on it, by cancellation, or by retirement. `AwaitingFrame` used to be
        // reachable with only the first of those, which is what made the
        // overlay's hidden state permanent — and then the way out that was
        // added revealed the window without a frame, which is what made it
        // empty.
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        let mut by_frame = PresentationState {
            policy: ContentPolicy::RequiresContent,
            phase: state.phase,
            next_generation: state.next_generation,
            content_ready: true,
            content_revision: 1,
            accepted_revision: None,
        };
        assert_eq!(by_frame.frame_rendered(), Some(ticket));

        let mut by_cancel = PresentationState {
            policy: ContentPolicy::RequiresContent,
            phase: PresentationPhase::AwaitingFrame(ticket),
            next_generation: state.next_generation,
            content_ready: false,
            content_revision: 0,
            accepted_revision: None,
        };
        assert_eq!(by_cancel.cancel(ticket), Some(42));

        let mut by_retire = PresentationState {
            policy: ContentPolicy::RequiresContent,
            phase: PresentationPhase::AwaitingFrame(ticket),
            next_generation: state.next_generation,
            content_ready: false,
            content_revision: 0,
            accepted_revision: None,
        };
        assert!(by_retire.retire().is_some());
    }

    #[test]
    fn duplicate_render_callbacks_queue_only_one_reveal() {
        let mut state = PresentationState::default();
        state.attach(42).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
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

#[cfg(test)]
mod content_gate_tests {
    use super::{ContentPolicy, PresentationPhase, PresentationState};

    /// A state machine attached to a window, with nothing on that window yet.
    fn parked() -> PresentationState {
        let mut state = PresentationState::default();
        state.attach(4242).unwrap();
        state
    }

    #[test]
    fn an_overlay_that_draws_itself_needs_nobody_to_announce_its_content() {
        // The regression the policy replaces: `content_ready` was a flag the
        // owner had to remember to set, and three of the five owners did not.
        // The recording visualiser, the action palette and the selection
        // chooser each had every frame refused as empty and died at the
        // deadline — nothing appeared at all.
        let mut state = PresentationState::new(ContentPolicy::AlwaysRenderable);
        state.attach(4242).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        assert_eq!(
            state.frame_rendered(),
            Some(ticket),
            "a window with nothing to wait for is revealed by its first frame"
        );
    }

    #[test]
    fn always_renderable_policy_cannot_be_disabled_by_a_content_report() {
        let mut state = PresentationState::new(ContentPolicy::AlwaysRenderable);
        state.attach(4242).unwrap();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(false);
        state.await_frame(ticket).unwrap();

        assert_eq!(state.frame_rendered(), Some(ticket));
    }

    #[test]
    fn putting_an_overlay_away_forgets_what_was_on_it() {
        // Otherwise the next show could be revealed by a frame drawn before its
        // own content arrived — the previous translation, one show late.
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(ticket).unwrap();
        assert_eq!(state.frame_rendered(), Some(ticket));
        state.cancel(ticket);

        let next = state.begin_prepare().unwrap();
        state.await_frame(next).unwrap();

        assert_eq!(
            state.frame_rendered(),
            None,
            "the new show starts as empty as the first one did"
        );
    }

    #[test]
    fn a_frame_drawn_before_there_was_anything_to_draw_reveals_nothing() {
        // The defect this replaces, exactly: the card is transparent until the
        // owner has content for it, the first frame is drawn in that state, and
        // promoting it uncloaked an empty rectangle over the user's text.
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        // Nothing has been put on the window: this is the state a first show
        // starts in, with the card still fully transparent.
        state.await_frame(ticket).unwrap();

        assert_eq!(
            state.frame_rendered(),
            None,
            "an empty surface is not a presentation"
        );
        assert_eq!(state.phase, PresentationPhase::AwaitingFrame(ticket));
    }

    #[test]
    fn a_frame_drawn_with_content_is_the_one_that_reveals() {
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.content_changed(true);
        state.await_frame(ticket).unwrap();

        assert_eq!(state.frame_rendered(), Some(ticket));
        assert_eq!(state.phase, PresentationPhase::RevealQueued(ticket));
    }

    #[test]
    fn a_final_result_arriving_before_the_first_frame_re_arms_the_same_show() {
        // `Completed` lands while the `Partial` is still being drawn. It must
        // be a normal transition — the frame that eventually reveals the window
        // has to be one drawn with the final text.
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(ticket).unwrap();

        let (revision, waiting) = state.content_changed(true);

        assert_eq!(
            waiting,
            Some(ticket),
            "the show in flight is told to redraw"
        );
        assert_eq!(revision, 2);
        assert_eq!(state.frame_rendered(), Some(ticket));
    }

    #[test]
    fn a_final_result_arriving_after_the_frame_but_before_the_reveal_takes_it_back() {
        // The gap the first fix left open: the frame is accepted, the reveal is
        // queued onto the event loop, and the final text lands before it runs.
        // Revealing then puts the partial on screen with nobody left to ask for
        // a frame of the final text.
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(ticket).unwrap();
        assert_eq!(state.frame_rendered(), Some(ticket));
        assert!(state.can_reveal(ticket), "the reveal is queued");

        let (_, waiting) = state.content_changed(true);

        assert_eq!(waiting, Some(ticket), "the show is told to draw again");
        assert!(
            !state.can_reveal(ticket),
            "and the queued reveal is taken back rather than showing the partial"
        );
        assert_eq!(state.phase, PresentationPhase::AwaitingFrame(ticket));

        // The next frame carries the final text and reveals it.
        assert_eq!(state.frame_rendered(), Some(ticket));
        assert!(state.can_reveal(ticket));
        assert!(state.complete_reveal(ticket));
    }

    #[test]
    fn a_final_result_that_is_empty_holds_the_window_back() {
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(ticket).unwrap();
        state.content_changed(false);

        assert_eq!(
            state.frame_rendered(),
            None,
            "an emptied card must not be shown"
        );
    }

    #[test]
    fn a_deadline_that_finds_nothing_drawn_parks_instead_of_revealing() {
        // What the watchdog now does. It used to promote the ticket and
        // uncloak; the only thing on the window was the transparent card.
        let mut state = parked();
        let ticket = state.begin_prepare().unwrap();
        state.await_frame(ticket).unwrap();

        let window = state.cancel(ticket);

        assert_eq!(window, Some(4242));
        assert_eq!(
            state.phase,
            PresentationPhase::Parked {
                window_handle: 4242
            },
            "a presentation with no frame ends where it started, not on screen"
        );
    }

    #[test]
    fn an_older_show_cannot_reveal_over_a_newer_one() {
        let mut state = parked();
        let first = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(first).unwrap();
        // The user translates again before the first one is on screen.
        state.cancel(first);
        let second = state.begin_prepare().unwrap();
        state.content_changed(true);
        state.await_frame(second).unwrap();

        assert!(second.generation() > first.generation());
        assert!(
            !state.complete_reveal(first),
            "the abandoned show cannot finish"
        );
        assert_eq!(state.frame_rendered(), Some(second));
        assert!(state.complete_reveal(second));
    }
}
