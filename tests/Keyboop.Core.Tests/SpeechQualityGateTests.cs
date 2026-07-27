using Keyboop.Core.Speech;

namespace Keyboop.Core.Tests;

public sealed class SpeechQualityGateTests
{
    [Fact]
    public void RejectsShortAndQuietAudio()
    {
        Assert.False(SpeechQualityGate.HasAudibleSignal(new float[1_599]));
        Assert.False(SpeechQualityGate.HasAudibleSignal(
            Enumerable.Repeat(0.0005f, 16_000).ToArray()));
    }

    [Fact]
    public void AcceptsSeveralAudibleFrames()
    {
        var samples = new float[16_000];
        for (var index = 0; index < 960; index++)
        {
            samples[index] = index % 2 == 0 ? 0.02f : -0.02f;
        }

        Assert.True(SpeechQualityGate.HasAudibleSignal(samples));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData(". .")]
    [InlineData("—, … !")]
    public void RejectsEmptyOrPunctuationOnlyTranscripts(string? transcript)
    {
        Assert.False(SpeechQualityGate.HasMeaningfulTranscript(transcript));
    }

    [Theory]
    [InlineData("Привет, мир!")]
    [InlineData("Hello.")]
    [InlineData("42")]
    public void AcceptsTranscriptsWithLettersOrDigits(string transcript)
    {
        Assert.True(SpeechQualityGate.HasMeaningfulTranscript(transcript));
    }
}
