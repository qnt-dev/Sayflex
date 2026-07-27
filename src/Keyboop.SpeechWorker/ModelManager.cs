using System.Security.Cryptography;
using System.Text.Json;
using Whisper.net.Ggml;

namespace Keyboop.SpeechWorker;

internal sealed class ModelManager : IDisposable
{
    private static readonly Dictionary<string, ModelArtifact> KnownArtifacts =
        new Dictionary<string, ModelArtifact>(StringComparer.OrdinalIgnoreCase)
        {
            ["large-v3-turbo"] = new(
                1_624_555_275,
                "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69")
        };
    private static readonly JsonSerializerOptions ManifestJsonOptions = new()
    {
        WriteIndented = true
    };
    private readonly SemaphoreSlim _downloadGate = new(1, 1);

    internal ModelManager(string? rootDirectory = null)
    {
        RootDirectory = rootDirectory
            ?? Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Keyboop",
                "models");
    }

    internal string RootDirectory { get; }

    public void Dispose() => _downloadGate.Dispose();

    internal string GetModelPath(string model) =>
        Path.Combine(RootDirectory, $"ggml-{NormalizeModel(model)}.bin");

    internal bool HasLocalSpeechModel(string model)
    {
        var path = GetModelPath(model);
        if (!File.Exists(path))
        {
            return false;
        }

        var file = new FileInfo(path);
        if (KnownArtifacts.TryGetValue(NormalizeModel(model), out var artifact))
        {
            return file.Length == artifact.Bytes;
        }

        return file.Length > 1024 * 1024;
    }

    internal async Task<string> EnsureSpeechModelAsync(
        string model,
        CancellationToken cancellationToken)
    {
        var path = GetModelPath(model);
        if (await IsModelValidAsync(model, path, cancellationToken)
            .ConfigureAwait(false))
        {
            return path;
        }

        await _downloadGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (await IsModelValidAsync(model, path, cancellationToken)
                .ConfigureAwait(false))
            {
                return path;
            }

            Directory.CreateDirectory(RootDirectory);
            var temporaryPath = path + ".download";
            if (File.Exists(temporaryPath))
            {
                File.Delete(temporaryPath);
            }

            await using (var modelStream = await WhisperGgmlDownloader.Default
                .GetGgmlModelAsync(ToGgmlType(model), cancellationToken: cancellationToken)
                .ConfigureAwait(false))
            await using (var destination = new FileStream(
                temporaryPath,
                FileMode.CreateNew,
                FileAccess.Write,
                FileShare.None,
                1024 * 1024,
                FileOptions.Asynchronous | FileOptions.SequentialScan))
            {
                await modelStream.CopyToAsync(destination, cancellationToken)
                    .ConfigureAwait(false);
                await destination.FlushAsync(cancellationToken).ConfigureAwait(false);
            }

            var digest = await ComputeSha256Async(temporaryPath, cancellationToken)
                .ConfigureAwait(false);
            ValidateKnownArtifact(model, temporaryPath, digest);
            File.Move(temporaryPath, path, overwrite: true);
            await WriteManifestAsync(model, path, digest, cancellationToken)
                .ConfigureAwait(false);
            return path;
        }
        finally
        {
            _downloadGate.Release();
        }
    }

    private static async Task<bool> IsModelValidAsync(
        string model,
        string path,
        CancellationToken cancellationToken)
    {
        if (!File.Exists(path))
        {
            return false;
        }

        var file = new FileInfo(path);
        if (file.Length <= 1024 * 1024)
        {
            return false;
        }

        if (!KnownArtifacts.TryGetValue(NormalizeModel(model), out var artifact))
        {
            return true;
        }

        if (file.Length != artifact.Bytes)
        {
            return false;
        }

        var digest = await ComputeSha256Async(path, cancellationToken)
            .ConfigureAwait(false);
        return string.Equals(
            digest,
            artifact.Sha256,
            StringComparison.OrdinalIgnoreCase);
    }

    private static void ValidateKnownArtifact(
        string model,
        string path,
        string digest)
    {
        if (!KnownArtifacts.TryGetValue(NormalizeModel(model), out var artifact))
        {
            return;
        }

        var bytes = new FileInfo(path).Length;
        if (bytes != artifact.Bytes
            || !string.Equals(
                digest,
                artifact.Sha256,
                StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException(
                $"Downloaded model failed integrity verification. "
                + $"Expected {artifact.Bytes} bytes / {artifact.Sha256}, "
                + $"received {bytes} bytes / {digest}.");
        }
    }

    private async Task WriteManifestAsync(
        string model,
        string path,
        string digest,
        CancellationToken cancellationToken)
    {
        var manifestPath = Path.Combine(RootDirectory, "manifest.json");
        var manifest = new
        {
            model = NormalizeModel(model),
            file = Path.GetFileName(path),
            sha256 = digest,
            bytes = new FileInfo(path).Length,
            downloadedAt = DateTimeOffset.UtcNow
        };
        await File.WriteAllTextAsync(
            manifestPath,
            JsonSerializer.Serialize(manifest, ManifestJsonOptions),
            cancellationToken).ConfigureAwait(false);
    }

    private static async Task<string> ComputeSha256Async(
        string path,
        CancellationToken cancellationToken)
    {
        await using var stream = new FileStream(
            path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            1024 * 1024,
            FileOptions.Asynchronous | FileOptions.SequentialScan);
        var digest = await SHA256.HashDataAsync(stream, cancellationToken)
            .ConfigureAwait(false);
        return Convert.ToHexString(digest).ToLowerInvariant();
    }

    private static GgmlType ToGgmlType(string model) =>
        NormalizeModel(model) switch
        {
            "large-v3-turbo" => GgmlType.LargeV3Turbo,
            "large-v3" => GgmlType.LargeV3,
            "medium" => GgmlType.Medium,
            "small" => GgmlType.Small,
            _ => throw new ArgumentOutOfRangeException(
                nameof(model),
                model,
                "Supported models: large-v3-turbo, large-v3, medium, small.")
        };

    private static string NormalizeModel(string model) =>
        model.Trim().ToLowerInvariant().Replace('_', '-');

    private sealed record ModelArtifact(long Bytes, string Sha256);
}
