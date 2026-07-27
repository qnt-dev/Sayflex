namespace Keyboop.Protocol;

public sealed record AppSettings
{
    public bool AutoCorrectionEnabled { get; init; }

    public bool CorrectOnSpace { get; init; } = true;

    public bool CorrectOnEnter { get; init; } = true;

    public bool CorrectOnTab { get; init; } = true;

    public bool DictationEnabled { get; init; } = true;

    public bool StartWithWindows { get; init; }

    public bool KeepDictationHistory { get; init; }

    public string SpeechModel { get; init; } = "large-v3-turbo";

    public string SpeechLanguage { get; init; } = "auto";

    public string PreferredSpeechBackend { get; init; } = "cuda";

    public string ManualCorrectionHotkey { get; init; } = "Ctrl+Alt+K";

    public string PushToTalkHotkey { get; init; } = "Ctrl+Win";

    public string PasteLastTranscriptHotkey { get; init; } = "Shift+Alt+X";

    public string PauseHotkey { get; init; } = "Ctrl+Alt+P";

    public IReadOnlyList<string> DisabledApplications { get; init; } = [];
}
