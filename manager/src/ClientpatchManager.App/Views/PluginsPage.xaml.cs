using System.Windows.Controls;
using ClientpatchManager.App.ViewModels;

namespace ClientpatchManager.App.Views;

/// <summary>Plugins page: scan/toggle BepInEx plugins, with a bepinex-off state.</summary>
public partial class PluginsPage : Page
{
    public PluginsPage(PluginsViewModel viewModel)
    {
        DataContext = viewModel;
        InitializeComponent();
    }
}
