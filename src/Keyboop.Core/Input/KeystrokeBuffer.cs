using System.Globalization;

namespace Keyboop.Core.Input;

public sealed class KeystrokeBuffer
{
    private const int GroupMaxCharacters = 200;
    private static readonly TimeSpan GroupMaxIdle = TimeSpan.FromSeconds(8);
    private readonly Func<DateTimeOffset> _clock;
    private readonly List<BufferedWord> _sessionWords = [];
    private DateTimeOffset _lastActivity;

    public KeystrokeBuffer(Func<DateTimeOffset>? clock = null)
    {
        _clock = clock ?? (() => DateTimeOffset.UtcNow);
        _lastActivity = _clock();
    }

    public string CurrentWord { get; private set; } = string.Empty;

    public string LastWord { get; private set; } = string.Empty;

    public string LastTail { get; private set; } = string.Empty;

    public IReadOnlyList<BufferedWord> SessionWords => _sessionWords;

    public void Append(string text)
    {
        ArgumentNullException.ThrowIfNull(text);
        CurrentWord += text;
        _lastActivity = _clock();
    }

    public void Backspace()
    {
        _lastActivity = _clock();
        if (CurrentWord.Length == 0)
        {
            Clear();
            return;
        }

        CurrentWord = RemoveLastTextElement(CurrentWord);
        if (CurrentWord.Length == 0)
        {
            LastWord = string.Empty;
            LastTail = string.Empty;
            _sessionWords.Clear();
        }
    }

    public void Boundary(string whitespace)
    {
        ArgumentNullException.ThrowIfNull(whitespace);
        _lastActivity = _clock();
        if (CurrentWord.Length > 0)
        {
            _sessionWords.Add(new BufferedWord(CurrentWord, whitespace));
            LastWord = CurrentWord;
            LastTail = whitespace;
            CurrentWord = string.Empty;
            return;
        }

        if (LastWord.Length == 0)
        {
            return;
        }

        LastTail += whitespace;
        if (_sessionWords.Count > 0)
        {
            var last = _sessionWords[^1];
            _sessionWords[^1] = last with { Tail = last.Tail + whitespace };
        }
    }

    public void Clear()
    {
        CurrentWord = string.Empty;
        LastWord = string.Empty;
        LastTail = string.Empty;
        _sessionWords.Clear();
        _lastActivity = _clock();
    }

    public void InvalidateGroupHistory() => _sessionWords.Clear();

    public ConversionWord? WordForConversion()
    {
        if (CurrentWord.Length > 0)
        {
            return new ConversionWord(
                CurrentWord,
                CountTextElements(CurrentWord),
                string.Empty);
        }

        return LastWord.Length > 0
            ? new ConversionWord(
                LastWord,
                CountTextElements(LastWord) + CountTextElements(LastTail),
                LastTail)
            : null;
    }

    public ConversionGroup? GroupForConversion()
    {
        if (_clock() - _lastActivity > GroupMaxIdle)
        {
            return null;
        }

        var words = new List<BufferedWord>(_sessionWords);
        if (CurrentWord.Length > 0)
        {
            words.Add(new BufferedWord(CurrentWord, string.Empty));
        }

        if (words.Count < 2)
        {
            return null;
        }

        var total = 0;
        foreach (var item in words)
        {
            if (CountTextElements(item.Word) != item.Word.Length
                || item.Tail.Contains('\n', StringComparison.Ordinal)
                || item.Tail.Contains('\t', StringComparison.Ordinal))
            {
                return null;
            }

            total += CountTextElements(item.Word) + CountTextElements(item.Tail);
        }

        return total is > 0 and <= GroupMaxCharacters
            ? new ConversionGroup(words, total)
            : null;
    }

    public void ApplyConversion(string converted)
    {
        ArgumentNullException.ThrowIfNull(converted);
        if (CurrentWord.Length > 0)
        {
            CurrentWord = converted;
            return;
        }

        if (LastWord.Length == 0)
        {
            return;
        }

        LastWord = converted;
        if (_sessionWords.Count > 0)
        {
            _sessionWords[^1] = _sessionWords[^1] with { Word = converted };
        }
    }

    public string? ContextWord(bool forCurrent)
    {
        if (_sessionWords.Count == 0)
        {
            return null;
        }

        if (forCurrent)
        {
            return _sessionWords[^1].Word;
        }

        return _sessionWords.Count >= 2 ? _sessionWords[^2].Word : null;
    }

    private static int CountTextElements(string value) =>
        new StringInfo(value).LengthInTextElements;

    private static string RemoveLastTextElement(string value)
    {
        var indexes = StringInfo.ParseCombiningCharacters(value);
        return indexes.Length <= 1 ? string.Empty : value[..indexes[^1]];
    }
}

public sealed record BufferedWord(string Word, string Tail);

public sealed record ConversionWord(string Word, int DeleteCount, string Tail);

public sealed record ConversionGroup(IReadOnlyList<BufferedWord> Words, int DeleteCount);

