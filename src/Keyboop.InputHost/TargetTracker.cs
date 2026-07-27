using Keyboop.Core.Input;
using System.Runtime.InteropServices;

namespace Keyboop.InputHost;

internal static class TargetTracker
{
    private const uint GaRoot = 2;

    internal static TargetSnapshot Capture()
    {
        var window = NativeMethods.GetForegroundWindow();
        if (window == 0)
        {
            return default;
        }

        var threadId = NativeMethods.GetWindowThreadProcessId(window, out var processId);
        var information = new NativeMethods.GuiThreadInfo
        {
            Size = (uint)Marshal.SizeOf<NativeMethods.GuiThreadInfo>()
        };
        if (NativeMethods.GetGuiThreadInfo(threadId, ref information)
            && information.Focus != 0)
        {
            var focusThreadId = NativeMethods.GetWindowThreadProcessId(
                information.Focus,
                out var focusProcessId);
            if (focusThreadId != 0)
            {
                window = information.Focus;
                threadId = focusThreadId;
                processId = focusProcessId;
            }
        }

        var layout = NativeMethods.GetKeyboardLayout(threadId);
        return new TargetSnapshot(
            window,
            threadId,
            layout,
            processId);
    }

    internal static TargetSnapshot CaptureTopLevel()
    {
        var window = NativeMethods.GetForegroundWindow();
        if (window == 0)
        {
            return default;
        }

        var root = NativeMethods.GetAncestor(window, GaRoot);
        if (root != 0)
        {
            window = root;
        }

        var threadId = NativeMethods.GetWindowThreadProcessId(window, out var processId);
        return new TargetSnapshot(
            window,
            threadId,
            NativeMethods.GetKeyboardLayout(threadId),
            processId);
    }

    internal static bool IsForeground(TargetSnapshot target)
    {
        if (target.WindowHandle == 0 || target.ProcessId == 0)
        {
            return false;
        }

        var current = CaptureTopLevel();
        var expectedRoot = NativeMethods.GetAncestor(
            (nint)target.WindowHandle,
            GaRoot);
        if (expectedRoot == 0)
        {
            expectedRoot = (nint)target.WindowHandle;
        }

        return current.WindowHandle == expectedRoot
            && current.ProcessId == target.ProcessId;
    }
}
