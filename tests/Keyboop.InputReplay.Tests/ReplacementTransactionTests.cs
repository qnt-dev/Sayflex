using Keyboop.Core.Input;

namespace Keyboop.InputReplay.Tests;

public sealed class ReplacementTransactionTests
{
    private static readonly PhysicalKeyEvent Boundary =
        new(0x20, 0x39, false, 0, 10, 100);

    private static readonly TargetSnapshot Target =
        new(1234, 50, 0x04090409, 500);

    [Fact]
    public void CommitPreservesEveryQueuedPhysicalEventInOrder()
    {
        var started = DateTimeOffset.UtcNow;
        var transaction = new ReplacementTransaction(
            new ReplacementPlan(7, "привет ", Boundary),
            Target,
            started);
        var queued = Enumerable.Range(0, 20)
            .Select(index => new PhysicalKeyEvent(
                (uint)('A' + index),
                (uint)(30 + index),
                false,
                0,
                (ulong)(11 + index),
                101 + index))
            .ToArray();

        foreach (var input in queued)
        {
            Assert.True(transaction.TryQueue(input, started + TimeSpan.FromMilliseconds(1)));
        }

        var outcome = transaction.Complete(Target, started + TimeSpan.FromMilliseconds(5));

        Assert.True(outcome.ShouldCommit);
        Assert.Equal(queued, outcome.EventsToReplay);
        Assert.Equal(TransactionStatus.Committed, transaction.Status);
    }

    [Fact]
    public void TargetChangeFailsOpenWithOriginalQueue()
    {
        var started = DateTimeOffset.UtcNow;
        var transaction = new ReplacementTransaction(
            new ReplacementPlan(7, "привет ", Boundary),
            Target,
            started);
        var input = new PhysicalKeyEvent(0x41, 0x1e, false, 0, 11, 101);
        Assert.True(transaction.TryQueue(input, started));

        var changed = Target with { WindowHandle = 9999 };
        var outcome = transaction.Complete(changed, started + TimeSpan.FromMilliseconds(2));

        Assert.False(outcome.ShouldCommit);
        Assert.Equal("target-changed", outcome.Reason);
        Assert.Equal([input], outcome.EventsToReplay);
    }

    [Fact]
    public void DeadlineFailsOpen()
    {
        var started = DateTimeOffset.UtcNow;
        var transaction = new ReplacementTransaction(
            new ReplacementPlan(7, "привет ", Boundary),
            Target,
            started);

        var outcome = transaction.Complete(Target, started + TimeSpan.FromMilliseconds(36));

        Assert.False(outcome.ShouldCommit);
        Assert.Equal("deadline", outcome.Reason);
        Assert.Equal(TransactionStatus.Cancelled, transaction.Status);
    }

    [Fact]
    public void QueueOverflowCancelsInsteadOfDroppingSilently()
    {
        var started = DateTimeOffset.UtcNow;
        var transaction = new ReplacementTransaction(
            new ReplacementPlan(7, "привет ", Boundary),
            Target,
            started,
            maximumQueuedEvents: 2);

        Assert.True(transaction.TryQueue(
            new PhysicalKeyEvent(1, 1, false, 0, 1, 1),
            started));
        Assert.True(transaction.TryQueue(
            new PhysicalKeyEvent(2, 2, false, 0, 2, 2),
            started));
        Assert.False(transaction.TryQueue(
            new PhysicalKeyEvent(3, 3, false, 0, 3, 3),
            started));
        Assert.Equal(TransactionStatus.Cancelled, transaction.Status);
    }
}

