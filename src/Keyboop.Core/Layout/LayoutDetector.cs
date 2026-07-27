namespace Keyboop.Core.Layout;

public sealed class LayoutDetector
{
    public const double DefaultMargin = 2.0;
    public const double LiveImpossible = -13.0;
    public const double LiveMargin = 6.0;

    private readonly LanguageData _data;

    public LayoutDetector(LanguageData data)
    {
        _data = data ?? throw new ArgumentNullException(nameof(data));
    }

    public SwapDecision Decide(
        string raw,
        DetectorLexicon? lexicon = null,
        string? previousWord = null)
    {
        ArgumentNullException.ThrowIfNull(raw);
        lexicon ??= DetectorLexicon.Empty;

        var context = GetContext(previousWord);
        var core = LetterCore(raw);
        var hadDigits = core.Any(char.IsDigit);
        if (hadDigits)
        {
            core = TrimEdgeDigits(core);
        }

        var word = core.ToLowerInvariant();
        if (word.Length < (hadDigits ? 4 : 1)
            || !word.All(LayoutKeymap.IsLayoutLetter))
        {
            return SwapDecision.Keep(raw, raw, "non-layout-token");
        }

        if (lexicon.Ignored.Contains(word))
        {
            return SwapDecision.Keep(raw, raw, "user-ignored");
        }

        if (lexicon.Learned.Contains(word))
        {
            return SwapDecision.Keep(raw, raw, "undo-learned");
        }

        if (ExtraWords.DefaultKeep.Contains(word))
        {
            return SwapDecision.Keep(raw, raw, "default-keep");
        }

        var sourceCyrillic = LayoutKeymap.HasCyrillic(word);
        var sourceLatin = LayoutKeymap.HasLatin(word);
        if (sourceCyrillic == sourceLatin)
        {
            return SwapDecision.Keep(raw, raw, "mixed-or-unknown-script");
        }

        var toCyrillic = !sourceCyrillic;
        var candidate = LayoutKeymap.Convert(core, toCyrillic).ToLowerInvariant();
        if (candidate == word || !candidate.All(char.IsLetter))
        {
            return SwapDecision.Keep(raw, candidate, "invalid-candidate");
        }

        var sourceIsRealWord = _data.WordsRu.Contains(word) || _data.WordsEn.Contains(word);
        if (ExtraWords.ForceSwapAbbreviations.Contains(candidate) && !sourceIsRealWord)
        {
            return SwapDecision.Convert(toCyrillic, raw, candidate, "known-abbreviation");
        }

        if (lexicon.ForceSwap.Contains(candidate))
        {
            return SwapDecision.Convert(toCyrillic, raw, candidate, "user-force-target");
        }

        if (ExtraWords.ForceSwapAbbreviations.Contains(word)
            || lexicon.ForceSwap.Contains(word))
        {
            return SwapDecision.Keep(raw, candidate, "known-intentional-token");
        }

        if (word.Length >= 2
            && sourceIsRealWord
            && !(sourceLatin && ExtraWords.ForceRussianAmbiguous.Contains(word)))
        {
            return SwapDecision.Keep(raw, candidate, "valid-source-word");
        }

        if (word.Length == 1)
        {
            if (sourceLatin && ExtraWords.RussianSingleLetters.Contains(candidate))
            {
                if (previousWord is not null)
                {
                    var previous = previousWord.ToLowerInvariant();
                    if (context == ContextHint.Latin
                        && ExtraWords.EnglishLabelClassifiers.Contains(previous))
                    {
                        return SwapDecision.Keep(raw, candidate, "latin-label-marker");
                    }

                    if (context == ContextHint.Cyrillic
                        && ExtraWords.RussianLabelClassifiers.Contains(previous))
                    {
                        return SwapDecision.Keep(raw, candidate, "russian-label-marker");
                    }
                }

                return SwapDecision.Convert(true, raw, candidate, "single-russian-letter");
            }

            return SwapDecision.Keep(raw, candidate, "ambiguous-single-letter");
        }

        if (sourceLatin)
        {
            if (_data.WordsEn.Contains(word)
                && !ExtraWords.ForceRussianAmbiguous.Contains(word))
            {
                if (context == ContextHint.Cyrillic
                    && _data.WordsRu.Contains(candidate)
                    && !ExtraWords.EnglishKeepShort.Contains(word))
                {
                    return SwapDecision.Convert(true, raw, candidate, "cyrillic-context-collision");
                }

                return SwapDecision.Keep(raw, candidate, "english-dictionary");
            }

            if (_data.WordsRu.Contains(candidate))
            {
                if (context == ContextHint.Latin
                    && ExtraWords.EnglishKeepShort.Contains(word))
                {
                    return SwapDecision.Keep(raw, candidate, "latin-context-short-token");
                }

                return SwapDecision.Convert(true, raw, candidate, "russian-dictionary-target");
            }
        }
        else
        {
            if (_data.WordsRu.Contains(word))
            {
                if (context == ContextHint.Latin && _data.WordsEn.Contains(candidate))
                {
                    return SwapDecision.Convert(false, raw, candidate, "latin-context-collision");
                }

                return SwapDecision.Keep(raw, candidate, "russian-dictionary");
            }

            if (_data.WordsEn.Contains(candidate))
            {
                return SwapDecision.Convert(false, raw, candidate, "english-dictionary-target");
            }
        }

        if (word.Length < 4)
        {
            return SwapDecision.Keep(raw, candidate, "short-unknown-token");
        }

        var originalScore = _data.Plausibility(word, sourceCyrillic);
        var candidateScore = _data.Plausibility(candidate, toCyrillic);
        if (candidateScore > originalScore + DefaultMargin)
        {
            return SwapDecision.Convert(
                toCyrillic,
                raw,
                candidate,
                "trigram-margin",
                originalScore,
                candidateScore);
        }

        if (originalScore <= -19.0 && candidateScore > originalScore + 1.0)
        {
            return SwapDecision.Convert(
                toCyrillic,
                raw,
                candidate,
                "impossible-source",
                originalScore,
                candidateScore);
        }

        return SwapDecision.Keep(
            raw,
            candidate,
            "insufficient-evidence",
            originalScore,
            candidateScore);
    }

    public SwapDecision LiveDecide(string raw)
    {
        ArgumentNullException.ThrowIfNull(raw);
        var core = TrimEdgeDigits(LetterCore(raw));
        var word = core.ToLowerInvariant();
        if (word.Length < 4 || !word.All(LayoutKeymap.IsLayoutLetter))
        {
            return SwapDecision.Keep(raw, raw, "live-short-or-invalid");
        }

        var sourceCyrillic = LayoutKeymap.HasCyrillic(word);
        var sourceLatin = LayoutKeymap.HasLatin(word);
        if (sourceCyrillic == sourceLatin)
        {
            return SwapDecision.Keep(raw, raw, "live-mixed-or-unknown");
        }

        var toCyrillic = !sourceCyrillic;
        var candidate = LayoutKeymap.Convert(core, toCyrillic).ToLowerInvariant();
        if (candidate == word || !candidate.All(char.IsLetter))
        {
            return SwapDecision.Keep(raw, candidate, "live-invalid-candidate");
        }

        if ((sourceLatin && _data.WordsEn.Contains(word))
            || (sourceCyrillic && _data.WordsRu.Contains(word)))
        {
            return SwapDecision.Keep(raw, candidate, "live-valid-source");
        }

        var originalScore = _data.Plausibility(word, sourceCyrillic);
        var candidateScore = _data.Plausibility(candidate, toCyrillic);
        return originalScore <= LiveImpossible
               && candidateScore > originalScore + LiveMargin
            ? SwapDecision.Convert(
                toCyrillic,
                raw,
                candidate,
                "live-trigram",
                originalScore,
                candidateScore)
            : SwapDecision.Keep(
                raw,
                candidate,
                "live-insufficient-evidence",
                originalScore,
                candidateScore);
    }

    public string ConvertForDecision(string raw, SwapDecision decision)
    {
        ArgumentNullException.ThrowIfNull(raw);
        if (!decision.ShouldConvert)
        {
            return raw;
        }

        return LayoutKeymap.SmartConvert(
            raw,
            decision.ToCyrillic,
            decision.ToCyrillic ? _data.WordsRu.Contains : _data.WordsEn.Contains);
    }

    public static string LetterCore(string raw)
    {
        ArgumentNullException.ThrowIfNull(raw);
        var start = 0;
        var end = raw.Length;
        while (start < end
               && !LayoutKeymap.IsLayoutLetter(raw[start])
               && !char.IsDigit(raw[start]))
        {
            start++;
        }

        while (end > start
               && !LayoutKeymap.IsLayoutLetter(raw[end - 1])
               && !char.IsDigit(raw[end - 1]))
        {
            end--;
        }

        return raw[start..end];
    }

    private static ContextHint GetContext(string? word)
    {
        if (string.IsNullOrEmpty(word))
        {
            return ContextHint.None;
        }

        if (LayoutKeymap.HasCyrillic(word))
        {
            return ContextHint.Cyrillic;
        }

        return LayoutKeymap.HasLatin(word) ? ContextHint.Latin : ContextHint.None;
    }

    private static string TrimEdgeDigits(string value)
    {
        var start = 0;
        var end = value.Length;
        while (start < end && char.IsDigit(value[start]))
        {
            start++;
        }

        while (end > start && char.IsDigit(value[end - 1]))
        {
            end--;
        }

        return value[start..end];
    }

    private enum ContextHint
    {
        None,
        Cyrillic,
        Latin
    }
}
