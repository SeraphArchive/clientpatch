using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;
using Wpf.Ui.Controls;

namespace ClientpatchManager.App.Views;

/// <summary>
/// Minimal window that hosts the runtime-dump handoff progress view. Shown (instead of the
/// full shell) when the manager is launched with <c>--generate-from-dump</c> by clientpatch.
/// </summary>
public partial class HandoffWindow : FluentWindow
{
    public HandoffWindow(HandoffViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
    }

    /// <summary>Keep the console pinned to the newest line as tool output streams in.</summary>
    private void ConsoleBox_TextChanged(object sender, TextChangedEventArgs e) =>
        ConsoleScroller.ScrollToEnd();
}
