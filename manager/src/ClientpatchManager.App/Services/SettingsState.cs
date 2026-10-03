using ClientpatchManager.Core.Settings;
using ClientpatchManager.Core.Config;

namespace ClientpatchManager.App.Services;

/// <summary>
/// Shared, mutable holder for the current <see cref="ManagerSettings"/>. The DI
/// container registers a single instance so every page reads the same live values
/// (game dir, launch method, feed) and the Settings page can update + persist them
/// in one place. Raises <see cref="Changed"/> when settings are replaced.
/// </summary>
public sealed class SettingsState
{
    private readonly SettingsStore _store;

    public SettingsState(SettingsStore store, ManagerSettings initial)
    {
        _store = store;
        ConfigTemplate.Ensure(initial.GameDir);
        Current = initial;
    }

    /// <summary>The live settings snapshot.</summary>
    public ManagerSettings Current { get; private set; }

    /// <summary>Raised after <see cref="Update"/> replaces the current settings.</summary>
    public event Action? Changed;

    /// <summary>Replace and persist the settings, then notify listeners.</summary>
    public void Update(ManagerSettings settings)
    {
        ConfigTemplate.Ensure(settings.GameDir);
        _store.Save(settings);
        Current = settings;
        Changed?.Invoke();
    }
}
