using Keyboop.Core.Input;

namespace Keyboop.Core.Tests;

public sealed class TextInsertionPolicyTests
{
    [Theory]
    [InlineData("wezterm-gui")]
    [InlineData("WEZTERM-GUI")]
    public void WezTermUsesDirectUnicode(string processName)
    {
        Assert.True(TextInsertionPolicy.PreferDirectUnicode(processName));
    }

    [Theory]
    [InlineData("notepad")]
    [InlineData("chrome")]
    [InlineData(null)]
    public void OtherApplicationsKeepClipboardFirstFallback(string? processName)
    {
        Assert.False(TextInsertionPolicy.PreferDirectUnicode(processName));
    }
}
