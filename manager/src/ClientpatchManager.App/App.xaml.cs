using System.IO;
using System.Net.Http;
using System.Windows;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Navigation;
using ClientpatchManager.App.Services;
using ClientpatchManager.App.Theme;
using ClientpatchManager.App.ViewModels;
using ClientpatchManager.App.Views;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Feed;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Plugins;
using ClientpatchManager.Core.Settings;
using ClientpatchManager.Core.Update;
using Microsoft.Extensions.DependencyInjection;
using Wpf.Ui.Abstractions;
using GitHubClient = Octokit.GitHubClient;
using IGitHubClient = Octokit.IGitHubClient;
using ProductHeaderValue = Octokit.ProductHeaderValue;

namespace ClientpatchManager.App;

/// <summary>
/// Composition root: builds the DI container, wires every Core service, loads
/// persisted settings, applies the saved UI language, parses startup args, and
/// shows the appropriate window.
/// </summary>
public partial class App : Application
{
    private IServiceProvider? _services;

    /// <summary>The built service provider (available after startup).</summary>
    public IServiceProvider Services =>
        _services ?? throw new InvalidOperationException("Service provider not initialized.");

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        if (e.Args.FirstOrDefault() is "--apply-clientpatch-update" or "--restore-clientpatch-backup") {
            ShutdownMode = ShutdownMode.OnExplicitShutdown;
            ApplyBundleUpdate(e.Args);
            return;
        }

        // A faulted async command rethrows on the UI context (CommunityToolkit AwaitAndThrowIfFailed);
        // without a handler that terminates the process. Log and keep the manager alive.
        DispatcherUnhandledException += (_, args) =>
        {
            args.Handled = true;
            try
            {
                File.AppendAllText(
                    Path.Combine(DefaultSettingsDir(), "manager-errors.log"),
                    $"[{DateTimeOffset.Now:O}] {args.Exception}\n\n");
            }
            catch { /* logging must never throw here */ }
        };

        var settingsDir = DefaultSettingsDir();
        var settingsStore = new SettingsStore(settingsDir);
        var settings = settingsStore.Load();

        _services = BuildServiceProvider(settingsStore, settings);

        // Settings edits to the release feed take effect immediately, not on next launch.
        var settingsState = _services.GetRequiredService<SettingsState>();
        settingsState.Changed += () =>
        {
            var (owner, repo) = ParseFeedRepo(settingsState.Current.FeedRepo);
            _services.GetRequiredService<GitHubReleaseFeed>().SetEndpoint(owner, repo);
        };

        // Apply the saved UI language (null => follow the system culture).
        var loc = _services.GetRequiredService<LocalizationService>();
        if (!string.IsNullOrWhiteSpace(settings.Language))
            loc.SetLanguage(settings.Language);

        // A handoff from clientpatch (missing interop, missing payload, or a clean-boot
        // relaunch). This is a separate window, not the main shell: finish the job, relaunch
        // the game, then exit this instance.
        var setup = SetupArgs.Parse(e.Args);
        if (setup is not null)
        {
            ShowHandoff(settings, setup);
            return;
        }

        // Resolve the shell and apply the saved theme in two phases: now, before the
        // first render, so the theme dictionaries are in place; and again from
        // SourceInitialized, once an HWND exists for the Mica backdrop and the
        // system-theme watcher. A single post-Show apply leaves the chrome black.
        var main = _services.GetRequiredService<MainWindow>();
        MainWindow = main;
        var themes = _services.GetRequiredService<ThemeService>();
        themes.Apply(settings.Theme);
        main.SourceInitialized += (_, _) => themes.Apply(settings.Theme);
        // Self-heal a first-composition glitch (chrome stays un-blended until a redraw):
        // re-apply just the backdrop once the first frame is up.
        main.ContentRendered += (_, _) => themes.ReapplyBackdrop();
        main.Show();

        // "Check for updates on launch" checks in the background. It must not navigate:
        // the window opens on Home, and the result surfaces there through UpdateAvailability.
        if (settings.CheckOnLaunch)
        {
            var updates = _services.GetRequiredService<UpdatesViewModel>();
            Dispatcher.BeginInvoke(async () =>
            {
                try { await updates.RefreshCommand.ExecuteAsync(null); }
                catch { /* a failed background check is silent; the page reports on demand */ }
            });
        }
    }

    private async void ApplyBundleUpdate(string[] args) {
        try {
            if (args.Length != 5 || !int.TryParse(args[4], out var parentId))
                throw new ArgumentException("Invalid bundle update handoff.");
            if (!string.Equals(Path.GetFullPath(args[2]).TrimEnd(Path.DirectorySeparatorChar),
                Path.GetFullPath(AppContext.BaseDirectory).TrimEnd(Path.DirectorySeparatorChar), StringComparison.OrdinalIgnoreCase))
                throw new ArgumentException("Bundle update must run from its staging directory.");
            System.Diagnostics.Process? parent = null;
            try { parent = System.Diagnostics.Process.GetProcessById(parentId); }
            catch (ArgumentException) { /* parent already exited */ }
            if (parent is not null) {
                using (parent) {
                    using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(60));
                    await parent.WaitForExitAsync(timeout.Token);
                }
            }
            if (args[0] == "--restore-clientpatch-backup")
                new BackupService().Restore(args[1], args[3]);
            else
                UpdateInstaller.ApplyClientpatchBundle(args[1], args[2], args[3], new BackupService());
            var manager = Path.Combine(args[1], "manager.exe");
            if (File.Exists(manager))
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(manager) {
                    UseShellExecute = true, WorkingDirectory = args[1]
                })?.Dispose();
            Shutdown(0);
        }
        catch (Exception ex) {
            MessageBox.Show(ex.Message, "clientpatch update failed", MessageBoxButton.OK, MessageBoxImage.Error);
            Shutdown(2);
        }
    }

    /// <summary>
    /// Shows the handoff progress window. On success the process exits 0, after the relaunch
    /// has been started, so this instance does not stay open beside the restarted game.
    /// On failure the window stays so the error is readable; closing it exits 1.
    /// A generate handoff with no dump dir is itself a failure (exit code 2).
    /// </summary>
    private void ShowHandoff(ManagerSettings settings, SetupRequest request)
    {
        if (request.Kind == SetupKind.Generate && string.IsNullOrWhiteSpace(request.DumpDir))
        {
            Shutdown(2);
            return;
        }

        var loc = Services.GetRequiredService<LocalizationService>();
        // Prefer the explicit --game-dir clientpatch passes (it always knows the game dir)
        // over the manager's persisted setting, which may be empty on a fresh install.
        if (string.IsNullOrWhiteSpace(request.GameDir) && !string.IsNullOrWhiteSpace(settings.GameDir))
            request = request with { GameDir = settings.GameDir };
        var vm = new HandoffViewModel(
            Services.GetRequiredService<IInteropService>(),
            Services.GetRequiredService<IPayloadInstaller>(),
            Services.GetRequiredService<ILauncher>(),
            Services.GetRequiredService<IInstallDetector>(),
            loc,
            request);

        var window = new HandoffWindow(vm);
        MainWindow = window;

        // Same two-phase theme apply as the main shell (see OnStartup).
        var themes = Services.GetRequiredService<ThemeService>();
        themes.SetWindow(window);
        themes.Apply(settings.Theme);
        window.SourceInitialized += (_, _) => themes.Apply(settings.Theme);
        window.ContentRendered += (_, _) => themes.ReapplyBackdrop();

        // On success the process exits 0 (and the relaunch has already been kicked off).
        // On failure the window STAYS so the error is readable; closing it exits 1.
        vm.Completed += success => Dispatcher.BeginInvoke(
            System.Windows.Threading.DispatcherPriority.ApplicationIdle, () =>
        {
            // ApplicationIdle, not Normal: the step/console updates are posted at Normal
            // and must paint before the window closes, or a fast run shows nothing.
            if (success)
                Shutdown(0);
            else
                Environment.ExitCode = 1;
        });

        // Loaded can fire inside Show(). Subscribing afterwards misses it, and the
        // window then sits on the idle steps forever (the game is already closed).
        var started = false;
        void Start()
        {
            if (started)
                return;
            started = true;
            window.Dispatcher.BeginInvoke(async () => await vm.RunAsync());
        }
        window.Loaded += (_, _) => Start();
        window.Show();
        Start();
    }

    /// <summary>
    /// Builds and populates the DI container. Registered as singletons since the
    /// manager is a single-window desktop app with process-lifetime services.
    /// </summary>
    private static ServiceProvider BuildServiceProvider(SettingsStore settingsStore, ManagerSettings settings)
    {
        var (owner, repo) = ParseFeedRepo(settings.FeedRepo);

        var sc = new ServiceCollection();

        // Persistence + settings. SettingsState is the shared, mutable holder the
        // pages read/write; the immutable snapshot stays registered for startup wiring.
        sc.AddSingleton(settingsStore);
        sc.AddSingleton(settings);
        sc.AddSingleton(sp => new SettingsState(
            sp.GetRequiredService<SettingsStore>(), sp.GetRequiredService<ManagerSettings>()));

        // App-layer shell affordances (folder pick / open).
        sc.AddSingleton<ShellService>();

        // Shared launch-time update-check result (written by UpdatesViewModel, read by Home).
        sc.AddSingleton<UpdateAvailability>();

        // Shared infrastructure.
        sc.AddSingleton(new HttpClient());
        sc.AddSingleton<IGitHubClient>(_ => new GitHubClient(new ProductHeaderValue("ClientpatchManager")));

        // Task 3: install detection.
        sc.AddSingleton<IInstallDetector, InstallDetector>();

        // Task 5: clientpatch config.
        sc.AddSingleton<IConfigService, ConfigService>();

        // Task 6: backups.
        sc.AddSingleton<IBackupService, BackupService>();

        // Task 7: GitHub release feed (owner/repo derived from FeedRepo; endpoint is
        // re-pointed live when Settings changes it).
        sc.AddSingleton<GitHubReleaseFeed>(sp =>
            new GitHubReleaseFeed(sp.GetRequiredService<IGitHubClient>(), owner, repo));
        sc.AddSingleton<IReleaseFeed>(sp => sp.GetRequiredService<GitHubReleaseFeed>());

        // Task 8: downloader + update installer.
        sc.AddSingleton<IFileDownloader>(sp => new HttpFileDownloader(sp.GetRequiredService<HttpClient>()));
        sc.AddSingleton<IUpdateInstaller>(sp =>
            new UpdateInstaller(sp.GetRequiredService<IFileDownloader>(), sp.GetRequiredService<IBackupService>()));

        // Task 9: plugin management.
        sc.AddSingleton<IPluginService, PluginService>();

        // Tasks 11/12: interop tool cache, runner, and service. Tools come from their own
        // upstream repositories (IToolReleaseSource), independent of the clientpatch feed.
        sc.AddSingleton<IToolRunner, ProcessToolRunner>();
        sc.AddSingleton<IToolReleaseSource>(sp =>
            new GitHubToolReleaseSource(sp.GetRequiredService<IGitHubClient>()));
        sc.AddSingleton<IToolCache>(sp => new ToolCache(
            sp.GetRequiredService<IFileDownloader>(),
            sp.GetRequiredService<IToolReleaseSource>()));
        sc.AddSingleton<IInteropService>(sp =>
            new InteropService(sp.GetRequiredService<IToolRunner>(), sp.GetRequiredService<IToolCache>()));

        // Task 13: launcher. Payload install is the handoff for a missing BepInEx zip.
        sc.AddSingleton<ILauncher, Launcher>();
        sc.AddSingleton<IPayloadInstaller>(sp =>
            new PayloadInstaller(sp.GetRequiredService<IFileDownloader>()));

        // Task 14: localization.
        sc.AddSingleton<LocalizationService>();

        // Task 16: shell, theme, navigation, and placeholder pages.
        sc.AddSingleton<ThemeService>();
        sc.AddSingleton<INavigationViewPageProvider, DependencyInjectionPageProvider>();
        sc.AddSingleton<MainViewModel>();
        sc.AddSingleton<MainWindow>();

        // Task 17: page view-models. Transient so each navigation gets fresh state.
        sc.AddTransient<HomeViewModel>();
        sc.AddTransient<SettingsViewModel>();
        sc.AddTransient<ModulesViewModel>();
        sc.AddTransient<UpdatesViewModel>();
        sc.AddTransient<PluginsViewModel>();
        sc.AddTransient<InteropViewModel>();
        sc.AddTransient<LogsViewModel>();

        // Navigation destinations. Transient so each navigation gets a fresh page instance.
        sc.AddTransient<HomePage>();
        sc.AddTransient<UpdatesPage>();
        sc.AddTransient<ModulesPage>();
        sc.AddTransient<PluginsPage>();
        sc.AddTransient<InteropPage>();
        sc.AddTransient<LogsPage>();
        sc.AddTransient<SettingsPage>();

        return sc.BuildServiceProvider();
    }

    /// <summary>
    /// Splits a <c>owner/repo</c> feed string into its parts. Falls back to the
    /// default repo's parts when the value is empty or malformed.
    /// </summary>
    internal static (string Owner, string Repo) ParseFeedRepo(string feedRepo)
    {
        if (!string.IsNullOrWhiteSpace(feedRepo))
        {
            var parts = feedRepo.Split('/', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
            if (parts.Length == 2)
                return (parts[0], parts[1]);
        }

        var defParts = ManagerSettings.DefaultFeedRepo.Split('/');
        return (defParts[0], defParts[1]);
    }

    private static string DefaultSettingsDir() =>
        Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "ClientpatchManager");
}
