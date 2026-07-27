using System.Drawing;
using System.Windows;
using Keyboop.Protocol;
using WinForms = System.Windows.Forms;

namespace Keyboop.UI;

public partial class App : System.Windows.Application, IDisposable
{
    private readonly SettingsStore _settingsStore = new();
    private Mutex? _instanceMutex;
    private WinForms.NotifyIcon? _trayIcon;
    private Icon? _applicationIcon;
    private ProcessSupervisor? _supervisor;
    private MainWindow? _window;
    private SelectionCorrectionService? _selectionCorrectionService;
    private SelectionHotkeyService? _selectionHotkeyService;
    private DictationIndicatorWindow? _dictationIndicator;
    private CancellationTokenSource? _speechMonitorCancellation;
    private Task? _speechMonitorTask;
    private ComponentState? _lastSpeechState;

    internal AppSettings Settings { get; private set; } = new();

    internal ComponentClient InputHost { get; private set; } = null!;

    internal ComponentClient SpeechWorker { get; private set; } = null!;

    protected override async void OnStartup(StartupEventArgs e)
    {
        // WPF's font cache still reads the legacy lowercase "windir" variable.
        // Some launchers provide a deliberately minimal environment, so restore it
        // before the first Window type is initialized.
        if (string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("windir")))
        {
            Environment.SetEnvironmentVariable(
                "windir",
                Environment.GetFolderPath(Environment.SpecialFolder.Windows));
        }

        base.OnStartup(e);
        _instanceMutex = new Mutex(
            initiallyOwned: true,
            "Local\\Keyboop.UI",
            out var isFirstInstance);
        if (!isFirstInstance)
        {
            _instanceMutex.Dispose();
            _instanceMutex = null;
            Shutdown();
            return;
        }

        try
        {
            Settings = await _settingsStore.LoadAsync();
        }
        catch (ProtocolException)
        {
            Settings = new AppSettings();
        }

        if (Settings.AutoCorrectionEnabled)
        {
            Settings = Settings with { AutoCorrectionEnabled = false };
            await _settingsStore.SaveAsync(Settings);
        }

        InputHost = new ComponentClient(ProtocolConstants.InputHostPipeName);
        SpeechWorker = new ComponentClient(ProtocolConstants.SpeechWorkerPipeName);
        _supervisor = new ProcessSupervisor(InputHost, SpeechWorker);
        await _supervisor.EnsureRunningAsync();

        CreateTrayIcon();
        StartSpeechStatusMonitor();
        InitializeSelectionCorrection();
        ShowMainWindow();
    }

    internal async Task ApplySettingsAsync(AppSettings settings)
    {
        Settings = settings;
        await _settingsStore.SaveAsync(settings);
        await Task.WhenAll(
            SendSettingsBestEffortAsync(InputHost, settings),
            SendSettingsBestEffortAsync(SpeechWorker, settings));
    }

    internal void ShowMainWindow()
    {
        if (_window is null)
        {
            _window = new MainWindow();
            _window.Closed += (_, _) => _window = null;
        }

        _window.Show();
        _window.WindowState = WindowState.Normal;
        _window.Activate();
    }

    internal async Task ExitAsync()
    {
        DisposeSelectionCorrection();
        await StopSpeechStatusMonitorAsync();
        DisposeTrayIcon();
        if (_supervisor is not null)
        {
            await _supervisor.DisposeAsync();
            _supervisor = null;
        }

        Shutdown();
    }

    protected override void OnExit(ExitEventArgs e)
    {
        Dispose();
        base.OnExit(e);
    }

    public void Dispose()
    {
        DisposeSelectionCorrection();
        CancelSpeechStatusMonitor();
        DisposeTrayIcon();
        if (_supervisor is not null)
        {
            _supervisor.DisposeAsync().AsTask().GetAwaiter().GetResult();
            _supervisor = null;
        }

        _instanceMutex?.ReleaseMutex();
        _instanceMutex?.Dispose();
        _instanceMutex = null;
        GC.SuppressFinalize(this);
    }

    private void CreateTrayIcon()
    {
        var menu = new WinForms.ContextMenuStrip();
        menu.Items.Add("Открыть Keyboop", null, (_, _) => Dispatcher.Invoke(ShowMainWindow));
        menu.Items.Add("-");
        menu.Items.Add("Выход", null, async (_, _) => await Dispatcher.InvokeAsync(ExitAsync));
        _applicationIcon = LoadApplicationIcon();
        _trayIcon = new WinForms.NotifyIcon
        {
            Icon = _applicationIcon ?? SystemIcons.Application,
            Text = "Keyboop",
            Visible = true,
            ContextMenuStrip = menu
        };
        _trayIcon.DoubleClick += (_, _) => Dispatcher.Invoke(ShowMainWindow);
    }

    private void DisposeTrayIcon()
    {
        _trayIcon?.Dispose();
        _trayIcon = null;
        _applicationIcon?.Dispose();
        _applicationIcon = null;
    }

    private static Icon? LoadApplicationIcon()
    {
        var executable = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(executable))
        {
            return null;
        }

        try
        {
            return Icon.ExtractAssociatedIcon(executable);
        }
        catch (ArgumentException)
        {
            return null;
        }
    }

    private void InitializeSelectionCorrection()
    {
        UiDiagnostics.Write("Initializing selection correction hotkey.");
        _selectionCorrectionService = new SelectionCorrectionService(ShowNotification);
        try
        {
            _selectionHotkeyService = new SelectionHotkeyService(
                _selectionCorrectionService.RunAsync);
            UiDiagnostics.Write("Selection correction hotkey registered.");
        }
        catch (InvalidOperationException exception)
        {
            UiDiagnostics.Write(
                $"Selection hotkey registration failed: {exception.GetType().Name}: {exception.Message}");
            _selectionCorrectionService.Dispose();
            _selectionCorrectionService = null;
            ShowNotification(
                "Keyboop: горячая клавиша недоступна",
                exception.Message);
        }
    }

    private void DisposeSelectionCorrection()
    {
        _selectionHotkeyService?.Dispose();
        _selectionHotkeyService = null;
        _selectionCorrectionService?.Dispose();
        _selectionCorrectionService = null;
    }

    private void StartSpeechStatusMonitor()
    {
        _speechMonitorCancellation = new CancellationTokenSource();
        _speechMonitorTask = MonitorSpeechStatusAsync(_speechMonitorCancellation.Token);
    }

    private async Task StopSpeechStatusMonitorAsync()
    {
        var cancellation = Interlocked.Exchange(
            ref _speechMonitorCancellation,
            null);
        var monitor = Interlocked.Exchange(ref _speechMonitorTask, null);
        cancellation?.Cancel();
        if (monitor is not null)
        {
            try
            {
                await monitor.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
            }
        }

        cancellation?.Dispose();
        if (_dictationIndicator is not null)
        {
            _dictationIndicator.Close();
            _dictationIndicator = null;
        }
    }

    private void CancelSpeechStatusMonitor()
    {
        var cancellation = Interlocked.Exchange(
            ref _speechMonitorCancellation,
            null);
        _ = Interlocked.Exchange(ref _speechMonitorTask, null);
        cancellation?.Cancel();
        cancellation?.Dispose();
        if (_dictationIndicator is not null)
        {
            _dictationIndicator.Close();
            _dictationIndicator = null;
        }
    }

    private async Task MonitorSpeechStatusAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            StatusMessage? status = null;
            try
            {
                var response = await SpeechWorker.RequestAsync(
                    ProtocolEnvelope.Create(
                        MessageKind.StatusRequest,
                        new AcknowledgementMessage(true, "Speech status monitor")),
                    TimeSpan.FromSeconds(1),
                    cancellationToken).ConfigureAwait(false);
                if (response.Kind == MessageKind.Status)
                {
                    status = response.GetPayload<StatusMessage>();
                }
            }
            catch (Exception) when (!cancellationToken.IsCancellationRequested)
            {
            }

            if (status is not null)
            {
                await Dispatcher.InvokeAsync(
                    () => ApplySpeechStatus(status),
                    System.Windows.Threading.DispatcherPriority.Background,
                    cancellationToken);
            }

            var delay = status?.State is ComponentState.Listening
                or ComponentState.Processing
                or ComponentState.Degraded
                ? TimeSpan.FromMilliseconds(200)
                : TimeSpan.FromMilliseconds(50);
            await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
        }
    }

    private void ApplySpeechStatus(StatusMessage status)
    {
        var previous = _lastSpeechState;
        _lastSpeechState = status.State;
        if (status.State is ComponentState.Listening
            or ComponentState.Processing
            or ComponentState.Degraded)
        {
            _dictationIndicator ??= new DictationIndicatorWindow();
            _dictationIndicator.ShowState(status);
        }
        else
        {
            _dictationIndicator?.Hide();
        }

        if (status.State == ComponentState.Degraded
            && previous != ComponentState.Degraded)
        {
            _trayIcon?.ShowBalloonTip(
                4000,
                "Keyboop остановил запись",
                "Достигнут лимит 20 минут. Отпустите Ctrl+Win для распознавания.",
                WinForms.ToolTipIcon.Warning);
        }
    }

    private void ShowNotification(string title, string message)
    {
        UiDiagnostics.Write($"Notification: {title}: {message}");
        _trayIcon?.ShowBalloonTip(
            4000,
            title,
            message,
            WinForms.ToolTipIcon.Info);
    }

    private static async Task SendSettingsBestEffortAsync(
        ComponentClient client,
        AppSettings settings)
    {
        try
        {
            _ = await client.RequestAsync(
                ProtocolEnvelope.Create(MessageKind.UpdateSettings, settings),
                TimeSpan.FromSeconds(2));
        }
        catch (Exception)
        {
            // The supervisor reconnects a restarted component.
        }
    }
}
