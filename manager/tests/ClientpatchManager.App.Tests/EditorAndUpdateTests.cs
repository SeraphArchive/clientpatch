using System.IO;
using System.Net;
using System.Net.Http;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.App.ViewModels;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Feed;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Plugins;
using ClientpatchManager.Core.Settings;
using ClientpatchManager.Core.Update;

public sealed class EditorAndUpdateTests : IDisposable {
    readonly string game = Path.Combine(Path.GetTempPath(), "cpm-app-test-" + Guid.NewGuid());
    const string Toml = "[loader]\nmodules=['lilypad']\n[lilypad]\napi_base='http://old/'\nplatform_base='http://old'\nsigning='noop'\n";
    string ConfigPath => Path.Combine(game, "clientpatch", "clientpatch.toml");
    public EditorAndUpdateTests() { Directory.CreateDirectory(Path.Combine(game, "clientpatch")); File.WriteAllText(ConfigPath, Toml); }
    public void Dispose() => Directory.Delete(game, true);
    SettingsState Settings() => new(new SettingsStore(Path.Combine(game, "settings")), ManagerSettings.Default with { GameDir = game });
    ModulesViewModel Editor(IBackupService? backup = null) => new(new ConfigService(), backup ?? new BackupService(), new ShellService(), Settings(), new LocalizationService());

    [Fact] public void Settings_initialization_and_install_selection_copy_only_missing_config() {
        File.Delete(ConfigPath);
        var example = Path.Combine(game, "clientpatch", "clientpatch.example.toml");
        File.WriteAllText(example, Toml);
        var settings = Settings();
        Assert.Equal(Toml, File.ReadAllText(ConfigPath));
        File.WriteAllText(example, "new template");
        settings.Update(settings.Current with { Language = "ja" });
        Assert.Equal(Toml, File.ReadAllText(ConfigPath));
        var second = Path.Combine(game, "second-install");
        Directory.CreateDirectory(Path.Combine(second, "clientpatch"));
        File.WriteAllText(Path.Combine(second, "clientpatch", "clientpatch.example.toml"), "second template");
        settings.Update(settings.Current with { GameDir = second });
        Assert.Equal("second template", File.ReadAllText(Path.Combine(second, "clientpatch", "clientpatch.toml")));
    }

    [Fact] public void Missing_example_does_not_invent_a_user_config() {
        File.Delete(ConfigPath);
        Settings();
        Assert.False(File.Exists(ConfigPath));
    }

    [Fact] public void Form_and_raw_tabs_save_all_module_switches_without_loader_array() {
        var vm = Editor();
        vm.LilypadEnabled = false; vm.SteamEnabled = true;
        vm.TitlebarEnabled = true; vm.IsolationEnabled = true;
        vm.BepInExEnabled = true; vm.InteropDumpEnabled = true; vm.ReportEnabled = false;
        vm.ShowTomlTabCommand.Execute(null);
        var service = new ConfigService(); var cfg = service.Load(vm.RawToml);
        Assert.False(cfg.Lilypad.Enabled); Assert.True(cfg.Steam.Enabled);
        Assert.True(cfg.BepInEx!.Enabled); Assert.True(cfg.InteropDump!.Enabled); Assert.False(cfg.Lilypad.Report.Enabled);
        Assert.DoesNotContain("modules=", vm.RawToml.Replace(" ", ""));
        cfg.BepInEx = cfg.BepInEx with { Enabled = false };
        cfg.InteropDump = cfg.InteropDump with { Enabled = false };
        vm.RawToml = service.ApplyToText(vm.RawToml, cfg);
        vm.ShowFormTabCommand.Execute(null);
        Assert.False(vm.BepInExEnabled); Assert.False(vm.InteropDumpEnabled);
        vm.SaveCommand.Execute(null);
        var saved = File.ReadAllText(ConfigPath); var loaded = service.Load(saved);
        Assert.False(loaded.Lilypad.Enabled); Assert.True(loaded.Steam.Enabled);
        Assert.True(loaded.Titlebar.Enabled); Assert.True(loaded.Isolation.Enabled);
        Assert.False(loaded.BepInEx!.Enabled); Assert.False(loaded.InteropDump!.Enabled); Assert.False(loaded.Lilypad.Report.Enabled);
        Assert.DoesNotContain("modules=", saved.Replace(" ", ""));
    }

    [Fact] public void Editing_lilypad_preserves_report_url_and_saves_child_switch() {
        File.WriteAllText(ConfigPath, "[lilypad]\nenable=true\napi_base='http://api/'\nplatform_base='http://platform'\n[lilypad.report]\nenable=true\nurl='http://report/'\n");
        var vm = Editor(); vm.ApiBase = "http://changed/"; vm.ReportEnabled = false;
        vm.SaveCommand.Execute(null);
        var cfg = new ConfigService().Load(File.ReadAllText(ConfigPath));
        Assert.Equal("http://changed/", cfg.Lilypad.ApiBase);
        Assert.False(cfg.Lilypad.Report.Enabled); Assert.Equal("http://report/", cfg.Lilypad.Report.Url);
    }

    [Fact] public void Tabs_share_the_same_draft() {
        var vm = Editor(); vm.ApiBase = "http://form/"; vm.ShowTomlTabCommand.Execute(null);
        Assert.Contains("http://form/", vm.RawToml);
        vm.RawToml = vm.RawToml.Replace("http://form/", "http://raw/"); vm.ShowFormTabCommand.Execute(null);
        Assert.Equal("http://raw/", vm.ApiBase);
        vm.SaveCommand.Execute(null); Assert.Contains("http://raw/", File.ReadAllText(ConfigPath));
    }
    [Fact] public void Invalid_raw_text_cannot_be_hidden_or_saved() {
        var vm = Editor(); vm.ShowTomlTabCommand.Execute(null); vm.RawToml = "[broken";
        vm.ShowFormTabCommand.Execute(null); Assert.False(vm.ShowForm);
        vm.SaveCommand.Execute(null); Assert.Equal(Toml, File.ReadAllText(ConfigPath));
    }
    [Fact] public void Backup_failure_preserves_config_and_draft() {
        var vm = Editor(new BrokenBackup()); vm.ApiBase = "http://new/"; vm.SaveCommand.Execute(null);
        Assert.Equal(Toml, File.ReadAllText(ConfigPath)); Assert.Equal("http://new/", vm.ApiBase); Assert.Contains("backup failure", vm.SaveMessage);
    }
    [Fact] public void External_edits_are_not_overwritten() {
        var vm = Editor(); vm.ApiBase = "http://draft/";
        var external = Toml.Replace("http://old/", "http://external/"); File.WriteAllText(ConfigPath, external);
        vm.SaveCommand.Execute(null); Assert.Equal(external, File.ReadAllText(ConfigPath)); Assert.Equal("http://draft/", vm.ApiBase);
    }
    [Fact] public void Duplicate_plugin_conflict_is_visible_and_keeps_toggle_enabled() {
        File.WriteAllText(ConfigPath,"[bepinex]\nenable=true\nroot='Mods'");
        var plugins=Path.Combine(game,"Mods","plugins"); Directory.CreateDirectory(plugins);
        File.WriteAllText(Path.Combine(plugins,"A.dll"),"new"); File.WriteAllText(Path.Combine(plugins,"A.dll.disabled"),"old");
        var vm=new PluginsViewModel(new PluginService(),new ConfigService(),new ShellService(),Settings(),new LocalizationService(),new BackupService());
        Assert.Single(vm.Plugins); vm.Plugins.Single().Enabled=false;
        Assert.True(vm.Plugins.Single().Enabled); Assert.True(vm.HasError); Assert.Contains("Resolve the duplicate",vm.ErrorMessage);
    }
    [Fact] public async Task Update_snapshots_release_and_chooses_payload_instead_of_checksum() {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.version"), "0.0.0");
        var feed = new MutableFeed(); var installer = new GatedInstaller(); var backup = new BackupService();
        var vm = new UpdatesViewModel(feed, installer, backup, new InstallDetector(), new ConfigService(), Settings(), new UpdateAvailability(), new HttpClient(new ChecksumHandler()), new LocalizationService());
        await vm.RefreshCommand.ExecuteAsync(null);
        var updating = vm.UpdateCommand.ExecuteAsync(null); await installer.Started.Task;
        feed.Version = "2.0.0"; await vm.RefreshCommand.ExecuteAsync(null);
        Assert.Equal(1, feed.Queries); Assert.True(vm.IsBusy);
        installer.Resume.SetResult(); await updating;
        Assert.Equal("payload-1.0.0", installer.AssetUrl); Assert.Equal("1.0.0", installer.Version);
        Assert.Equal(new string('a', 64), installer.Hash);
    }
    [Fact] public void Plugin_toggle_refuses_a_root_changed_since_scan() {
        File.WriteAllText(ConfigPath, "[bepinex]\nenable=true\nroot='Mods'");
        foreach (var root in new[] { "Mods", "OtherMods" }) {
            Directory.CreateDirectory(Path.Combine(game, root, "plugins"));
            File.WriteAllText(Path.Combine(game, root, "plugins", "A.dll"), root);
        }
        var vm = new PluginsViewModel(new PluginService(), new ConfigService(), new ShellService(), Settings(), new LocalizationService(), new BackupService());
        File.WriteAllText(ConfigPath, "[bepinex]\nenable=true\nroot='OtherMods'");
        vm.Plugins.Single().Enabled = false;
        Assert.True(vm.HasError); Assert.True(vm.Plugins.Single().Enabled);
        Assert.True(File.Exists(Path.Combine(game, "Mods", "plugins", "A.dll")));
        Assert.True(File.Exists(Path.Combine(game, "OtherMods", "plugins", "A.dll")));
    }
    [Fact] public void Plugin_toggle_rejects_a_linked_bepinex_root_even_without_backups() {
        File.WriteAllText(ConfigPath, "[bepinex]\nenable=true\nroot='Mods'");
        var external = Path.Combine(game, "external");
        var link = Path.Combine(game, "Mods");
        Directory.CreateDirectory(Path.Combine(link, "plugins"));
        File.WriteAllText(Path.Combine(link, "plugins", "A.dll"), "keep");
        var settings = Settings(); settings.Update(settings.Current with { BackupBeforeChanges = false });
        var vm = new PluginsViewModel(new PluginService(), new ConfigService(), new ShellService(), settings, new LocalizationService(), new BackupService());
        Directory.Move(link, external);
        var plugin = Path.Combine(external, "plugins", "A.dll");
        var command = new System.Diagnostics.ProcessStartInfo("cmd.exe") { UseShellExecute = false, CreateNoWindow = true };
        foreach (var arg in new[] { "/c", "mklink", "/J", link, external }) command.ArgumentList.Add(arg);
        using var process = System.Diagnostics.Process.Start(command)!; process.WaitForExit(); Assert.Equal(0, process.ExitCode);
        try {
            vm.Plugins.Single().Enabled = false;
            Assert.True(vm.HasError); Assert.True(vm.Plugins.Single().Enabled);
            Assert.Equal("keep", File.ReadAllText(plugin)); Assert.False(File.Exists(plugin + ".disabled"));
            vm.RescanCommand.Execute(null);
            Assert.True(vm.HasError); Assert.Empty(vm.Plugins);
            var reopened = new PluginsViewModel(new PluginService(), new ConfigService(), new ShellService(), settings, new LocalizationService(), new BackupService());
            Assert.True(reopened.HasError); Assert.Empty(reopened.Plugins);
        } finally { Directory.Delete(link); }
    }
    [Fact] public async Task Invalid_checksum_aborts_before_install() {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        var feed = new MutableFeed(); var installer = new GatedInstaller();
        var vm = new UpdatesViewModel(feed, installer, new BackupService(), new InstallDetector(), new ConfigService(), Settings(), new UpdateAvailability(), new HttpClient(new ChecksumHandler("invalid")), new LocalizationService());
        await vm.RefreshCommand.ExecuteAsync(null); await vm.UpdateCommand.ExecuteAsync(null);
        Assert.False(installer.Started.Task.IsCompleted); Assert.Contains("checksum", vm.StatusMessage);
    }

    [Fact] public async Task Failed_refresh_discards_previous_update_candidates() {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        var feed = new MutableFeed(); var installer = new GatedInstaller(); var availability = new UpdateAvailability();
        var vm = new UpdatesViewModel(feed, installer, new BackupService(), new InstallDetector(), new ConfigService(), Settings(), availability, new HttpClient(), new LocalizationService());
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.True(vm.UpdateAvailable);
        feed.Fail = true;
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.False(vm.UpdateAvailable); Assert.False(vm.UpdateCommand.CanExecute(null));
        Assert.False(availability.Available); Assert.Empty(vm.Components);
    }

    [Fact] public async Task Changing_feed_after_check_requires_a_new_selection_before_installing() {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        var settings = Settings(); var installer = new GatedInstaller(); var feed = new MutableFeed();
        var vm = new UpdatesViewModel(feed, installer, new BackupService(), new InstallDetector(), new ConfigService(), settings, new UpdateAvailability(), new HttpClient(), new LocalizationService());
        await vm.RefreshCommand.ExecuteAsync(null);
        settings.Update(settings.Current with { FeedRepo = "custom/releases" });
        await vm.UpdateCommand.ExecuteAsync(null);
        Assert.False(installer.Started.Task.IsCompleted); Assert.Equal(2, feed.Queries);
    }

    [Theory]
    [InlineData("2.0.0", "1.0.0", false)]
    [InlineData("1.0.0", "1.0.0-beta.2", false)]
    [InlineData("1.0.0-beta.9", "1.0.0-beta.10", true)]
    [InlineData("1.0.0+old", "1.0.0+new", false)]
    public async Task Update_does_not_offer_a_downgrade(string installed, string available, bool expected) {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        File.WriteAllText(Path.Combine(game, "manager.exe"), "manager");
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.version"), installed);
        var vm = new UpdatesViewModel(new MutableFeed { Version = available }, new GatedInstaller(), new BackupService(), new InstallDetector(), new ConfigService(), Settings(), new UpdateAvailability(), new HttpClient(), new LocalizationService());
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.Equal(expected, vm.UpdateAvailable);
    }

    [Fact] public void Failed_settings_save_keeps_the_current_snapshot() {
        var folder = Path.Combine(game, "blocked-settings");
        File.WriteAllText(folder, "not a directory");
        var initial = ManagerSettings.Default with { GameDir = game };
        var settings = new SettingsState(new SettingsStore(folder), initial);
        var notifications = 0; settings.Changed += () => notifications++;
        Assert.ThrowsAny<IOException>(() => settings.Update(initial with { FeedRepo = "custom/releases" }));
        Assert.Equal(initial, settings.Current); Assert.Equal(0, notifications);
    }

    [Theory]
    [InlineData("1.0", "1.0.0")]
    [InlineData("1.0.0", "invalid")]
    public async Task Uncomparable_versions_are_reported_as_unknown(string installed, string available) {
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "game"); File.WriteAllText(Path.Combine(game, "version.dll"), "old");
        File.WriteAllText(Path.Combine(game, "manager.exe"), "manager");
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.version"), installed);
        var loc = new LocalizationService();
        var vm = new UpdatesViewModel(new MutableFeed { Version = available }, new GatedInstaller(), new BackupService(), new InstallDetector(), new ConfigService(), Settings(), new UpdateAvailability(), new HttpClient(), loc);
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.False(vm.UpdateAvailable); Assert.Equal(loc["valueUnknown"], vm.Components[0].StatusText);
    }
    sealed class BrokenBackup : IBackupService {
        public BackupEntry Backup(string dir, IEnumerable<string> paths) => throw new IOException("backup failure");
        public IReadOnlyList<BackupEntry> List(string dir) => Array.Empty<BackupEntry>(); public void Restore(string dir, string id) => throw new NotSupportedException();
    }
    sealed class MutableFeed : IReleaseFeed {
        public string Version = "1.0.0"; public int Queries; public bool Fail;
        public Task<FeedResult> QueryAsync(Channel channel, CancellationToken ct) { Queries++; if (Fail) return Task.FromResult(new FeedResult(false, [], "offline")); return Task.FromResult(new FeedResult(true, new[] { new ReleaseInfo("clientpatch", Version, channel, new[] {
            new ReleaseAsset($"clientpatch-v{Version}-win-x64.zip.sha256", "http://fake/checksum", 64), new ReleaseAsset($"clientpatch-v{Version}-win-x64.zip", "payload-" + Version, 100)
        }) }, null)); }
    }
    sealed class ChecksumHandler(string? checksum = null) : HttpMessageHandler {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct) => Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(checksum ?? new string('a', 64)) });
    }
    sealed class GatedInstaller : IUpdateInstaller {
        public readonly TaskCompletionSource Started = new(TaskCreationOptions.RunContinuationsAsynchronously), Resume = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public string? AssetUrl, Version, Hash;
        public async Task<SwapResult> InstallClientpatchReleaseAsync(string dir, ReleaseAsset asset, string? hash, string version, CancellationToken ct) {
            AssetUrl = asset.DownloadUrl; Version = version; Hash = hash; Started.SetResult(); await Resume.Task; return new(true, null);
        }
        public Task<SwapResult> InstallAsync(string dir, ReleaseAsset asset, string dest, string? hash, CancellationToken ct) => throw new NotSupportedException();
        public Task<SwapResult> InstallZipAsync(string dir, ReleaseAsset asset, string dest, string? hash, CancellationToken ct) => throw new NotSupportedException();
    }
}
