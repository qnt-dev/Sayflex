using System.Text;

namespace Keyboop.Core.Layout;

public static class LayoutKeymap
{
    private static readonly (char En, char Ru)[] BasePairs =
    [
        ('`', 'ё'), ('q', 'й'), ('w', 'ц'), ('e', 'у'), ('r', 'к'), ('t', 'е'),
        ('y', 'н'), ('u', 'г'), ('i', 'ш'), ('o', 'щ'), ('p', 'з'), ('[', 'х'),
        (']', 'ъ'), ('a', 'ф'), ('s', 'ы'), ('d', 'в'), ('f', 'а'), ('g', 'п'),
        ('h', 'р'), ('j', 'о'), ('k', 'л'), ('l', 'д'), (';', 'ж'), ('\'', 'э'),
        ('z', 'я'), ('x', 'ч'), ('c', 'с'), ('v', 'м'), ('b', 'и'), ('n', 'т'),
        ('m', 'ь'), (',', 'б'), ('.', 'ю'), ('/', '.')
    ];

    private static readonly Dictionary<char, char> EnToRu = BuildMap(true);
    private static readonly Dictionary<char, char> RuToEn = BuildMap(false);
    private static readonly HashSet<char> TrailingPunctuation = ['.', ',', '!', '?', ';', ':', '…'];

    public static string Convert(string text, bool toCyrillic)
    {
        ArgumentNullException.ThrowIfNull(text);
        var map = toCyrillic ? EnToRu : RuToEn;
        var output = new StringBuilder(text.Length);
        foreach (var character in text)
        {
            output.Append(map.TryGetValue(character, out var converted) ? converted : character);
        }

        return output.ToString();
    }

    public static string SmartConvert(
        string word,
        bool toCyrillic,
        Func<string, bool>? isValidTarget = null)
    {
        ArgumentNullException.ThrowIfNull(word);
        if (toCyrillic && isValidTarget is not null)
        {
            var full = Convert(word, true);
            if (isValidTarget(full.ToLowerInvariant()))
            {
                return full;
            }
        }

        var coreLength = word.Length;
        while (coreLength > 0 && TrailingPunctuation.Contains(word[coreLength - 1]))
        {
            coreLength--;
        }

        if (coreLength == 0)
        {
            return word;
        }

        return Convert(word[..coreLength], toCyrillic) + word[coreLength..];
    }

    public static bool IsLayoutLetter(char character) =>
        char.IsLetter(character)
        || (EnToRu.TryGetValue(character, out var ru) && char.IsLetter(ru))
        || (RuToEn.TryGetValue(character, out var en) && char.IsLetter(en));

    public static bool HasCyrillic(string text) =>
        text.Any(static character => character is >= '\u0400' and <= '\u04ff');

    public static bool HasLatin(string text) =>
        text.Any(static character =>
            character is >= 'A' and <= 'Z' or >= 'a' and <= 'z');

    private static Dictionary<char, char> BuildMap(bool toCyrillic)
    {
        var result = new Dictionary<char, char>();
        foreach (var (en, ru) in BasePairs)
        {
            var source = toCyrillic ? en : ru;
            var target = toCyrillic ? ru : en;
            result[source] = target;
            if (char.IsLetter(en))
            {
                result[char.ToUpperInvariant(source)] = char.ToUpperInvariant(target);
            }
        }

        return result;
    }
}
