using System.Collections.ObjectModel;
using System.IO;
using System.Net.Http;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Models;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>One module-health row on the Home page (localized name + state text).</summary>
public sealed class ModuleHealthItem
{
    public ModuleHealthItem(string name, string stateText)
    {
        Name = name;
        StateText = stateText;
    }

    public string Name { get; }
    public string StateText { get; }
}

/// <summary>
/// Home page VM: status tiles from <see cref="IInstallDetector"/> (+ interop currency
/// and a LilyPad reachability probe), a split Launch button over <see cref="ILauncher"/>,
/// open-folder, and a module-health list parsed from the proxy's log with a config-based
/// fallback. Refresh computes raw state (off the UI thread); Render applies the active
/// language — so a language switch re-renders without re-reading files or re-probing.
/// </summary>
public sealed partial class HomeViewModel : LocalizedViewModel
{
    private const int LogTailBytes = 256 * 1024;

    private readonly IInstallDetector _detector;
    private readonly IInteropService _interop;
    private readonly IConfigService _config;
    private readonly ILauncher _launcher;
    private readonly ShellService _shell;
    private readonly SettingsState _settings;
    private readonly UpdateAvailability _availability;
    private readonly HttpClient _http;

    [ObservableProperty] private string _gameStatus = "";
    [ObservableProperty] private string _patchVersion = "";
    [ObservableProperty] private string _serverStatus = "";
    [ObservableProperty] private string _buildId = "";
    [ObservableProperty] private string _launchModeLabel = "";
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasLaunchError))]
    private string? _launchError;
    [ObservableProperty] private bool _isBusy;

    /// <summary>True when a launch error message is present (drives the InfoBar).</summary>
    public bool HasLaunchError => !string.IsNullOrEmpty(LaunchError);

    /// <summary>The alternate launch method offered by the split button's caret.</summary>
    private LaunchMethod _activeMethod;

    public ObservableCollection<ModuleHealthItem> Modules { get; } = new();

    [ObservableProperty] private bool _showHealthNote;

    // Raw (language-neutral) state, computed by Refresh and re-rendered on language switch.
    private enum ServerState { Off, Reachable, Unreachable }
    private bool _hasGameDir;
    private DeploymentState _state;
    private string? _deployedVersion;
    private string? _buildIdValue;
    private ServerState _server;
    private readonly List<(string Key, bool Ok)> _health = new();
    private bool _healthNote;

    public HomeViewModel(
        IInstallDetector detector,
        IInteropService interop,
        IConfigService config,
        ILauncher launcher,
        ShellService shell,
        SettingsState settings,
        UpdateAvailability availability,
        HttpClient http,
        LocalizationService loc) : base(loc)
    {
        _detector = detector;
        _interop = interop;
        _config = config;
        _launcher = launcher;
        _shell = shell;
        _settings = settings;
        _availability = availability;
        _http = http;
        _activeMethod = settings.Current.LaunchMethod;
        UpdateLaunchModeLabel();
        // A launch-method change made on the Settings page has to reach a Home
        // page that is still alive in the navigation cache.
        settings.Changed += () =>
        {
            _activeMethod = settings.Current.LaunchMethod;
            UpdateLaunchModeLabel();
        };
        // The launch-time update check writes here from its own view-model; re-render
        // so the patch tile shows the result without revisiting the page.
        availability.Changed += Render;
    }

    private int _refreshGeneration;

    /// <summary>Load status tiles, module health, and probe LilyPad. Called on navigation.</summary>
    [RelayCommand]
    private async Task RefreshAsync()
    {
        // A second refresh can start while the first is awaiting IO. Only the newest
        // one may write the tiles, or they end up mixing two probes.
        var generation = ++_refreshGeneration;
        var dir = _settings.Current.GameDir;
        _hasGameDir = !string.IsNullOrWhiteSpace(dir);

        if (!_hasGameDir)
        {
            _state = DeploymentState.NotFound;
            _deployedVersion = null;
            _buildIdValue = null;
            _server = ServerState.Unreachable;
            _health.Clear();
            _healthNote = false;
            Render();
            return;
        }

        // IO (detect, currency fingerprint, config parse, log read) off the UI thread: the
        // currency check hashes the first 4 MiB of GameAssembly.dll and the log can be large.
        var install = await Task.Run(() => {
            ConfigTemplate.Ensure(dir);
            return _detector.Detect(dir!);
        });
        var cfg = await Task.Run(() => TryLoadConfig(install.ConfigPath));
        if (generation != _refreshGeneration)
            return;

        var buildId = await Task.Run(() =>
        {
            try { return _interop.CheckCurrency(dir!, cfg?.InteropDump?.OutDir).BuildId; }
            catch { return null; }
        });

        var logText = await Task.Run(() => TryReadLogTail(LogPath(install, cfg)));
        var server = await ProbeServerAsync(cfg);
        if (generation != _refreshGeneration || dir != _settings.Current.GameDir)
            return;

        _state = install.State;
        _deployedVersion = install.State == DeploymentState.Deployed ? install.DeployedVersion : null;
        _buildIdValue = buildId;
        _server = server;
        _health.Clear();
        if (logText is not null)
        {
            _healthNote = false;
            _health.Add(("hLilypad", logText.Contains("lilypad init") || logText.Contains("[lilypad]") || logText.Contains("REDIR")));
            _health.Add(("hSteam", logText.Contains("PAYMENT_Initialize: enabled") || logText.Contains("steam")));
        }
        else
        {
            // Fallback: reflect the config's enabled modules, flagged as a stand-in.
            _healthNote = true;
            _health.Add(("hLilypad", cfg?.Lilypad.Enabled == true));
            _health.Add(("hSteam", cfg?.Steam.Enabled == true));
        }

        Render();
    }

    /// <summary>The newest per-launch log file (in clientpatch/logs), matching the
    /// [loader] log stem when configured. Null when no log exists yet.</summary>
    private static string? LogPath(GameInstall install, ClientpatchConfig? cfg)
    {
        if (install.GameDir is null)
            return null;
        return ClientpatchManager.Core.Install.InstallLayout.LatestLog(install.GameDir, cfg?.LoaderLog);
    }

    /// <summary>Launch the game with the currently selected method; never crash on missing paths.</summary>
    [RelayCommand]
    private void Launch()
    {
        LaunchError = null;
        var dir = _settings.Current.GameDir ?? "";
        var install = _detector.Detect(dir);
        try
        {
            _launcher.Launch(install, _activeMethod);
        }
        catch (Exception)
        {
            LaunchError = Loc["launchError"];
        }
    }

    /// <summary>Toggle the split button between Steam and Direct launch.</summary>
    [RelayCommand]
    private void ToggleLaunchMethod()
    {
        _activeMethod = _activeMethod == LaunchMethod.Steam ? LaunchMethod.Direct : LaunchMethod.Steam;
        UpdateLaunchModeLabel();
    }

    [RelayCommand]
    private void OpenFolder() => _shell.OpenFolder(_settings.Current.GameDir);

    private void UpdateLaunchModeLabel() =>
        LaunchModeLabel = _activeMethod == LaunchMethod.Steam ? Loc["viaSteam"] : Loc["direct"];

    /// <summary>Apply the raw state to the bound properties in the active language.</summary>
    private void Render()
    {
        if (!_hasGameDir)
        {
            GameStatus = Loc["statusNotFound"];
            PatchVersion = Loc["statusNotDeployed"];
            ServerStatus = Loc["serverUnreachable"];
            BuildId = Loc["valueUnknown"];
            Modules.Clear();
            return;
        }

        GameStatus = _state switch
        {
            DeploymentState.Deployed => Loc["ok"],
            DeploymentState.GameOnly => Loc["statusGameOnly"],
            _ => Loc["statusNotFound"],
        };

        var version = _state == DeploymentState.Deployed
            ? _deployedVersion ?? Loc["valueUnknown"]   // deployed but undetectable version
            : Loc["statusNotDeployed"];
        PatchVersion = _availability.Available && _availability.LatestVersion is not null
            ? $"{version} → {_availability.LatestVersion}"
            : version;

        ServerStatus = _server switch
        {
            ServerState.Off => Loc["off"],
            ServerState.Reachable => Loc["stServerV"],
            _ => Loc["serverUnreachable"],
        };

        BuildId = _buildIdValue ?? Loc["valueUnknown"];

        ShowHealthNote = _healthNote;
        Modules.Clear();
        foreach (var (key, ok) in _health)
            Modules.Add(new ModuleHealthItem(Loc[key], ok ? Loc["ok"] : Loc["off"]));
    }

    /// <summary>Read the log's tail (the file is append-only and unbounded).</summary>
    private static string? TryReadLogTail(string? path)
    {
        try
        {
            if (path is null || !File.Exists(path))
                return null;
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            if (fs.Length > LogTailBytes)
                fs.Seek(-LogTailBytes, SeekOrigin.End);
            using var reader = new StreamReader(fs);
            return reader.ReadToEnd();
        }
        catch
        {
            return null;
        }
    }

    private ClientpatchConfig? TryLoadConfig(string? configPath)
    {
        try
        {
            if (configPath is null || !File.Exists(configPath))
                return null;
            return _config.Load(File.ReadAllText(configPath));
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Probe the configured LilyPad api_base with a short timeout.</summary>
    private async Task<ServerState> ProbeServerAsync(ClientpatchConfig? cfg)
    {
        var apiBase = cfg?.Lilypad.ApiBase;
        if (cfg?.Lilypad.Enabled != true || string.IsNullOrWhiteSpace(apiBase))
        {
            return ServerState.Off;
        }

        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            using var req = new HttpRequestMessage(HttpMethod.Head, apiBase);
            using var resp = await _http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, cts.Token);
            return ServerState.Reachable;
        }
        catch
        {
            return ServerState.Unreachable;
        }
    }

    protected override void OnLanguageChangedCore()
    {
        UpdateLaunchModeLabel();
        // Raw state is cached: re-render in the new language without re-reading anything.
        Render();
    }
}
