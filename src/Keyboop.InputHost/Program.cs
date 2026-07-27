using System.Runtime.Versioning;
using Keyboop.Protocol;

namespace Keyboop.InputHost;

internal static class Program
{
    [SupportedOSPlatform("windows10.0.17763")]
    private static int Main()
    {
        Console.OutputEncoding = System.Text.Encoding.UTF8;
        if (!OperatingSystem.IsWindowsVersionAtLeast(10, 0, 17763))
        {
            Console.Error.WriteLine("Keyboop.InputHost requires Windows 10 1809 or newer.");
            return 2;
        }

        try
        {
            var settings = new SettingsStore().LoadAsync().GetAwaiter().GetResult();
            using var host = new WindowsInputHost(settings);
            Console.WriteLine("Keyboop InputHost is active. Press Ctrl+C to stop.");
            return host.Run();
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(
                $"InputHost failed: {exception.GetType().Name}: {exception.Message}");
            return 1;
        }
    }
}
