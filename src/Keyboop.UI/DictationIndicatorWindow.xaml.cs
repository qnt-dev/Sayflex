using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using Keyboop.Protocol;
using MediaBrush = System.Windows.Media.Brush;
using MediaColor = System.Windows.Media.Color;
using WinForms = System.Windows.Forms;

namespace Keyboop.UI;

public partial class DictationIndicatorWindow : Window
{
    private static readonly MediaBrush SignalBrush =
        new SolidColorBrush(MediaColor.FromRgb(255, 212, 0));
    private static readonly MediaBrush FrameBrush =
        new SolidColorBrush(MediaColor.FromRgb(44, 47, 49));
    private static readonly MediaBrush WarningBrush =
        new SolidColorBrush(MediaColor.FromRgb(255, 174, 0));
    private static readonly MediaBrush GlyphBrush =
        new SolidColorBrush(MediaColor.FromRgb(17, 18, 20));
    private static readonly double[] BarProfile = [0.25, 0.4, 0.62, 0.82, 1, 1, 0.82, 0.62, 0.4, 0.25];

    private readonly DispatcherTimer _animationTimer;
    private readonly Border[] _bars;
    private ComponentState _state = ComponentState.Ready;
    private double _targetLevel;
    private double _displayLevel;
    private double _phase;

    public DictationIndicatorWindow()
    {
        InitializeComponent();
        _bars = [Bar0, Bar1, Bar2, Bar3, Bar4, Bar5, Bar6, Bar7, Bar8, Bar9];
        _animationTimer = new DispatcherTimer(
            TimeSpan.FromMilliseconds(55),
            DispatcherPriority.Render,
            Animate,
            Dispatcher);
        SourceInitialized += OnSourceInitialized;
        Closed += (_, _) => _animationTimer.Stop();
    }

    internal void ShowState(StatusMessage status)
    {
        _state = status.State;
        var microphoneLevel = Math.Clamp(status.AudioLevel ?? 0, 0, 1);
        _targetLevel = Math.Clamp(Math.Sqrt(microphoneLevel) * 2.35, 0, 1);
        Width = 112;
        StopDisc.Visibility = Visibility.Visible;
        SetBarBrush(SignalBrush);
        StopDisc.Background = SignalBrush;
        StatusGlyph.Background = GlyphBrush;
        switch (status.State)
        {
            case ComponentState.Listening:
                Pill.BorderBrush = status.Detail.StartsWith(
                    "Less than one minute",
                    StringComparison.OrdinalIgnoreCase)
                    ? WarningBrush
                    : FrameBrush;
                StatusGlyph.Width = 7;
                StatusGlyph.Height = 7;
                StatusGlyph.CornerRadius = new CornerRadius(2);
                break;
            case ComponentState.Processing:
                Pill.BorderBrush = FrameBrush;
                StatusGlyph.Width = 5;
                StatusGlyph.Height = 5;
                StatusGlyph.CornerRadius = new CornerRadius(2.5);
                _targetLevel = 0.18;
                break;
            case ComponentState.Degraded:
                Pill.BorderBrush = WarningBrush;
                SetBarBrush(WarningBrush);
                StopDisc.Background = WarningBrush;
                StatusGlyph.Width = 7;
                StatusGlyph.Height = 2;
                StatusGlyph.CornerRadius = new CornerRadius(1);
                _targetLevel = 0.12;
                break;
            default:
                Hide();
                return;
        }

        PositionAtBottomCenter();
        if (!IsVisible)
        {
            Show();
        }

        if (!_animationTimer.IsEnabled)
        {
            _animationTimer.Start();
        }
    }

    private void Animate(object? sender, EventArgs eventArgs)
    {
        if (!IsVisible)
        {
            return;
        }

        _phase += _state == ComponentState.Processing ? 0.18 : 0.29;
        _displayLevel += (_targetLevel - _displayLevel) * 0.18;
        var processingPulse = (Math.Sin(_phase * 0.72) + 1) / 2;
        var energy = _state == ComponentState.Listening
            ? Math.Max(0.07, _displayLevel)
            : _targetLevel + (processingPulse * 0.08);
        for (var index = 0; index < _bars.Length; index++)
        {
            var travellingWave = (Math.Sin(_phase + (index * 0.62)) + 1) / 2;
            var height = 3
                + (BarProfile[index] * 5)
                + (travellingWave * energy * 12);
            _bars[index].Height = Math.Clamp(height, 3, 19);
            _bars[index].Opacity = 0.5 + (Math.Min(1, energy) * 0.5);
        }

        StopDisc.Opacity = _state == ComponentState.Processing
            ? 0.72 + (processingPulse * 0.28)
            : 1;
    }

    private void OnSourceInitialized(object? sender, EventArgs eventArgs)
    {
        var handle = new WindowInteropHelper(this).Handle;
        NativeSelectionMethods.MakeWindowNonActivating(handle);
    }

    private void PositionAtBottomCenter()
    {
        var foreground = NativeSelectionMethods.GetForegroundWindow();
        var workingArea = foreground != 0
            ? WinForms.Screen.FromHandle(foreground).WorkingArea
            : WinForms.Screen.PrimaryScreen?.WorkingArea
                ?? WinForms.SystemInformation.WorkingArea;
        var dpi = VisualTreeHelper.GetDpi(this);
        Left = (workingArea.Left / dpi.DpiScaleX)
            + ((workingArea.Width / dpi.DpiScaleX) - Width) / 2;
        Top = (workingArea.Bottom / dpi.DpiScaleY) - Height - 22;
    }

    private void SetBarBrush(MediaBrush brush)
    {
        foreach (var bar in _bars)
        {
            bar.Background = brush;
        }
    }
}
