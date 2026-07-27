using System.IO.Pipes;
using System.Reflection;
using System.Runtime.Versioning;
using System.Collections.Concurrent;
using Keyboop.Protocol;

namespace Keyboop.SpeechWorker;

[SupportedOSPlatform("windows10.0.17763")]
internal sealed class SpeechControlServer : IAsyncDisposable
{
    private readonly SpeechEngine _engine;
    private readonly CancellationTokenSource _shutdown;
    private readonly Task _serverTask;
    private readonly ConcurrentDictionary<int, Task> _clients = new();
    private int _clientSequence;

    internal SpeechControlServer(
        SpeechEngine engine,
        CancellationTokenSource shutdown)
    {
        _engine = engine;
        _shutdown = shutdown;
        _serverTask = RunServerAsync();
    }

    internal Task Completion => _serverTask;

    public async ValueTask DisposeAsync()
    {
        _shutdown.Cancel();
        try
        {
            await _serverTask.ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (_shutdown.IsCancellationRequested)
        {
        }

        if (!_clients.IsEmpty)
        {
            await Task.WhenAll(_clients.Values).ConfigureAwait(false);
        }
    }

    private async Task RunServerAsync()
    {
        while (!_shutdown.IsCancellationRequested)
        {
            var pipe = new NamedPipeServerStream(
                ProtocolConstants.SpeechWorkerPipeName,
                PipeDirection.InOut,
                NamedPipeServerStream.MaxAllowedServerInstances,
                PipeTransmissionMode.Byte,
                PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
            try
            {
                await pipe.WaitForConnectionAsync(_shutdown.Token).ConfigureAwait(false);
            }
            catch
            {
                await pipe.DisposeAsync().ConfigureAwait(false);
                throw;
            }

            var clientId = Interlocked.Increment(ref _clientSequence);
            var task = HandleClientOwnedAsync(pipe, _shutdown.Token);
            _clients[clientId] = task;
            _ = task.ContinueWith(
                completed =>
                {
                    _ = completed.Exception;
                    return _clients.TryRemove(clientId, out _);
                },
                CancellationToken.None,
                TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
        }
    }

    private async Task HandleClientOwnedAsync(
        NamedPipeServerStream pipe,
        CancellationToken cancellationToken)
    {
        await using (pipe.ConfigureAwait(false))
        {
            try
            {
                await HandleClientAsync(pipe, cancellationToken).ConfigureAwait(false);
            }
            catch (IOException)
            {
                // Client restarts are expected.
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
            }
            catch (ProtocolException exception)
            {
                Console.Error.WriteLine($"Speech protocol error: {exception.Message}");
            }
        }
    }

    private async Task HandleClientAsync(
        NamedPipeServerStream pipe,
        CancellationToken cancellationToken)
    {
        await PipeFraming.WriteAsync(
            pipe,
            ProtocolEnvelope.Create(
                MessageKind.Hello,
                new HelloMessage(
                    ComponentKind.SpeechWorker,
                    Environment.ProcessId,
                    Assembly.GetExecutingAssembly().GetName().Version?.ToString() ?? "dev")),
            cancellationToken).ConfigureAwait(false);

        while (pipe.IsConnected)
        {
            var envelope = await PipeFraming.ReadAsync(pipe, cancellationToken)
                .ConfigureAwait(false);
            if (envelope is null)
            {
                return;
            }

            var response = await DispatchAsync(envelope, cancellationToken)
                .ConfigureAwait(false);
            await PipeFraming.WriteAsync(pipe, response, cancellationToken)
                .ConfigureAwait(false);
        }
    }

    private async Task<ProtocolEnvelope> DispatchAsync(
        ProtocolEnvelope envelope,
        CancellationToken cancellationToken)
    {
        try
        {
            switch (envelope.Kind)
            {
                case MessageKind.StatusRequest:
                    return ProtocolEnvelope.Create(
                        MessageKind.Status,
                        new StatusMessage(
                            ComponentKind.SpeechWorker,
                            _engine.State,
                            _engine.StatusDetail,
                            DateTimeOffset.UtcNow,
                            _engine.RuntimeDetail,
                            _engine.AudioLevel),
                        envelope.RequestId);
                case MessageKind.UpdateSettings:
                    _engine.UpdateSettings(envelope.GetPayload<AppSettings>());
                    return Accepted(envelope, "Settings applied.");
                case MessageKind.DownloadModels:
                    await _engine.EnsureModelAsync(cancellationToken).ConfigureAwait(false);
                    return Accepted(envelope, $"Model ready. {_engine.RuntimeDetail}");
                case MessageKind.BeginDictation:
                    await _engine.BeginDictationAsync(
                            envelope.GetPayload<BeginDictationMessage>(),
                            cancellationToken)
                        .ConfigureAwait(false);
                    return Accepted(envelope, "Listening.");
                case MessageKind.EndDictation:
                    var transcript = await _engine.EndDictationAsync(
                            envelope.GetPayload<DictationSessionRequest>(),
                            cancellationToken)
                        .ConfigureAwait(false);
                    return ProtocolEnvelope.Create(
                        MessageKind.Transcript,
                        transcript,
                        envelope.RequestId);
                case MessageKind.CancelDictation:
                    await _engine.CancelDictationAsync(
                            envelope.GetPayload<DictationSessionRequest>(),
                            cancellationToken)
                        .ConfigureAwait(false);
                    return Accepted(envelope, "Dictation cancelled.");
                case MessageKind.GetLastTranscript:
                    return ProtocolEnvelope.Create(
                        MessageKind.Transcript,
                        _engine.GetLastTranscript(),
                        envelope.RequestId);
                case MessageKind.GetDictationTranscript:
                    return ProtocolEnvelope.Create(
                        MessageKind.Transcript,
                        await _engine.GetDictationTranscriptAsync(
                                envelope.GetPayload<DictationSessionRequest>(),
                                cancellationToken)
                            .ConfigureAwait(false),
                        envelope.RequestId);
                case MessageKind.GetDictationHistory:
                    return ProtocolEnvelope.Create(
                        MessageKind.DictationHistory,
                        new DictationHistoryMessage(
                            await _engine.GetHistoryAsync(cancellationToken)
                                .ConfigureAwait(false)),
                        envelope.RequestId);
                case MessageKind.DeleteDictationHistoryEntry:
                    await _engine.DeleteHistoryEntryAsync(
                        envelope.GetPayload<DictationHistoryEntryRequest>().Id,
                        cancellationToken).ConfigureAwait(false);
                    return Accepted(envelope, "History entry deleted.");
                case MessageKind.ClearDictationHistory:
                    await _engine.ClearHistoryAsync(cancellationToken)
                        .ConfigureAwait(false);
                    return Accepted(envelope, "Dictation history cleared.");
                case MessageKind.Shutdown:
                    _shutdown.Cancel();
                    return Accepted(envelope, "SpeechWorker is stopping.");
                default:
                    return Error(
                        envelope,
                        "unsupported_message",
                        $"SpeechWorker does not accept {envelope.Kind}.",
                        false);
            }
        }
        catch (Exception exception) when (
            exception is not OperationCanceledException
            || !cancellationToken.IsCancellationRequested)
        {
            return Error(
                envelope,
                "speech_error",
                $"{exception.GetType().Name}: {exception.Message}",
                true);
        }
    }

    private static ProtocolEnvelope Accepted(
        ProtocolEnvelope request,
        string detail) =>
        ProtocolEnvelope.Create(
            MessageKind.Acknowledgement,
            new AcknowledgementMessage(true, detail),
            request.RequestId);

    private static ProtocolEnvelope Error(
        ProtocolEnvelope request,
        string code,
        string detail,
        bool retryable) =>
        ProtocolEnvelope.Create(
            MessageKind.Error,
            new ErrorMessage(code, detail, retryable),
            request.RequestId);
}
