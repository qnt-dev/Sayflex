using Keyboop.Core.Layout;

namespace Keyboop.UI;

internal sealed class SelectionCorrectionService : IDisposable
{
    private readonly SemaphoreSlim _operationGate = new(1, 1);
    private readonly Action<string, string> _notify;

    internal SelectionCorrectionService(Action<string, string> notify)
    {
        _notify = notify;
    }

    public void Dispose() => _operationGate.Dispose();

    internal async Task RunAsync()
    {
        UiDiagnostics.Write("Selection correction requested.");
        if (!await _operationGate.WaitAsync(0))
        {
            UiDiagnostics.Write("Selection correction ignored: operation already active.");
            return;
        }

        ClipboardSelectionTransaction? transaction = null;
        try
        {
            var target = NativeSelectionMethods.GetForegroundWindow();
            UiDiagnostics.Write($"Selection target captured: 0x{target:x}.");
            await NativeSelectionMethods.WaitForSelectionHotkeyReleaseAsync();
            transaction = await ClipboardSelectionTransaction.CaptureAsync(target);
            if (transaction is null)
            {
                UiDiagnostics.Write("Selection capture returned no text.");
                _notify(
                    "Keyboop",
                    "Не удалось прочитать выделенный текст. Выделите обычный текст и повторите Ctrl+Alt+K.");
                return;
            }

            UiDiagnostics.Write(
                $"Selection captured: {transaction.SelectedText.Length} UTF-16 code units.");
            var candidates = BuildCandidates(transaction.SelectedText);
            UiDiagnostics.Write($"Selection candidates built: {candidates.Count}.");
            if (candidates.Count == 0)
            {
                transaction.Cancel();
                _notify(
                    "Keyboop",
                    "В выделении нет символов русской или английской раскладки.");
                return;
            }

            var overlay = new SelectionOverlayWindow(
                transaction.SelectedText,
                candidates);
            UiDiagnostics.Write("Selection overlay created.");
            _ = overlay.ShowDialog();
            UiDiagnostics.Write(
                $"Selection overlay closed: selected={overlay.SelectedCandidate is not null}.");
            if (overlay.SelectedCandidate is null)
            {
                transaction.Cancel();
                return;
            }

            UiDiagnostics.Write("Applying selected correction.");
            await transaction.ApplyAsync(overlay.SelectedCandidate.Text);
            UiDiagnostics.Write("Selected correction applied.");
        }
        catch (Exception exception)
        {
            UiDiagnostics.Write(
                $"Selection correction failed: {exception.GetType().Name}: {exception.Message}");
            transaction?.Cancel();
            _notify("Keyboop: исправление не выполнено", exception.Message);
        }
        finally
        {
            _operationGate.Release();
        }
    }

    private static List<SelectionCandidate> BuildCandidates(string source)
    {
        var conversions = SelectionLayoutConverter.BuildCandidates(source);
        var candidates = new List<SelectionCandidate>(conversions.Count);
        foreach (var conversion in conversions)
        {
            var toRussian =
                conversion.Direction == LayoutConversionDirection.ToRussian;
            candidates.Add(
                new SelectionCandidate(
                    candidates.Count + 1,
                    toRussian ? "Русская раскладка" : "Английская раскладка",
                    toRussian
                        ? "EN → RU, с сохранением регистра и пунктуации"
                        : "RU → EN, с сохранением регистра и пунктуации",
                    conversion.Text));
        }

        return candidates;
    }
}
