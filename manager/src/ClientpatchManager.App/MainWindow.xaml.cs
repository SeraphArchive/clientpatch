using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Navigation;
using ClientpatchManager.App.Theme;
using ClientpatchManager.App.ViewModels;
using ClientpatchManager.App.Views;
using Wpf.Ui.Abstractions;
using Wpf.Ui.Controls;

namespace ClientpatchManager.App;

/// <summary>
/// The Fluent shell window. Hosts the left NavigationView (six destinations +
/// footer Settings), applies the saved theme, and keeps nav labels + window title
/// in sync with the active UI language.
/// </summary>
public partial class MainWindow : FluentWindow
{
    private readonly LocalizationService _loc;

    public MainWindow(
        MainViewModel viewModel,
        LocalizationService loc,
        ThemeService theme,
        INavigationViewPageProvider pageProvider)
    {
        _loc = loc;
        DataContext = viewModel;

        InitializeComponent();

        // Let "system" theme track live OS appearance changes on this window.
        theme.SetWindow(this);

        // Resolve nav pages through the DI container (their ctors need services).
        RootNavigation.SetPageProviderService(pageProvider);

        ApplyNavLabels();
        _loc.LanguageChanged += OnLanguageChanged;
        Closed += (_, _) => _loc.LanguageChanged -= OnLanguageChanged;

        // Open on Home once the view is ready.
        Loaded += (_, _) => RootNavigation.Navigate(typeof(HomePage));
    }

    private void OnLanguageChanged() => ApplyNavLabels();

    /// <summary>Push localized labels onto the nav items (keys per the mockup).</summary>
    private void ApplyNavLabels()
    {
        NavHome.Content = _loc["navHome"];
        NavUpdates.Content = _loc["navVersion"];
        NavModules.Content = _loc["navModules"];
        NavPlugins.Content = _loc["navPlugins"];
        NavInterop.Content = _loc["navInterop"];
        NavLogs.Content = _loc["navLogs"];
        NavSettings.Content = _loc["navSettings"];
    }
}
