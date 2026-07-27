using Keyboop.Protocol;

namespace Keyboop.InputReplay.Tests;

public sealed class DictationHistoryStoreTests
{
    [Fact]
    public async Task LatestTranscriptPersistsAcrossStoreInstances()
    {
        var root = CreateTemporaryRoot();
        try
        {
            var older = Entry("older", DateTimeOffset.UtcNow.AddMinutes(-1));
            var newer = Entry("newer", DateTimeOffset.UtcNow);
            using (var writer = new DictationHistoryStore(root))
            {
                await writer.AppendAsync(older);
                await writer.AppendAsync(newer);
            }

            using var reader = new DictationHistoryStore(root);
            var entries = await reader.LoadAsync();

            Assert.Equal([newer, older], entries);
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public async Task DeleteAndClearRemoveRecoverableText()
    {
        var root = CreateTemporaryRoot();
        try
        {
            var first = Entry("first", DateTimeOffset.UtcNow.AddSeconds(-1));
            var second = Entry("second", DateTimeOffset.UtcNow);
            using var store = new DictationHistoryStore(root);
            await store.AppendAsync(first);
            await store.AppendAsync(second);

            await store.DeleteAsync(second.Id);
            Assert.Equal([first], await store.LoadAsync());

            await store.ClearAsync();
            Assert.Empty(await store.LoadAsync());
            Assert.False(File.Exists(store.HistoryPath));
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public async Task RepeatedPersistenceOfOneSessionIsIdempotent()
    {
        var root = CreateTemporaryRoot();
        try
        {
            var sessionId = Guid.NewGuid();
            var createdAt = DateTimeOffset.UtcNow;
            var first = Entry("draft", createdAt, sessionId);
            var completed = Entry("completed", createdAt, sessionId);
            using var store = new DictationHistoryStore(root);

            await store.AppendAsync(first);
            await store.AppendAsync(completed);

            var actual = Assert.Single(await store.LoadAsync());
            Assert.Equal("completed", actual.Text);
            Assert.Equal(sessionId, actual.SessionId);
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    private static DictationHistoryEntry Entry(
        string text,
        DateTimeOffset createdAt,
        Guid? sessionId = null)
    {
        var identity = sessionId ?? Guid.NewGuid();
        return
        new(
            identity,
            createdAt,
            text,
            "ru",
            TimeSpan.FromSeconds(2),
            TimeSpan.FromMilliseconds(300),
            identity);
    }

    private static string CreateTemporaryRoot()
    {
        var root = Path.Combine(
            Path.GetTempPath(),
            "Keyboop.Tests",
            Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        return root;
    }
}
