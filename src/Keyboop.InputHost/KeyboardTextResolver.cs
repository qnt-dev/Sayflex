namespace Keyboop.InputHost;

internal sealed class KeyboardTextResolver
{
    private const uint PreserveDeadKeyState = 1u << 2;
    private readonly byte[] _keyboardState = new byte[256];

    internal void Track(
        NativeMethods.KbdLlHookStruct input,
        bool keyUp) =>
        UpdateState(input.VirtualKey, keyUp);

    internal unsafe string ResolveText(
        NativeMethods.KbdLlHookStruct input,
        nint keyboardLayout)
    {
        if (IsModifier(input.VirtualKey))
        {
            return string.Empty;
        }

        Span<char> output = stackalloc char[8];
        fixed (byte* state = _keyboardState)
        fixed (char* characters = output)
        {
            var length = NativeMethods.ToUnicodeEx(
                input.VirtualKey,
                input.ScanCode,
                state,
                characters,
                output.Length,
                PreserveDeadKeyState,
                keyboardLayout);
            return length > 0
                ? new string(output[..Math.Min(length, output.Length)])
                : string.Empty;
        }
    }

    internal bool HasShortcutModifier =>
        IsDown(NativeMethods.VkControl)
        || IsDown(NativeMethods.VkLcontrol)
        || IsDown(NativeMethods.VkRcontrol)
        || IsDown(NativeMethods.VkLwin)
        || IsDown(NativeMethods.VkRwin)
        || IsDown(NativeMethods.VkMenu)
        || IsDown(NativeMethods.VkLmenu)
        || IsDown(NativeMethods.VkRmenu);

    internal bool IsControlDown =>
        IsDown(NativeMethods.VkControl)
        || IsDown(NativeMethods.VkLcontrol)
        || IsDown(NativeMethods.VkRcontrol);

    internal bool IsLeftControlDown =>
        IsDown(NativeMethods.VkLcontrol);

    internal bool IsShiftDown =>
        IsDown(NativeMethods.VkShift)
        || IsDown(NativeMethods.VkLshift)
        || IsDown(NativeMethods.VkRshift);

    internal bool IsAltDown =>
        IsDown(NativeMethods.VkMenu)
        || IsDown(NativeMethods.VkLmenu)
        || IsDown(NativeMethods.VkRmenu);

    internal bool IsWindowsDown =>
        IsDown(NativeMethods.VkLwin)
        || IsDown(NativeMethods.VkRwin);

    internal static bool IsModifierKey(uint virtualKey) => IsModifier(virtualKey);

    internal void Reset() => Array.Clear(_keyboardState);

    private void UpdateState(uint virtualKey, bool keyUp)
    {
        if (virtualKey >= _keyboardState.Length)
        {
            return;
        }

        if (virtualKey == NativeMethods.VkCapital && !keyUp)
        {
            _keyboardState[virtualKey] ^= 0x01;
        }

        if (keyUp)
        {
            _keyboardState[virtualKey] &= 0x7f;
        }
        else
        {
            _keyboardState[virtualKey] |= 0x80;
        }

        MirrorModifier(virtualKey, NativeMethods.VkLshift, NativeMethods.VkRshift, NativeMethods.VkShift);
        MirrorModifier(virtualKey, NativeMethods.VkLcontrol, NativeMethods.VkRcontrol, NativeMethods.VkControl);
        MirrorModifier(virtualKey, NativeMethods.VkLmenu, NativeMethods.VkRmenu, NativeMethods.VkMenu);
    }

    private void MirrorModifier(uint virtualKey, uint left, uint right, uint general)
    {
        if (virtualKey != left && virtualKey != right)
        {
            return;
        }

        // The generic slot is a derived aggregate for side-specific events.
        // Including its previous value here would latch the modifier forever
        // after the first key-down/key-up cycle.
        _keyboardState[general] = (byte)(
            (_keyboardState[left] | _keyboardState[right])
            & 0x80);
    }

    private bool IsDown(uint virtualKey) =>
        virtualKey < _keyboardState.Length
        && (_keyboardState[virtualKey] & 0x80) != 0;

    private static bool IsModifier(uint virtualKey) =>
        virtualKey is NativeMethods.VkShift
            or NativeMethods.VkControl
            or NativeMethods.VkMenu
            or NativeMethods.VkLshift
            or NativeMethods.VkRshift
            or NativeMethods.VkLcontrol
            or NativeMethods.VkRcontrol
            or NativeMethods.VkLmenu
            or NativeMethods.VkRmenu
            or NativeMethods.VkLwin
            or NativeMethods.VkRwin;
}
