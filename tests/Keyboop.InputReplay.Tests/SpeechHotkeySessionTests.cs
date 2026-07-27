using System.Collections.Concurrent;
using Keyboop.Core.Input;
using Keyboop.InputHost;

namespace Keyboop.InputReplay.Tests;

public sealed class SpeechHotkeySessionTests
{
    [Fact]
    public void RecoveringActiveSessionQueuesEndBeforeRecover()
    {
        var sink = new RecordingSpeechActionSink();
        var session = new SpeechHotkeySession(sink);
        var dictationTarget = Target(1);
        var recoveryTarget = Target(2);

        Assert.True(session.TryBegin(dictationTarget));
        Assert.True(session.TryRecoverLast(recoveryTarget));

        Assert.Equal(
            ["Begin:1", "End:1", "Recover:2"],
            sink.ActionLabels);
        Assert.Single(sink.SessionIds);
        Assert.DoesNotContain(Guid.Empty, sink.SessionIds);
        Assert.False(session.IsActive);
    }

    [Fact]
    public void ConcurrentEndAndRecoverCannotReverseActionOrder()
    {
        for (var iteration = 0; iteration < 100; iteration++)
        {
            var sink = new RecordingSpeechActionSink();
            var session = new SpeechHotkeySession(sink);
            Assert.True(session.TryBegin(Target(1)));

            Parallel.Invoke(
                () => session.TryEnd(),
                () => session.TryRecoverLast(Target(2)));

            Assert.Equal(
                ["Begin:1", "End:1", "Recover:2"],
                sink.ActionLabels);
            Assert.Single(sink.SessionIds);
        }
    }

    [Fact]
    public void RecoverAfterCompletedSessionDoesNotQueueDuplicateEnd()
    {
        var sink = new RecordingSpeechActionSink();
        var session = new SpeechHotkeySession(sink);
        Assert.True(session.TryBegin(Target(1)));
        Assert.True(session.TryEnd());

        Assert.False(session.TryRecoverLast(Target(2)));

        Assert.Equal(
            ["Begin:1", "End:1", "Recover:2"],
            sink.ActionLabels);
        Assert.Single(sink.SessionIds);
    }

    [Fact]
    public void SuccessiveRecordingsReceiveDifferentSessionIds()
    {
        var sink = new RecordingSpeechActionSink();
        var session = new SpeechHotkeySession(sink);

        Assert.True(session.TryBegin(Target(1)));
        Assert.True(session.TryEnd());
        Assert.True(session.TryBegin(Target(2)));
        Assert.True(session.TryRecoverLast(Target(3)));

        var beginIds = sink.Actions
            .Where(action => action.Kind == "Begin")
            .Select(action => action.SessionId)
            .ToArray();
        Assert.Equal(2, beginIds.Length);
        Assert.NotEqual(beginIds[0], beginIds[1]);
        Assert.Equal(
            beginIds[1],
            sink.Actions.Last(action => action.Kind == "Recover").SessionId);
    }

    private static TargetSnapshot Target(long windowHandle) =>
        new(windowHandle, 10, 20, 30);

    private sealed class RecordingSpeechActionSink : ISpeechActionSink
    {
        internal ConcurrentQueue<RecordedAction> Actions { get; } = new();

        internal IReadOnlyList<string> ActionLabels =>
            Actions
                .Select(
                    action =>
                        $"{action.Kind}:{action.Target.WindowHandle}")
                .ToArray();

        internal IReadOnlyList<Guid> SessionIds =>
            Actions
                .Where(action => action.SessionId is not null)
                .Select(action => action.SessionId!.Value)
                .Distinct()
                .ToArray();

        public bool TryBegin(
            Guid sessionId,
            DateTimeOffset startedAt,
            TargetSnapshot target)
        {
            Actions.Enqueue(new RecordedAction("Begin", sessionId, target));
            return true;
        }

        public bool TryEnd(Guid sessionId, TargetSnapshot target)
        {
            Actions.Enqueue(new RecordedAction("End", sessionId, target));
            return true;
        }

        public bool TryRecover(Guid? sessionId, TargetSnapshot target)
        {
            Actions.Enqueue(new RecordedAction("Recover", sessionId, target));
            return true;
        }
    }

    private sealed record RecordedAction(
        string Kind,
        Guid? SessionId,
        TargetSnapshot Target);
}
