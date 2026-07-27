namespace Keyboop.InputHost;

internal sealed class RecoveryHotkeyState
{
    private bool _pending;
    private bool _xDown;

    internal RecoveryHotkeyDecision Handle(
        uint virtualKey,
        bool keyUp,
        PhysicalModifierState modifiers)
    {
        if (virtualKey == NativeMethods.VkX)
        {
            if (!keyUp)
            {
                if (_xDown)
                {
                    return new RecoveryHotkeyDecision(
                        Suppress: _pending,
                        CaptureTarget: false,
                        Trigger: false);
                }

                if (!GlobalHotkeyRules.CanRecoverLastTranscript(modifiers))
                {
                    return default;
                }

                _xDown = true;
                _pending = true;
                return new RecoveryHotkeyDecision(
                    Suppress: true,
                    CaptureTarget: true,
                    Trigger: false);
            }

            if (!_xDown)
            {
                return default;
            }

            _xDown = false;
            return new RecoveryHotkeyDecision(
                Suppress: true,
                CaptureTarget: false,
                Trigger: TryComplete(modifiers));
        }

        if (!_pending
            || !keyUp
            || virtualKey is not (
                NativeMethods.VkShift
                or NativeMethods.VkLshift
                or NativeMethods.VkRshift
                or NativeMethods.VkMenu
                or NativeMethods.VkLmenu
                or NativeMethods.VkRmenu))
        {
            return default;
        }

        return new RecoveryHotkeyDecision(
            Suppress: false,
            CaptureTarget: false,
            Trigger: TryComplete(modifiers));
    }

    private bool TryComplete(PhysicalModifierState modifiers)
    {
        if (_xDown || modifiers.ShiftDown || modifiers.AltDown)
        {
            return false;
        }

        _pending = false;
        return true;
    }
}

internal readonly record struct RecoveryHotkeyDecision(
    bool Suppress,
    bool CaptureTarget,
    bool Trigger);
