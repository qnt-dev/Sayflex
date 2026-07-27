using System.Globalization;
using System.IO;

namespace Keyboop.UI;

internal static class UiDiagnostics
{
    private static readonly object SyncRoot = new();

    internal static void Write(string message)
    {
        if (!string.Equals(
            Environment.GetEnvironmentVariable("KEYBOOP_DIAGNOSTICS"),
            "1",
            StringComparison.Ordinal))
        {
            return;
        }

        try
        {
            var directory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Keyboop");
            Directory.CreateDirectory(directory);
            var line = string.Create(
                CultureInfo.InvariantCulture,
                $"{DateTimeOffset.Now:O} {message}{Environment.NewLine}");
            lock (SyncRoot)
            {
                File.AppendAllText(Path.Combine(directory, "ui-debug.log"), line);
            }
        }
        catch (Exception)
        {
            // Diagnostics must never affect input or the user-facing operation.
        }
    }
}
