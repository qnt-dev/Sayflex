using Keyboop.Core.Input;

namespace Keyboop.InputHost;

internal interface ISpeechActionSink
{
    bool TryBegin(
        Guid sessionId,
        DateTimeOffset startedAt,
        TargetSnapshot target);

    bool TryEnd(Guid sessionId, TargetSnapshot target);

    bool TryRecover(Guid? sessionId, TargetSnapshot target);
}

internal sealed class SpeechHotkeySession
{
    private readonly object _gate = new();
    private readonly ISpeechActionSink _sink;
    private TargetSnapshot _dictationTarget;
    private Guid? _activeSessionId;
    private Guid? _latestSessionId;

    internal SpeechHotkeySession(ISpeechActionSink sink)
    {
        _sink = sink;
    }

    internal bool IsActive
    {
        get
        {
            lock (_gate)
            {
                return _activeSessionId is not null;
            }
        }
    }

    internal bool TryBegin(TargetSnapshot target)
    {
        lock (_gate)
        {
            if (_activeSessionId is not null)
            {
                return false;
            }

            var sessionId = Guid.NewGuid();
            if (!_sink.TryBegin(sessionId, DateTimeOffset.UtcNow, target))
            {
                return false;
            }

            _dictationTarget = target;
            _activeSessionId = sessionId;
            _latestSessionId = sessionId;
            return true;
        }
    }

    internal bool TryEnd()
    {
        lock (_gate)
        {
            return TryEndLocked();
        }
    }

    internal bool TryRecoverLast(TargetSnapshot target)
    {
        lock (_gate)
        {
            var endedActiveDictation = TryEndLocked();
            _ = _sink.TryRecover(_latestSessionId, target);
            return endedActiveDictation;
        }
    }

    private bool TryEndLocked()
    {
        if (_activeSessionId is not { } sessionId)
        {
            return false;
        }

        if (!_sink.TryEnd(sessionId, _dictationTarget))
        {
            return false;
        }

        // End and Recover carry the same immutable identity. Pipe scheduling can
        // no longer turn "recover this recording" into "read a global last value".
        _activeSessionId = null;
        return true;
    }
}
