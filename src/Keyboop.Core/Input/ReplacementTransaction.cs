namespace Keyboop.Core.Input;

public sealed class ReplacementTransaction
{
    public const int DefaultMaximumQueuedEvents = 32;
    public static readonly TimeSpan DefaultDeadline = TimeSpan.FromMilliseconds(35);

    private readonly List<PhysicalKeyEvent> _queuedEvents = [];
    private readonly DateTimeOffset _deadline;
    private readonly int _maximumQueuedEvents;
    private TransactionStatus _status;

    public ReplacementTransaction(
        ReplacementPlan plan,
        TargetSnapshot target,
        DateTimeOffset startedAt,
        TimeSpan? deadline = null,
        int maximumQueuedEvents = DefaultMaximumQueuedEvents)
    {
        ArgumentNullException.ThrowIfNull(plan);
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(maximumQueuedEvents);

        Plan = plan;
        Target = target;
        StartedAt = startedAt;
        _deadline = startedAt + (deadline ?? DefaultDeadline);
        _maximumQueuedEvents = maximumQueuedEvents;
        _status = TransactionStatus.Prepared;
    }

    public ReplacementPlan Plan { get; }

    public TargetSnapshot Target { get; }

    public DateTimeOffset StartedAt { get; }

    public TransactionStatus Status => _status;

    public IReadOnlyList<PhysicalKeyEvent> QueuedEvents => _queuedEvents;

    public bool TryQueue(PhysicalKeyEvent input, DateTimeOffset now)
    {
        if (_status != TransactionStatus.Prepared)
        {
            return false;
        }

        if (now > _deadline || _queuedEvents.Count >= _maximumQueuedEvents)
        {
            _status = TransactionStatus.Cancelled;
            return false;
        }

        _queuedEvents.Add(input);
        return true;
    }

    public ReplacementOutcome Complete(TargetSnapshot currentTarget, DateTimeOffset now)
    {
        if (_status != TransactionStatus.Prepared)
        {
            return ReplacementOutcome.Replay(
                _queuedEvents,
                _status == TransactionStatus.Cancelled
                    ? "already-cancelled"
                    : "invalid-state");
        }

        if (now > _deadline)
        {
            _status = TransactionStatus.Cancelled;
            return ReplacementOutcome.Replay(_queuedEvents, "deadline");
        }

        if (currentTarget != Target)
        {
            _status = TransactionStatus.Cancelled;
            return ReplacementOutcome.Replay(_queuedEvents, "target-changed");
        }

        _status = TransactionStatus.Committed;
        return ReplacementOutcome.Commit(Plan, _queuedEvents);
    }

    public ReplacementOutcome Cancel(string reason)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(reason);
        _status = TransactionStatus.Cancelled;
        return ReplacementOutcome.Replay(_queuedEvents, reason);
    }
}

public enum TransactionStatus
{
    Prepared,
    Committed,
    Cancelled
}

public readonly record struct TargetSnapshot(
    long WindowHandle,
    uint ThreadId,
    long KeyboardLayout,
    uint ProcessId);

public readonly record struct PhysicalKeyEvent(
    uint VirtualKey,
    uint ScanCode,
    bool KeyUp,
    uint Flags,
    ulong Sequence,
    long Timestamp);

public sealed record ReplacementPlan(
    int DeleteCount,
    string Replacement,
    PhysicalKeyEvent Boundary)
{
    public int DeleteCount { get; init; } = DeleteCount >= 0
        ? DeleteCount
        : throw new ArgumentOutOfRangeException(nameof(DeleteCount));

    public string Replacement { get; init; } =
        Replacement ?? throw new ArgumentNullException(nameof(Replacement));
}

public sealed record ReplacementOutcome(
    bool ShouldCommit,
    ReplacementPlan? Plan,
    IReadOnlyList<PhysicalKeyEvent> EventsToReplay,
    string Reason)
{
    public static ReplacementOutcome Commit(
        ReplacementPlan plan,
        IReadOnlyList<PhysicalKeyEvent> queued) =>
        new(true, plan, queued.ToArray(), "commit");

    public static ReplacementOutcome Replay(
        IReadOnlyList<PhysicalKeyEvent> queued,
        string reason) =>
        new(false, null, queued.ToArray(), reason);
}
