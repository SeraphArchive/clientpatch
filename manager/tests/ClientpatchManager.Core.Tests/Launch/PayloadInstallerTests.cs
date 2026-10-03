using System.IO.Compression;
using ClientpatchManager.Core.Launch;
using ClientpatchManager.Core.Update;

public class PayloadInstallerTests {
    static string Tmp() {
        var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        Directory.CreateDirectory(d);
        return d;
    }

    sealed class ZipDl : IFileDownloader {
        readonly string _zip;
        public ZipDl(string zip) { _zip = zip; }
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct) {
            var p = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName() + ".zip");
            File.Copy(_zip, p);
            return Task.FromResult(p);
        }
    }

    static string MakeZip(Action<ZipArchive> fill) {
        var path = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName() + ".zip");
        using (var zip = ZipFile.Open(path, ZipArchiveMode.Create))
            fill(zip);
        return path;
    }

    [Fact]
    public async Task Install_extracts_and_renames_the_autoload_hijack() {
        var game = Tmp();
        var zip = MakeZip(z => {
            var hijack = z.CreateEntry("winhttp.dll");
            using (var w = new StreamWriter(hijack.Open())) w.Write("doorstop");
            var core = z.CreateEntry("BepInEx/core/BepInEx.Unity.IL2CPP.dll");
            using (var w = new StreamWriter(core.Open())) w.Write("core");
        });

        await new PayloadInstaller(new ZipDl(zip)).InstallAsync(game, "https://example/bepinex.zip", "doorstop.dll", "BepInEx", default);

        Assert.False(File.Exists(Path.Combine(game, "winhttp.dll")));
        Assert.Equal("doorstop", File.ReadAllText(Path.Combine(game, "doorstop.dll")));
        Assert.Equal("core", File.ReadAllText(Path.Combine(game, "BepInEx", "core", PayloadInstaller.CoreAssembly)));
    }

    [Fact]
    public async Task Incomplete_zip_fails_and_does_not_claim_success() {
        var game = Tmp();
        var zip = MakeZip(z => {
            var e = z.CreateEntry("readme.txt");
            using var w = new StreamWriter(e.Open());
            w.Write("nope");
        });
        var ex = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            new PayloadInstaller(new ZipDl(zip)).InstallAsync(game, "https://example/bepinex.zip", "doorstop.dll", "BepInEx", default));
        Assert.Contains("incomplete", ex.Message);
    }

    [Fact]
    public async Task Install_refuses_an_entry_that_escapes_the_game_dir() {
        var game = Tmp();
        var zip = MakeZip(z => {
            var e = z.CreateEntry("../outside.txt");
            using var w = new StreamWriter(e.Open());
            w.Write("escape");
        });
        var ex = await Assert.ThrowsAsync<InvalidOperationException>(() =>
            new PayloadInstaller(new ZipDl(zip)).InstallAsync(game, "https://example/bepinex.zip", "doorstop.dll", "BepInEx", default));
        Assert.Contains("escapes", ex.Message);
        Assert.False(File.Exists(Path.Combine(Directory.GetParent(game)!.FullName, "outside.txt")));
    }
}
