using System.IO.Compression;
using ClientpatchManager.Core.Update;

namespace ClientpatchManager.Core.Interop;

/// <summary>
/// Caches the interop tools under <c>&lt;root&gt;/tools/&lt;tool&gt;/&lt;version&gt;/</c>, where
/// <c>&lt;tool&gt;</c> is the <see cref="InteropTool"/> enum name and <c>&lt;version&gt;</c> is the
/// tool's own upstream release tag (each tool comes from its own repository — see
/// <see cref="IToolReleaseSource"/>) — so an upstream update produces a new directory and a stale
/// tool can never satisfy a newer release. A completed extraction is stamped with a
/// <c>.ready</c> marker; a partial extraction is never mistaken for a usable tool, and a failed
/// update leaves the previously working version untouched.
/// </summary>
public sealed class ToolCache : IToolCache {
    const string ReadyMarker = ".ready";

    readonly string _root;
    readonly IFileDownloader? _downloader;
    readonly IToolReleaseSource? _source;

    /// <summary>
    /// Test/explicit-root constructor. <paramref name="root"/> is the cache root (the
    /// <c>tools/</c> subtree lives beneath it). Download dependencies are optional so tests
    /// that only exercise <see cref="IsCached"/> need not supply them.
    /// </summary>
    public ToolCache(string root, IFileDownloader? downloader = null, IToolReleaseSource? source = null) {
        _root = root;
        _downloader = downloader;
        _source = source;
    }

    /// <summary>Production constructor: defaults the cache root to
    /// <c>%LOCALAPPDATA%/ClientpatchManager</c> and wires the download dependencies.</summary>
    public ToolCache(IFileDownloader downloader, IToolReleaseSource source)
        : this(DefaultRoot(), downloader, source) { }

    static string DefaultRoot() =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ClientpatchManager");

    string ToolRoot(InteropTool tool) => Path.Combine(_root, "tools", tool.ToString());

    static string ToolDir(string root, InteropTool tool, string version) =>
        Path.Combine(root, "tools", tool.ToString(), version);

    /// <summary>True iff some version of the tool completed extraction (UI-level check).</summary>
    public bool IsCached(InteropTool tool) => CachedDir(tool) is not null;

    /// <summary>
    /// Ensures the tool's latest upstream release is extracted and returns its version
    /// directory. Downloads and extracts into a staging directory first; the previously
    /// working version is replaced only after the new one is complete.
    /// </summary>
    public async Task<string> EnsureAsync(InteropTool tool, CancellationToken ct) {
        ct.ThrowIfCancellationRequested();
        var cached = CachedDir(tool);
        if (_downloader is null || _source is null)
            return cached ?? throw new InvalidOperationException("ToolCache was constructed without download dependencies; cannot fetch tools.");

        ToolRelease release;
        try {
            var query = _source.LatestAsync(tool, ct);
            release = cached is null ? await query.ConfigureAwait(false) : await query.WaitAsync(TimeSpan.FromSeconds(3), ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception ex) when (cached is not null && ex is HttpRequestException or InvalidOperationException or TimeoutException) { return cached; }

        if (Path.GetFileName(release.Version) != release.Version || release.Version is "." or ".." || release.Version.Contains(':'))
            throw new InvalidOperationException("Tool release has an unsafe version identifier.");
        var dir = ToolDir(_root, tool, release.Version);
        if (File.Exists(Path.Combine(dir, ReadyMarker)))
            return dir;

        string archive;
        try { archive = await _downloader.DownloadToTempAsync(release.DownloadUrl, ct).ConfigureAwait(false); }
        catch (HttpRequestException) when (cached is not null) { return cached; }
        var staging = dir + ".cpm-extract-" + Guid.NewGuid().ToString("N");
        try {
            await UpdateInstaller.VerifyAsync(archive, release.Sha256, ct).ConfigureAwait(false);
            Directory.CreateDirectory(staging);

            using var zip = ZipFile.OpenRead(archive);
            var extractedAny = false;
            var stagingFull = Path.GetFullPath(staging) + Path.DirectorySeparatorChar;
            foreach (var entry in zip.Entries) {
                ct.ThrowIfCancellationRequested();
                var rel = entry.FullName.Replace('\\', '/');
                if (rel.Length == 0) continue;

                var destPath = Path.GetFullPath(Path.Combine(staging, rel));
                // Guard against zip-slip traversal outside the staging dir (Windows paths
                // are case-insensitive; OrdinalIgnoreCase is the correct comparison).
                if (!destPath.StartsWith(stagingFull, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException($"zip entry escapes tool dir: {entry.FullName}");

                if (rel.EndsWith('/')) { Directory.CreateDirectory(destPath); continue; }
                Directory.CreateDirectory(Path.GetDirectoryName(destPath)!);
                entry.ExtractToFile(destPath, overwrite: true);
                extractedAny = true;
            }

            if (!extractedAny)
                throw new InvalidOperationException($"tool bundle for {tool} contained no files");

            // Stamp the marker inside staging: only a complete extraction moves into place.
            File.WriteAllText(Path.Combine(staging, ReadyMarker), string.Empty);

            // Swap in the complete tree; the previous version survives a failed extraction.
            Directory.CreateDirectory(Path.GetDirectoryName(dir)!);
            if (Directory.Exists(dir) && !File.Exists(Path.Combine(dir, ReadyMarker)))
                Directory.Move(dir, dir + ".incomplete-" + Guid.NewGuid().ToString("N"));
            try { Directory.Move(staging, dir); }
            catch (IOException) when (File.Exists(Path.Combine(dir, ReadyMarker))) { /* another extraction won; keep its complete tree */ }

            return dir;
        }
        finally {
            try { if (Directory.Exists(staging)) Directory.Delete(staging, recursive: true); } catch { /* best-effort */ }
            try { if (File.Exists(archive)) File.Delete(archive); } catch { /* best-effort */ }
        }
    }

    string? CachedDir(InteropTool tool) {
        var root = ToolRoot(tool);
        if (!Directory.Exists(root)) return null;
        return Directory.GetDirectories(root).Where(d => File.Exists(Path.Combine(d, ReadyMarker)))
            .Where(d => tool != InteropTool.Il2CppDumper || Path.GetFileName(d) == GitHubToolReleaseSource.DumperVersion)
            .OrderByDescending(Directory.GetLastWriteTimeUtc).FirstOrDefault();
    }
}
