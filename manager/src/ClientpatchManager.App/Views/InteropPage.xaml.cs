using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Interop page: currency banner, check, and offline three-step generation.</summary>
public partial class InteropPage : Page
{
    public InteropPage(InteropViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
    }

    /// <summary>Keep the console pinned to the newest line as tool output streams in.</summary>
    private void ConsoleBox_TextChanged(object sender, TextChangedEventArgs e) =>
        ConsoleScroller.ScrollToEnd();
}
