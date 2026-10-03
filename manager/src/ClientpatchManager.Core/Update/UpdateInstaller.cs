using System.Reflection.PortableExecutable;
using System.Security.Cryptography;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Feed;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Launch;

namespace ClientpatchManager.Core.Update;

/// <summary>Verify downloaded artifacts before a backed-up, recoverable commit.</summary>
public sealed class UpdateInstaller(IFileDownloader downloader, IBackupService backup) : IUpdateInstaller {
    public async Task<SwapResult> InstallClientpatchReleaseAsync(string gameDir, ReleaseAsset asset, string? expectedSha256, string version, CancellationToken ct) {
        string? temp = null, stage = null;
        var handedOff = false;
        try {
            temp = await downloader.DownloadToTempAsync(asset.DownloadUrl, ct).ConfigureAwait(false);
            await VerifyAsync(temp, expectedSha256, ct).ConfigureAwait(false);
            stage = Path.Combine(Path.GetTempPath(), "clientpatch-update-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(stage);
            FileTransaction.ExtractZip(temp, stage, ct, IsClientpatchBundlePath);
            ValidateClientpatchBundle(stage, version);
            GameOperation.RequireStopped(gameDir);
            ct.ThrowIfCancellationRequested();
            var installedManager = FileTransaction.Resolve(gameDir, "manager.exe");
            if (string.Equals(Environment.ProcessPath, installedManager, StringComparison.OrdinalIgnoreCase)) {
                // Run the NEW bundled manager from staging. It waits for this executable to
                // exit before committing the whole bundle with the existing recovery journal.
                var start = new System.Diagnostics.ProcessStartInfo(Path.Combine(stage, "manager.exe")) {
                    UseShellExecute = false, WorkingDirectory = stage, CreateNoWindow = true
                };
                foreach (var argument in new[] { "--apply-clientpatch-update", Path.GetFullPath(gameDir), stage, version, Environment.ProcessId.ToString() })
                    start.ArgumentList.Add(argument);
                using var helper = System.Diagnostics.Process.Start(start) ?? throw new IOException("Could not start the bundle updater.");
                handedOff = true;
                return new(true, null, RestartRequired: true);
            }
            ApplyClientpatchBundle(gameDir, stage, version, backup);
            return new(true, null);
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception ex) { return new(false, ex.Message); }
        finally {
            DeleteTemp(temp);
            if (!handedOff && stage is not null) FileTransaction.DeleteStage(stage);
        }
    }

    public static void ApplyClientpatchBundle(string gameDir, string stage, string version, IBackupService backup) {
        ValidateClientpatchBundle(stage, version);
        using var operation = GameOperation.Acquire(gameDir);
        var files = FileTransaction.FilesFrom(stage)
            .Where(item => IsClientpatchBundlePath(item.Key))
            .ToDictionary(item => Path.GetRelativePath(stage, item.Value!).Replace('\\', '/'),
                item => item.Value, StringComparer.OrdinalIgnoreCase);
        // Updates preserve user configuration; the bundled template is for first installation.
        var config = "clientpatch/clientpatch.toml";
        files.Remove(config);
        if (!File.Exists(FileTransaction.Resolve(gameDir, config)))
            files[config] = Path.Combine(stage, "clientpatch", InstallLayout.ExampleConfigFileName);
        var sidecar = Path.Combine(stage, "release-version");
        File.WriteAllText(sidecar, version);
        var gate = "clientpatch/" + InstallDetector.VersionSidecarName;
        files[gate] = sidecar;
        FileTransaction.Commit(gameDir, files, backup, currencyGate: gate);
    }

    internal static bool IsClientpatchBundlePath(string relative) {
        var path = relative.Replace('\\', '/');
        while (path.StartsWith("./", StringComparison.Ordinal)) path = path[2..];
        return path.Equals("version.dll", StringComparison.OrdinalIgnoreCase)
            || path.Equals("manager.exe", StringComparison.OrdinalIgnoreCase)
            || path.StartsWith("clientpatch/", StringComparison.OrdinalIgnoreCase);
    }

    static void ValidateClientpatchBundle(string stage, string version) {
        ValidateDll(Path.Combine(stage, "version.dll"));
        using var stream = File.OpenRead(Path.Combine(stage, "manager.exe"));
        using var pe = new PEReader(stream);
        if (pe.PEHeaders.CoffHeader.Machine != Machine.Amd64 || pe.PEHeaders.PEHeader?.Magic != PEMagic.PE32Plus
            || (pe.PEHeaders.CoffHeader.Characteristics & Characteristics.Dll) != 0)
            throw new InvalidDataException("Bundle manager is not a Windows x64 executable.");
        if (!File.Exists(Path.Combine(stage, "clientpatch", InstallLayout.ExampleConfigFileName)))
            throw new InvalidDataException("Bundle is missing its configuration template.");
        if (File.ReadAllText(Path.Combine(stage, "clientpatch", InstallDetector.VersionSidecarName)).Trim() != version)
            throw new InvalidDataException("Bundle version does not match the selected release.");
    }

    public Task<SwapResult> InstallAsync(string gameDir, ReleaseAsset asset, string destRelativePath, string? expectedSha256, CancellationToken ct) =>
        InstallFileAsync(gameDir, asset, destRelativePath, expectedSha256, null, ct);
    public Task<SwapResult> InstallReleaseAsync(string gameDir, ReleaseAsset asset, string destRelativePath, string? expectedSha256, string version, CancellationToken ct) =>
        InstallFileAsync(gameDir, asset, destRelativePath, expectedSha256, version, ct);

    async Task<SwapResult> InstallFileAsync(string gameDir, ReleaseAsset asset, string relative, string? hash, string? version, CancellationToken ct) {
        string? temp = null, staging = null;
        try {
            temp = await downloader.DownloadToTempAsync(asset.DownloadUrl, ct).ConfigureAwait(false);
            await VerifyAsync(temp, hash, ct).ConfigureAwait(false);
            if (version is not null) ValidateDll(temp);
            using var operation = GameOperation.Acquire(gameDir);
            var files = new Dictionary<string, string?> { [relative] = temp };
            if (version is not null) {
                staging = FileTransaction.NewStage(gameDir);
                var sidecar = Path.Combine(staging, "version"); File.WriteAllText(sidecar, version);
                files[Path.Combine(InstallLayout.DataDirName, InstallDetector.VersionSidecarName)] = sidecar;
            }
            ct.ThrowIfCancellationRequested();
            FileTransaction.Commit(gameDir, files, backup, version is null ? null : Path.Combine(InstallLayout.DataDirName, InstallDetector.VersionSidecarName));
            return new(true, null);
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception ex) { return new(false, ex.Message); }
        finally { DeleteTemp(temp); if (staging is not null) FileTransaction.DeleteStage(staging); }
    }

    public Task<SwapResult> InstallZipAsync(string gameDir, ReleaseAsset asset, string relativeDir, string? hash, CancellationToken ct) =>
        InstallZipFileAsync(gameDir, asset, relativeDir, hash, null, ct);
    public Task<SwapResult> InstallZipReleaseAsync(string gameDir, ReleaseAsset asset, string relativeDir, string? hash, string version, CancellationToken ct) =>
        InstallZipFileAsync(gameDir, asset, relativeDir, hash, version, ct);
    async Task<SwapResult> InstallZipFileAsync(string gameDir, ReleaseAsset asset, string relativeDir, string? hash, string? version, CancellationToken ct) {
        string? temp = null, staging = null;
        try {
            temp = await downloader.DownloadToTempAsync(asset.DownloadUrl, ct).ConfigureAwait(false);
            await VerifyAsync(temp, hash, ct).ConfigureAwait(false);
            using var operation = GameOperation.Acquire(gameDir);
            staging = FileTransaction.NewStage(gameDir);
            FileTransaction.ExtractZip(temp, staging, ct);
            var configPath = InstallLayout.Resolve(gameDir, InstallLayout.ConfigFileName);
            var config = File.Exists(configPath) ? new Config.ConfigService().Load(File.ReadAllText(configPath)) : null;
            var isPayload = relativeDir == "." && (version is not null || File.Exists(Path.Combine(staging, "winhttp.dll")));
            if (isPayload) PayloadInstaller.Normalize(staging, config?.BepInEx?.Doorstop ?? "doorstop.dll", config?.BepInEx?.Root ?? "BepInEx");
            var files = FileTransaction.FilesFrom(staging, relativeDir);
            if (files.Count == 0) throw new IOException("Archive contained no installable files.");
            if (isPayload) files["winhttp.dll"] = null;
            if (version is not null) {
                var sidecar = Path.Combine(staging, "release-version"); File.WriteAllText(sidecar, version);
                files["clientpatch/bepinex.version"] = sidecar;
            }
            ct.ThrowIfCancellationRequested(); FileTransaction.Commit(gameDir, files, backup, version is null ? null : "clientpatch/bepinex.version");
            return new(true, null);
        }
        catch (OperationCanceledException) { throw; }
        catch (Exception ex) { return new(false, ex.Message); }
        finally { DeleteTemp(temp); if (staging is not null) FileTransaction.DeleteStage(staging); }
    }

    internal static void ValidateDll(string path) {
        using var stream = File.OpenRead(path);
        using var pe = new PEReader(stream);
        if (pe.PEHeaders.CoffHeader.Machine != Machine.Amd64 || pe.PEHeaders.PEHeader?.Magic != PEMagic.PE32Plus || (pe.PEHeaders.CoffHeader.Characteristics & Characteristics.Dll) == 0)
            throw new InvalidDataException("Release artifact is not a Windows x64 DLL.");
    }
    internal static async Task VerifyAsync(string path, string? expected, CancellationToken ct) {
        if (expected is null) return;
        using var stream = File.OpenRead(path);
        var actual = Convert.ToHexString(await SHA256.HashDataAsync(stream, ct).ConfigureAwait(false));
        if (!actual.Equals(expected.Trim(), StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("hash mismatch");
    }
    internal static void DeleteTemp(string? path) { try { if (path is not null) File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { } }
}
