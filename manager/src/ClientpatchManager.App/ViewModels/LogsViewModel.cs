using System.IO;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ClientpatchManager.App.ViewModels;

/// <summary>
/// Logs page VM: tails the newest per-launch log (clientpatch/logs) into a text view,
/// with an auto-scroll toggle, a clear-view action (view only, never touches the file),
/// and open-file. A lightweight timer re-reads the file so new lines appear while open;
/// a fresh launch is picked up by resolving the newest log again on each poll.
/// </summary>
public sealed partial class LogsViewModel : LocalizedViewModel
{
    /// <summary>Cap the tail so a large log never bloats the UI.</summary>
    private const int MaxChars = 200_000;

    private readonly ShellService _shell;
    private readonly ClientpatchManager.Core.Config.IConfigService _config;
    private readonly Services.SettingsState _settings;
    private readonly System.Windows.Threading.DispatcherTimer _timer;
    private long _lastLength = -1;
    private string? _lastPath;
    private string? _logPath;

    [ObservableProperty] private string _logText = "";
    [ObservableProperty] private bool _autoScroll = true;
    [ObservableProperty] private bool _isEmpty;

    /// <summary>Raised after the log text updates so the view can scroll to the end.</summary>
    public event Action? ScrollToEndRequested;

    public LogsViewModel(
        ShellService shell,
        ClientpatchManager.Core.Config.IConfigService config,
        Services.SettingsState settings,
        LocalizationService loc) : base(loc)
    {
        _shell = shell;
        _config = config;
        _settings = settings;
        _timer = new System.Windows.Threading.DispatcherTimer
        {
            Interval = TimeSpan.FromSeconds(1),
        };
        _timer.Tick += (_, _) => Reload(force: false);
        ResolveLogPath();
        Reload(force: true);
    }

    /// <summary>The newest per-launch log file in clientpatch/logs, matching the
    /// [loader] log stem when the config names one. Null when none exists yet.</summary>
    private void ResolveLogPath()
    {
        var dir = _settings.Current.GameDir;
        if (string.IsNullOrWhiteSpace(dir))
        {
            _logPath = null;
            return;
        }
        string? stem = null;
        try
        {
            var configPath = ClientpatchManager.Core.Install.InstallLayout.Resolve(dir!, "clientpatch.toml");
            if (File.Exists(configPath))
            {
                var configured = _config.Load(File.ReadAllText(configPath)).LoaderLog;
                if (!string.IsNullOrWhiteSpace(configured))
                    stem = configured;
            }
        }
        catch { /* keep the default stem */ }
        _logPath = ClientpatchManager.Core.Install.InstallLayout.LatestLog(dir!, stem);
    }

    /// <summary>Start polling the log (called when the page is shown).</summary>
    public void Start()
    {
        ResolveLogPath();
        Reload(force: true);
        _timer.Start();
    }

    /// <summary>Stop polling (called when the page is hidden/unloaded).</summary>
    public void Stop() => _timer.Stop();

    [RelayCommand]
    private void Refresh() => Reload(force: true);

    /// <summary>Clear the on-screen view only; the log file is never modified.</summary>
    [RelayCommand]
    private void Clear()
    {
        LogText = "";
        IsEmpty = true;
    }

    [RelayCommand]
    private void OpenFile() => _shell.OpenFile(_logPath);

    private void Reload(bool force)
    {
        // A fresh game launch starts a new log file; resolve the newest on every
        // poll so the view follows into it (and reset the length tracker).
        ResolveLogPath();
        var path = _logPath;
        if (path is null || !File.Exists(path))
        {
            IsEmpty = true;
            LogText = "";
            _lastPath = null;
            _lastLength = -1;
            return;
        }
        if (!string.Equals(path, _lastPath, StringComparison.OrdinalIgnoreCase))
        {
            _lastPath = path;
            _lastLength = -1;
            force = true;
        }

        try
        {
            var info = new FileInfo(path);
            // Skip re-reading when the file has not grown since last poll.
            if (!force && info.Length == _lastLength)
                return;
            _lastLength = info.Length;

            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            if (fs.Length > MaxChars)
                fs.Seek(-MaxChars, SeekOrigin.End);
            using var reader = new StreamReader(fs);
            var text = reader.ReadToEnd();

            IsEmpty = text.Length == 0;
            LogText = text;
            if (AutoScroll)
                ScrollToEndRequested?.Invoke();
        }
        catch
        {
            // Transient IO (log being written); leave the current text in place.
        }
    }

}
