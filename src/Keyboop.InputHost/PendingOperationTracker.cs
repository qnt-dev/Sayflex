using System.Collections.Concurrent;

namespace Keyboop.InputHost;

internal sealed class PendingOperationTracker
{
    private readonly ConcurrentDictionary<int, Task> _operations = new();
    private int _sequence;

    internal void Track(Task operation)
    {
        ArgumentNullException.ThrowIfNull(operation);

        var id = Interlocked.Increment(ref _sequence);
        _operations[id] = operation;
        _ = operation.ContinueWith(
            completed =>
            {
                _ = completed.Exception;
                return _operations.TryRemove(id, out _);
            },
            CancellationToken.None,
            TaskContinuationOptions.ExecuteSynchronously,
            TaskScheduler.Default);
    }

    internal async Task WaitForIdleAsync(
        CancellationToken cancellationToken = default)
    {
        while (true)
        {
            var pending = _operations.Values.ToArray();
            if (pending.Length == 0)
            {
                return;
            }

            await Task.WhenAll(pending)
                .WaitAsync(cancellationToken)
                .ConfigureAwait(false);
        }
    }
}
