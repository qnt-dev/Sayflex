using Keyboop.Protocol;
using Keyboop.SpeechWorker;

namespace Keyboop.InputReplay.Tests;

public sealed class DictationSessionRegistryTests
{
    [Fact]
    public async Task RepeatedEndSharesOneCompletion()
    {
        var registry = new DictationSessionRegistry();
        var sessionId = Guid.NewGuid();
        var startedAt = DateTimeOffset.UtcNow;

        Assert.Equal(
            DictationSessionStartResult.Started,
            registry.TryBegin(sessionId, startedAt));
        Assert.Equal(
            DictationSessionStartResult.AlreadyKnown,
            registry.TryBegin(sessionId, startedAt));

        var owner = registry.BeginEnd(sessionId);
        var duplicate = registry.BeginEnd(sessionId);

        Assert.True(owner.OwnsFinalization);
        Assert.False(duplicate.OwnsFinalization);
        Assert.Same(owner.Completion, duplicate.Completion);

        var expected = Transcript(sessionId, "current");
        registry.Complete(sessionId, expected);

        Assert.Equal(expected, await owner.Completion);
        Assert.Equal(expected, await duplicate.Completion);
    }

    [Fact]
    public void ConcurrentEndHasExactlyOneOwner()
    {
        var registry = new DictationSessionRegistry();
        var sessionId = Guid.NewGuid();
        Assert.Equal(
            DictationSessionStartResult.Started,
            registry.TryBegin(sessionId, DateTimeOffset.UtcNow));

        var leases = new DictationSessionEndLease[100];
        Parallel.For(
            0,
            leases.Length,
            index => leases[index] = registry.BeginEnd(sessionId));

        Assert.Single(leases, lease => lease.OwnsFinalization);
        Assert.All(
            leases,
            lease => Assert.Same(leases[0].Completion, lease.Completion));
    }

    [Fact]
    public async Task CompletionIsResolvedBySessionNotCompletionOrder()
    {
        var registry = new DictationSessionRegistry();
        var firstId = Guid.NewGuid();
        var secondId = Guid.NewGuid();

        Assert.Equal(
            DictationSessionStartResult.Started,
            registry.TryBegin(firstId, DateTimeOffset.UtcNow.AddSeconds(-1)));
        var first = registry.BeginEnd(firstId);
        Assert.Equal(
            DictationSessionStartResult.Started,
            registry.TryBegin(secondId, DateTimeOffset.UtcNow));
        var second = registry.BeginEnd(secondId);

        var firstTranscript = Transcript(firstId, "first");
        var secondTranscript = Transcript(secondId, "second");
        registry.Complete(secondId, secondTranscript);
        registry.Complete(firstId, firstTranscript);

        Assert.Equal(firstTranscript, await first.Completion);
        Assert.Equal(secondTranscript, await second.Completion);
        Assert.Equal(
            firstTranscript,
            await registry.TryGetCompletion(firstId)!);
        Assert.Equal(
            secondTranscript,
            await registry.TryGetCompletion(secondId)!);
    }

    [Fact]
    public void ASecondRecordingCannotStealActiveCapture()
    {
        var registry = new DictationSessionRegistry();

        Assert.Equal(
            DictationSessionStartResult.Started,
            registry.TryBegin(Guid.NewGuid(), DateTimeOffset.UtcNow));
        Assert.Equal(
            DictationSessionStartResult.AnotherSessionIsRecording,
            registry.TryBegin(Guid.NewGuid(), DateTimeOffset.UtcNow));
    }

    private static TranscriptMessage Transcript(Guid sessionId, string text) =>
        new(
            text,
            "en",
            TimeSpan.FromSeconds(1),
            TimeSpan.FromMilliseconds(100),
            true,
            sessionId);
}
