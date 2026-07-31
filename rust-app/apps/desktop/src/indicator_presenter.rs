use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, RenderingState};

use crate::RecordingIndicator;

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
            return Err("recording overlay has no native window handle".to_owned());
        }
        match self.phase {
            PresentationPhase::Detached => {
                self.phase = PresentationPhase::Parked { window_handle };
                Ok(())
            }
            PresentationPhase::Parked {
                window_handle: current,
            } if current == window_handle => Ok(()),
            _ => Err(format!(
                "recording overlay cannot attach in phase {:?}",
                self.phase
            )),
        }
    }

    fn begin_prepare(&mut self) -> Result<PresentationTicket, String> {
        let PresentationPhase::Parked { window_handle } = self.phase else {
            return Err(format!(
                "recording overlay cannot prepare in phase {:?}",
                self.phase
            ));
        };
        let ticket = PresentationTicket {
            generation: self.next_generation,
            window_handle,
        };
        self.next_generation = self.next_generation.saturating_add(1);
        self.phase = PresentationPhase::Preparing(ticket);
        Ok(ticket)
    }

    fn await_frame(&mut self, ticket: PresentationTicket) -> Result<(), String> {
        if self.phase != PresentationPhase::Preparing(ticket) {
            return Err(format!(
                "recording overlay rejected prepared frame generation={} phase={:?}",
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
            PresentationPhase::Detached => return None,
            PresentationPhase::Parked { .. } => return None,
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

#[derive(Clone)]
pub(crate) struct IndicatorPresenter {
    state: Arc<Mutex<PresentationState>>,
}

impl IndicatorPresenter {
    pub(crate) fn install(indicator: &RecordingIndicator) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(PresentationState::default()));
        let render_state = Arc::clone(&state);
        indicator
            .window()
            .set_rendering_notifier(move |rendering_state, _graphics_api| {
                if !matches!(rendering_state, RenderingState::AfterRendering) {
                    return;
                }
                let ticket = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .frame_rendered();
                let Some(ticket) = ticket else {
                    return;
                };

                let queued_state = Arc::clone(&render_state);
                let queue_result = slint::invoke_from_event_loop(move || {
                    let can_reveal = queued_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .can_reveal(ticket);
                    if !can_reveal {
                        return;
                    }

                    let result = rust_platform_windows::verify_overlay_window_contract(
                        ticket.window_handle,
                        true,
                    )
                    .and_then(|()| {
                        rust_platform_windows::reveal_overlay_window(ticket.window_handle)
                    })
                    .and_then(|()| {
                        rust_platform_windows::verify_overlay_window_contract(
                            ticket.window_handle,
                            false,
                        )
                    });

                    match result {
                        Ok(()) => {
                            let completed = queued_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .complete_reveal(ticket);
                            if completed {
                                rust_app_core::diagnostics::log_line(&format!(
                                    "recording_overlay transition=presented generation={} hwnd={}",
                                    ticket.generation, ticket.window_handle
                                ));
                            }
                        }
                        Err(error) => {
                            queued_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .cancel(ticket);
                            let _ =
                                rust_platform_windows::conceal_overlay_window(ticket.window_handle);
                            rust_app_core::diagnostics::log_line(&format!(
                                "recording_overlay transition=failed generation={} \
stage=reveal error={error}",
                                ticket.generation
                            ));
                        }
                    }
                });

                if let Err(error) = queue_result {
                    render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cancel(ticket);
                    let _ = rust_platform_windows::conceal_overlay_window(ticket.window_handle);
                    rust_app_core::diagnostics::log_line(&format!(
                        "recording_overlay transition=failed generation={} \
stage=queue_reveal error={error}",
                        ticket.generation
                    ));
                }
            })
            .map_err(|error| {
                format!("recording overlay renderer cannot acknowledge frames: {error:?}")
            })?;
        Ok(Self { state })
    }

    pub(crate) fn attach(&self, window_handle: u64) -> Result<(), String> {
        rust_platform_windows::verify_overlay_window_contract(window_handle, true)?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attach(window_handle)?;
        rust_app_core::diagnostics::log_line(&format!(
            "recording_overlay transition=parked hwnd={window_handle}"
        ));
        Ok(())
    }

    pub(crate) fn begin_prepare(&self) -> Result<PresentationTicket, String> {
        let ticket = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_prepare()?;
        if let Err(error) = rust_platform_windows::conceal_overlay_window(ticket.window_handle)
            .and_then(|()| {
                rust_platform_windows::verify_overlay_window_contract(ticket.window_handle, true)
            })
        {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(ticket);
            return Err(error);
        }
        rust_app_core::diagnostics::log_line(&format!(
            "recording_overlay transition=preparing generation={} hwnd={}",
            ticket.generation, ticket.window_handle
        ));
        Ok(ticket)
    }

    pub(crate) fn await_frame(
        &self,
        indicator: &RecordingIndicator,
        ticket: PresentationTicket,
    ) -> Result<(), String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .await_frame(ticket)?;
        indicator.window().request_redraw();
        Ok(())
    }

    pub(crate) fn abort_prepare(
        &self,
        indicator: &RecordingIndicator,
        ticket: PresentationTicket,
        stage: &str,
        error: &str,
    ) {
        let window_handle = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel(ticket);
        if let Some(window_handle) = window_handle {
            let _ = rust_platform_windows::conceal_overlay_window(window_handle);
        }
        indicator.set_prepared(false);
        indicator.window().request_redraw();
        rust_app_core::diagnostics::log_line(&format!(
            "recording_overlay transition=failed generation={} stage={stage} error={error}",
            ticket.generation
        ));
    }

    pub(crate) fn retire(&self, indicator: &RecordingIndicator, reason: &str) {
        let retirement = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retire();
        let Some((window_handle, generation)) = retirement else {
            return;
        };
        let conceal_result =
            rust_platform_windows::conceal_overlay_window(window_handle).and_then(|()| {
                rust_platform_windows::verify_overlay_window_contract(window_handle, true)
            });
        indicator.set_prepared(false);
        indicator.window().request_redraw();
        match conceal_result {
            Ok(()) => rust_app_core::diagnostics::log_line(&format!(
                "recording_overlay transition=parked generation={} reason={reason}",
                generation.unwrap_or_default()
            )),
            Err(error) => rust_app_core::diagnostics::log_line(&format!(
                "recording_overlay transition=failed generation={} \
stage=retire reason={reason} error={error}",
                generation.unwrap_or_default()
            )),
        }
    }
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
    }
}
