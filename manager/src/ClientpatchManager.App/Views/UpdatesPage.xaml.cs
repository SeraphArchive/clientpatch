using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Updates page: feed check, component list, install, and rollback.</summary>
public partial class UpdatesPage : Page
{
    public UpdatesPage(UpdatesViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
        // Not navigation-cached, so this fires once per visit — one query, no overlap.
        Loaded += async (_, _) => await viewModel.RefreshCommand.ExecuteAsync(null);
    }
}
