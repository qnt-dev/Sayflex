using System.Text.Json;
using Keyboop.Protocol;

namespace Keyboop.InputReplay.Tests;

public sealed class ProtocolBehaviorTests
{
    [Fact]
    public void DefaultRepeatLastHotkeyMatchesWindowsProductContract()
    {
        Assert.Equal("Shift+Alt+X", new AppSettings().PasteLastTranscriptHotkey);
    }

    [Fact]
    public void DictationHistoryRemainsExplicitOptInByDefault()
    {
        Assert.False(new AppSettings().KeepDictationHistory);
    }

    [Fact]
    public void DictationHistoryRoundTripsWithoutChangingText()
    {
        var sessionId = Guid.NewGuid();
        var expected = new DictationHistoryMessage(
        [
            new DictationHistoryEntry(
                sessionId,
                DateTimeOffset.UtcNow,
                "recoverable text",
                "en",
                TimeSpan.FromSeconds(1.5),
                TimeSpan.FromMilliseconds(250),
                sessionId)
        ]);

        var json = JsonSerializer.Serialize(expected, ProtocolJson.Options);
        var actual = JsonSerializer.Deserialize<DictationHistoryMessage>(
            json,
            ProtocolJson.Options);

        Assert.NotNull(actual);
        Assert.Equal(expected.Entries, actual.Entries);
    }

    [Fact]
    public void LegacyHistoryWithoutSessionIdRemainsReadable()
    {
        const string json =
            """
            {
              "entries": [
                {
                  "id": "11223344-5566-7788-99aa-bbccddeeff00",
                  "createdAt": "2026-07-24T10:00:00+00:00",
                  "text": "legacy",
                  "language": "en",
                  "audioDuration": "00:00:01",
                  "processingDuration": "00:00:00.1"
                }
              ]
            }
            """;

        var actual = JsonSerializer.Deserialize<DictationHistoryMessage>(
            json,
            ProtocolJson.Options);

        Assert.NotNull(actual);
        Assert.Equal(Guid.Empty, Assert.Single(actual.Entries).SessionId);
    }

    [Fact]
    public void SessionRequestsRoundTripWithImmutableIdentity()
    {
        var sessionId = Guid.NewGuid();
        var startedAt = DateTimeOffset.UtcNow;
        var begin = new BeginDictationMessage(sessionId, startedAt);
        var end = new DictationSessionRequest(sessionId);

        var beginJson = JsonSerializer.Serialize(begin, ProtocolJson.Options);
        var endJson = JsonSerializer.Serialize(end, ProtocolJson.Options);

        Assert.Equal(
            begin,
            JsonSerializer.Deserialize<BeginDictationMessage>(
                beginJson,
                ProtocolJson.Options));
        Assert.Equal(
            end,
            JsonSerializer.Deserialize<DictationSessionRequest>(
                endJson,
                ProtocolJson.Options));
    }

    [Fact]
    public void SpeechStatusRoundTripsNormalizedAudioLevel()
    {
        var expected = new StatusMessage(
            ComponentKind.SpeechWorker,
            ComponentState.Listening,
            "Listening.",
            DateTimeOffset.UtcNow,
            "Cuda",
            0.42);

        var json = JsonSerializer.Serialize(expected, ProtocolJson.Options);
        var actual = JsonSerializer.Deserialize<StatusMessage>(
            json,
            ProtocolJson.Options);

        Assert.NotNull(actual);
        Assert.Equal(expected.AudioLevel, actual.AudioLevel);
    }
}
