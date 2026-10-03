using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Logs page: tail the newest per-launch log (clientpatch/logs) with auto-scroll, clear-view, and open-file.</summary>
public partial class LogsPage : Page
{
    private readonly LogsViewModel _viewModel;

    public LogsPage(LogsViewModel viewModel)
    {
        _viewModel = viewModel;
        DataContext = viewModel;
        InitializeComponent();

        Loaded += (_, _) =>
        {
            _viewModel.ScrollToEndRequested += OnScrollToEnd;
            _viewModel.Start();
        };
        // Unloaded fires on every navigation away, and the page is cached, so the
        // handler must be re-attached on the next Loaded or auto-scroll dies.
        Unloaded += (_, _) =>
        {
            _viewModel.Stop();
            _viewModel.ScrollToEndRequested -= OnScrollToEnd;
        };
    }

    private void OnScrollToEnd() => LogScroller.ScrollToEnd();
}
