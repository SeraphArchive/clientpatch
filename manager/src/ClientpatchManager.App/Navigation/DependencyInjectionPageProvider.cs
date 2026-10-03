using Microsoft.Extensions.DependencyInjection;
using Wpf.Ui.Abstractions;

namespace ClientpatchManager.App.Navigation;

/// <summary>
/// Bridges Wpf.Ui's <see cref="NavigationView"/> to the DI container so pages are
/// resolved (with their constructor dependencies, e.g. LocalizationService)
/// instead of new'd parameterlessly by the default activator.
/// </summary>
public sealed class DependencyInjectionPageProvider(IServiceProvider services) : INavigationViewPageProvider
{
    private readonly IServiceProvider _services = services;

    /// <summary>Resolve a navigation page instance from the container.</summary>
    public object? GetPage(Type pageType) => _services.GetService(pageType);
}
