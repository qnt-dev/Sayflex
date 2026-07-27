namespace Keyboop.Core.Input;

public static class TextInsertionPolicy
{
    public static bool PreferDirectUnicode(string? processName) =>
        string.Equals(
            processName,
            "wezterm-gui",
            StringComparison.OrdinalIgnoreCase);
}
