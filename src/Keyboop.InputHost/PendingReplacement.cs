using System.Diagnostics;
using Keyboop.Core.Input;

namespace Keyboop.InputHost;

internal sealed class PendingReplacement
{
    private const int MaximumQueuedEvents = 512;
    private static readonly TimeSpan Deadline = TimeSpan.FromMilliseconds(35);
    private readonly object _gate = new();
    private readonly List<PhysicalKeyEvent> _queued = [];
    private bool _completed;

    internal PendingReplacement(
        ReplacementPlan plan,
        TargetSnapshot target,
        string converted)
    {
        Plan = plan;
        Target = target;
        Converted = converted;
        CreatedTimestamp = Stopwatch.GetTimestamp();
    }

    internal ReplacementPlan Plan { get; }

    internal TargetSnapshot Target { get; }

    internal string Converted { get; }

    internal long CreatedTimestamp { get; }

    internal bool IsExpired() =>
        Stopwatch.GetElapsedTime(CreatedTimestamp) > Deadline;

    internal bool TryQueue(PhysicalKeyEvent input)
    {
        lock (_gate)
        {
            if (_completed || _queued.Count >= MaximumQueuedEvents)
            {
                return false;
            }

            _queued.Add(input);
            return true;
        }
    }

    internal IReadOnlyList<PhysicalKeyEvent> Drain()
    {
        lock (_gate)
        {
            var copy = _queued.ToArray();
            _queued.Clear();
            return copy;
        }
    }

    internal bool TryFinishWhenEmpty()
    {
        lock (_gate)
        {
            if (_queued.Count != 0)
            {
                return false;
            }

            _completed = true;
            return true;
        }
    }
}
