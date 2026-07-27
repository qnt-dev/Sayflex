using Keyboop.Core.Layout;

namespace Keyboop.Core.Tests;

public sealed class LayoutDetectorTests
{
    private static readonly Lazy<LayoutDetector> Shared =
        new(() => new LayoutDetector(LanguageData.LoadEmbedded()));

    public static TheoryData<string, string?, bool, string> Cases => new()
    {
        { "ghbdtn", null, true, "привет" },
        { "руддщ", null, true, "hello" },
        { "hello", null, false, "hello" },
        { "привет", null, false, "привет" },
        { "ghbdtn.", null, true, "привет." },
        { "(tckb", null, true, "(если" },
        { "ghbdtn2", null, true, "привет2" },
        { "h2o", null, false, "h2o" },
        { "vitamin", null, false, "vitamin" },
        { "d", "vitamin", false, "d" },
        { "d", "комната", true, "в" },
        { "yt", "привет", true, "не" },
        { "in", "hello", false, "in" },
        { "вк", null, false, "вк" }
    };

    [Theory]
    [MemberData(nameof(Cases))]
    public void DecideMatchesReferenceGoldenCases(
        string input,
        string? previous,
        bool shouldConvert,
        string expected)
    {
        var decision = Shared.Value.Decide(input, previousWord: previous);
        var output = Shared.Value.ConvertForDecision(input, decision);

        Assert.Equal(shouldConvert, decision.ShouldConvert);
        Assert.Equal(expected, output);
    }

    [Fact]
    public void IgnoredWordIsNeverConverted()
    {
        var lexicon = new DetectorLexicon(
            new HashSet<string>(StringComparer.Ordinal) { "ghbdtn" },
            new HashSet<string>(StringComparer.Ordinal),
            new HashSet<string>(StringComparer.Ordinal));

        var decision = Shared.Value.Decide("ghbdtn", lexicon);

        Assert.False(decision.ShouldConvert);
        Assert.Equal("user-ignored", decision.Reason);
    }

    [Fact]
    public void LanguageResourcesAreLoaded()
    {
        var data = LanguageData.LoadEmbedded();

        Assert.True(data.WordsRu.Count > 100_000);
        Assert.True(data.WordsEn.Count > 50_000);
        Assert.True(data.TrigramsRu.Count > 1_000);
        Assert.True(data.TrigramsEn.Count > 1_000);
    }
}
