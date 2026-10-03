using CommunityToolkit.Mvvm.ComponentModel;
using ClientpatchManager.App.I18n;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// View-model for the shell window. Exposes the localized application title and
/// re-raises change notifications when the UI language switches so bound XAML
/// (title bar, window title) updates live. Nav item labels are refreshed by the
/// window's code-behind against the same <see cref="LocalizationService"/>.
/// </summary>
public sealed partial class MainViewModel : ObservableObject
{
    private readonly LocalizationService _loc;

    public MainViewModel(LocalizationService loc)
    {
        _loc = loc;
        _loc.LanguageChanged += OnLanguageChanged;
    }

    /// <summary>Localized application title (mockup title bar + window caption).</summary>
    public string AppTitle => _loc["appTitle"];

    private void OnLanguageChanged() => OnPropertyChanged(nameof(AppTitle));
}
