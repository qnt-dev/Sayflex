using System.Text.Json;

namespace Keyboop.Protocol;

public enum MessageKind
{
    Hello,
    StatusRequest,
    Status,
    UpdateSettings,
    InsertText,
    BeginDictation,
    EndDictation,
    CancelDictation,
    GetLastTranscript,
    GetDictationTranscript,
    GetDictationHistory,
    DeleteDictationHistoryEntry,
    ClearDictationHistory,
    DictationHistory,
    DownloadModels,
    ModelProgress,
    Transcript,
    Shutdown,
    Acknowledgement,
    Error
}

public enum ComponentKind
{
    UserInterface,
    InputHost,
    SpeechWorker
}

public enum ComponentState
{
    Starting,
    Ready,
    Listening,
    Processing,
    Downloading,
    Degraded,
    Stopped,
    Failed
}

public sealed record ProtocolEnvelope(
    int Version,
    MessageKind Kind,
    Guid RequestId,
    JsonElement Payload)
{
    public static ProtocolEnvelope Create<T>(
        MessageKind kind,
        T payload,
        Guid? requestId = null) =>
        new(
            ProtocolConstants.Version,
            kind,
            requestId ?? Guid.NewGuid(),
            JsonSerializer.SerializeToElement(payload, ProtocolJson.Options));

    public T GetPayload<T>() =>
        Payload.Deserialize<T>(ProtocolJson.Options)
        ?? throw new ProtocolException($"The {Kind} message has an empty payload.");
}

public sealed record HelloMessage(
    ComponentKind Component,
    int ProcessId,
    string ProductVersion);

public sealed record StatusMessage(
    ComponentKind Component,
    ComponentState State,
    string Detail,
    DateTimeOffset Timestamp,
    string? Backend = null,
    double? AudioLevel = null);

public sealed record AcknowledgementMessage(bool Accepted, string Detail);

public sealed record ErrorMessage(string Code, string Detail, bool Retryable);

public sealed record InsertTextMessage(
    string Text,
    bool AddTrailingSpace = false,
    bool RestoreTarget = true);

public sealed record BeginDictationMessage(
    Guid SessionId,
    DateTimeOffset StartedAt);

public sealed record DictationSessionRequest(Guid SessionId);

public sealed record TranscriptMessage(
    string Text,
    string Language,
    TimeSpan AudioDuration,
    TimeSpan ProcessingDuration,
    bool IsFinal,
    Guid SessionId = default);

public sealed record DictationHistoryEntry(
    Guid Id,
    DateTimeOffset CreatedAt,
    string Text,
    string Language,
    TimeSpan AudioDuration,
    TimeSpan ProcessingDuration,
    Guid SessionId = default);

public sealed record DictationHistoryMessage(
    IReadOnlyList<DictationHistoryEntry> Entries);

public sealed record DictationHistoryEntryRequest(Guid Id);

public sealed record ModelProgressMessage(
    string Model,
    long BytesReceived,
    long? TotalBytes,
    string State);

public sealed class ProtocolException : Exception
{
    public ProtocolException(string message)
        : base(message)
    {
    }
}
