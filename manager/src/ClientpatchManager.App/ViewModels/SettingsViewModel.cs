using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.App.Theme;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Settings;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// Settings page VM: game dir (browse + Steam auto-detect), launch method, feed repo,
/// interface language (via <see cref="LocalizationService"/>), theme (via
/// <see cref="ThemeService"/>), and the check-on-launch / backup toggles. Every change
/// persists through <see cref="SettingsState"/> so the rest of the app sees it live.
/// </summary>
public sealed partial class SettingsViewModel : LocalizedViewModel
{
    private readonly SettingsState _settings;
    private readonly ThemeService _theme;
    private readonly ShellService _shell;
    private bool _loading;

    [ObservableProperty] private string? _gameDir;
    [ObservableProperty] private bool _launchSteam;
    [ObservableProperty] private string _feedRepo = "";
    [ObservableProperty] private string _language = "system";
    [ObservableProperty] private string _themeToken = "system";
    [ObservableProperty] private bool _checkOnLaunch;
    [ObservableProperty] private bool _backupBeforeChanges;

    public SettingsViewModel(
        SettingsState settings,
        ThemeService theme,
        ShellService shell,
        LocalizationService loc) : base(loc)
    {
        _settings = settings;
        _theme = theme;
        _shell = shell;
        LoadFromSettings();
    }

    private void LoadFromSettings()
    {
        _loading = true;
        var s = _settings.Current;
        GameDir = s.GameDir;
        LaunchSteam = s.LaunchMethod == LaunchMethod.Steam;
        FeedRepo = s.FeedRepo;
        Language = string.IsNullOrWhiteSpace(s.Language) ? "system" : s.Language!;
        ThemeToken = s.Theme;
        CheckOnLaunch = s.CheckOnLaunch;
        BackupBeforeChanges = s.BackupBeforeChanges;
        _loading = false;
    }

    /// <summary>Browse for a game folder.</summary>
    [RelayCommand]
    private void Browse()
    {
        var picked = _shell.PickFolder(GameDir);
        if (picked is not null)
        {
            GameDir = picked;
            Persist();
        }
    }

    /// <summary>Auto-detect the HBR install from Steam's library folders.</summary>
    [RelayCommand]
    private void AutoDetect()
    {
        var root = SteamRootLocator.FindSteamRoot();
        if (root is null)
            return;
        var found = SteamLibraryLocator.FindHbrGameDir(SteamRootLocator.LibraryRoots(root));
        if (found is not null)
        {
            GameDir = found;
            Persist();
        }
    }

    partial void OnLaunchSteamChanged(bool value) => Persist();

    [RelayCommand] private void ChooseSteam() => LaunchSteam = true;
    [RelayCommand] private void ChooseDirect() => LaunchSteam = false;
    partial void OnFeedRepoChanged(string value) => Persist();
    partial void OnCheckOnLaunchChanged(bool value) => Persist();
    partial void OnBackupBeforeChangesChanged(bool value) => Persist();

    partial void OnLanguageChanged(string value)
    {
        if (_loading) return;
        // "system" persists as null (follow the OS culture).
        Loc.SetLanguage(value == "system" ? "" : value);
        Persist();
    }

    partial void OnThemeTokenChanged(string value)
    {
        if (_loading) return;
        _theme.Apply(value);
        Persist();
    }

    /// <summary>Write the current field values back into shared settings + disk.</summary>
    private void Persist()
    {
        if (_loading) return;
        _settings.Update(new ManagerSettings(
            GameDir: string.IsNullOrWhiteSpace(GameDir) ? null : GameDir,
            LaunchMethod: LaunchSteam ? LaunchMethod.Steam : LaunchMethod.Direct,
            FeedRepo: FeedRepo,
            Language: Language == "system" ? null : Language,
            Theme: ThemeToken,
            CheckOnLaunch: CheckOnLaunch,
            BackupBeforeChanges: BackupBeforeChanges));
    }
}
