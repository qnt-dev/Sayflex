using System.Diagnostics;
using System.IO;
using Keyboop.Protocol;

namespace Keyboop.UI;

internal sealed class ProcessSupervisor : IAsyncDisposable
{
#if DEBUG
    private const string DevelopmentConfiguration = "Debug";
#else
    private const string DevelopmentConfiguration = "Release";
#endif

    private readonly ComponentClient _inputHost;
    private readonly ComponentClient _speechWorker;
    private Process? _inputProcess;
    private Process? _speechProcess;

    internal ProcessSupervisor(
        ComponentClient inputHost,
        ComponentClient speechWorker)
    {
        _inputHost = inputHost;
        _speechWorker = speechWorker;
    }

    internal async Task EnsureRunningAsync(CancellationToken cancellationToken = default)
    {
        _inputProcess = await EnsureComponentAsync(
            _inputHost,
            ResolveExecutable(
                "Keyboop.InputHost.exe",
                Path.Combine(
                    "Keyboop.InputHost",
                    "bin",
                    DevelopmentConfiguration,
                    "net10.0",
                    "win-x64",
                    "Keyboop.InputHost.exe")),
            cancellationToken);
        _speechProcess = await EnsureComponentAsync(
            _speechWorker,
            ResolveExecutable(
                "Keyboop.SpeechWorker.exe",
                Path.Combine(
                    "Keyboop.SpeechWorker",
                    "bin",
                    DevelopmentConfiguration,
                    "net10.0-windows",
                    "win-x64",
                    "Keyboop.SpeechWorker.exe")),
            cancellationToken);
    }

    public async ValueTask DisposeAsync()
    {
        await ShutdownBestEffortAsync(_inputHost).ConfigureAwait(false);
        await ShutdownBestEffortAsync(_speechWorker).ConfigureAwait(false);
        await _inputHost.DisposeAsync().ConfigureAwait(false);
        await _speechWorker.DisposeAsync().ConfigureAwait(false);
        _inputProcess?.Dispose();
        _speechProcess?.Dispose();
    }

    private static async Task<Process?> EnsureComponentAsync(
        ComponentClient client,
        string executable,
        CancellationToken cancellationToken)
    {
        if (await client.TryConnectAsync(TimeSpan.FromMilliseconds(350), cancellationToken)
            .ConfigureAwait(false))
        {
            return null;
        }

        if (!File.Exists(executable))
        {
            return null;
        }

        var process = Process.Start(new ProcessStartInfo
        {
            FileName = executable,
            WorkingDirectory = Path.GetDirectoryName(executable)!,
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden
        });
        for (var attempt = 0; attempt < 20; attempt++)
        {
            await Task.Delay(100, cancellationToken).ConfigureAwait(false);
            if (await client.TryConnectAsync(
                TimeSpan.FromMilliseconds(250),
                cancellationToken).ConfigureAwait(false))
            {
                break;
            }
        }

        return process;
    }

    private static string ResolveExecutable(
        string installedName,
        string developmentPath)
    {
        var installed = Path.Combine(AppContext.BaseDirectory, installedName);
        if (File.Exists(installed))
        {
            return installed;
        }

        return Path.GetFullPath(
            Path.Combine(
                AppContext.BaseDirectory,
                "..",
                "..",
                "..",
                "..",
                developmentPath));
    }

    private static async Task ShutdownBestEffortAsync(ComponentClient client)
    {
        if (!client.IsConnected)
        {
            return;
        }

        try
        {
            _ = await client.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.Shutdown,
                    new AcknowledgementMessage(true, "UI shutdown")),
                TimeSpan.FromSeconds(2)).ConfigureAwait(false);
        }
        catch (Exception)
        {
        }
    }
}
