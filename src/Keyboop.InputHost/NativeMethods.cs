using System.Runtime.InteropServices;

namespace Keyboop.InputHost;

internal static partial class NativeMethods
{
    internal const int WhKeyboardLl = 13;
    internal const int HcAction = 0;
    internal const uint WmKeyDown = 0x0100;
    internal const uint WmKeyUp = 0x0101;
    internal const uint WmSysKeyDown = 0x0104;
    internal const uint WmSysKeyUp = 0x0105;
    internal const uint WmQuit = 0x0012;
    internal const uint WmAppHostCommand = 0x8001;

    internal const uint LlkhfExtended = 0x01;
    internal const uint LlkhfInjected = 0x10;
    internal const uint LlkhfAltDown = 0x20;
    internal const uint LlkhfUp = 0x80;

    internal const uint InputKeyboard = 1;
    internal const uint KeyeventfExtendedKey = 0x0001;
    internal const uint KeyeventfKeyUp = 0x0002;
    internal const uint KeyeventfUnicode = 0x0004;
    internal const uint KeyeventfScancode = 0x0008;

    internal const uint VkBack = 0x08;
    internal const uint VkTab = 0x09;
    internal const uint VkReturn = 0x0d;
    internal const uint VkShift = 0x10;
    internal const uint VkControl = 0x11;
    internal const uint VkMenu = 0x12;
    internal const uint VkCapital = 0x14;
    internal const uint VkEscape = 0x1b;
    internal const uint VkSpace = 0x20;
    internal const uint VkD = 0x44;
    internal const uint VkP = 0x50;
    internal const uint VkV = 0x56;
    internal const uint VkX = 0x58;
    internal const uint VkPrior = 0x21;
    internal const uint VkNext = 0x22;
    internal const uint VkEnd = 0x23;
    internal const uint VkHome = 0x24;
    internal const uint VkLeft = 0x25;
    internal const uint VkUp = 0x26;
    internal const uint VkRight = 0x27;
    internal const uint VkDown = 0x28;
    internal const uint VkLwin = 0x5b;
    internal const uint VkRwin = 0x5c;
    internal const uint VkDelete = 0x2e;
    internal const uint VkLshift = 0xa0;
    internal const uint VkRshift = 0xa1;
    internal const uint VkLcontrol = 0xa2;
    internal const uint VkRcontrol = 0xa3;
    internal const uint VkLmenu = 0xa4;
    internal const uint VkRmenu = 0xa5;

    [LibraryImport("user32.dll", EntryPoint = "SetWindowsHookExW", SetLastError = true)]
    internal static partial nint SetWindowsHookEx(
        int idHook,
        LowLevelKeyboardProc callback,
        nint module,
        uint threadId);

    [LibraryImport("user32.dll", EntryPoint = "UnhookWindowsHookEx", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool UnhookWindowsHookEx(nint hook);

    [LibraryImport("user32.dll", EntryPoint = "CallNextHookEx")]
    internal static partial nint CallNextHookEx(
        nint hook,
        int code,
        nuint message,
        nint data);

    [LibraryImport("user32.dll", EntryPoint = "GetMessageW", SetLastError = true)]
    internal static partial int GetMessage(
        out Message message,
        nint window,
        uint minimum,
        uint maximum);

    [LibraryImport("user32.dll", EntryPoint = "TranslateMessage")]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool TranslateMessage(in Message message);

    [LibraryImport("user32.dll", EntryPoint = "DispatchMessageW")]
    internal static partial nint DispatchMessage(in Message message);

    [LibraryImport("user32.dll", EntryPoint = "PostThreadMessageW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool PostThreadMessage(
        uint threadId,
        uint message,
        nuint wParam,
        nint lParam);

    [LibraryImport("kernel32.dll", EntryPoint = "GetCurrentThreadId")]
    internal static partial uint GetCurrentThreadId();

    [LibraryImport("kernel32.dll", EntryPoint = "GetModuleHandleW", StringMarshalling = StringMarshalling.Utf16)]
    internal static partial nint GetModuleHandle(string? moduleName);

    [LibraryImport("user32.dll", EntryPoint = "GetForegroundWindow")]
    internal static partial nint GetForegroundWindow();

    [LibraryImport("user32.dll", EntryPoint = "GetWindowThreadProcessId")]
    internal static partial uint GetWindowThreadProcessId(
        nint window,
        out uint processId);

    [LibraryImport("user32.dll", EntryPoint = "GetAncestor")]
    internal static partial nint GetAncestor(nint window, uint flags);

    [LibraryImport("user32.dll", EntryPoint = "GetGUIThreadInfo", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool GetGuiThreadInfo(
        uint threadId,
        ref GuiThreadInfo information);

    [LibraryImport("user32.dll", EntryPoint = "GetKeyboardLayout")]
    internal static partial nint GetKeyboardLayout(uint threadId);

    [LibraryImport("user32.dll", EntryPoint = "ToUnicodeEx")]
    internal static unsafe partial int ToUnicodeEx(
        uint virtualKey,
        uint scanCode,
        byte* keyboardState,
        char* buffer,
        int bufferSize,
        uint flags,
        nint keyboardLayout);

    [LibraryImport("user32.dll", EntryPoint = "SendInput", SetLastError = true)]
    internal static unsafe partial uint SendInput(
        uint count,
        Input* inputs,
        int size);

    [LibraryImport("user32.dll", EntryPoint = "GetAsyncKeyState")]
    internal static partial short GetAsyncKeyState(int virtualKey);

    [LibraryImport("user32.dll", EntryPoint = "GetClipboardSequenceNumber")]
    internal static partial uint GetClipboardSequenceNumber();

    internal delegate nint LowLevelKeyboardProc(
        int code,
        nuint message,
        nint data);

    [StructLayout(LayoutKind.Sequential)]
    internal struct KbdLlHookStruct
    {
        internal uint VirtualKey;
        internal uint ScanCode;
        internal uint Flags;
        internal uint Time;
        internal nuint ExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Point
    {
        internal int X;
        internal int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Rectangle
    {
        internal int Left;
        internal int Top;
        internal int Right;
        internal int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct GuiThreadInfo
    {
        internal uint Size;
        internal uint Flags;
        internal nint Active;
        internal nint Focus;
        internal nint Capture;
        internal nint MenuOwner;
        internal nint MoveSize;
        internal nint Caret;
        internal Rectangle CaretRectangle;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Message
    {
        internal nint Window;
        internal uint Value;
        internal nuint WParam;
        internal nint LParam;
        internal uint Time;
        internal Point Point;
        internal uint Private;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Input
    {
        internal uint Type;
        internal InputUnion Union;
    }

    // INPUT contains a union whose largest native member is MOUSEINPUT (32 bytes
    // on x64), even when this application only sends KEYBDINPUT. Without the
    // explicit union size, Marshal/sizeof produces a 32-byte INPUT instead of the
    // required 40 bytes and SendInput fails with ERROR_INVALID_PARAMETER.
    [StructLayout(LayoutKind.Explicit, Size = 32)]
    internal struct InputUnion
    {
        [FieldOffset(0)]
        internal KeyboardInput Keyboard;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct KeyboardInput
    {
        internal ushort VirtualKey;
        internal ushort ScanCode;
        internal uint Flags;
        internal uint Time;
        internal nuint ExtraInfo;
    }
}
