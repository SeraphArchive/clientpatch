using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Settings page: game dir, launch method, feed, language, theme, toggles.</summary>
public partial class SettingsPage : Page
{
    public SettingsPage(SettingsViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
    }
}
