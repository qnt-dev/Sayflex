using System.ComponentModel;
using System.Runtime.InteropServices;

namespace Keyboop.UI;

internal static partial class NativeSelectionMethods
{
    internal const int WmHotkey = 0x0312;
    internal const uint ModAlt = 0x0001;
    internal const uint ModControl = 0x0002;
    internal const uint ModNoRepeat = 0x4000;
    internal const uint VkK = 0x4b;

    private const int GwlExstyle = -20;
    private const long WsExTransparent = 0x00000020L;
    private const long WsExToolwindow = 0x00000080L;
    private const long WsExNoactivate = 0x08000000L;

    private const uint InputKeyboard = 1;
    private const uint KeyeventfKeyUp = 0x0002;
    private const ushort VkControl = 0x11;
    private const int VkMenu = 0x12;
    private const ushort VkC = 0x43;
    private const ushort VkV = 0x56;

    [LibraryImport("user32.dll", EntryPoint = "RegisterHotKey", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool RegisterHotKey(
        nint window,
        int id,
        uint modifiers,
        uint virtualKey);

    [LibraryImport("user32.dll", EntryPoint = "UnregisterHotKey", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool UnregisterHotKey(nint window, int id);

    [LibraryImport("user32.dll", EntryPoint = "GetForegroundWindow")]
    internal static partial nint GetForegroundWindow();

    [LibraryImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static partial nint GetWindowLongPointer(nint window, int index);

    [LibraryImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static partial nint SetWindowLongPointer(
        nint window,
        int index,
        nint value);

    [LibraryImport("user32.dll", EntryPoint = "SetForegroundWindow")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool SetForegroundWindow(nint window);

    [LibraryImport("user32.dll", EntryPoint = "IsWindow")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool IsWindow(nint window);

    [LibraryImport("user32.dll", EntryPoint = "GetCursorPos")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool GetCursorPosition(out NativePoint point);

    [LibraryImport("user32.dll", EntryPoint = "GetClipboardSequenceNumber")]
    internal static partial uint GetClipboardSequenceNumber();

    [LibraryImport("user32.dll", EntryPoint = "GetAsyncKeyState")]
    private static partial short GetAsyncKeyState(int virtualKey);

    [LibraryImport("user32.dll", EntryPoint = "SendInput", SetLastError = true)]
    private static unsafe partial uint SendInput(
        uint count,
        NativeInput* inputs,
        int size);

    internal static unsafe void SendCopy() => SendControlChord(VkC);

    internal static unsafe void SendPaste() => SendControlChord(VkV);

    internal static void MakeWindowNonActivating(nint window)
    {
        var styles = GetWindowLongPointer(window, GwlExstyle).ToInt64();
        var updated = styles | WsExNoactivate | WsExToolwindow | WsExTransparent;
        _ = SetWindowLongPointer(window, GwlExstyle, (nint)updated);
    }

    internal static async Task WaitForSelectionHotkeyReleaseAsync(
        CancellationToken cancellationToken = default)
    {
        for (var attempt = 0; attempt < 50; attempt++)
        {
            if (!IsKeyDown(VkControl)
                && !IsKeyDown(VkMenu)
                && !IsKeyDown((int)VkK))
            {
                return;
            }

            await Task.Delay(10, cancellationToken);
        }

        throw new InvalidOperationException(
            "Клавиши Ctrl+Alt+K остались нажаты. Отпустите их и повторите.");
    }

    private static unsafe void SendControlChord(ushort key)
    {
        var inputs = new[]
        {
            Keyboard(VkControl, false),
            Keyboard(key, false),
            Keyboard(key, true),
            Keyboard(VkControl, true)
        };
        fixed (NativeInput* pointer = inputs)
        {
            var sent = SendInput((uint)inputs.Length, pointer, sizeof(NativeInput));
            if (sent != inputs.Length)
            {
                var error = Marshal.GetLastPInvokeError();
                throw new Win32Exception(
                    error,
                    $"SendInput accepted {sent} of {inputs.Length} shortcut events.");
            }
        }
    }

    private static NativeInput Keyboard(ushort key, bool keyUp) =>
        new()
        {
            Type = InputKeyboard,
            Union = new NativeInputUnion
            {
                Keyboard = new NativeKeyboardInput
                {
                    VirtualKey = key,
                    Flags = keyUp ? KeyeventfKeyUp : 0
                }
            }
        };

    private static bool IsKeyDown(int virtualKey) =>
        (GetAsyncKeyState(virtualKey) & 0x8000) != 0;

    [StructLayout(LayoutKind.Sequential)]
    internal struct NativePoint
    {
        internal int X;
        internal int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeInput
    {
        internal uint Type;
        internal NativeInputUnion Union;
    }

    [StructLayout(LayoutKind.Explicit, Size = 32)]
    private struct NativeInputUnion
    {
        [FieldOffset(0)]
        internal NativeKeyboardInput Keyboard;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeKeyboardInput
    {
        internal ushort VirtualKey;
        internal ushort ScanCode;
        internal uint Flags;
        internal uint Time;
        internal nuint ExtraInfo;
    }
}
