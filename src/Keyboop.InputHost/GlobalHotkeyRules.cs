namespace Keyboop.InputHost;

internal readonly record struct PhysicalModifierState(
    bool ControlDown,
    bool ShiftDown,
    bool AltDown,
    bool WindowsDown)
{
    internal static PhysicalModifierState Capture() =>
        new(
            IsDown(NativeMethods.VkControl)
                || IsDown(NativeMethods.VkLcontrol)
                || IsDown(NativeMethods.VkRcontrol),
            IsDown(NativeMethods.VkShift)
                || IsDown(NativeMethods.VkLshift)
                || IsDown(NativeMethods.VkRshift),
            IsDown(NativeMethods.VkMenu)
                || IsDown(NativeMethods.VkLmenu)
                || IsDown(NativeMethods.VkRmenu),
            IsDown(NativeMethods.VkLwin)
                || IsDown(NativeMethods.VkRwin));

    internal PhysicalModifierState ApplyEvent(uint virtualKey, bool keyUp) =>
        virtualKey switch
        {
            NativeMethods.VkControl
                or NativeMethods.VkLcontrol
                or NativeMethods.VkRcontrol =>
                this with { ControlDown = !keyUp },
            NativeMethods.VkLwin or NativeMethods.VkRwin =>
                this with { WindowsDown = !keyUp },
            _ => this
        };

    internal static bool IsPhysicalControlDown() =>
        IsDown(NativeMethods.VkControl)
        || IsDown(NativeMethods.VkLcontrol)
        || IsDown(NativeMethods.VkRcontrol);

    private static bool IsDown(uint virtualKey) =>
        (NativeMethods.GetAsyncKeyState((int)virtualKey) & 0x8000) != 0;
}

internal static class GlobalHotkeyRules
{
    internal static bool IsWindowsKey(uint virtualKey) =>
        virtualKey is NativeMethods.VkLwin or NativeMethods.VkRwin;

    internal static bool IsControlKey(uint virtualKey) =>
        virtualKey is NativeMethods.VkControl
            or NativeMethods.VkLcontrol
            or NativeMethods.VkRcontrol;

    internal static bool CanBeginDictation(PhysicalModifierState modifiers) =>
        modifiers.ControlDown
        && modifiers.WindowsDown
        && !modifiers.ShiftDown
        && !modifiers.AltDown;

    internal static bool CanRecoverLastTranscript(PhysicalModifierState modifiers) =>
        !modifiers.ControlDown
        && modifiers.ShiftDown
        && modifiers.AltDown;

    internal static bool CanTogglePause(PhysicalModifierState modifiers) =>
        modifiers.ControlDown
        && !modifiers.ShiftDown
        && modifiers.AltDown;
}
