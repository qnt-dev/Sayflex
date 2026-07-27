namespace Keyboop.Core.Speech;

public static class SpeechQualityGate
{
    private const int MinimumSamples = 1_600;
    private const int AnalysisFrameSamples = 320;
    private const double AudibleFrameRms = 0.0035;

    public static bool HasAudibleSignal(ReadOnlySpan<float> samples)
    {
        if (samples.Length < MinimumSamples)
        {
            return false;
        }

        var frameCount = samples.Length / AnalysisFrameSamples;
        var requiredAudibleFrames = Math.Min(3, frameCount);
        var audibleFrames = 0;
        for (var frame = 0; frame < frameCount; frame++)
        {
            var frameSamples = samples.Slice(
                frame * AnalysisFrameSamples,
                AnalysisFrameSamples);
            var sumOfSquares = 0d;
            foreach (var sample in frameSamples)
            {
                var bounded = Math.Clamp(sample, -1f, 1f);
                sumOfSquares += bounded * bounded;
            }

            var rms = Math.Sqrt(sumOfSquares / frameSamples.Length);
            if (rms < AudibleFrameRms)
            {
                continue;
            }

            audibleFrames++;
            if (audibleFrames >= requiredAudibleFrames)
            {
                return true;
            }
        }

        return false;
    }

    public static bool HasMeaningfulTranscript(string? transcript) =>
        !string.IsNullOrWhiteSpace(transcript)
        && transcript.Any(char.IsLetterOrDigit);
}
