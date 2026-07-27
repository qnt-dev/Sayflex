using System.Text.Json;

namespace Keyboop.Protocol;

public sealed class DictationHistoryStore : IDisposable
{
    public const int MaximumEntries = 200;

    private readonly SemaphoreSlim _gate = new(1, 1);

    public DictationHistoryStore(string? rootDirectory = null)
    {
        RootDirectory = rootDirectory
            ?? Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Keyboop");
    }

    public string RootDirectory { get; }

    public string HistoryPath =>
        Path.Combine(RootDirectory, "dictation-history.json");

    public async Task<IReadOnlyList<DictationHistoryEntry>> LoadAsync(
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            return await LoadCoreAsync(cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task AppendAsync(
        DictationHistoryEntry entry,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(entry.Text);
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var entries = await LoadCoreAsync(cancellationToken).ConfigureAwait(false);
            entries.RemoveAll(
                candidate =>
                    candidate.Id == entry.Id
                    || (entry.SessionId != Guid.Empty
                        && candidate.SessionId == entry.SessionId));
            entries.Add(entry);
            entries = entries
                .OrderByDescending(candidate => candidate.CreatedAt)
                .ThenByDescending(candidate => candidate.Id)
                .Take(MaximumEntries)
                .ToList();
            await SaveCoreAsync(entries, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task DeleteAsync(
        Guid id,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var entries = await LoadCoreAsync(cancellationToken).ConfigureAwait(false);
            if (entries.RemoveAll(entry => entry.Id == id) == 0)
            {
                return;
            }

            await SaveCoreAsync(entries, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task ClearAsync(CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (File.Exists(HistoryPath))
            {
                File.Delete(HistoryPath);
            }
        }
        finally
        {
            _gate.Release();
        }
    }

    public void Dispose()
    {
        _gate.Dispose();
        GC.SuppressFinalize(this);
    }

    private async Task<List<DictationHistoryEntry>> LoadCoreAsync(
        CancellationToken cancellationToken)
    {
        try
        {
            await using var stream = File.OpenRead(HistoryPath);
            var entries = await JsonSerializer.DeserializeAsync<List<DictationHistoryEntry>>(
                stream,
                ProtocolJson.Options,
                cancellationToken).ConfigureAwait(false);
            return (entries ?? [])
                .Where(entry => !string.IsNullOrWhiteSpace(entry.Text))
                .OrderByDescending(entry => entry.CreatedAt)
                .ThenByDescending(entry => entry.Id)
                .Take(MaximumEntries)
                .ToList();
        }
        catch (FileNotFoundException)
        {
            return [];
        }
        catch (DirectoryNotFoundException)
        {
            return [];
        }
        catch (JsonException exception)
        {
            throw new ProtocolException(
                $"Dictation history is invalid: {exception.Message}");
        }
    }

    private async Task SaveCoreAsync(
        IReadOnlyList<DictationHistoryEntry> entries,
        CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(RootDirectory);
        var temporaryPath =
            $"{HistoryPath}.{Environment.ProcessId}.{Guid.NewGuid():N}.tmp";
        try
        {
            await using (var stream = new FileStream(
                temporaryPath,
                FileMode.CreateNew,
                FileAccess.Write,
                FileShare.None,
                16 * 1024,
                FileOptions.WriteThrough | FileOptions.Asynchronous))
            {
                await JsonSerializer.SerializeAsync(
                    stream,
                    entries,
                    ProtocolJson.Options,
                    cancellationToken).ConfigureAwait(false);
                await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
            }

            File.Move(temporaryPath, HistoryPath, overwrite: true);
        }
        finally
        {
            try
            {
                File.Delete(temporaryPath);
            }
            catch (IOException)
            {
            }
            catch (UnauthorizedAccessException)
            {
            }
        }
    }
}
