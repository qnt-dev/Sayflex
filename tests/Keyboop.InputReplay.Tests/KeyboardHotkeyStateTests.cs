using Keyboop.InputHost;

namespace Keyboop.InputReplay.Tests;

public sealed class KeyboardHotkeyStateTests
{
    [Theory]
    [InlineData(0xa0)]
    [InlineData(0xa1)]
    [InlineData(0xa2)]
    [InlineData(0xa3)]
    [InlineData(0xa4)]
    [InlineData(0xa5)]
    public void SideSpecificModifierDoesNotRemainLatchedAfterRelease(uint virtualKey)
    {
        var resolver = new KeyboardTextResolver();
        var input = new NativeMethods.KbdLlHookStruct
        {
            VirtualKey = virtualKey
        };

        resolver.Track(input, keyUp: false);
        Assert.True(IsTracked(resolver, virtualKey));

        resolver.Track(input, keyUp: true);
        Assert.False(IsTracked(resolver, virtualKey));
    }

    [Fact]
    public void SoloWindowsKeyCannotBeginDictation()
    {
        var modifiers = new PhysicalModifierState(
            ControlDown: false,
            ShiftDown: false,
            AltDown: false,
            WindowsDown: true);

        Assert.False(GlobalHotkeyRules.CanBeginDictation(modifiers));
    }

    [Fact]
    public void DictationRequiresExactPhysicalControlWindowsChord()
    {
        Assert.True(
            GlobalHotkeyRules.CanBeginDictation(
                new PhysicalModifierState(
                    ControlDown: true,
                    ShiftDown: false,
                    AltDown: false,
                    WindowsDown: true)));
        Assert.False(
            GlobalHotkeyRules.CanBeginDictation(
                new PhysicalModifierState(
                    ControlDown: true,
                    ShiftDown: true,
                    AltDown: false,
                    WindowsDown: true)));
        Assert.False(
            GlobalHotkeyRules.CanBeginDictation(
                new PhysicalModifierState(
                    ControlDown: true,
                    ShiftDown: false,
                    AltDown: true,
                    WindowsDown: true)));
        Assert.False(
            GlobalHotkeyRules.CanBeginDictation(
                new PhysicalModifierState(
                    ControlDown: true,
                    ShiftDown: false,
                    AltDown: false,
                    WindowsDown: false)));
    }

    [Theory]
    [InlineData(NativeMethods.VkLcontrol)]
    [InlineData(NativeMethods.VkRcontrol)]
    public void ControlPressedSecondBeginsDictation(uint controlKey)
    {
        var beforeEvent = new PhysicalModifierState(
            ControlDown: false,
            ShiftDown: false,
            AltDown: false,
            WindowsDown: true);

        var duringEvent = beforeEvent.ApplyEvent(controlKey, keyUp: false);

        Assert.True(GlobalHotkeyRules.CanBeginDictation(duringEvent));
    }

    [Theory]
    [InlineData(NativeMethods.VkLwin)]
    [InlineData(NativeMethods.VkRwin)]
    public void WindowsPressedSecondBeginsDictation(uint windowsKey)
    {
        var beforeEvent = new PhysicalModifierState(
            ControlDown: true,
            ShiftDown: false,
            AltDown: false,
            WindowsDown: false);

        var duringEvent = beforeEvent.ApplyEvent(windowsKey, keyUp: false);

        Assert.True(GlobalHotkeyRules.CanBeginDictation(duringEvent));
    }

    [Fact]
    public void OrdinaryXCannotRecoverLastTranscript()
    {
        Assert.False(
            GlobalHotkeyRules.CanRecoverLastTranscript(
                new PhysicalModifierState(
                    ControlDown: false,
                    ShiftDown: false,
                    AltDown: false,
                    WindowsDown: false)));
        Assert.True(
            GlobalHotkeyRules.CanRecoverLastTranscript(
                new PhysicalModifierState(
                    ControlDown: false,
                    ShiftDown: true,
                    AltDown: true,
                    WindowsDown: false)));
    }

    [Fact]
    public void RecoveryWaitsUntilShiftAltAndXAreReleased()
    {
        var state = new RecoveryHotkeyState();
        var held = new PhysicalModifierState(
            ControlDown: false,
            ShiftDown: true,
            AltDown: true,
            WindowsDown: false);

        var pressed = state.Handle(NativeMethods.VkX, keyUp: false, held);
        Assert.True(pressed.Suppress);
        Assert.True(pressed.CaptureTarget);
        Assert.False(pressed.Trigger);

        var xReleased = state.Handle(NativeMethods.VkX, keyUp: true, held);
        Assert.True(xReleased.Suppress);
        Assert.False(xReleased.Trigger);

        var shiftReleased = state.Handle(
            NativeMethods.VkLshift,
            keyUp: true,
            held with { ShiftDown = false });
        Assert.False(shiftReleased.Trigger);

        var altReleased = state.Handle(
            NativeMethods.VkLmenu,
            keyUp: true,
            held with { ShiftDown = false, AltDown = false });
        Assert.True(altReleased.Trigger);
    }

    [Fact]
    public void RecoveryTriggersWhenXIsReleasedLast()
    {
        var state = new RecoveryHotkeyState();
        var held = new PhysicalModifierState(
            ControlDown: false,
            ShiftDown: true,
            AltDown: true,
            WindowsDown: false);

        _ = state.Handle(NativeMethods.VkX, keyUp: false, held);
        _ = state.Handle(
            NativeMethods.VkLshift,
            keyUp: true,
            held with { ShiftDown = false });
        _ = state.Handle(
            NativeMethods.VkLmenu,
            keyUp: true,
            held with { ShiftDown = false, AltDown = false });

        var xReleased = state.Handle(
            NativeMethods.VkX,
            keyUp: true,
            default);

        Assert.True(xReleased.Suppress);
        Assert.True(xReleased.Trigger);
    }

    private static bool IsTracked(
        KeyboardTextResolver resolver,
        uint virtualKey) =>
        virtualKey switch
        {
            NativeMethods.VkLcontrol or NativeMethods.VkRcontrol =>
                resolver.IsControlDown,
            NativeMethods.VkLshift or NativeMethods.VkRshift =>
                resolver.IsShiftDown,
            NativeMethods.VkLmenu or NativeMethods.VkRmenu =>
                resolver.IsAltDown,
            _ => throw new ArgumentOutOfRangeException(nameof(virtualKey))
        };
}
