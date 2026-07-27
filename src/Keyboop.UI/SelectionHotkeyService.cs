using System.Windows.Interop;

namespace Keyboop.UI;

internal sealed class SelectionHotkeyService : IDisposable
{
    private const int HotkeyId = 0x4b42;
    private readonly HwndSource _source;
    private readonly Func<Task> _onHotkey;
    private bool _disposed;

    internal SelectionHotkeyService(Func<Task> onHotkey)
    {
        _onHotkey = onHotkey;
        var parameters = new HwndSourceParameters("Keyboop.SelectionHotkey")
        {
            Width = 0,
            Height = 0,
            WindowStyle = 0
        };
        _source = new HwndSource(parameters);
        _source.AddHook(WindowProcedure);
        if (!NativeSelectionMethods.RegisterHotKey(
            _source.Handle,
            HotkeyId,
            NativeSelectionMethods.ModControl
                | NativeSelectionMethods.ModAlt
                | NativeSelectionMethods.ModNoRepeat,
            NativeSelectionMethods.VkK))
        {
            _source.Dispose();
            throw new InvalidOperationException(
                "Не удалось зарегистрировать Ctrl+Alt+K. Сочетание уже занято.");
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _ = NativeSelectionMethods.UnregisterHotKey(_source.Handle, HotkeyId);
        _source.RemoveHook(WindowProcedure);
        _source.Dispose();
    }

    private nint WindowProcedure(
        nint window,
        int message,
        nint wParam,
        nint lParam,
        ref bool handled)
    {
        if (message == NativeSelectionMethods.WmHotkey
            && wParam == (nint)HotkeyId)
        {
            handled = true;
            UiDiagnostics.Write("Selection correction hotkey received.");
            _ = _onHotkey();
        }

        return 0;
    }
}
