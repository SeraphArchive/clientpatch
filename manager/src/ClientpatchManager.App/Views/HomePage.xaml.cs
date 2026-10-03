using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Home page: status tiles, split Launch button, and module health.</summary>
public partial class HomePage : Page
{
    public HomePage(HomeViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
        Loaded += async (_, _) => await viewModel.RefreshCommand.ExecuteAsync(null);
    }
}
