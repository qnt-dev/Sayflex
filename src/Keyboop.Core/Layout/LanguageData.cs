using System.Reflection;
using System.Text.Json;

namespace Keyboop.Core.Layout;

public sealed class LanguageData
{
    private const double MissingTrigramPenalty = -20.0;

    public LanguageData(
        IReadOnlyDictionary<string, double> trigramsRu,
        IReadOnlyDictionary<string, double> trigramsEn,
        IReadOnlySet<string> wordsRu,
        IReadOnlySet<string> wordsEn)
    {
        TrigramsRu = trigramsRu;
        TrigramsEn = trigramsEn;
        WordsRu = wordsRu;
        WordsEn = wordsEn;
    }

    public IReadOnlyDictionary<string, double> TrigramsRu { get; }

    public IReadOnlyDictionary<string, double> TrigramsEn { get; }

    public IReadOnlySet<string> WordsRu { get; }

    public IReadOnlySet<string> WordsEn { get; }

    public static LanguageData LoadEmbedded(Assembly? assembly = null)
    {
        assembly ??= typeof(LanguageData).Assembly;
        var wordsRu = LoadSet(assembly, "words_ru").Union(ExtraWords.RussianWords);
        var wordsEn = LoadSet(assembly, "words_en").Union(ExtraWords.EnglishWords);

        return new LanguageData(
            LoadDictionary(assembly, "trigrams_ru"),
            LoadDictionary(assembly, "trigrams_en"),
            wordsRu.ToHashSet(StringComparer.Ordinal),
            wordsEn.ToHashSet(StringComparer.Ordinal));
    }

    public double Plausibility(string word, bool cyrillic)
    {
        ArgumentNullException.ThrowIfNull(word);
        var table = cyrillic ? TrigramsRu : TrigramsEn;
        var padded = $" {word.ToLowerInvariant()} ";
        if (padded.Length < 3)
        {
            return double.NegativeInfinity;
        }

        var sum = 0.0;
        var count = padded.Length - 2;
        for (var index = 0; index < count; index++)
        {
            var trigram = padded.Substring(index, 3);
            sum += table.TryGetValue(trigram, out var score) ? score : MissingTrigramPenalty;
        }

        return sum / count;
    }

    private static Dictionary<string, double> LoadDictionary(
        Assembly assembly,
        string name)
    {
        using var stream = OpenResource(assembly, name);
        return JsonSerializer.Deserialize<Dictionary<string, double>>(stream)
            ?? throw new InvalidDataException($"Language resource '{name}' is empty.");
    }

    private static HashSet<string> LoadSet(Assembly assembly, string name)
    {
        using var stream = OpenResource(assembly, name);
        var words = JsonSerializer.Deserialize<string[]>(stream)
            ?? throw new InvalidDataException($"Language resource '{name}' is empty.");
        return words.ToHashSet(StringComparer.Ordinal);
    }

    private static Stream OpenResource(Assembly assembly, string name)
    {
        var resourceName = $"Keyboop.Core.Resources.{name}.json";
        return assembly.GetManifestResourceStream(resourceName)
            ?? throw new FileNotFoundException(
                $"Embedded language resource '{resourceName}' was not found.");
    }
}
