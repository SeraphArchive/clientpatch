using System.IO;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Config;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// Modules page VM: a typed Form over <see cref="IConfigService.Load"/> plus a raw TOML
/// tab with live <see cref="IConfigService.RawValidate"/> feedback. Save applies the form
/// (or the raw text) back with <see cref="IConfigService.ApplyToText"/>, backing up the
/// config first when the setting is on. Open-in-editor hands the file to the OS editor.
/// </summary>
public sealed partial class ModulesViewModel : LocalizedViewModel
{
    private readonly IConfigService _config;
    private readonly IBackupService _backup;
    private readonly ShellService _shell;
    private readonly SettingsState _settings;

    /// <summary>The unmodified TOML the form was parsed from; ApplyToText edits onto this.</summary>
    private string _originalToml = "";
    private string _diskToml = "";
    private string? _loadedPath;

    [ObservableProperty] private bool _showForm = true;
    [ObservableProperty] private string _rawToml = "";
    [ObservableProperty] private string _validationMessage = "";
    [ObservableProperty] private bool _isValid = true;
    [ObservableProperty] private string _saveMessage = "";
    [ObservableProperty] private bool _hasConfig;

    // LilyPad
    [ObservableProperty] private bool _lilypadEnabled;
    [ObservableProperty] private string _apiBase = "";
    [ObservableProperty] private string _platformBase = "";
    [ObservableProperty] private string _signing = "";
    [ObservableProperty] private string _publicKeyPath = "";

    // Steam
    [ObservableProperty] private bool _steamEnabled;
    [ObservableProperty] private string _steamMode = "";
    [ObservableProperty] private string _country = "";
    [ObservableProperty] private string _uiLanguage = "";

    // Titlebar / Isolation
    [ObservableProperty] private bool _titlebarEnabled;
    [ObservableProperty] private bool _isolationEnabled;
    [ObservableProperty] private bool _interopDumpEnabled;
    [ObservableProperty] private bool _bepInExEnabled;
    [ObservableProperty] private bool _reportEnabled;

    public ModulesViewModel(
        IConfigService config,
        IBackupService backup,
        ShellService shell,
        SettingsState settings,
        LocalizationService loc) : base(loc)
    {
        _config = config;
        _backup = backup;
        _shell = shell;
        _settings = settings;
        Reload();
    }

    private string? ConfigPath =>
        string.IsNullOrWhiteSpace(_settings.Current.GameDir)
            ? null
            : ClientpatchManager.Core.Install.InstallLayout.Resolve(_settings.Current.GameDir!, "clientpatch.toml");

    /// <summary>Load clientpatch.toml into the form + raw editor.</summary>
    [RelayCommand]
    private void Reload()
    {
        SaveMessage = "";
        var path = ConfigPath;
        if (path is null || !File.Exists(path))
        {
            HasConfig = false;
            _originalToml = "";
            RawToml = "";
            return;
        }

        try
        {
            _originalToml = File.ReadAllText(path);
            _diskToml = _originalToml;
            _loadedPath = path;
            RawToml = _originalToml;
            HasConfig = true;
            var cfg = _config.Load(_originalToml);
            PopulateForm(cfg);
        }
        catch (FormatException)
        {
            // The file exists but does not parse: keep it editable on the raw tab.
            // Clearing HasConfig here would make Save a no-op, so the file could
            // never be fixed from the editor.
            HasConfig = true;
            ShowForm = false;
            Validate();
        }
        catch
        {
            HasConfig = false;
        }
    }

    private void PopulateForm(ClientpatchConfig cfg)
    {
        LilypadEnabled = cfg.Lilypad.Enabled;
        ApiBase = cfg.Lilypad.ApiBase;
        PlatformBase = cfg.Lilypad.PlatformBase;
        Signing = cfg.Lilypad.Signing;
        PublicKeyPath = cfg.Lilypad.PublicKeyPath;

        SteamEnabled = cfg.Steam.Enabled;
        SteamMode = cfg.Steam.Mode;
        Country = cfg.Steam.Country;
        UiLanguage = cfg.Steam.UiLanguage;

        TitlebarEnabled = cfg.Titlebar.Enabled;
        IsolationEnabled = cfg.Isolation.Enabled;
        InteropDumpEnabled = cfg.InteropDump?.Enabled == true;
        BepInExEnabled = cfg.BepInEx?.Enabled == true;
        ReportEnabled = cfg.Lilypad.Report.Enabled;
    }

    partial void OnRawTomlChanged(string value)
    {
        if (ShowForm) return;
        Validate();
    }

    private void Validate()
    {
        var err = _config.RawValidate(RawToml);
        IsValid = string.IsNullOrEmpty(err);
        ValidationMessage = IsValid ? Loc["tomlValid"] : err;
    }

    [RelayCommand]
    private void ShowFormTab()
    {
        if (ShowForm) return;
        Validate();
        if (!IsValid) return;
        _originalToml = RawToml;
        PopulateForm(_config.Load(RawToml));
        ShowForm = true;
    }

    [RelayCommand]
    private void ShowTomlTab()
    {
        if (ShowForm && HasConfig)
        {
            try { RawToml = _config.ApplyToText(_originalToml, BuildConfigFromForm()); }
            catch (Exception ex) { SaveMessage = ex.Message; return; }
        }
        ShowForm = false;
        Validate();
    }

    [RelayCommand]
    private void OpenInEditor() => _shell.OpenFile(ConfigPath);

    /// <summary>Apply the form (or raw TOML) back onto the file, backing up first when enabled.</summary>
    [RelayCommand]
    private void Save()
    {
        SaveMessage = "";
        var path = ConfigPath;
        if (path is null || !HasConfig)
            return;

        try
        {
            using var operation = ClientpatchManager.Core.Install.GameOperation.Acquire(_settings.Current.GameDir!);
            if (!string.Equals(path, _loadedPath, StringComparison.OrdinalIgnoreCase) || File.ReadAllText(path) != _diskToml)
                throw new IOException("The config changed outside this editor. Reload it before saving; your draft has been kept.");
            string result;
            if (ShowForm)
            {
                // The form was populated from a parse. If the file on disk no longer
                // parses, applying stale form state would overwrite it — refuse.
                try { _config.Load(_originalToml); }
                catch (FormatException)
                {
                    ShowForm = false;
                    Validate();
                    return;
                }
                var edited = BuildConfigFromForm();
                result = _config.ApplyToText(_originalToml, edited);
            }
            else
            {
                // Raw tab: validate before writing; refuse to save invalid TOML.
                var err = _config.RawValidate(RawToml);
                if (!string.IsNullOrEmpty(err))
                {
                    IsValid = false;
                    ValidationMessage = err;
                    return;
                }
                result = _config.ApplyToText(RawToml, _config.Load(RawToml));
            }

            var staging = ClientpatchManager.Core.Install.FileTransaction.NewStage(_settings.Current.GameDir!);
            try {
                var staged = Path.Combine(staging, "config"); File.WriteAllText(staged, result);
                ClientpatchManager.Core.Install.FileTransaction.Commit(_settings.Current.GameDir!,
                    new Dictionary<string, string?> { ["clientpatch/clientpatch.toml"] = staged },
                    _settings.Current.BackupBeforeChanges ? _backup : null);
            }
            finally { ClientpatchManager.Core.Install.FileTransaction.DeleteStage(staging); }
            _diskToml = result;
            _originalToml = result;
            RawToml = result;
            // Re-parse so the form reflects exactly what was written.
            PopulateForm(_config.Load(result));
            SaveMessage = Loc["modSaved"];
        }
        catch (Exception ex)
        {
            // A save must never crash the manager (read-only file, locked file, full disk).
            SaveMessage = ex.Message;
        }
    }

    private ClientpatchConfig BuildConfigFromForm()
    {
        var cfg = _config.Load(_originalToml);
        cfg.Lilypad = new LilypadSection(LilypadEnabled, ApiBase, PlatformBase, Signing, PublicKeyPath) {
            Report = cfg.Lilypad.Report with { Enabled = ReportEnabled }
        };
        cfg.Steam = new SteamSection(SteamEnabled, SteamMode, Country, UiLanguage);
        cfg.Titlebar = cfg.Titlebar with { Enabled = TitlebarEnabled };
        cfg.Isolation = cfg.Isolation with { Enabled = IsolationEnabled };
        cfg.InteropDump = (cfg.InteropDump ?? new(false)) with { Enabled = InteropDumpEnabled };
        cfg.BepInEx = (cfg.BepInEx ?? new(false)) with { Enabled = BepInExEnabled };

        return cfg;
    }

}
