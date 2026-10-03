using System.Collections.ObjectModel;
using System.IO;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Plugins;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>A plugin row with an enable toggle bound back to <see cref="IPluginService.SetEnabled"/>.</summary>
public sealed partial class PluginRow : ObservableObject
{
    private readonly Action<PluginRow, bool> _onToggle;
    private bool _suppress;

    public PluginRow(string fileName, string displayName, bool enabled, Action<PluginRow, bool> onToggle)
    {
        FileName = fileName;
        DisplayName = displayName;
        _enabled = enabled;
        _onToggle = onToggle;
    }

    public string FileName { get; }
    public string DisplayName { get; }

    [ObservableProperty] private bool _enabled;

    partial void OnEnabledChanged(bool value)
    {
        if (_suppress) return;
        _onToggle(this, value);
    }

    /// <summary>Set the toggle without firing the service call (used to revert on failure).</summary>
    public void SetEnabledSilently(bool value)
    {
        _suppress = true;
        Enabled = value;
        _suppress = false;
    }
}

/// <summary>
/// Plugins page VM: scans the BepInEx plugins folder via <see cref="IPluginService"/>,
/// toggles each plugin, rescans, and opens the folder. When the bepinex module is not
/// enabled in config, shows an explanatory disabled state instead of a list.
/// </summary>
public sealed partial class PluginsViewModel : LocalizedViewModel
{
    private readonly IPluginService _plugins;
    private readonly IConfigService _config;
    private readonly IBackupService _backup;
    private readonly ShellService _shell;
    private readonly Services.SettingsState _settings;
    private string? _scannedRoot;

    [ObservableProperty] private bool _bepinexEnabled;
    [ObservableProperty] private bool _isEmpty;
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    private string _errorMessage = "";
    public bool HasError => ErrorMessage.Length > 0;

    public ObservableCollection<PluginRow> Plugins { get; } = new();

    public PluginsViewModel(
        IPluginService plugins,
        IConfigService config,
        ShellService shell,
        Services.SettingsState settings,
        LocalizationService loc,
        IBackupService backup) : base(loc)
    {
        _plugins = plugins;
        _config = config;
        _backup = backup;
        _shell = shell;
        _settings = settings;
        Rescan();
    }

    private string? BepinexRoot =>
        string.IsNullOrWhiteSpace(_settings.Current.GameDir)
            ? null
            : Path.Combine(_settings.Current.GameDir!, ReadBepinexConfig()?.Root ?? "BepInEx");

    /// <summary>Re-read the config gate and re-scan the plugins folder.</summary>
    [RelayCommand]
    private void Rescan()
    {
        Plugins.Clear();
        _scannedRoot = null;
        ErrorMessage = "";
        try
        {
            BepinexEnabled = IsBepinexModuleEnabled();
            if (!BepinexEnabled)
            {
                IsEmpty = false;
                return;
            }

            var root = BepinexRoot;
            if (root is null)
            {
                IsEmpty = true;
                return;
            }

            _ = FileTransaction.Resolve(_settings.Current.GameDir!,
                Path.GetRelativePath(_settings.Current.GameDir!, Path.Combine(root, "plugins")));
            foreach (var p in _plugins.Scan(root))
                Plugins.Add(new PluginRow(p.FileName, p.DisplayName, p.Enabled, OnToggle));

            _scannedRoot = root;
            IsEmpty = Plugins.Count == 0;
        }
        catch (Exception ex)
        {
            Plugins.Clear();
            IsEmpty = true;
            ErrorMessage = ex.Message;
        }
    }

    [RelayCommand]
    private void OpenFolder()
    {
        var root = BepinexRoot;
        if (root is not null)
            _shell.OpenFolder(Path.Combine(root, "plugins"));
    }

    private void OnToggle(PluginRow row, bool enabled)
    {
        ErrorMessage = "";
        try
        {
            var root = BepinexRoot ?? throw new IOException("Game folder is not configured.");
            var gameDir = _settings.Current.GameDir!;
            using var operation = GameOperation.Acquire(gameDir);
            GameOperation.RequireStopped(gameDir);
            if (!BepinexEnabled || !IsBepinexModuleEnabled() || !Plugins.Contains(row)
                || !string.Equals(root, _scannedRoot, StringComparison.OrdinalIgnoreCase))
                throw new IOException("The plugin location or configuration changed. Rescan before changing plugins.");
            var relative = Path.GetRelativePath(gameDir, Path.Combine(root, "plugins", row.FileName));
            // Core receives the BepInEx root; validate its ancestors against the
            // actual game root even when the optional backup is disabled.
            _ = FileTransaction.Resolve(gameDir, relative);
            _ = FileTransaction.Resolve(gameDir, relative + ".disabled");
            if (_settings.Current.BackupBeforeChanges) {
                _backup.Backup(gameDir, new[] { relative, relative + ".disabled" });
            }
            _plugins.SetEnabled(root, row.FileName, enabled);
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
            // Revert the toggle so the UI reflects the on-disk truth.
            row.SetEnabledSilently(!enabled);
        }
    }

    private bool IsBepinexModuleEnabled()
    {
        try
        {
            var dir = _settings.Current.GameDir;
            if (string.IsNullOrWhiteSpace(dir))
                return false;
            var cfgPath = ClientpatchManager.Core.Install.InstallLayout.Resolve(dir!, "clientpatch.toml");
            if (!File.Exists(cfgPath))
                return false;
            var cfg = _config.Load(File.ReadAllText(cfgPath));
            return cfg.BepInEx?.Enabled == true;
        }
        catch
        {
            return false;
        }
    }

    private BepInExSection? ReadBepinexConfig()
    {
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir)) return null;
        var path = ClientpatchManager.Core.Install.InstallLayout.Resolve(dir, "clientpatch.toml");
        return File.Exists(path) ? _config.Load(File.ReadAllText(path)).BepInEx : null;
    }
}
