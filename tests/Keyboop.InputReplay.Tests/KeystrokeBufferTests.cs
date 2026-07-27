using Keyboop.Core.Input;

namespace Keyboop.InputReplay.Tests;

public sealed class KeystrokeBufferTests
{
    [Fact]
    public void BoundaryCapturesWordAndTail()
    {
        var buffer = new KeystrokeBuffer();
        buffer.Append("ghbdtn");
        buffer.Boundary(" ");

        Assert.Equal("", buffer.CurrentWord);
        Assert.Equal("ghbdtn", buffer.LastWord);
        Assert.Equal(" ", buffer.LastTail);
        Assert.Equal(new ConversionWord("ghbdtn", 7, " "), buffer.WordForConversion());
    }

    [Fact]
    public void BackspaceAcrossUnknownBoundaryClearsContext()
    {
        var buffer = new KeystrokeBuffer();
        buffer.Append("hello");
        buffer.Boundary(" ");
        buffer.Backspace();

        Assert.Null(buffer.WordForConversion());
        Assert.Empty(buffer.SessionWords);
    }

    [Fact]
    public void GroupExpiresAndRejectsCombiningText()
    {
        var now = DateTimeOffset.UtcNow;
        var buffer = new KeystrokeBuffer(() => now);
        buffer.Append("hello");
        buffer.Boundary(" ");
        buffer.Append("ghbdtn");
        Assert.NotNull(buffer.GroupForConversion());

        now += TimeSpan.FromSeconds(9);
        Assert.Null(buffer.GroupForConversion());

        buffer.Clear();
        buffer.Append("e\u0301");
        buffer.Boundary(" ");
        buffer.Append("word");
        Assert.Null(buffer.GroupForConversion());
    }
}

