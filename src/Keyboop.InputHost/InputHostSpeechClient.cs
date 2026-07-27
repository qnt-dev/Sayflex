using System.IO.Pipes;
using System.Threading.Channels;
using Keyboop.Core.Input;
using Keyboop.Protocol;

namespace Keyboop.InputHost;

internal sealed class InputHostSpeechClient : ISpeechActionSink, IAsyncDisposable
{
    private readonly Channel<SpeechAction> _actions =
        Channel.CreateUnbounded<SpeechAction>(
            new UnboundedChannelOptions
            {
                SingleReader = true,
                SingleWriter = false,
                AllowSynchronousContinuations = false
            });
    private readonly PendingOperationTracker _finalizations = new();
    private readonly Action<TranscriptMessage, TargetSnapshot> _onTranscript;
    private readonly Action<TranscriptMessage, TargetSnapshot> _onRecoveryTranscript;
    private readonly Func<
        ProtocolEnvelope,
        TimeSpan,
        CancellationToken,
        Task<ProtocolEnvelope>> _request;
    private readonly CancellationTokenSource _shutdown = new();
    private readonly Task _worker;

    internal InputHostSpeechClient(
        Action<TranscriptMessage, TargetSnapshot> onTranscript,
        Action<TranscriptMessage, TargetSnapshot> onRecoveryTranscript,
        Func<
            ProtocolEnvelope,
            TimeSpan,
            CancellationToken,
            Task<ProtocolEnvelope>>? request = null)
    {
        _onTranscript = onTranscript;
        _onRecoveryTranscript = onRecoveryTranscript;
        _request = request ?? RequestAsync;
        _worker = Task.Run(ProcessActionsAsync);
    }

    public bool TryBegin(
        Guid sessionId,
        DateTimeOffset startedAt,
        TargetSnapshot target) =>
        _actions.Writer.TryWrite(
            new SpeechAction(
                SpeechActionKind.Begin,
                sessionId,
                startedAt,
                target));

    public bool TryEnd(Guid sessionId, TargetSnapshot target) =>
        _actions.Writer.TryWrite(
            new SpeechAction(
                SpeechActionKind.End,
                sessionId,
                default,
                target));

    public bool TryRecover(Guid? sessionId, TargetSnapshot target) =>
        _actions.Writer.TryWrite(
            new SpeechAction(
                SpeechActionKind.Recover,
                sessionId,
                default,
                target));

    public async ValueTask DisposeAsync()
    {
        _actions.Writer.TryComplete();
        _shutdown.Cancel();
        try
        {
            await _worker.ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (_shutdown.IsCancellationRequested)
        {
        }

        await _finalizations.WaitForIdleAsync().ConfigureAwait(false);

        _shutdown.Dispose();
    }

    private async Task ProcessActionsAsync()
    {
        await foreach (var action in _actions.Reader.ReadAllAsync(_shutdown.Token))
        {
            switch (action.Kind)
            {
                case SpeechActionKind.Begin:
                    await BeginAsync(action).ConfigureAwait(false);
                    break;
                case SpeechActionKind.End:
                    TrackFinalization(FinalizeAsync(action));
                    break;
                case SpeechActionKind.Recover:
                    await RecoverAsync(action).ConfigureAwait(false);
                    break;
            }
        }
    }

    private async Task BeginAsync(SpeechAction action)
    {
        try
        {
            var response = await _request(
                ProtocolEnvelope.Create(
                    MessageKind.BeginDictation,
                    new BeginDictationMessage(
                        action.SessionId
                            ?? throw new ProtocolException(
                                "Begin action has no dictation session ID."),
                        action.StartedAt)),
                TimeSpan.FromSeconds(5),
                _shutdown.Token).ConfigureAwait(false);
            LogErrorResponse(response);
        }
        catch (Exception exception) when (
            exception is not OperationCanceledException
            || !_shutdown.IsCancellationRequested)
        {
            LogConnectionFailure(exception);
        }
    }

    private async Task FinalizeAsync(SpeechAction action)
    {
        try
        {
            var response = await _request(
                ProtocolEnvelope.Create(
                    MessageKind.EndDictation,
                    new DictationSessionRequest(
                        action.SessionId
                            ?? throw new ProtocolException(
                                "End action has no dictation session ID."))),
                TimeSpan.FromMinutes(10),
                _shutdown.Token).ConfigureAwait(false);
            if (response.Kind == MessageKind.Transcript)
            {
                ForwardTranscript(
                    response.GetPayload<TranscriptMessage>(),
                    action.Target);
            }
            else
            {
                LogErrorResponse(response);
            }
        }
        catch (Exception exception) when (
            exception is not OperationCanceledException
            || !_shutdown.IsCancellationRequested)
        {
            LogConnectionFailure(exception);
        }
    }

    private async Task RecoverAsync(SpeechAction action)
    {
        try
        {
            var sessionId = action.SessionId;
            var request = sessionId is null
                ? ProtocolEnvelope.Create(
                    MessageKind.GetLastTranscript,
                    new AcknowledgementMessage(
                        true,
                        "Recover latest completed transcript"))
                : ProtocolEnvelope.Create(
                    MessageKind.GetDictationTranscript,
                    new DictationSessionRequest(sessionId.Value));
            var response = await _request(
                request,
                sessionId is null
                    ? TimeSpan.FromSeconds(3)
                    : TimeSpan.FromMinutes(10),
                _shutdown.Token).ConfigureAwait(false);
            if (response.Kind == MessageKind.Transcript)
            {
                ForwardTranscript(
                    response.GetPayload<TranscriptMessage>(),
                    action.Target,
                    _onRecoveryTranscript);
            }
            else
            {
                LogErrorResponse(response);
            }
        }
        catch (Exception exception) when (
            exception is not OperationCanceledException
            || !_shutdown.IsCancellationRequested)
        {
            LogConnectionFailure(exception);
        }
    }

    private void TrackFinalization(Task task)
    {
        _finalizations.Track(task);
    }

    private void ForwardTranscript(
        TranscriptMessage transcript,
        TargetSnapshot target)
    {
        ForwardTranscript(transcript, target, _onTranscript);
    }

    private static void ForwardTranscript(
        TranscriptMessage transcript,
        TargetSnapshot target,
        Action<TranscriptMessage, TargetSnapshot> destination)
    {
        if (!string.IsNullOrWhiteSpace(transcript.Text))
        {
            destination(transcript, target);
        }
    }

    private static async Task<ProtocolEnvelope> RequestAsync(
        ProtocolEnvelope request,
        TimeSpan timeout,
        CancellationToken cancellationToken)
    {
        using var timeoutSource = CancellationTokenSource.CreateLinkedTokenSource(
            cancellationToken);
        timeoutSource.CancelAfter(timeout);
        await using var pipe = new NamedPipeClientStream(
            ".",
            ProtocolConstants.SpeechWorkerPipeName,
            PipeDirection.InOut,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        await pipe.ConnectAsync(TimeSpan.FromSeconds(2), timeoutSource.Token)
            .ConfigureAwait(false);
        var hello = await PipeFraming.ReadAsync(pipe, timeoutSource.Token)
            .ConfigureAwait(false);
        if (hello?.Kind != MessageKind.Hello)
        {
            throw new ProtocolException("SpeechWorker did not send hello.");
        }

        await PipeFraming.WriteAsync(pipe, request, timeoutSource.Token)
            .ConfigureAwait(false);
        return await PipeFraming.ReadAsync(pipe, timeoutSource.Token)
            .ConfigureAwait(false)
            ?? throw new EndOfStreamException("SpeechWorker closed the pipe.");
    }

    private static void LogErrorResponse(ProtocolEnvelope response)
    {
        if (response.Kind == MessageKind.Error)
        {
            Console.Error.WriteLine(
                $"Speech hotkey failed: {response.GetPayload<ErrorMessage>().Detail}");
        }
    }

    private static void LogConnectionFailure(Exception exception) =>
        Console.Error.WriteLine(
            $"Speech worker connection failed: {exception.GetType().Name}: {exception.Message}");

    private sealed record SpeechAction(
        SpeechActionKind Kind,
        Guid? SessionId,
        DateTimeOffset StartedAt,
        TargetSnapshot Target);

    private enum SpeechActionKind
    {
        Begin,
        End,
        Recover
    }
}
