using System.Runtime.InteropServices;
using WinForms = System.Windows.Forms;

namespace Keyboop.UI;

internal sealed class ClipboardSelectionTransaction
{
    private readonly ClipboardSnapshot _originalClipboard;
    private uint _ownedSequence;

    private ClipboardSelectionTransaction(
        nint targetWindow,
        string selectedText,
        ClipboardSnapshot originalClipboard,
        uint ownedSequence)
    {
        TargetWindow = targetWindow;
        SelectedText = selectedText;
        _originalClipboard = originalClipboard;
        _ownedSequence = ownedSequence;
    }

    internal nint TargetWindow { get; }

    internal string SelectedText { get; }

    internal static async Task<ClipboardSelectionTransaction?> CaptureAsync(
        nint targetWindow,
        CancellationToken cancellationToken = default)
    {
        if (targetWindow == 0 || !NativeSelectionMethods.IsWindow(targetWindow))
        {
            return null;
        }

        var original = ClipboardSnapshot.Capture();
        var before = NativeSelectionMethods.GetClipboardSequenceNumber();
        NativeSelectionMethods.SendCopy();

        uint sequence = before;
        for (var attempt = 0; attempt < 24; attempt++)
        {
            await Task.Delay(25, cancellationToken);
            sequence = NativeSelectionMethods.GetClipboardSequenceNumber();
            if (sequence != before)
            {
                break;
            }
        }

        if (sequence == before)
        {
            return null;
        }

        var selected = TryGetUnicodeText();
        if (string.IsNullOrEmpty(selected))
        {
            original.Restore();
            return null;
        }

        return new ClipboardSelectionTransaction(
            targetWindow,
            selected,
            original,
            sequence);
    }

    internal async Task ApplyAsync(
        string replacement,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(replacement);
        if (!NativeSelectionMethods.IsWindow(TargetWindow))
        {
            RestoreIfOwned();
            return;
        }

        var restoreData = NativeSelectionMethods.GetClipboardSequenceNumber() == _ownedSequence
            ? _originalClipboard
            : ClipboardSnapshot.Capture();
        SetReplacementClipboard(replacement);
        _ownedSequence = NativeSelectionMethods.GetClipboardSequenceNumber();

        if (!NativeSelectionMethods.SetForegroundWindow(TargetWindow))
        {
            RestoreIfOwned(restoreData);
            throw new InvalidOperationException(
                "Не удалось вернуть фокус в исходное приложение.");
        }

        await Task.Delay(50, cancellationToken);
        NativeSelectionMethods.SendPaste();
        await Task.Delay(120, cancellationToken);
        RestoreIfOwned(restoreData);
    }

    internal void Cancel() => RestoreIfOwned();

    private void RestoreIfOwned(ClipboardSnapshot? data = null)
    {
        if (NativeSelectionMethods.GetClipboardSequenceNumber() != _ownedSequence)
        {
            return;
        }

        (data ?? _originalClipboard).Restore();
    }

    private static string? TryGetUnicodeText()
    {
        try
        {
            return WinForms.Clipboard.ContainsText(WinForms.TextDataFormat.UnicodeText)
                ? WinForms.Clipboard.GetText(WinForms.TextDataFormat.UnicodeText)
                : null;
        }
        catch (ExternalException)
        {
            return null;
        }
    }

    private static void SetReplacementClipboard(string replacement)
    {
        var data = new WinForms.DataObject();
        data.SetText(replacement, WinForms.TextDataFormat.UnicodeText);
        data.SetData("ExcludeClipboardContentFromMonitorProcessing", false);
        data.SetData("CanIncludeInClipboardHistory", false);
        data.SetData("CanUploadToCloudClipboard", false);
        WinForms.Clipboard.SetDataObject(data, true);
    }

}
