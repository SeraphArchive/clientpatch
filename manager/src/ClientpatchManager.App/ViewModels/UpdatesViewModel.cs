using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using ClientpatchManager.App.I18n;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Feed;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Models;
using ClientpatchManager.Core.Update;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>One component row on the Updates page (clientpatch / BepInEx).</summary>
public sealed class ComponentRow
{
    public ComponentRow(string name, string detail, string statusText, bool meaningful)
    {
        Name = name;
        Detail = detail;
        StatusText = statusText;
        Meaningful = meaningful;
    }

    public string Name { get; }
    public string Detail { get; }
    public string StatusText { get; }
    /// <summary>False dims the row (e.g. BepInEx when its module is off).</summary>
    public bool Meaningful { get; }
}

/// <summary>
/// Updates page VM: compares the deployed versions to the latest releases from
/// <see cref="IReleaseFeed"/> on the selected channel for clientpatch and BepInEx,
/// with BepInEx included only when its module is on,
/// installs what is newer via <see cref="IUpdateInstaller"/>, and rolls back through
/// <see cref="IBackupService"/>. A component whose deployed version cannot be
/// determined says so; nothing is reported "up to date" without a comparison.
/// </summary>
public sealed partial class UpdatesViewModel : LocalizedViewModel
{

    private readonly IReleaseFeed _feed;
    private readonly IUpdateInstaller _installer;
    private readonly IBackupService _backup;
    private readonly IInstallDetector _detector;
    private readonly IConfigService _config;
    private readonly Services.SettingsState _settings;
    private readonly Services.UpdateAvailability _availability;
    private readonly System.Net.Http.HttpClient _http;

    [ObservableProperty] private bool _channelStable = true;
    [ObservableProperty] private string _currentVersion = "";
    [ObservableProperty] private string _latestVersion = "";
    [ObservableProperty] private bool _updateAvailable;
    [ObservableProperty] private string _statusMessage = "";
    [ObservableProperty] private bool _isBusy;
    [ObservableProperty] private bool _hasBackups;
    [ObservableProperty] private string _backupSummary = "";
    [ObservableProperty] private bool _hasComponents;

    private ReleaseInfo? _latestClientpatch;
    private ReleaseInfo? _latestBepinex;
    private bool _clientpatchNeedsUpdate;
    private bool _bepinexNeedsUpdate;
    private int _refreshGeneration;
    private bool _updating;
    private string? _selectionDir;
    private string? _selectionFeed;
    private bool _selectionStable;

    public ObservableCollection<ComponentRow> Components { get; } = new();

    public UpdatesViewModel(
        IReleaseFeed feed,
        IUpdateInstaller installer,
        IBackupService backup,
        IInstallDetector detector,
        IConfigService config,
        Services.SettingsState settings,
        Services.UpdateAvailability availability,
        System.Net.Http.HttpClient http,
        LocalizationService loc) : base(loc)
    {
        _feed = feed;
        _installer = installer;
        _backup = backup;
        _detector = detector;
        _config = config;
        _settings = settings;
        _availability = availability;
        _http = http;
    }

    partial void OnChannelStableChanged(bool value) => _ = ReloadFeedAsync();

    [RelayCommand] private void ChooseStable() => ChannelStable = true;
    [RelayCommand] private void ChooseBeta() => ChannelStable = false;

    /// <summary>Query the feed for the selected channel and rebuild the component list.</summary>
    [RelayCommand]
    private Task RefreshAsync() => ReloadFeedAsync();

    /// <summary>
    /// The refresh body, separate from the command so <see cref="UpdateAsync"/> can
    /// call it. Awaiting the command from inside itself deadlocks: the command waits
    /// on the task that is currently running.
    /// </summary>
    private async Task ReloadFeedAsync()
    {
        if (_updating) return;
        var generation = ++_refreshGeneration;
        var dir = _settings.Current.GameDir;
        var feed = _settings.Current.FeedRepo;
        var stable = ChannelStable;

        ClearCandidates();
        StatusMessage = Loc["verChecking"];
        IsBusy = true;
        try
        {
            var install = string.IsNullOrWhiteSpace(dir) ? null : _detector.Detect(dir!);
            CurrentVersion = install?.DeployedVersion ?? Loc["valueUnknown"];
            var channel = stable ? Channel.Stable : Channel.Beta;
            var result = await _feed.QueryAsync(channel, CancellationToken.None);
            if (generation != _refreshGeneration || _updating || dir != _settings.Current.GameDir
                || feed != _settings.Current.FeedRepo || stable != ChannelStable) return;
            if (!result.Ok)
            {
                StatusMessage = Loc["verError"];
                LatestVersion = Loc["valueUnknown"];
                _availability.Set(null, false);
                return;
            }

            StatusMessage = result.Releases.Count == 0 ? Loc["verNoReleases"] : "";
            _selectionDir = dir;
            _selectionFeed = feed;
            _selectionStable = stable;
            BuildComponents(result.Releases, install);
        }
        catch
        {
            if (generation != _refreshGeneration || _updating) return;
            ClearCandidates();
            StatusMessage = Loc["verError"];
            LatestVersion = Loc["valueUnknown"];
            _availability.Set(null, false);
        }
        finally
        {
            if (generation == _refreshGeneration && !_updating) { IsBusy = false; RefreshBackups(); }
        }
    }

    private void ClearCandidates()
    {
        _latestClientpatch = null;
        _latestBepinex = null;
        _clientpatchNeedsUpdate = false;
        _bepinexNeedsUpdate = false;
        UpdateAvailable = false;
        Components.Clear();
        HasComponents = false;
        LatestVersion = Loc["valueUnknown"];
        _availability.Set(null, false);
    }

    private void BuildComponents(IReadOnlyList<ReleaseInfo> releases, GameInstall? install)
    {
        Components.Clear();

        _latestClientpatch = ReleaseSelection.Latest(releases, "clientpatch");
        _latestBepinex = ReleaseSelection.Latest(releases, "bepinex");
        LatestVersion = _latestClientpatch?.Version ?? Loc["valueUnknown"];

        var deployed = install is not null && install.State == DeploymentState.Deployed;

        // clientpatch: only updateable on a deployed install (the manager never does first-time setup).
        var cpCurrent = deployed ? install!.DeployedVersion : null;
        var cpOrder = ReleaseSelection.CompareVersions(_latestClientpatch?.Version, cpCurrent);
        var cpReleaseKnown = ReleaseSelection.CompareVersions(_latestClientpatch?.Version, _latestClientpatch?.Version).HasValue;
        _clientpatchNeedsUpdate = deployed && cpReleaseKnown
            && (string.IsNullOrWhiteSpace(cpCurrent)
                || cpOrder is > 0);
        Components.Add(new ComponentRow(
            Loc["compPatch"],
            $"{cpCurrent ?? Loc["valueUnknown"]} → {_latestClientpatch?.Version ?? Loc["valueUnknown"]}",
            StatusText(_clientpatchNeedsUpdate, deployed && cpReleaseKnown
                && (string.IsNullOrWhiteSpace(cpCurrent) || cpOrder.HasValue)),
            meaningful: true));

        // BepInEx: only meaningful when its module is enabled and a core dll is present.
        var bepinOn = deployed && IsBepinexEnabled(install!);
        var bepinCurrent = bepinOn ? ReadBepinexVersion(install!.GameDir) : null;
        _bepinexNeedsUpdate = bepinOn && _latestBepinex is not null
            && !BepinexVersionsMatch(bepinCurrent, _latestBepinex.Version);
        Components.Add(new ComponentRow(
            Loc["compBepin"],
            bepinOn
                ? $"{bepinCurrent ?? Loc["valueUnknown"]} → {_latestBepinex?.Version ?? Loc["valueUnknown"]}"
                : Loc["bepinNote"],
            bepinOn
                ? StatusText(_bepinexNeedsUpdate, _latestBepinex is not null)
                : Loc["bepinNote"],
            meaningful: bepinOn));

        UpdateAvailable = _clientpatchNeedsUpdate || _bepinexNeedsUpdate;
        _availability.Set(_latestClientpatch?.Version, UpdateAvailable);
        HasComponents = true;
    }

    private string StatusText(bool needsUpdate, bool comparable) =>
        !comparable ? Loc["valueUnknown"]
        : needsUpdate ? Loc["needUpdate"]
        : Loc["upToDate"];

    /// <summary>Only an exact installed build identity establishes currency.</summary>
    private static bool BepinexVersionsMatch(string? deployed, string latest)
    {
        if (string.IsNullOrWhiteSpace(deployed))
            return false;
        return string.Equals(deployed, latest, StringComparison.OrdinalIgnoreCase);
    }

    private bool IsBepinexEnabled(GameInstall install)
    {
        try
        {
            if (install.ConfigPath is null || !File.Exists(install.ConfigPath))
                return false;
            var cfg = _config.Load(File.ReadAllText(install.ConfigPath));
            return cfg.BepInEx?.Enabled == true;
        }
        catch
        {
            return false;
        }
    }

    private static string? ReadSidecar(string gameDir, string name)
    {
        try
        {
            var path = ClientpatchManager.Core.Install.InstallLayout.Resolve(gameDir, name);
            if (!File.Exists(path))
                return null;
            var text = File.ReadAllText(path).Trim();
            return text.Length == 0 ? null : text;
        }
        catch
        {
            return null;
        }
    }

    private string? ReadBepinexVersion(string gameDir)
    {
        try
        {
            var path = InstallLayout.Resolve(gameDir, "clientpatch.toml");
            var root = File.Exists(path) ? _config.Load(File.ReadAllText(path)).BepInEx?.Root ?? "BepInEx" : "BepInEx";
            var core = Path.Combine(gameDir, root, "core", "BepInEx.Core.dll");
            if (!File.Exists(core))
                return null;
            var recorded = ReadSidecar(gameDir, "bepinex.version");
            if (recorded is not null) return recorded;
            var info = FileVersionInfo.GetVersionInfo(core);
            var v = info.ProductVersion ?? info.FileVersion;
            return string.IsNullOrWhiteSpace(v) ? null : v;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Install every component that has an update (BepInEx, then the clientpatch bundle when
    /// its module is on). Backups happen inside the installer.</summary>
    [RelayCommand(CanExecute = nameof(CanUpdate))]
    private async Task UpdateAsync()
    {
        var dir = _settings.Current.GameDir;
        if (_updating || IsBusy || string.IsNullOrWhiteSpace(dir))
            return;
        if (_selectionDir != dir || _selectionFeed != _settings.Current.FeedRepo || _selectionStable != ChannelStable)
        {
            await ReloadFeedAsync();
            return;
        }

        var clientpatch = _clientpatchNeedsUpdate ? _latestClientpatch : null;
        var bepinex = _bepinexNeedsUpdate ? _latestBepinex : null;
        _updating = true;
        ++_refreshGeneration;
        IsBusy = true;
        StatusMessage = Loc["verUpdating"];
        var failures = new List<string>();
        try
        {
            if (bepinex is not null)
            {
                var bepAsset = ReleaseAssets.Select(bepinex);
                var bepHash = await TryFetchSidecarSha256Async(bepinex, bepAsset, CancellationToken.None);
                var bepResult = await _installer.InstallZipReleaseAsync(dir!, bepAsset, ".", bepHash, bepinex.Version, CancellationToken.None);
                if (!bepResult.Ok) failures.Add($"BepInEx: {bepResult.Error}");
            }

            if (clientpatch is not null)
            {
                var cpAsset = ReleaseAssets.Select(clientpatch);
                var hash = await TryFetchSidecarSha256Async(clientpatch, cpAsset, CancellationToken.None);
                var r = await _installer.InstallClientpatchReleaseAsync(dir!, cpAsset, hash, clientpatch.Version, CancellationToken.None);
                if (!r.Ok)
                    failures.Add($"clientpatch: {r.Error}");
                if (r.RestartRequired) {
                    System.Windows.Application.Current.Shutdown();
                    return;
                }
            }

        }
        catch (Exception ex)
        {
            failures.Add(ex.Message);
        }
        finally
        {
            _updating = false;
            IsBusy = false;
            RefreshBackups();
        }
        await ReloadFeedAsync();
        StatusMessage = failures.Count == 0 ? Loc["modSaved"] : string.Join("; ", failures);
    }

    private bool CanUpdate() => UpdateAvailable && !IsBusy;

    /// <summary>
    /// Looks for a sidecar <c>&lt;assetName&gt;.sha256</c> asset in the same release and, if present,
    /// fetches its contents and extracts the hex digest. Returns null when no sidecar is available
    /// (the honest "no hash to verify against" case). A present but unreadable/invalid checksum
    /// stops installation. The sidecar body may be a bare hex digest or the common
    /// "&lt;hash&gt;  &lt;filename&gt;" shasum format; we take the first token.
    /// </summary>
    private async Task<string?> TryFetchSidecarSha256Async(ReleaseInfo release, ReleaseAsset asset, CancellationToken ct)
    {
        try
        {
            var sidecar = release.Assets
                .FirstOrDefault(a => string.Equals(a.Name, asset.Name + ".sha256", StringComparison.OrdinalIgnoreCase));
            if (sidecar is null) return null;
            if (string.IsNullOrWhiteSpace(sidecar.DownloadUrl)) throw new InvalidDataException("Release checksum URL is empty.");

            var body = await _http.GetStringAsync(sidecar.DownloadUrl, ct).ConfigureAwait(false);
            var token = body.Trim().Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault();
            if (string.IsNullOrWhiteSpace(token))
                throw new InvalidDataException("Release checksum is empty.");

            // A SHA-256 hex digest is 64 chars; reject anything that isn't a clean hex string so a
            // stray HTML error page never becomes a bogus "hash" that fails every install.
            token = token.Trim();
            if (token.Length != 64 || !token.All(Uri.IsHexDigit))
                throw new InvalidDataException("Release checksum is invalid.");
            return token;
        }
        catch (Exception ex) { throw new IOException("Could not verify the release checksum; installation was stopped.", ex); }
    }

    /// <summary>Restore the most recent backup.</summary>
    [RelayCommand]
    private void Rollback()
    {
        if (IsBusy || _updating) return;
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir))
            return;
        try
        {
            var latest = _backup.List(dir!).OrderByDescending(b => b.TakenAt).FirstOrDefault();
            if (latest is null)
                return;
            if (_backup.RestoreWithHandoff(dir!, latest.Id))
            {
                System.Windows.Application.Current.Shutdown();
                return;
            }
            StatusMessage = Loc["modSaved"];
            _ = ReloadFeedAsync();
        }
        catch (Exception ex)
        {
            StatusMessage = $"{Loc["verError"]}: {ex.Message}";
        }
    }

    private void RefreshBackups()
    {
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir))
        {
            HasBackups = false;
            BackupSummary = Loc["verNoBackups"];
            return;
        }
        try
        {
            var list = _backup.List(dir!);
            HasBackups = list.Count > 0;
            BackupSummary = HasBackups ? Loc["backupNote"] : Loc["verNoBackups"];
        }
        catch
        {
            HasBackups = false;
            BackupSummary = Loc["verNoBackups"];
        }
    }

    partial void OnUpdateAvailableChanged(bool value) => UpdateCommand.NotifyCanExecuteChanged();
    partial void OnIsBusyChanged(bool value) => UpdateCommand.NotifyCanExecuteChanged();
}
