using Keyboop.Core.Layout;

namespace Keyboop.Core.Tests;

public sealed class LayoutKeymapTests
{
    [Theory]
    [InlineData("ghbdtn", true, "привет")]
    [InlineData("руддщ", false, "hello")]
    [InlineData("Ghbdtn", true, "Привет")]
    [InlineData("руддщ!", false, "hello!")]
    public void ConvertMapsStandardRuEnPhysicalKeys(
        string input,
        bool toCyrillic,
        string expected)
    {
        Assert.Equal(expected, LayoutKeymap.Convert(input, toCyrillic));
    }

    [Fact]
    public void SmartConvertPreservesSemanticTrailingPunctuation()
    {
        Assert.Equal("привет.", LayoutKeymap.SmartConvert("ghbdtn.", true));
    }

    [Theory]
    [InlineData("ghbdtn", LayoutConversionDirection.ToRussian, "привет")]
    [InlineData("руддщ", LayoutConversionDirection.ToEnglish, "hello")]
    [InlineData(
        "Ghbdtn, vbh!",
        LayoutConversionDirection.ToRussian,
        "Привет, мир!")]
    public void SelectionCandidatesContainThePhysicalLayoutAlternative(
        string input,
        LayoutConversionDirection expectedDirection,
        string expectedText)
    {
        var candidate = Assert.Single(
            SelectionLayoutConverter.BuildCandidates(input));

        Assert.Equal(expectedDirection, candidate.Direction);
        Assert.Equal(expectedText, candidate.Text);
    }

    [Theory]
    [InlineData("")]
    [InlineData("12345")]
    [InlineData("!?.")]
    public void SelectionCandidatesIgnoreTextWithoutLayoutLetters(string input)
    {
        Assert.Empty(SelectionLayoutConverter.BuildCandidates(input));
    }

    [Theory]
    [InlineData(",f,f", "баба")]
    [InlineData(";bpyь", "жизнь")]
    [InlineData("[hello]", "[руддщ]")]
    [InlineData("ghbdtn.", "привет.")]
    public void SelectionConversionDistinguishesWordKeysFromPunctuation(
        string input,
        string expected)
    {
        Assert.Equal(
            expected,
            SelectionLayoutConverter.ConvertSelection(input, toCyrillic: true));
    }

    [Theory]
    [InlineData("(tckb", "tckb")]
    [InlineData("«ghbdtn!»", "ghbdtn")]
    [InlineData("ghbdtn2", "ghbdtn2")]
    public void LetterCoreRemovesOnlyNonLayoutEdges(string input, string expected)
    {
        Assert.Equal(expected, LayoutDetector.LetterCore(input));
    }
}
