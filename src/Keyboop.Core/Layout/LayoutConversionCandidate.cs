namespace Keyboop.Core.Layout;

public sealed record LayoutConversionCandidate(
    LayoutConversionDirection Direction,
    string Text);

public enum LayoutConversionDirection
{
    ToRussian,
    ToEnglish
}
