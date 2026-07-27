using System.ComponentModel;
using System.Runtime.InteropServices;
using Keyboop.Core.Input;

namespace Keyboop.InputHost;

internal sealed class InputInjector
{
    private const int ExpectedInputSizeX64 = 40;
    private readonly nuint _syntheticMarker;
    private readonly nuint _replayMarker;
    private readonly object _sendGate = new();

    internal InputInjector(nuint syntheticMarker, nuint replayMarker)
    {
        if (IntPtr.Size != 8 || Marshal.SizeOf<NativeMethods.Input>() != ExpectedInputSizeX64)
        {
            throw new PlatformNotSupportedException(
                $"Unexpected Win32 INPUT ABI size: {Marshal.SizeOf<NativeMethods.Input>()}.");
        }

        _syntheticMarker = syntheticMarker;
        _replayMarker = replayMarker;
    }

    internal void Replace(
        int deleteCount,
        string replacement,
        PhysicalKeyEvent boundary)
    {
        var inputs = new List<NativeMethods.Input>(
            (deleteCount * 2) + (replacement.Length * 2) + 2);
        for (var index = 0; index < deleteCount; index++)
        {
            inputs.Add(VirtualKey(NativeMethods.VkBack, false));
            inputs.Add(VirtualKey(NativeMethods.VkBack, true));
        }

        foreach (var character in replacement)
        {
            inputs.Add(Unicode(character, false));
            inputs.Add(Unicode(character, true));
        }

        inputs.Add(VirtualKey(boundary.VirtualKey, false));
        inputs.Add(VirtualKey(boundary.VirtualKey, true));
        Send(inputs);
    }

    internal void ReplayBoundaryAndEvents(
        PhysicalKeyEvent boundary,
        IReadOnlyList<PhysicalKeyEvent> events)
    {
        var inputs = new List<NativeMethods.Input>(events.Count + 2)
        {
            VirtualKey(boundary.VirtualKey, false),
            VirtualKey(boundary.VirtualKey, true)
        };
        inputs.AddRange(events.Select(input => Physical(input, _syntheticMarker)));
        Send(inputs);
    }

    internal void Replay(IReadOnlyList<PhysicalKeyEvent> events)
    {
        if (events.Count == 0)
        {
            return;
        }

        Send(events.Select(input => Physical(input, _replayMarker)).ToArray());
    }

    internal void InsertUnicode(string text, bool addTrailingSpace)
    {
        if (string.IsNullOrEmpty(text) && !addTrailingSpace)
        {
            return;
        }

        var inputs = new List<NativeMethods.Input>((text.Length * 2) + 2);
        foreach (var character in text)
        {
            inputs.Add(Unicode(character, false));
            inputs.Add(Unicode(character, true));
        }

        if (addTrailingSpace)
        {
            inputs.Add(Unicode(' ', false));
            inputs.Add(Unicode(' ', true));
        }

        Send(inputs);
    }

    internal void Paste()
    {
        Send(
        [
            VirtualKey(NativeMethods.VkControl, false),
            VirtualKey(NativeMethods.VkV, false),
            VirtualKey(NativeMethods.VkV, true),
            VirtualKey(NativeMethods.VkControl, true)
        ]);
    }

    internal void ReplaceCurrent(int deleteCount, string replacement)
    {
        var inputs = new List<NativeMethods.Input>(
            (deleteCount * 2) + (replacement.Length * 2));
        for (var index = 0; index < deleteCount; index++)
        {
            inputs.Add(VirtualKey(NativeMethods.VkBack, false));
            inputs.Add(VirtualKey(NativeMethods.VkBack, true));
        }

        foreach (var character in replacement)
        {
            inputs.Add(Unicode(character, false));
            inputs.Add(Unicode(character, true));
        }

        Send(inputs);
    }

    private NativeMethods.Input VirtualKey(uint virtualKey, bool keyUp) =>
        new()
        {
            Type = NativeMethods.InputKeyboard,
            Union = new NativeMethods.InputUnion
            {
                Keyboard = new NativeMethods.KeyboardInput
                {
                    VirtualKey = (ushort)virtualKey,
                    Flags = keyUp ? NativeMethods.KeyeventfKeyUp : 0,
                    ExtraInfo = _syntheticMarker
                }
            }
        };

    private NativeMethods.Input Unicode(char character, bool keyUp) =>
        new()
        {
            Type = NativeMethods.InputKeyboard,
            Union = new NativeMethods.InputUnion
            {
                Keyboard = new NativeMethods.KeyboardInput
                {
                    ScanCode = character,
                    Flags = NativeMethods.KeyeventfUnicode
                        | (keyUp ? NativeMethods.KeyeventfKeyUp : 0),
                    ExtraInfo = _syntheticMarker
                }
            }
        };

    private static NativeMethods.Input Physical(
        PhysicalKeyEvent input,
        nuint marker)
    {
        var flags = NativeMethods.KeyeventfScancode;
        if (input.KeyUp)
        {
            flags |= NativeMethods.KeyeventfKeyUp;
        }

        if ((input.Flags & NativeMethods.LlkhfExtended) != 0)
        {
            flags |= NativeMethods.KeyeventfExtendedKey;
        }

        return new NativeMethods.Input
        {
            Type = NativeMethods.InputKeyboard,
            Union = new NativeMethods.InputUnion
            {
                Keyboard = new NativeMethods.KeyboardInput
                {
                    ScanCode = (ushort)input.ScanCode,
                    Flags = flags,
                    ExtraInfo = marker
                }
            }
        };
    }

    private unsafe void Send(IReadOnlyList<NativeMethods.Input> inputs)
    {
        if (inputs.Count == 0)
        {
            return;
        }

        lock (_sendGate)
        {
            var buffer = inputs as NativeMethods.Input[] ?? inputs.ToArray();
            fixed (NativeMethods.Input* pointer = buffer)
            {
                var sent = NativeMethods.SendInput(
                    (uint)buffer.Length,
                    pointer,
                    sizeof(NativeMethods.Input));
                if (sent != buffer.Length)
                {
                    var error = Marshal.GetLastPInvokeError();
                    throw new Win32Exception(
                        error,
                        $"SendInput accepted {sent} of {buffer.Length} events (Win32 {error}).");
                }
            }
        }
    }
}
