using ClientpatchManager.Core.Feed;

namespace ClientpatchManager.Core.Update;

/// <summary>
/// Seam for fetching a release asset to a local temp file. Tests inject a fake;
/// the real implementation streams over HTTP.
/// </summary>
public interface IFileDownloader {
    Task<string> DownloadToTempAsync(string url, CancellationToken ct);
}

/// <summary>Outcome of an install attempt. <see cref="Error"/> is null on success.</summary>
public record SwapResult(bool Ok, string? Error, bool RestartRequired = false);

/// <summary>
/// Downloads a release asset, verifies its SHA-256, backs up the existing file,
/// then atomically swaps it in. Safety-critical: a mismatched hash must never touch
/// the destination.
/// </summary>
public interface IUpdateInstaller {
    Task<SwapResult> InstallClientpatchReleaseAsync(string gameDir, ReleaseAsset asset, string? expectedSha256, string version, CancellationToken ct);
    Task<SwapResult> InstallAsync(string gameDir, ReleaseAsset asset, string destRelativePath, string? expectedSha256, CancellationToken ct);
    Task<SwapResult> InstallReleaseAsync(string gameDir, ReleaseAsset asset, string destRelativePath, string? expectedSha256, string version, CancellationToken ct) =>
        InstallAsync(gameDir, asset, destRelativePath, expectedSha256, ct);

    /// <summary>Installs a zip asset into a directory under the game dir (langpacks, BepInEx
    /// bundles), backing up every file the archive would overwrite.</summary>
    Task<SwapResult> InstallZipAsync(string gameDir, ReleaseAsset asset, string destRelativeDir, string? expectedSha256, CancellationToken ct);
    Task<SwapResult> InstallZipReleaseAsync(string gameDir, ReleaseAsset asset, string destRelativeDir, string? expectedSha256, string version, CancellationToken ct) =>
        InstallZipAsync(gameDir, asset, destRelativeDir, expectedSha256, ct);
}
