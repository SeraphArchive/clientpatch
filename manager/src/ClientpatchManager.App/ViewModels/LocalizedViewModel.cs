using CommunityToolkit.Mvvm.ComponentModel;
using ClientpatchManager.App.I18n;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// Base view-model that exposes the <see cref="LocalizationService"/> to XAML via a
/// <c>[key]</c> indexer and re-raises change notifications for every bound string when
/// the UI language switches, so pages update live without per-key wiring.
/// </summary>
public abstract partial class LocalizedViewModel : ObservableObject
{
    protected LocalizedViewModel(LocalizationService loc)
    {
        Loc = loc;
        Loc.LanguageChanged += OnLanguageChanged;
    }

    /// <summary>The active localization service (also used by derived VMs for messages).</summary>
    protected LocalizationService Loc { get; }

    /// <summary>Localized string lookup for XAML bindings: <c>{Binding [key]}</c>.</summary>
    public string this[string key] => Loc[key];

    /// <summary>Refresh the indexer binding and let derived VMs react to a language switch.</summary>
    private void OnLanguageChanged()
    {
        // Null/empty property name signals "all properties changed" to WPF, which
        // re-evaluates the indexer for every bound key on the page.
        OnPropertyChanged((string?)null);
        OnLanguageChangedCore();
    }

    /// <summary>Hook for derived VMs to rebuild localized child content on a language switch.</summary>
    protected virtual void OnLanguageChangedCore() { }
}
