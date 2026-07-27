using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using Keyboop.Core.Input;

namespace Keyboop.InputHost;

internal sealed class ClipboardTextInserter : IAsyncDisposable
{
    private static readonly TimeSpan ClipboardRetryDelay =
        TimeSpan.FromMilliseconds(20);
    private static readonly TimeSpan PasteObservationDelay =
        TimeSpan.FromMilliseconds(250);

    private readonly BlockingCollection<InsertionRequest> _requests = new();
    private readonly InputInjector _injector;
    private readonly Thread _thread;

    internal ClipboardTextInserter(InputInjector injector)
    {
        _injector = injector;
        _thread = new Thread(ProcessRequests)
        {
            IsBackground = true,
            Name = "Keyboop clipboard insertion"
        };
        _thread.SetApartmentState(ApartmentState.STA);
        _thread.Start();
    }

    internal bool TryQueue(
        string text,
        bool addTrailingSpace,
        TargetSnapshot target)
    {
        if (string.IsNullOrWhiteSpace(text) || _requests.IsAddingCompleted)
        {
            return false;
        }

        try
        {
            _requests.Add(new InsertionRequest(text, addTrailingSpace, target));
            return true;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    internal bool TryQueueRecovery(string text)
    {
        if (string.IsNullOrWhiteSpace(text) || _requests.IsAddingCompleted)
        {
            return false;
        }

        try
        {
            _requests.Add(
                new InsertionRequest(
                    Text: text,
                    AddTrailingSpace: false,
                    Target: default,
                    Purpose: InsertionPurpose.Recovery));
            return true;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    public ValueTask DisposeAsync()
    {
        _requests.CompleteAdding();
        _ = _thread.Join(TimeSpan.FromSeconds(2));
        _requests.Dispose();
        return ValueTask.CompletedTask;
    }

    private void ProcessRequests()
    {
        _ = Application.OleRequired();
        foreach (var request in _requests.GetConsumingEnumerable())
        {
            try
            {
                ProcessRequest(request);
            }
            catch (Exception exception) when (
                exception is ExternalException
                    or InvalidOperationException
                    or System.ComponentModel.Win32Exception)
            {
                Console.Error.WriteLine(
                    $"Text insertion failed: {exception.GetType().Name}.");
            }
        }
    }

    private void ProcessRequest(InsertionRequest request)
    {
        if (request.Purpose == InsertionPurpose.Recovery)
        {
            ProcessRecovery(request);
            return;
        }

        if (!TargetTracker.IsForeground(request.Target))
        {
            return;
        }

        if (PreferUnicodeInsertion(request.Target))
        {
            _injector.InsertUnicode(request.Text, request.AddTrailingSpace);
            return;
        }

        var original = new DataObject();
        var originalHadData = false;
        var clipboardWasReplaced = false;
        var replacementSequence = 0u;
        var pasted = false;
        try
        {
            original = CaptureClipboard();
            originalHadData = original.GetFormats(autoConvert: false).Length > 0;
            var text = request.AddTrailingSpace
                ? request.Text + " "
                : request.Text;
            RetryClipboard(
                () =>
                {
                    Clipboard.SetText(text, TextDataFormat.UnicodeText);
                    return true;
                });
            replacementSequence = NativeMethods.GetClipboardSequenceNumber();
            clipboardWasReplaced = true;

            if (!TargetTracker.IsForeground(request.Target))
            {
                return;
            }

            _injector.Paste();
            pasted = true;
            Thread.Sleep(PasteObservationDelay);
        }
        catch (ExternalException exception)
        {
            Console.Error.WriteLine(
                $"Clipboard insertion failed: {exception.GetType().Name}.");
        }
        catch (InvalidOperationException exception)
        {
            Console.Error.WriteLine(
                $"Clipboard insertion failed: {exception.GetType().Name}.");
        }
        finally
        {
            if (clipboardWasReplaced
                && NativeMethods.GetClipboardSequenceNumber() == replacementSequence)
            {
                try
                {
                    if (originalHadData)
                    {
                        RetryClipboard(
                            () =>
                            {
                                Clipboard.SetDataObject(original, copy: true);
                                return true;
                            });
                    }
                    else
                    {
                        RetryClipboard(
                            () =>
                            {
                                Clipboard.Clear();
                                return true;
                            });
                    }
                }
                catch (ExternalException exception)
                {
                    Console.Error.WriteLine(
                        $"Clipboard restore failed: {exception.GetType().Name}.");
                }
            }
        }

        if (!pasted && TargetTracker.IsForeground(request.Target))
        {
            _injector.InsertUnicode(request.Text, request.AddTrailingSpace);
        }
    }

    private static void ProcessRecovery(InsertionRequest request)
    {
        RetryClipboard(
            () =>
            {
                Clipboard.SetText(request.Text, TextDataFormat.UnicodeText);
                return true;
            });
    }

    private static bool PreferUnicodeInsertion(TargetSnapshot target)
    {
        try
        {
            using var process = Process.GetProcessById((int)target.ProcessId);
            return TextInsertionPolicy.PreferDirectUnicode(process.ProcessName);
        }
        catch (ArgumentException)
        {
            return false;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    private static T RetryClipboard<T>(Func<T> operation)
    {
        ExternalException? lastException = null;
        for (var attempt = 0; attempt < 10; attempt++)
        {
            try
            {
                return operation();
            }
            catch (ExternalException exception)
            {
                lastException = exception;
                Thread.Sleep(ClipboardRetryDelay);
            }
        }

        throw new InvalidOperationException(
            "The clipboard operation failed after all retries.",
            lastException);
    }

    private static DataObject CaptureClipboard()
    {
        var source = RetryClipboard(Clipboard.GetDataObject);
        var snapshot = new DataObject();
        if (source is null)
        {
            return snapshot;
        }

        foreach (var format in source.GetFormats(autoConvert: false))
        {
            try
            {
                var value = source.GetData(format, autoConvert: false);
                if (value is Stream stream)
                {
                    var copy = new MemoryStream();
                    var originalPosition = stream.CanSeek ? stream.Position : 0;
                    if (stream.CanSeek)
                    {
                        stream.Position = 0;
                    }

                    stream.CopyTo(copy);
                    copy.Position = 0;
                    if (stream.CanSeek)
                    {
                        stream.Position = originalPosition;
                    }

                    value = copy;
                }

                if (value is not null)
                {
                    snapshot.SetData(format, autoConvert: false, value);
                }
            }
            catch (ExternalException)
            {
                // Some clipboard owners advertise formats that cannot be rendered.
            }
        }

        return snapshot;
    }

    private sealed record InsertionRequest(
        string Text,
        bool AddTrailingSpace,
        TargetSnapshot Target,
        InsertionPurpose Purpose = InsertionPurpose.LiveDictation);

    private enum InsertionPurpose
    {
        LiveDictation,
        Recovery
    }
}
