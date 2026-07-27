using System.IO;
using System.Runtime.InteropServices;
using WinForms = System.Windows.Forms;

namespace Keyboop.UI;

internal sealed class ClipboardSnapshot
{
    private ClipboardSnapshot(WinForms.DataObject? data)
    {
        Data = data;
    }

    private WinForms.DataObject? Data { get; }

    internal static ClipboardSnapshot Capture()
    {
        WinForms.IDataObject? source;
        try
        {
            source = WinForms.Clipboard.GetDataObject();
        }
        catch (ExternalException exception)
        {
            throw new InvalidOperationException(
                "Не удалось безопасно сохранить текущее содержимое буфера обмена.",
                exception);
        }

        if (source is null)
        {
            return new ClipboardSnapshot(null);
        }

        var snapshot = new WinForms.DataObject();
        var formats = source.GetFormats(autoConvert: false);
        try
        {
            foreach (var format in formats)
            {
                var value = source.GetData(format, autoConvert: false);
                if (value is null)
                {
                    throw new InvalidOperationException(
                        $"Формат буфера обмена «{format}» нельзя сохранить.");
                }

                snapshot.SetData(
                    format,
                    autoConvert: false,
                    CloneValue(value));
            }
        }
        catch (Exception exception) when (
            exception is not InvalidOperationException)
        {
            throw new InvalidOperationException(
                "Не удалось материализовать все форматы буфера обмена.",
                exception);
        }

        return new ClipboardSnapshot(snapshot);
    }

    internal void Restore()
    {
        try
        {
            if (Data is null)
            {
                WinForms.Clipboard.Clear();
            }
            else
            {
                WinForms.Clipboard.SetDataObject(Data, copy: true);
            }
        }
        catch (ExternalException)
        {
            // Another process owns the clipboard. The caller already verified the
            // sequence, so a retry could overwrite a newer user value.
        }
    }

    private static object CloneValue(object value) =>
        value switch
        {
            string text => text,
            byte[] bytes => bytes.ToArray(),
            MemoryStream stream => new MemoryStream(
                stream.ToArray(),
                writable: false),
            ICloneable cloneable => cloneable.Clone(),
            _ => value
        };
}
