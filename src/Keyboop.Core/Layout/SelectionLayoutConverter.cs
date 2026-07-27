namespace Keyboop.Core.Layout;

public static class SelectionLayoutConverter
{
    public static List<LayoutConversionCandidate> BuildCandidates(string source)
    {
        ArgumentNullException.ThrowIfNull(source);
        var candidates = new List<LayoutConversionCandidate>(2);

        var toRussian = ConvertSelection(source, toCyrillic: true);
        if (!string.Equals(source, toRussian, StringComparison.Ordinal)
            && LayoutKeymap.HasCyrillic(toRussian))
        {
            candidates.Add(
                new LayoutConversionCandidate(
                    LayoutConversionDirection.ToRussian,
                    toRussian));
        }

        var toEnglish = ConvertSelection(source, toCyrillic: false);
        if (!string.Equals(source, toEnglish, StringComparison.Ordinal)
            && LayoutKeymap.HasLatin(toEnglish))
        {
            candidates.Add(
                new LayoutConversionCandidate(
                    LayoutConversionDirection.ToEnglish,
                    toEnglish));
        }

        return candidates;
    }

    public static string ConvertSelection(string source, bool toCyrillic)
    {
        ArgumentNullException.ThrowIfNull(source);
        var output = new System.Text.StringBuilder(source.Length);
        for (var index = 0; index < source.Length; index++)
        {
            var character = source[index];
            var converted = LayoutKeymap.Convert(
                character.ToString(),
                toCyrillic)[0];
            output.Append(
                ShouldPreservePunctuation(source, index, character, converted)
                    ? character
                    : converted);
        }

        return output.ToString();
    }

    private static bool ShouldPreservePunctuation(
        string source,
        int index,
        char character,
        char converted)
    {
        if (char.IsLetter(character)
            || !char.IsLetter(converted))
        {
            return false;
        }

        if (character == '[' && source.IndexOf(']', index + 1) >= 0)
        {
            return true;
        }

        if (character == ']' && source.LastIndexOf('[', index) >= 0)
        {
            return true;
        }

        if (character is not (',' or '.' or ';' or '/'))
        {
            return false;
        }

        return index + 1 >= source.Length
            || char.IsWhiteSpace(source[index + 1])
            || source[index + 1] is ')' or ']' or '}' or '!' or '?' or ':' or ';';
    }
}
