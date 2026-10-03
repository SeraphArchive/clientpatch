using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Update;

namespace ClientpatchManager.Core.Launch;

public interface IPayloadInstaller {
    Task InstallAsync(string gameDir, string url, string doorstopName, string bepinexRoot, CancellationToken ct);
}

/// <summary>Stages and validates the complete payload, then installs it with rollback.</summary>
public sealed class PayloadInstaller(IFileDownloader downloader) : IPayloadInstaller {
    public const string CoreAssembly = "BepInEx.Unity.IL2CPP.dll";

    public async Task InstallAsync(string gameDir, string url, string doorstopName, string bepinexRoot, CancellationToken ct) {
        if (string.IsNullOrWhiteSpace(gameDir) || string.IsNullOrWhiteSpace(url)) throw new ArgumentException("game dir and bepinex url are required");
        var archive = await downloader.DownloadToTempAsync(url, ct).ConfigureAwait(false);
        string? staging = null;
        try {
            using var operation = GameOperation.Acquire(gameDir);
            staging = FileTransaction.NewStage(gameDir);
            FileTransaction.ExtractZip(archive, staging, ct);
            Normalize(staging, doorstopName, bepinexRoot);
            var files = FileTransaction.FilesFrom(staging);
            files["winhttp.dll"] = null;
            ct.ThrowIfCancellationRequested();
            FileTransaction.Commit(gameDir, files, new BackupService());
        }
        finally { UpdateInstaller.DeleteTemp(archive); if (staging is not null) FileTransaction.DeleteStage(staging); }
    }

    internal static void Normalize(string staging, string doorstopName, string bepinexRoot) {
        if (doorstopName.Equals("winhttp.dll", StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("doorstop must have a non-autoload filename");
        var winhttp = Path.Combine(staging, "winhttp.dll");
        var door = FileTransaction.Resolve(staging, doorstopName);
        if (File.Exists(winhttp)) { Directory.CreateDirectory(Path.GetDirectoryName(door)!); File.Move(winhttp, door, true); }
        if (!File.Exists(door) || !File.Exists(Path.Combine(staging, "BepInEx", "core", CoreAssembly)))
            throw new InvalidOperationException("payload incomplete after extract");
        if (!bepinexRoot.Equals("BepInEx", StringComparison.OrdinalIgnoreCase)) {
            var custom = FileTransaction.Resolve(staging, bepinexRoot);
            Directory.CreateDirectory(Path.GetDirectoryName(custom)!);
            Directory.Move(Path.Combine(staging, "BepInEx"), custom);
            var ini = Path.Combine(staging, "doorstop_config.ini");
            if (File.Exists(ini)) File.WriteAllText(ini, File.ReadAllText(ini).Replace("BepInEx/", bepinexRoot.Replace('\\', '/') + "/").Replace("BepInEx\\", bepinexRoot + "\\"));
        }
    }
    internal static void ExtractSafe(string zipPath, string destDir) => FileTransaction.ExtractZip(zipPath, destDir);
}
