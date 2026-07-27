using System.Windows;
using System.Windows.Input;

namespace Keyboop.UI;

public partial class SelectionOverlayWindow : Window
{
    internal SelectionOverlayWindow(
        string source,
        IReadOnlyList<SelectionCandidate> candidates)
    {
        Candidates = candidates;
        InitializeComponent();
        DataContext = this;
        SourceTextBlock.Text = source;
        CandidateList.SelectedIndex = 0;
        Loaded += PositionNearCursor;
    }

    public IReadOnlyList<SelectionCandidate> Candidates { get; }

    internal SelectionCandidate? SelectedCandidate { get; private set; }

    private void Window_KeyDown(object sender, System.Windows.Input.KeyEventArgs eventArgs)
    {
        if (eventArgs.Key == Key.Escape)
        {
            DialogResult = false;
            eventArgs.Handled = true;
            return;
        }

        if (eventArgs.Key is Key.Enter or Key.Space)
        {
            AcceptSelected();
            eventArgs.Handled = true;
            return;
        }

        var number = eventArgs.Key switch
        {
            Key.D1 or Key.NumPad1 => 1,
            Key.D2 or Key.NumPad2 => 2,
            _ => 0
        };
        if (number > 0 && number <= Candidates.Count)
        {
            CandidateList.SelectedIndex = number - 1;
            AcceptSelected();
            eventArgs.Handled = true;
        }
    }

    private void CandidateList_SelectionChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs eventArgs)
    {
        if (eventArgs.AddedItems.Count > 0)
        {
            CandidateList.ScrollIntoView(eventArgs.AddedItems[0]);
        }
    }

    private void AcceptSelected()
    {
        SelectedCandidate = CandidateList.SelectedItem as SelectionCandidate;
        if (SelectedCandidate is not null)
        {
            DialogResult = true;
        }
    }

    private void PositionNearCursor(object sender, RoutedEventArgs eventArgs)
    {
        if (!NativeSelectionMethods.GetCursorPosition(out var point))
        {
            return;
        }

        var desiredLeft = point.X + 18;
        var desiredTop = point.Y + 18;
        Left = Math.Clamp(
            desiredLeft,
            SystemParameters.VirtualScreenLeft + 12,
            SystemParameters.VirtualScreenLeft
                + SystemParameters.VirtualScreenWidth
                - ActualWidth
                - 12);
        Top = Math.Clamp(
            desiredTop,
            SystemParameters.VirtualScreenTop + 12,
            SystemParameters.VirtualScreenTop
                + SystemParameters.VirtualScreenHeight
                - ActualHeight
                - 12);
        Activate();
        CandidateList.Focus();
    }
}
