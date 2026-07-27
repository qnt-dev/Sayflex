using Keyboop.InputHost;

namespace Keyboop.InputReplay.Tests;

public sealed class PendingOperationTrackerTests
{
    [Fact]
    public async Task WaitForIdleWaitsForTrackedOperation()
    {
        var tracker = new PendingOperationTracker();
        var completion = new TaskCompletionSource(
            TaskCreationOptions.RunContinuationsAsynchronously);
        tracker.Track(completion.Task);

        var waiting = tracker.WaitForIdleAsync();

        Assert.False(waiting.IsCompleted);
        completion.SetResult();
        await waiting;
    }

    [Fact]
    public async Task WaitForIdleIncludesOperationAddedWhileWaiting()
    {
        var tracker = new PendingOperationTracker();
        var first = new TaskCompletionSource(
            TaskCreationOptions.RunContinuationsAsynchronously);
        var second = new TaskCompletionSource(
            TaskCreationOptions.RunContinuationsAsynchronously);
        tracker.Track(first.Task);
        var waiting = tracker.WaitForIdleAsync();

        tracker.Track(second.Task);
        first.SetResult();
        await Task.Yield();

        Assert.False(waiting.IsCompleted);
        second.SetResult();
        await waiting;
    }

    [Fact]
    public async Task WaitForIdleReturnsWhenCompletedOperationWasTracked()
    {
        var tracker = new PendingOperationTracker();
        tracker.Track(Task.CompletedTask);

        await tracker.WaitForIdleAsync();
    }
}
