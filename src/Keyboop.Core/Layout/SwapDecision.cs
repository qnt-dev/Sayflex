namespace Keyboop.Core.Layout;

public readonly record struct SwapDecision(
    bool ShouldConvert,
    bool ToCyrillic,
    string Original,
    string Candidate,
    string Reason,
    double? OriginalScore = null,
    double? CandidateScore = null)
{
    public static SwapDecision Keep(
        string original,
        string candidate,
        string reason,
        double? originalScore = null,
        double? candidateScore = null) =>
        new(false, false, original, candidate, reason, originalScore, candidateScore);

    public static SwapDecision Convert(
        bool toCyrillic,
        string original,
        string candidate,
        string reason,
        double? originalScore = null,
        double? candidateScore = null) =>
        new(true, toCyrillic, original, candidate, reason, originalScore, candidateScore);
}

