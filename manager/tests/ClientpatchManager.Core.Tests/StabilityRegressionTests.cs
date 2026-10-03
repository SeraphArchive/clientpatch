using System.IO.Compression;
using System.Reflection;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Config;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Plugins;
using ClientpatchManager.Core.Update;

public sealed class StabilityRegressionTests : IDisposable {
    readonly string root = Path.Combine(Path.GetTempPath(), "cpm-regression-" + Guid.NewGuid());
    public StabilityRegressionTests() => Directory.CreateDirectory(root);
    public void Dispose() => Directory.Delete(root, true);
    string PathAt(string rel) { var p = Path.Combine(root, rel); Directory.CreateDirectory(Path.GetDirectoryName(p)!); return p; }
    sealed class Download(string path) : IFileDownloader {
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct) { var tmp = Path.GetTempFileName(); File.Copy(path, tmp, true); return Task.FromResult(tmp); }
    }
    [Fact] public async Task Update_rollback_restores_actual_game_file() {
        var dll = PathAt("version.dll"); File.WriteAllText(dll, "old");
        var download = PathAt("download"); File.WriteAllText(download, "new");
        var backup = new BackupService();
        Assert.True((await new UpdateInstaller(new Download(download), backup).InstallAsync(root, new("version.dll", "fake", 3), "version.dll", null, default)).Ok);
        backup.Restore(root, backup.List(root).Single().Id);
        Assert.Equal("old", File.ReadAllText(dll));
    }
    [Fact] public async Task Failed_zip_does_not_replace_earlier_files() {
        var first = PathAt("BepInEx/a.dll"); File.WriteAllText(first, "old");
        Directory.CreateDirectory(PathAt("BepInEx/b.dll"));
        var archive = PathAt("bundle.zip");
        using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create)) {
            foreach (var name in new[] { "BepInEx/a.dll", "BepInEx/b.dll" }) { using var writer = new StreamWriter(zip.CreateEntry(name).Open()); writer.Write("new"); }
        }
        var result = await new UpdateInstaller(new Download(archive), new BackupService()).InstallZipAsync(root, new("bundle.zip", "fake", 0), ".", null, default);
        Assert.False(result.Ok); Assert.Equal("old", File.ReadAllText(first));
    }
    [Fact] public void Rsa_without_key_and_wrong_types_are_rejected() {
        var service = new ConfigService();
        Assert.NotEmpty(service.RawValidate("[lilypad]\nenable=true\napi_base='http://a/'\nplatform_base='http://a'\nsigning='rsa'"));
        Assert.NotEmpty(service.RawValidate("[steam]\nmode=12"));
    }
    [Fact] public void Form_can_add_an_absent_section_and_field() {
        var service = new ConfigService(); const string text = "[loader]\nmodules=[]\n";
        var model = service.Load(text); model.Lilypad = new(true, "http://a/", "http://a", "noop");
        var saved = service.ApplyToText(text, model);
        Assert.Contains("[lilypad]", saved); Assert.Equal("http://a/", service.Load(saved).Lilypad.ApiBase);
    }
    [Fact] public void Bepinex_master_switch_is_read() {
        var model = new ConfigService().Load("[loader]\nmodules=['bepinex']\n[bepinex]\nenabled=false\nroot='Mods'");
        Assert.False(model.BepInEx!.Enabled);
    }
    [Fact] public void Disable_with_duplicate_names_reports_conflict() {
        var a = PathAt("plugins/A.dll"); File.WriteAllText(a, "new"); File.WriteAllText(a + ".disabled", "old");
        Assert.Throws<IOException>(() => new PluginService().SetEnabled(root, "A.dll", false));
        Assert.Equal("new", File.ReadAllText(a)); Assert.Equal("old", File.ReadAllText(a + ".disabled"));
    }
    [Fact] public async Task Repair_replaces_doorstop_without_leaving_autoload_dll() {
        File.WriteAllText(PathAt("doorstop.dll"), "old");
        var archive = PathAt("payload.zip");
        using (var zip = ZipFile.Open(archive, ZipArchiveMode.Create)) {
            foreach (var name in new[] { "winhttp.dll", "BepInEx/core/BepInEx.Unity.IL2CPP.dll" }) { using var writer = new StreamWriter(zip.CreateEntry(name).Open()); writer.Write("new"); }
        }
        await new PayloadInstaller(new Download(archive)).InstallAsync(root, "fake", "doorstop.dll", "BepInEx", default);
        Assert.False(File.Exists(PathAt("winhttp.dll"))); Assert.Equal("new", File.ReadAllText(PathAt("doorstop.dll")));
    }
}
