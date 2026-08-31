use std::collections::{HashMap, VecDeque};

use rust_protocol::{SessionId, Transcript};
use thiserror::Error;

const DEFAULT_RETAINED_SESSIONS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Listening,
    Processing,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginDisposition {
    Started,
    AlreadyKnown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndDisposition {
    StartProcessing,
    AwaitExisting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionDisposition {
    Stored,
    DuplicateIgnored,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("another dictation session is already listening")]
    AnotherSessionListening,
    #[error("dictation session is unknown")]
    UnknownSession,
    #[error("dictation session is not ready for this transition")]
    InvalidTransition,
    #[error("transcript session identity does not match the request")]
    TranscriptSessionMismatch,
}

#[derive(Debug)]
struct SessionRecord {
    phase: SessionPhase,
    started_unix_ms: u64,
    transcript: Option<Transcript>,
}

#[derive(Debug)]
pub struct SessionRegistry {
    sessions: HashMap<SessionId, SessionRecord>,
    history: VecDeque<Transcript>,
    listening_session: Option<SessionId>,
    latest_session: Option<SessionId>,
    retained_sessions: usize,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_RETAINED_SESSIONS)
    }
}

impl SessionRegistry {
    #[must_use]
    pub fn new(retained_sessions: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            history: VecDeque::new(),
            listening_session: None,
            latest_session: None,
            retained_sessions: retained_sessions.max(1),
        }
    }

    pub fn begin(
        &mut self,
        session_id: SessionId,
        started_unix_ms: u64,
    ) -> Result<BeginDisposition, SessionError> {
        if self.sessions.contains_key(&session_id) {
            return Ok(BeginDisposition::AlreadyKnown);
        }
        if self.listening_session.is_some() {
            return Err(SessionError::AnotherSessionListening);
        }

        self.prune_terminal_sessions();
        self.sessions.insert(
            session_id,
            SessionRecord {
                phase: SessionPhase::Listening,
                started_unix_ms,
                transcript: None,
            },
        );
        self.listening_session = Some(session_id);
        self.latest_session = Some(session_id);
        Ok(BeginDisposition::Started)
    }

    pub fn end(&mut self, session_id: SessionId) -> Result<EndDisposition, SessionError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(SessionError::UnknownSession)?;

        match session.phase {
            SessionPhase::Listening => {
                if self.listening_session != Some(session_id) {
                    return Err(SessionError::InvalidTransition);
                }
                session.phase = SessionPhase::Processing;
                self.listening_session = None;
                Ok(EndDisposition::StartProcessing)
            }
            SessionPhase::Processing | SessionPhase::Completed => Ok(EndDisposition::AwaitExisting),
            SessionPhase::Cancelled | SessionPhase::Failed => Err(SessionError::InvalidTransition),
        }
    }

    pub fn complete(
        &mut self,
        session_id: SessionId,
        transcript: Transcript,
    ) -> Result<CompletionDisposition, SessionError> {
        if transcript.session_id != session_id {
            return Err(SessionError::TranscriptSessionMismatch);
        }

        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(SessionError::UnknownSession)?;
        match session.phase {
            SessionPhase::Completed => {
                return Ok(CompletionDisposition::DuplicateIgnored);
            }
            SessionPhase::Processing => {}
            _ => return Err(SessionError::InvalidTransition),
        }

        session.phase = SessionPhase::Completed;
        session.transcript = Some(transcript.clone());
        self.history.push_front(transcript);
        while self.history.len() > self.retained_sessions {
            self.history.pop_back();
        }
        Ok(CompletionDisposition::Stored)
    }

    pub fn cancel(&mut self, session_id: SessionId) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(SessionError::UnknownSession)?;
        if session.phase != SessionPhase::Listening {
            return Err(SessionError::InvalidTransition);
        }
        session.phase = SessionPhase::Cancelled;
        if self.listening_session == Some(session_id) {
            self.listening_session = None;
        }
        Ok(())
    }

    pub fn fail(&mut self, session_id: SessionId) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(SessionError::UnknownSession)?;
        if matches!(
            session.phase,
            SessionPhase::Completed | SessionPhase::Cancelled
        ) {
            return Err(SessionError::InvalidTransition);
        }
        session.phase = SessionPhase::Failed;
        if self.listening_session == Some(session_id) {
            self.listening_session = None;
        }
        Ok(())
    }

    #[must_use]
    pub fn active_session(&self) -> Option<SessionId> {
        self.listening_session
    }

    #[must_use]
    pub fn latest_session(&self) -> Option<SessionId> {
        self.latest_session
    }

    #[must_use]
    pub fn phase(&self, session_id: SessionId) -> Option<SessionPhase> {
        self.sessions.get(&session_id).map(|session| session.phase)
    }

    #[must_use]
    pub fn transcript(&self, session_id: SessionId) -> Option<&Transcript> {
        self.sessions
            .get(&session_id)
            .and_then(|session| session.transcript.as_ref())
    }

    #[must_use]
    pub fn latest_transcript(&self) -> Option<&Transcript> {
        self.history.front()
    }

    pub fn history(&self) -> impl Iterator<Item = &Transcript> {
        self.history.iter()
    }

    fn prune_terminal_sessions(&mut self) {
        if self.sessions.len() < self.retained_sessions {
            return;
        }

        let mut removable: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                matches!(
                    session.phase,
                    SessionPhase::Completed | SessionPhase::Cancelled | SessionPhase::Failed
                )
                .then_some((*id, session.started_unix_ms))
            })
            .collect();
        removable.sort_by_key(|(_, started_unix_ms)| *started_unix_ms);

        let count = self
            .sessions
            .len()
            .saturating_sub(self.retained_sessions)
            .saturating_add(1);
        for (id, _) in removable.into_iter().take(count.max(1)) {
            self.sessions.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{SessionId, Transcript};

    use super::{
        CompletionDisposition, EndDisposition, SessionError, SessionPhase, SessionRegistry,
    };

    fn transcript(session_id: SessionId, text: &str) -> Transcript {
        Transcript {
            session_id,
            text: text.to_owned(),
            language: "ru".to_owned(),
            audio_duration_ms: 800,
            processing_duration_ms: 120,
            provider: None,
            latency: Default::default(),
        }
    }

    #[test]
    fn duplicate_end_does_not_start_second_finalization() {
        let mut registry = SessionRegistry::default();
        let session_id = SessionId::new();
        registry.begin(session_id, 1).unwrap();

        assert_eq!(
            registry.end(session_id).unwrap(),
            EndDisposition::StartProcessing
        );
        assert_eq!(
            registry.end(session_id).unwrap(),
            EndDisposition::AwaitExisting
        );
    }

    #[test]
    fn duplicate_completion_does_not_duplicate_history() {
        let mut registry = SessionRegistry::default();
        let session_id = SessionId::new();
        registry.begin(session_id, 1).unwrap();
        registry.end(session_id).unwrap();
        let result = transcript(session_id, "последняя запись");

        assert_eq!(
            registry.complete(session_id, result.clone()).unwrap(),
            CompletionDisposition::Stored
        );
        assert_eq!(
            registry.complete(session_id, result).unwrap(),
            CompletionDisposition::DuplicateIgnored
        );
        assert_eq!(registry.history().count(), 1);
    }

    #[test]
    fn latest_transcript_is_the_newest_completed_session() {
        let mut registry = SessionRegistry::default();
        let first = SessionId::new();
        registry.begin(first, 1).unwrap();
        registry.end(first).unwrap();
        registry
            .complete(first, transcript(first, "первая"))
            .unwrap();

        let second = SessionId::new();
        registry.begin(second, 2).unwrap();
        registry.end(second).unwrap();
        registry
            .complete(second, transcript(second, "вторая"))
            .unwrap();

        assert_eq!(registry.latest_transcript().unwrap().text, "вторая");
        assert_eq!(registry.transcript(first).unwrap().text, "первая");
    }

    #[test]
    fn latest_session_advances_before_transcription_completes() {
        let mut registry = SessionRegistry::default();
        let completed = SessionId::new();
        registry.begin(completed, 1).unwrap();
        registry.end(completed).unwrap();
        registry
            .complete(completed, transcript(completed, "готово"))
            .unwrap();

        let processing = SessionId::new();
        registry.begin(processing, 2).unwrap();
        registry.end(processing).unwrap();

        assert_eq!(registry.latest_session(), Some(processing));
        assert_eq!(registry.phase(processing), Some(SessionPhase::Processing));
        assert_eq!(registry.latest_transcript().unwrap().session_id, completed);
    }

    #[test]
    fn completion_cannot_be_attached_to_another_session() {
        let mut registry = SessionRegistry::default();
        let expected = SessionId::new();
        let stale = SessionId::new();
        registry.begin(expected, 1).unwrap();
        registry.end(expected).unwrap();

        assert_eq!(
            registry
                .complete(expected, transcript(stale, "чужая запись"))
                .unwrap_err(),
            SessionError::TranscriptSessionMismatch
        );
        assert_eq!(registry.phase(expected), Some(SessionPhase::Processing));
    }

    #[test]
    fn only_one_session_can_own_live_audio() {
        let mut registry = SessionRegistry::default();
        registry.begin(SessionId::new(), 1).unwrap();

        assert_eq!(
            registry.begin(SessionId::new(), 2).unwrap_err(),
            SessionError::AnotherSessionListening
        );
    }
}
