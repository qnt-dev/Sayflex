using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;

namespace Keyboop.Protocol;

public static class ProtocolConstants
{
    public const int Version = 2;
    public const int MaximumFrameBytes = 4 * 1024 * 1024;

    public static string InputHostPipeName => BuildPipeName("input");

    public static string SpeechWorkerPipeName => BuildPipeName("speech");

    private static string BuildPipeName(string component)
    {
        var identity = OperatingSystem.IsWindows()
            ? WindowsIdentity.GetCurrent().User?.Value
            : Environment.UserName;
        identity ??= Environment.UserName;
        var hash = SHA256.HashData(Encoding.UTF8.GetBytes(identity));
        return $"keyboop-{component}-{Convert.ToHexString(hash.AsSpan(0, 8)).ToLowerInvariant()}";
    }
}
