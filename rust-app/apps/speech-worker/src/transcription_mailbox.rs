use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    time::Duration,
};

use rust_cloud_speech::router::CancelSignal;
use rust_protocol::SessionId;

pub(crate) enum MailboxItem<C, W> {
    Control(C),
    Work(W),
}

/// A bounded control lane plus one replaceable work slot. Control can never sit
/// behind a recording, and overload cannot grow memory without bound.
pub(crate) struct TranscriptionMailbox<C, W> {
    control: SyncSender<C>,
    latest_work: Arc<Mutex<Option<W>>>,
}

pub(crate) struct TranscriptionMailboxReceiver<C, W> {
    control: Receiver<C>,
    latest_work: Arc<Mutex<Option<W>>>,
}

impl<C, W> Clone for TranscriptionMailbox<C, W> {
    fn clone(&self) -> Self {
        Self {
            control: self.control.clone(),
            latest_work: Arc::clone(&self.latest_work),
        }
    }
}

impl<C, W> TranscriptionMailbox<C, W> {
    pub(crate) fn bounded(control_capacity: usize) -> (Self, TranscriptionMailboxReceiver<C, W>) {
        let (control, receiver) = mpsc::sync_channel(control_capacity);
        let latest_work = Arc::new(Mutex::new(None));
        (
            Self {
                control,
                latest_work: Arc::clone(&latest_work),
            },
            TranscriptionMailboxReceiver {
                control: receiver,
                latest_work,
            },
        )
    }

    pub(crate) fn send_control(&self, command: C) -> Result<(), TrySendError<C>> {
        self.control.try_send(command)
    }

    /// Replaces and returns work that had not started. The caller can publish a
    /// typed cancellation for that superseded session immediately.
    pub(crate) fn send_latest(&self, work: W) -> Option<W> {
        self.latest_work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(work)
    }
}

impl<C, W> TranscriptionMailboxReceiver<C, W> {
    pub(crate) fn recv_next(&self, timeout: Duration) -> Option<MailboxItem<C, W>> {
        loop {
            match self.control.try_recv() {
                Ok(command) => return Some(MailboxItem::Control(command)),
                Err(TryRecvError::Disconnected) => {
                    return self.take_work().map(MailboxItem::Work);
                }
                Err(TryRecvError::Empty) => {}
            }
            if let Some(work) = self.take_work() {
                return Some(MailboxItem::Work(work));
            }
            match self.control.recv_timeout(timeout) {
                Ok(command) => return Some(MailboxItem::Control(command)),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return self.take_work().map(MailboxItem::Work);
                }
            }
        }
    }

    fn take_work(&self) -> Option<W> {
        self.latest_work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
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

    pub(crate) fn watcher(&self, session_id: SessionId) -> SessionWatcher {
        SessionWatcher {
            cancellations: self.clone(),
            session_id,
        }
    }
}

pub(crate) struct SessionWatcher {
    cancellations: SessionCancellationSet,
    session_id: SessionId,
}

impl CancelSignal for SessionWatcher {
    fn is_cancelled(&self) -> bool {
        self.cancellations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&self.session_id)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc::TrySendError, time::Duration};

    use rust_cloud_speech::router::CancelSignal;
    use rust_protocol::SessionId;

    use super::{MailboxItem, SessionCancellationSet, TranscriptionMailbox};

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
    fn overload_keeps_only_the_latest_pending_finalize() {
        let (mailbox, receiver) = TranscriptionMailbox::<&str, u32>::bounded(2);
        assert_eq!(mailbox.send_latest(1), None);
        assert_eq!(mailbox.send_latest(2), Some(1));
        assert!(matches!(
            receiver.recv_next(Duration::ZERO),
            Some(MailboxItem::Work(2))
        ));
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
            receiver.recv_next(Duration::ZERO),
            Some(MailboxItem::Control(1))
        ));
        assert!(matches!(
            receiver.recv_next(Duration::ZERO),
            Some(MailboxItem::Work(7))
        ));
    }
}
