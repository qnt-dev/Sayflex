use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Condvar, Mutex, MutexGuard, mpsc::TrySendError},
};

use rust_cloud_speech::router::CancelSignal;
use rust_protocol::SessionId;

pub(crate) enum MailboxItem<C, W> {
    Control(C),
    Work(W),
}

/// A bounded control lane plus one replaceable work slot. Control can never sit
/// behind a recording, and overload cannot grow memory without bound.
///
/// The receiver sleeps on a condition variable until something arrives. It
/// used to poll every 25 ms, about 35 wake-ups a second per thread while idle.
pub(crate) struct TranscriptionMailbox<C, W> {
    shared: Arc<Shared<C, W>>,
}

pub(crate) struct TranscriptionMailboxReceiver<C, W> {
    shared: Arc<Shared<C, W>>,
}

struct Shared<C, W> {
    state: Mutex<State<C, W>>,
    ready: Condvar,
}

struct State<C, W> {
    control: VecDeque<C>,
    control_capacity: usize,
    latest_work: Option<W>,
    senders: usize,
    receiver_alive: bool,
}

impl<C, W> Shared<C, W> {
    fn lock(&self) -> MutexGuard<'_, State<C, W>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<C, W> Clone for TranscriptionMailbox<C, W> {
    fn clone(&self) -> Self {
        self.shared.lock().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<C, W> Drop for TranscriptionMailbox<C, W> {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.senders -= 1;
        if state.senders == 0 {
            drop(state);
            self.shared.ready.notify_all();
        }
    }
}

impl<C, W> Drop for TranscriptionMailboxReceiver<C, W> {
    fn drop(&mut self) {
        self.shared.lock().receiver_alive = false;
    }
}

impl<C, W> TranscriptionMailbox<C, W> {
    pub(crate) fn bounded(control_capacity: usize) -> (Self, TranscriptionMailboxReceiver<C, W>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                control: VecDeque::with_capacity(control_capacity),
                control_capacity,
                latest_work: None,
                senders: 1,
                receiver_alive: true,
            }),
            ready: Condvar::new(),
        });
        (
            Self {
                shared: Arc::clone(&shared),
            },
            TranscriptionMailboxReceiver { shared },
        )
    }

    pub(crate) fn send_control(&self, command: C) -> Result<(), TrySendError<C>> {
        let mut state = self.shared.lock();
        if !state.receiver_alive {
            return Err(TrySendError::Disconnected(command));
        }
        if state.control.len() >= state.control_capacity {
            return Err(TrySendError::Full(command));
        }
        state.control.push_back(command);
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }

    /// Replaces and returns work that had not started. The caller can publish a
    /// typed cancellation for that superseded session immediately.
    pub(crate) fn send_latest(&self, work: W) -> Option<W> {
        let replaced = self.shared.lock().latest_work.replace(work);
        self.shared.ready.notify_one();
        replaced
    }
}

impl<C, W> TranscriptionMailboxReceiver<C, W> {
    /// Waits for the next item; control first. Returns `None` once every
    /// sender is gone and nothing is left.
    pub(crate) fn recv_next(&self) -> Option<MailboxItem<C, W>> {
        let mut state = self.shared.lock();
        loop {
            if let Some(command) = state.control.pop_front() {
                return Some(MailboxItem::Control(command));
            }
            if let Some(work) = state.latest_work.take() {
                return Some(MailboxItem::Work(work));
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// Per-session tombstones remain independently visible until their own
/// terminal result is acknowledged.
#[derive(Clone, Default)]
pub(crate) struct SessionCancellationSet(Arc<Mutex<HashSet<SessionId>>>);

impl SessionCancellationSet {
    pub(crate) fn cancel(&self, session_id: SessionId) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id);
    }

    pub(crate) fn complete(&self, session_id: SessionId) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session_id);
    }

    pub(crate) fn watcher(&self, session_id: SessionId) -> SessionPublicationGuard {
        SessionPublicationGuard::new(self.clone(), session_id)
    }
}

/// Linearizes terminal publication against cancellation for one session.
/// The cancellation mutex remains held while the terminal callbacks enqueue
/// UI/history output, so Cancel is ordered either wholly before or wholly
/// after that publication instead of racing between its checks.
#[derive(Clone)]
pub(crate) struct SessionPublicationGuard {
    cancellations: SessionCancellationSet,
    session_id: SessionId,
}

impl SessionPublicationGuard {
    pub(crate) fn new(cancellations: SessionCancellationSet, session_id: SessionId) -> Self {
        Self {
            cancellations,
            session_id,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&self.session_id)
    }

    pub(crate) fn try_publish(&self, publish: impl FnOnce()) -> bool {
        let cancellations = self
            .cancellations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancellations.contains(&self.session_id) {
            return false;
        }
        publish();
        true
    }

    pub(crate) fn complete(&self) {
        self.cancellations.complete(self.session_id);
    }
}

impl CancelSignal for SessionPublicationGuard {
    fn is_cancelled(&self) -> bool {
        SessionPublicationGuard::is_cancelled(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TrySendError;

    use rust_protocol::SessionId;

    use super::{
        MailboxItem, SessionCancellationSet, SessionPublicationGuard, TranscriptionMailbox,
    };

    #[test]
    fn overlapping_sessions_keep_independent_cancellation_tombstones() {
        let cancellations = SessionCancellationSet::default();
        let a = SessionId::new();
        let b = SessionId::new();
        let watch_a = cancellations.watcher(a);
        let watch_b = cancellations.watcher(b);

        cancellations.cancel(a);
        cancellations.cancel(b);
        assert!(watch_a.is_cancelled());
        assert!(watch_b.is_cancelled());

        cancellations.complete(a);
        assert!(!watch_a.is_cancelled());
        assert!(watch_b.is_cancelled());
    }

    #[test]
    fn cancelled_session_cannot_publish_until_terminal_acknowledgement() {
        let cancellations = SessionCancellationSet::default();
        let session_id = SessionId::new();
        let publication = SessionPublicationGuard::new(cancellations.clone(), session_id);
        let mut published = false;

        cancellations.cancel(session_id);
        assert!(!publication.try_publish(|| published = true));
        assert!(!published);
        assert!(publication.is_cancelled());

        publication.complete();
        assert!(!publication.is_cancelled());
    }

    #[test]
    fn overload_keeps_only_the_latest_pending_finalize() {
        let (mailbox, receiver) = TranscriptionMailbox::<&str, u32>::bounded(2);
        assert_eq!(mailbox.send_latest(1), None);
        assert_eq!(mailbox.send_latest(2), Some(1));
        assert!(matches!(receiver.recv_next(), Some(MailboxItem::Work(2))));
    }

    #[test]
    fn a_waiting_receiver_wakes_for_work_and_ends_when_senders_leave() {
        let (mailbox, receiver) = TranscriptionMailbox::<u32, u32>::bounded(1);
        let waiter = std::thread::spawn(move || {
            let first = receiver.recv_next();
            let second = receiver.recv_next();
            (
                matches!(first, Some(MailboxItem::Work(5))),
                second.is_none(),
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(30));
        mailbox.send_latest(5);
        std::thread::sleep(std::time::Duration::from_millis(30));
        drop(mailbox);
        assert_eq!(waiter.join().unwrap(), (true, true));
    }

    #[test]
    fn control_is_bounded_and_precedes_pending_work() {
        let (mailbox, receiver) = TranscriptionMailbox::<u32, u32>::bounded(1);
        mailbox.send_latest(7);
        mailbox.send_control(1).unwrap();
        assert!(matches!(
            mailbox.send_control(2),
            Err(TrySendError::Full(2))
        ));
        assert!(matches!(
            receiver.recv_next(),
            Some(MailboxItem::Control(1))
        ));
        assert!(matches!(receiver.recv_next(), Some(MailboxItem::Work(7))));
    }
}
