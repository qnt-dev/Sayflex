using Keyboop.Core.Input;
using Keyboop.InputHost;
using Keyboop.Protocol;

namespace Keyboop.InputReplay.Tests;

public sealed class InputHostSpeechClientTests
{
    [Fact]
    public async Task RecoveryRequestsTheSameSessionWhileEndIsStillPending()
    {
        var sessionId = Guid.NewGuid();
        var transcript = new TranscriptMessage(
            "current",
            "en",
            TimeSpan.FromSeconds(1),
            TimeSpan.FromMilliseconds(100),
            true,
            sessionId);
        var beginSeen = CompletionSource<Guid>();
        var endSeen = CompletionSource<Guid>();
        var recoverySeen = CompletionSource<Guid>();
        var releaseTranscript = CompletionSource<TranscriptMessage>();
        var inserted = CompletionSource<TranscriptMessage>();
        var recovered = CompletionSource<TranscriptMessage>();

        async Task<ProtocolEnvelope> RequestAsync(
            ProtocolEnvelope request,
            TimeSpan _,
            CancellationToken cancellationToken)
        {
            switch (request.Kind)
            {
                case MessageKind.BeginDictation:
                    beginSeen.TrySetResult(
                        request.GetPayload<BeginDictationMessage>().SessionId);
                    return ProtocolEnvelope.Create(
                        MessageKind.Acknowledgement,
                        new AcknowledgementMessage(true, "Listening."),
                        request.RequestId);
                case MessageKind.EndDictation:
                    endSeen.TrySetResult(
                        request.GetPayload<DictationSessionRequest>().SessionId);
                    return ProtocolEnvelope.Create(
                        MessageKind.Transcript,
                        await releaseTranscript.Task.WaitAsync(cancellationToken),
                        request.RequestId);
                case MessageKind.GetDictationTranscript:
                    recoverySeen.TrySetResult(
                        request.GetPayload<DictationSessionRequest>().SessionId);
                    return ProtocolEnvelope.Create(
                        MessageKind.Transcript,
                        await releaseTranscript.Task.WaitAsync(cancellationToken),
                        request.RequestId);
                default:
                    throw new InvalidOperationException(
                        $"Unexpected request {request.Kind}.");
            }
        }

        await using var client = new InputHostSpeechClient(
            (value, _) => inserted.TrySetResult(value),
            (value, _) => recovered.TrySetResult(value),
            RequestAsync);
        var target = new TargetSnapshot(1, 2, 3, 4);

        Assert.True(client.TryBegin(sessionId, DateTimeOffset.UtcNow, target));
        Assert.Equal(
            sessionId,
            await beginSeen.Task.WaitAsync(TimeSpan.FromSeconds(2)));
        Assert.True(client.TryEnd(sessionId, target));
        Assert.True(client.TryRecover(sessionId, target));

        Assert.Equal(
            sessionId,
            await endSeen.Task.WaitAsync(TimeSpan.FromSeconds(2)));
        Assert.Equal(
            sessionId,
            await recoverySeen.Task.WaitAsync(TimeSpan.FromSeconds(2)));

        releaseTranscript.TrySetResult(transcript);

        Assert.Equal(
            sessionId,
            (await inserted.Task.WaitAsync(TimeSpan.FromSeconds(2))).SessionId);
        Assert.Equal(
            sessionId,
            (await recovered.Task.WaitAsync(TimeSpan.FromSeconds(2))).SessionId);
    }

    private static TaskCompletionSource<T> CompletionSource<T>() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);
}
