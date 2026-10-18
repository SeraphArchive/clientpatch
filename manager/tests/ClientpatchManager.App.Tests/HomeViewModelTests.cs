using System.IO;
using System.Net;
using System.Net.Http;
using ClientpatchManager.App.I18n;
using ClientpatchManager.App.Services;
using ClientpatchManager.App.ViewModels;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Models;
using ClientpatchManager.Core.Settings;

public sealed class HomeViewModelTests : IDisposable {
    readonly string game = Path.Combine(Path.GetTempPath(), "cpm-home-test-" + Guid.NewGuid());
    string ConfigPath => Path.Combine(game, "clientpatch", "clientpatch.toml");
    const string PrivateConfig = "[lilypad]\nenable=true\napi_base='http://localhost/'\nplatform_base='http://localhost/'";

    public HomeViewModelTests() {
        Directory.CreateDirectory(Path.Combine(game, "clientpatch"));
        File.WriteAllText(Path.Combine(game, "GameAssembly.dll"), "fixture");
        File.WriteAllText(ConfigPath, PrivateConfig);
    }

    public void Dispose() => Directory.Delete(game, true);

    [Fact] public async Task Warning_refreshes_after_config_changes_and_does_not_block_launch() {
        var settings = new SettingsState(new SettingsStore(Path.Combine(game, "settings")), ManagerSettings.Default with { GameDir = game });
        var launcher = new RecordingLauncher();
        var loc = new LocalizationService();
        using var http = new HttpClient(new OfflineHandler());
        var vm = new HomeViewModel(new InstallDetector(), new NoInterop(), new ConfigService(), launcher,
            new ShellService(), settings, new UpdateAvailability(), http, loc);

        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.False(vm.ShowOfficialServerWarning);

        foreach (var text in new[] {
            "[lilypad]\nenable=false",
            PrivateConfig.Replace("api_base='http://localhost/'", "api_base='https://api.heaven-burns-red.wfs.games/'"),
            PrivateConfig.Replace("platform_base='http://localhost/'", "platform_base='https://gl-payment.gree-apps.net/'"),
            "", "[invalid"
        }) {
            File.WriteAllText(ConfigPath, text);
            await vm.RefreshCommand.ExecuteAsync(null);
            Assert.True(vm.ShowOfficialServerWarning);
            Assert.True(vm.LaunchCommand.CanExecute(null));
            vm.LaunchCommand.Execute(null);
            Assert.False(vm.HasLaunchError);
        }
        Assert.Equal(5, launcher.Launches);

        loc.SetLanguage("zh-Hans");
        Assert.True(vm.ShowOfficialServerWarning);
        Assert.Contains("官方服务器", vm["officialServerWarningTitle"]);
        Assert.Contains("EULA", vm["officialServerWarningMessage"]);

        File.WriteAllText(ConfigPath, PrivateConfig);
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.False(vm.ShowOfficialServerWarning);

        File.Delete(ConfigPath);
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.True(vm.ShowOfficialServerWarning);

        settings.Update(settings.Current with { GameDir = null });
        await vm.RefreshCommand.ExecuteAsync(null);
        Assert.False(vm.ShowOfficialServerWarning);
    }

    sealed class RecordingLauncher : ILauncher {
        public int Launches;
        public void Launch(GameInstall install, LaunchMethod method) => Launches++;
    }

    sealed class OfflineHandler : HttpMessageHandler {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct) =>
            Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK));
    }

    sealed class NoInterop : IInteropService {
        public InteropStatus CheckCurrency(string gameDir, string? dumpRootName = null) => new(false, null, 0);
        public Task<IReadOnlyList<StepResult>> GenerateOfflineAsync(string gameDir, string? dumpRootName,
            IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null) => throw new NotSupportedException();
        public Task<IReadOnlyList<StepResult>> GenerateFromDumpAsync(string dumpDir, string? gameDir,
            IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null) => throw new NotSupportedException();
    }
}
