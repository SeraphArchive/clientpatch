using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Modules page: Form + TOML tabs over clientpatch.toml.</summary>
public partial class ModulesPage : Page
{
    public ModulesPage(ModulesViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
    }
}
