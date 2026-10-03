using System.IO.Compression;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Update;
using Xunit;

public class ToolCacheTests {
    static string Tmp() { var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }

    [Fact] public void Uncached_tool_reports_false() {
        var cache = new ToolCache(Tmp());
        Assert.False(cache.IsCached(InteropTool.Senbei));
    }

    [Fact] public void Ready_marker_makes_it_cached() {
        var root = Tmp();
        var dir = Path.Combine(root, "tools", "Senbei", "v1.0.0");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, ".ready"), "");
        Assert.True(new ToolCache(root).IsCached(InteropTool.Senbei));
    }

    // ---- EnsureAsync against a fake upstream source + downloader -------------

    sealed class FakeSource : IToolReleaseSource {
        public string Version = "v1.0.0";
        public Task<ToolRelease> LatestAsync(InteropTool tool, CancellationToken ct) =>
            Task.FromResult(new ToolRelease(Version, "http://x/tool.zip"));
    }

    sealed class FakeDownloader : IFileDownloader {
        public bool Fail;
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct) {
            if (Fail) throw new HttpRequestException("simulated outage");
            var p = Path.GetTempFileName();
            using (var zip = new ZipArchive(File.Create(p), ZipArchiveMode.Create)) {
                // Upstream zip layout: a nested folder holding the executable.
                var entry = zip.CreateEntry("senbei-1.0.0-x86_64-pc-windows-msvc/senbei.exe");
                using var w = new StreamWriter(entry.Open());
                w.Write("tool-bytes");
            }
            return Task.FromResult(p);
        }
    }

    [Fact] public async Task Ensure_downloads_into_release_version_dir() {
        var root = Tmp();
        var cache = new ToolCache(root, new FakeDownloader(), new FakeSource());

        var dir = await cache.EnsureAsync(InteropTool.Senbei, default);

        Assert.Equal(Path.Combine(root, "tools", "Senbei", "v1.0.0"), dir);
        Assert.True(File.Exists(Path.Combine(dir, ".ready")));
        Assert.True(File.Exists(Path.Combine(dir, "senbei-1.0.0-x86_64-pc-windows-msvc", "senbei.exe")));
    }

    [Fact] public async Task Newer_upstream_version_supersedes_stale_tool() {
        var root = Tmp();
        var source = new FakeSource();
        var cache = new ToolCache(root, new FakeDownloader(), source);

        var first = await cache.EnsureAsync(InteropTool.Senbei, default);
        Assert.EndsWith("v1.0.0", first);

        source.Version = "v1.1.0";
        var second = await cache.EnsureAsync(InteropTool.Senbei, default);

        Assert.EndsWith("v1.1.0", second);
        Assert.True(Directory.Exists(first)); // another run may still be using the older version
    }

    [Fact] public async Task Failed_update_keeps_previous_working_tool() {
        var root = Tmp();
        var source = new FakeSource();
        var downloader = new FakeDownloader();
        var cache = new ToolCache(root, downloader, source);

        var first = await cache.EnsureAsync(InteropTool.Senbei, default);

        source.Version = "v1.1.0";
        downloader.Fail = true;
        Assert.Equal(first, await cache.EnsureAsync(InteropTool.Senbei, default));

        Assert.True(File.Exists(Path.Combine(first, ".ready"))); // previous version untouched
    }

    [Fact] public async Task Zip_slip_entry_is_rejected() {
        var root = Tmp();
        var cache = new ToolCache(root, new EvilDownloader(), new FakeSource());

        await Assert.ThrowsAsync<InvalidOperationException>(() => cache.EnsureAsync(InteropTool.Senbei, default));
        Assert.False(File.Exists(Path.Combine(root, "tools", "Senbei", "v1.0.0", ".ready")));
    }

    sealed class EvilDownloader : IFileDownloader {
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct) {
            var p = Path.GetTempFileName();
            using (var zip = new ZipArchive(File.Create(p), ZipArchiveMode.Create)) {
                zip.CreateEntry("../escape.txt");
                zip.CreateEntry("ok.txt");
            }
            return Task.FromResult(p);
        }
    }
}
