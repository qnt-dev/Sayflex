namespace Keyboop.Core.Layout;

public sealed record DetectorLexicon(
    IReadOnlySet<string> Ignored,
    IReadOnlySet<string> ForceSwap,
    IReadOnlySet<string> Learned)
{
    public static DetectorLexicon Empty { get; } = new(
        new HashSet<string>(StringComparer.Ordinal),
        new HashSet<string>(StringComparer.Ordinal),
        new HashSet<string>(StringComparer.Ordinal));
}

