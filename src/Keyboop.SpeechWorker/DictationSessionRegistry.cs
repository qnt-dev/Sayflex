using Keyboop.Protocol;

namespace Keyboop.SpeechWorker;

internal enum DictationSessionStartResult
{
    Started,
    AlreadyKnown,
    AnotherSessionIsRecording
}

internal readonly record struct DictationSessionEndLease(
    Guid SessionId,
    DateTimeOffset StartedAt,
    bool OwnsFinalization,
    Task<TranscriptMessage> Completion);

internal sealed class DictationSessionRegistry
{
    private const int MaximumRetainedSessions = 256;

    private readonly object _gate = new();
    private readonly Dictionary<Guid, SessionEntry> _sessions = [];
    private Guid? _recordingSessionId;

    internal DictationSessionStartResult TryBegin(
        Guid sessionId,
        DateTimeOffset startedAt)
    {
        if (sessionId == Guid.Empty)
        {
            throw new ArgumentException(
                "A dictation session ID cannot be empty.",
                nameof(sessionId));
        }

        lock (_gate)
        {
            if (_sessions.ContainsKey(sessionId))
            {
                return DictationSessionStartResult.AlreadyKnown;
            }

            if (_recordingSessionId is not null)
            {
                return DictationSessionStartResult.AnotherSessionIsRecording;
            }

            PruneCompletedSessions();
            _sessions.Add(sessionId, new SessionEntry(sessionId, startedAt));
            _recordingSessionId = sessionId;
            return DictationSessionStartResult.Started;
        }
    }

    internal DictationSessionEndLease BeginEnd(Guid sessionId)
    {
        lock (_gate)
        {
            var session = GetRequiredSession(sessionId);
            if (session.Phase == DictationSessionPhase.Recording)
            {
                if (_recordingSessionId != sessionId)
                {
                    throw new InvalidOperationException(
                        "The requested dictation session does not own audio capture.");
                }

                session.Phase = DictationSessionPhase.Processing;
                _recordingSessionId = null;
                return new DictationSessionEndLease(
                    session.Id,
                    session.StartedAt,
                    true,
                    session.Completion.Task);
            }

            return new DictationSessionEndLease(
                session.Id,
                session.StartedAt,
                false,
                session.Completion.Task);
        }
    }

    internal Task<TranscriptMessage>? TryGetCompletion(Guid sessionId)
    {
        lock (_gate)
        {
            return _sessions.TryGetValue(sessionId, out var session)
                ? session.Completion.Task
                : null;
        }
    }

    internal void Complete(Guid sessionId, TranscriptMessage transcript)
    {
        lock (_gate)
        {
            var session = GetRequiredSession(sessionId);
            session.Phase = DictationSessionPhase.Completed;
            session.Completion.TrySetResult(transcript);
        }
    }

    internal void Fail(Guid sessionId, Exception exception)
    {
        lock (_gate)
        {
            var session = GetRequiredSession(sessionId);
            if (_recordingSessionId == sessionId)
            {
                _recordingSessionId = null;
            }

            session.Phase = DictationSessionPhase.Failed;
            session.Completion.TrySetException(exception);
        }
    }

    internal bool Cancel(Guid sessionId)
    {
        lock (_gate)
        {
            if (!_sessions.TryGetValue(sessionId, out var session))
            {
                return false;
            }

            if (session.Phase != DictationSessionPhase.Recording)
            {
                return false;
            }

            if (_recordingSessionId == sessionId)
            {
                _recordingSessionId = null;
            }

            session.Phase = DictationSessionPhase.Cancelled;
            session.Completion.TrySetCanceled();
            return true;
        }
    }

    private SessionEntry GetRequiredSession(Guid sessionId)
    {
        if (sessionId == Guid.Empty
            || !_sessions.TryGetValue(sessionId, out var session))
        {
            throw new InvalidOperationException(
                $"Dictation session {sessionId:D} is unknown.");
        }

        return session;
    }

    private void PruneCompletedSessions()
    {
        if (_sessions.Count < MaximumRetainedSessions)
        {
            return;
        }

        var removable = _sessions.Values
            .Where(
                session =>
                    session.Phase
                    is DictationSessionPhase.Completed
                        or DictationSessionPhase.Failed
                        or DictationSessionPhase.Cancelled)
            .OrderBy(session => session.StartedAt)
            .Take(Math.Max(1, _sessions.Count - MaximumRetainedSessions + 1))
            .Select(session => session.Id)
            .ToArray();
        foreach (var sessionId in removable)
        {
            _sessions.Remove(sessionId);
        }
    }

    private sealed class SessionEntry
    {
        internal SessionEntry(Guid id, DateTimeOffset startedAt)
        {
            Id = id;
            StartedAt = startedAt;
        }

        internal Guid Id { get; }

        internal DateTimeOffset StartedAt { get; }

        internal TaskCompletionSource<TranscriptMessage> Completion { get; } =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        internal DictationSessionPhase Phase { get; set; } =
            DictationSessionPhase.Recording;
    }

    private enum DictationSessionPhase
    {
        Recording,
        Processing,
        Completed,
        Failed,
        Cancelled
    }
}
