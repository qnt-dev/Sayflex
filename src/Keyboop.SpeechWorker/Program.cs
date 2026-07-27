using System.Runtime.Versioning;
using System.Text.Json;
using Keyboop.Protocol;
using Whisper.net.Logger;

namespace Keyboop.SpeechWorker;

internal static class Program
{
    [SupportedOSPlatform("windows10.0.17763")]
    private static async Task<int> Main(string[] args)
    {
        Console.OutputEncoding = System.Text.Encoding.UTF8;
        if (!OperatingSystem.IsWindowsVersionAtLeast(10, 0, 17763))
        {
            Console.Error.WriteLine("Keyboop.SpeechWorker requires Windows 10 1809 or newer.");
            return 2;
        }

        using var shutdown = new CancellationTokenSource();
        Console.CancelKeyPress += (_, eventArgs) =>
        {
            eventArgs.Cancel = true;
            shutdown.Cancel();
        };

        try
        {
            using var diagnosticLogger = string.Equals(
                Environment.GetEnvironmentVariable("KEYBOOP_DIAGNOSTICS"),
                "1",
                StringComparison.Ordinal)
                ? LogProvider.AddConsoleLogging(WhisperLogLevel.Debug)
                : null;
            var settings = await new SettingsStore().LoadAsync(shutdown.Token);
            await using var engine = new SpeechEngine(settings);
            await engine.InitializeAsync(shutdown.Token);
            if (args.Contains(
                "--prepare-model",
                StringComparer.OrdinalIgnoreCase))
            {
                Console.WriteLine(
                    $"Preparing Whisper model {settings.SpeechModel}...");
                await engine.EnsureModelAsync(shutdown.Token);
                Console.WriteLine($"Model ready. {engine.RuntimeDetail}");
                return 0;
            }

            var fileArgument = Array.FindIndex(
                args,
                static argument => string.Equals(
                    argument,
                    "--transcribe-file",
                    StringComparison.OrdinalIgnoreCase));
            if (fileArgument >= 0)
            {
                if (fileArgument + 1 >= args.Length)
                {
                    throw new ArgumentException(
                        "--transcribe-file requires a WAV file path.");
                }

                var transcript = await engine.TranscribeWaveFileAsync(
                    Path.GetFullPath(args[fileArgument + 1]),
                    shutdown.Token);
                Console.WriteLine(
                    JsonSerializer.Serialize(
                        transcript,
                        ProtocolJson.Options));
                return 0;
            }

            await using var server = new SpeechControlServer(engine, shutdown);
            Console.WriteLine("Keyboop SpeechWorker is ready.");
            var warmup = WarmUpLocalModelAsync(engine, shutdown.Token);
            try
            {
                await server.Completion.WaitAsync(shutdown.Token);
            }
            finally
            {
                shutdown.Cancel();
                await warmup.ConfigureAwait(false);
            }

            return 0;
        }
        catch (OperationCanceledException) when (shutdown.IsCancellationRequested)
        {
            return 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(
                $"SpeechWorker failed: {exception.GetType().Name}: {exception.Message}");
            return 1;
        }
    }

    [SupportedOSPlatform("windows10.0.17763")]
    private static async Task WarmUpLocalModelAsync(
        SpeechEngine engine,
        CancellationToken cancellationToken)
    {
        if (!engine.HasLocalModel)
        {
            return;
        }

        try
        {
            await engine.EnsureModelAsync(cancellationToken).ConfigureAwait(false);
            Console.WriteLine($"Speech model warmed up. {engine.RuntimeDetail}");
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(
                $"Speech model warmup failed: {exception.GetType().Name}: "
                + exception.Message);
        }
    }
}
