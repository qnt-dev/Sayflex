using System.ComponentModel;
using System.Collections.ObjectModel;
using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using Keyboop.Protocol;

namespace Keyboop.UI;

public partial class MainWindow : Window
{
    private bool _initialized;
    private bool _dictationTestActive;
    private Guid? _dictationTestSessionId;
    private bool _historyRefreshActive;
    private readonly ObservableCollection<HistoryItemViewModel> _historyItems = [];

    private static App App => (App)System.Windows.Application.Current;

    public MainWindow()
    {
        InitializeComponent();
        HistoryListBox.ItemsSource = _historyItems;
        Loaded += OnLoaded;
        Activated += OnActivated;
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        e.Cancel = true;
        Hide();
    }

    private async void OnLoaded(object sender, RoutedEventArgs eventArgs)
    {
        var settings = App.Settings;
        DictationCheckBox.IsChecked = settings.DictationEnabled;
        KeepHistoryCheckBox.IsChecked = settings.KeepDictationHistory;
        _initialized = true;
        await Task.WhenAll(
            RefreshStatusAsync(),
            RefreshHistoryAsync());
    }

    private async void OnActivated(object? sender, EventArgs eventArgs)
    {
        if (_initialized)
        {
            await RefreshHistoryAsync();
        }
    }

    private async void SettingChanged(object sender, RoutedEventArgs eventArgs)
    {
        if (!_initialized)
        {
            return;
        }

        var settings = App.Settings with
        {
            AutoCorrectionEnabled = false,
            DictationEnabled = DictationCheckBox.IsChecked == true,
            KeepDictationHistory = KeepHistoryCheckBox.IsChecked == true
        };
        try
        {
            await App.ApplySettingsAsync(settings);
            OverallStatusText.Text = "Активен";
        }
        catch (Exception exception)
        {
            OverallStatusText.Text = "Ошибка настроек";
            SpeechStatusText.Text = exception.Message;
        }
    }

    private async void DownloadModelButton_Click(object sender, RoutedEventArgs eventArgs)
    {
        DownloadModelButton.IsEnabled = false;
        SpeechStatusText.Text = "Скачивание large-v3-turbo… Это около 1,6 ГБ.";
        try
        {
            var response = await App.SpeechWorker.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.DownloadModels,
                    new AcknowledgementMessage(true, "Prepare default model")),
                TimeSpan.FromHours(1));
            SpeechStatusText.Text = ResponseDetail(response);
        }
        catch (Exception exception)
        {
            SpeechStatusText.Text = $"Модель не подготовлена: {exception.Message}";
        }
        finally
        {
            DownloadModelButton.IsEnabled = true;
        }
    }

    private async void DictationTestButton_Click(object sender, RoutedEventArgs eventArgs)
    {
        DictationTestButton.IsEnabled = false;
        try
        {
            if (!_dictationTestActive)
            {
                var sessionId = Guid.NewGuid();
                var response = await App.SpeechWorker.RequestAsync(
                    ProtocolEnvelope.Create(
                        MessageKind.BeginDictation,
                        new BeginDictationMessage(
                            sessionId,
                            DateTimeOffset.UtcNow)),
                    TimeSpan.FromSeconds(5));
                if (response.Kind == MessageKind.Error)
                {
                    SpeechStatusText.Text = ResponseDetail(response);
                    return;
                }

                _dictationTestSessionId = sessionId;
                _dictationTestActive = true;
                DictationTestButton.Content = "Остановить и распознать";
                SpeechStatusText.Text = "Слушаю микрофон…";
            }
            else
            {
                SpeechStatusText.Text = "Распознавание…";
                var response = await App.SpeechWorker.RequestAsync(
                    ProtocolEnvelope.Create(
                        MessageKind.EndDictation,
                        new DictationSessionRequest(
                            _dictationTestSessionId
                                ?? throw new InvalidOperationException(
                                    "The microphone test session is missing."))),
                    TimeSpan.FromMinutes(10));
                if (response.Kind == MessageKind.Transcript)
                {
                    var transcript = response.GetPayload<TranscriptMessage>();
                    HistoryTextBox.Text = transcript.Text;
                    SpeechStatusText.Text =
                        $"{transcript.Language} · звук {transcript.AudioDuration.TotalSeconds:F1} с · "
                        + $"обработка {transcript.ProcessingDuration.TotalSeconds:F1} с";
                    await RefreshHistoryAsync();
                }
                else
                {
                    SpeechStatusText.Text = ResponseDetail(response);
                }

                _dictationTestActive = false;
                _dictationTestSessionId = null;
                DictationTestButton.Content = "Начать тест микрофона";
            }
        }
        catch (Exception exception)
        {
            _dictationTestActive = false;
            _dictationTestSessionId = null;
            DictationTestButton.Content = "Начать тест микрофона";
            SpeechStatusText.Text = exception.Message;
        }
        finally
        {
            DictationTestButton.IsEnabled = true;
        }
    }

    internal async Task RefreshHistoryAsync()
    {
        if (_historyRefreshActive)
        {
            return;
        }

        _historyRefreshActive = true;
        try
        {
            var selectedId = SelectedHistoryItem?.Entry.Id;
            var response = await App.SpeechWorker.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.GetDictationHistory,
                    new AcknowledgementMessage(true, "Refresh history")),
                TimeSpan.FromSeconds(3));
            if (response.Kind != MessageKind.DictationHistory)
            {
                HistoryStatusText.Text = ResponseDetail(response);
                return;
            }

            var entries = response
                .GetPayload<DictationHistoryMessage>()
                .Entries;
            _historyItems.Clear();
            foreach (var entry in entries)
            {
                _historyItems.Add(new HistoryItemViewModel(entry));
            }

            var selection = selectedId is { } id
                ? _historyItems.FirstOrDefault(item => item.Entry.Id == id)
                : null;
            HistoryListBox.SelectedItem =
                selection ?? _historyItems.FirstOrDefault();
            ClearHistoryButton.IsEnabled = _historyItems.Count > 0;
            HistoryStatusText.Text = _historyItems.Count switch
            {
                0 => "История пока пуста.",
                1 => "Сохранена 1 запись.",
                _ => $"Сохранено записей: {_historyItems.Count}."
            };
        }
        catch (Exception exception)
        {
            HistoryStatusText.Text =
                $"Не удалось прочитать историю: {exception.Message}";
        }
        finally
        {
            _historyRefreshActive = false;
        }
    }

    private void HistoryListBox_SelectionChanged(
        object sender,
        SelectionChangedEventArgs eventArgs)
    {
        var selected = SelectedHistoryItem;
        HistoryTextBox.Text = selected?.Entry.Text ?? string.Empty;
        CopyHistoryButton.IsEnabled = selected is not null;
        DeleteHistoryButton.IsEnabled = selected is not null;
    }

    private void CopyHistoryButton_Click(object sender, RoutedEventArgs eventArgs)
    {
        var selected = SelectedHistoryItem;
        if (selected is null)
        {
            return;
        }

        try
        {
            System.Windows.Clipboard.SetText(selected.Entry.Text);
            HistoryStatusText.Text =
                "Текст скопирован. Используйте Ctrl+V или Shift+Insert.";
        }
        catch (Exception exception) when (
            exception is System.Runtime.InteropServices.ExternalException
                or InvalidOperationException)
        {
            HistoryStatusText.Text =
                $"Не удалось заполнить буфер обмена: {exception.Message}";
        }
    }

    private async void DeleteHistoryButton_Click(
        object sender,
        RoutedEventArgs eventArgs)
    {
        var selected = SelectedHistoryItem;
        if (selected is null)
        {
            return;
        }

        DeleteHistoryButton.IsEnabled = false;
        try
        {
            var response = await App.SpeechWorker.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.DeleteDictationHistoryEntry,
                    new DictationHistoryEntryRequest(selected.Entry.Id)),
                TimeSpan.FromSeconds(3));
            HistoryStatusText.Text = ResponseDetail(response);
            await RefreshHistoryAsync();
        }
        catch (Exception exception)
        {
            HistoryStatusText.Text =
                $"Не удалось удалить запись: {exception.Message}";
        }
    }

    private async void ClearHistoryButton_Click(
        object sender,
        RoutedEventArgs eventArgs)
    {
        if (System.Windows.MessageBox.Show(
                this,
                "Удалить всю локальную историю диктовок?",
                "Keyboop",
                MessageBoxButton.YesNo,
                MessageBoxImage.Warning) != MessageBoxResult.Yes)
        {
            return;
        }

        ClearHistoryButton.IsEnabled = false;
        try
        {
            var response = await App.SpeechWorker.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.ClearDictationHistory,
                    new AcknowledgementMessage(true, "Clear history")),
                TimeSpan.FromSeconds(3));
            HistoryStatusText.Text = ResponseDetail(response);
            await RefreshHistoryAsync();
        }
        catch (Exception exception)
        {
            HistoryStatusText.Text =
                $"Не удалось очистить историю: {exception.Message}";
        }
    }

    private HistoryItemViewModel? SelectedHistoryItem =>
        HistoryListBox.SelectedItem as HistoryItemViewModel;

    private async Task RefreshStatusAsync()
    {
        var inputStatusTask = QueryStatusAsync(App.InputHost);
        var speechStatusTask = QueryStatusAsync(App.SpeechWorker);
        await Task.WhenAll(inputStatusTask, speechStatusTask);
        var inputStatus = await inputStatusTask;
        var speechStatus = await speechStatusTask;
        OverallStatusText.Text = inputStatus is not null && speechStatus is not null
            ? "Активен"
            : "Ограниченный режим";
        if (speechStatus is null)
        {
            SpeechStatusText.Text = "SpeechWorker недоступен.";
            return;
        }

        SetSpeechStatus(speechStatus);
        if (IsModelLoaded(speechStatus))
        {
            return;
        }

        for (var attempt = 0; attempt < 30 && IsVisible; attempt++)
        {
            await Task.Delay(TimeSpan.FromSeconds(1));
            speechStatus = await QueryStatusAsync(App.SpeechWorker);
            if (speechStatus is null)
            {
                continue;
            }

            SetSpeechStatus(speechStatus);
            if (IsModelLoaded(speechStatus))
            {
                return;
            }
        }
    }

    private static async Task<StatusMessage?> QueryStatusAsync(ComponentClient client)
    {
        try
        {
            var response = await client.RequestAsync(
                ProtocolEnvelope.Create(
                    MessageKind.StatusRequest,
                    new AcknowledgementMessage(true, "status")),
                TimeSpan.FromSeconds(2));
            return response.Kind == MessageKind.Status
                ? response.GetPayload<StatusMessage>()
                : null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    private void SetSpeechStatus(StatusMessage status)
    {
        if (status.State == ComponentState.Listening)
        {
            SpeechStatusText.Text = "Слушаю микрофон…";
            return;
        }

        SpeechStatusText.Text = IsModelLoaded(status)
            ? status.Backend?.Contains("Cuda", StringComparison.OrdinalIgnoreCase) == true
                ? "Модель готова · CUDA"
                : "Модель готова · CPU"
            : "Worker готов; модель загружается в фоне…";
    }

    private static bool IsModelLoaded(StatusMessage status) =>
        !status.Detail.StartsWith(
            "Model is not loaded.",
            StringComparison.OrdinalIgnoreCase);

    private static string ResponseDetail(ProtocolEnvelope response) =>
        response.Kind switch
        {
            MessageKind.Acknowledgement =>
                response.GetPayload<AcknowledgementMessage>().Detail,
            MessageKind.Error => response.GetPayload<ErrorMessage>().Detail,
            MessageKind.Status => response.GetPayload<StatusMessage>().Detail,
            _ => response.Kind.ToString()
        };
}

internal sealed class HistoryItemViewModel
{
    internal HistoryItemViewModel(DictationHistoryEntry entry)
    {
        Entry = entry;
        var singleLine = string.Join(
            " ",
            entry.Text.Split(
                (char[]?)null,
                StringSplitOptions.RemoveEmptyEntries
                    | StringSplitOptions.TrimEntries));
        Preview = singleLine.Length <= 180
            ? singleLine
            : singleLine[..177] + "…";
        Metadata = string.Create(
            CultureInfo.CurrentCulture,
            $"{entry.CreatedAt.ToLocalTime():g} · {entry.Language} · {entry.AudioDuration.TotalSeconds:F1} с");
    }

    internal DictationHistoryEntry Entry { get; }

    public string Preview { get; }

    public string Metadata { get; }
}
