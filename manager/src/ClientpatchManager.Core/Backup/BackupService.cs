using System.Globalization;

namespace ClientpatchManager.Core.Backup;

/// <summary>
/// Copies files to a timestamped backup folder before mutations and restores them.
/// Safety-critical: underpins the "never break launch" guarantee. Uses only System.IO.
/// </summary>
public sealed class BackupService : IBackupService {
    const string BackupRootName = ".clientpatch-backups";
    const string ManifestName = ".manifest";
    const string IdFormat = "yyyyMMddHHmmssfff";

    /// <summary>Backups live in <c>&lt;gameDir&gt;/clientpatch/.clientpatch-backups</c>.</summary>
    static string Root(string gameDir) =>
        Install.FileTransaction.Resolve(gameDir, Path.Combine(Install.InstallLayout.DataDirName, BackupRootName));

    public BackupEntry Backup(string gameDir, IEnumerable<string> relativePaths) {
        var takenAt = DateTimeOffset.UtcNow;
        var root = Root(gameDir);
        Directory.CreateDirectory(root);

        // Ensure a unique folder name even for two backups in the same millisecond.
        // The suffix is zero-padded so lexicographic order == chronological order.
        var baseId = takenAt.UtcDateTime.ToString(IdFormat, CultureInfo.InvariantCulture);
        var id = baseId;
        var backupDir = Path.Combine(root, id);
        int suffix = 1;
        while (Directory.Exists(backupDir)) {
            id = $"{baseId}-{suffix++:D4}";
            backupDir = Path.Combine(root, id);
        }
        Directory.CreateDirectory(backupDir);

        var files = new List<string>();
        foreach (var rel in relativePaths) {
            var normalized = Normalize(rel);
            files.Add(normalized);
            var src = Install.FileTransaction.Resolve(gameDir, normalized);
            if (!File.Exists(src)) {
                // Missing source is recorded (so Restore deletes a file the mutation created)
                // but not fatal.
                continue;
            }
            var dst = Path.Combine(backupDir, normalized);
            Directory.CreateDirectory(Path.GetDirectoryName(dst)!);
            File.Copy(src, dst, overwrite: true);
        }

        File.WriteAllText(Path.Combine(backupDir, ".layout"), "game-relative-v1");
        WriteManifest(backupDir, files);
        return new BackupEntry(id, takenAt, files);
    }

    public IReadOnlyList<BackupEntry> List(string gameDir) {
        var root = Root(gameDir);
        if (!Directory.Exists(root)) {
            return Array.Empty<BackupEntry>();
        }
        var entries = new List<BackupEntry>();
        foreach (var dir in Directory.EnumerateDirectories(root)) {
            var id = Path.GetFileName(dir);
            if (!File.Exists(Path.Combine(dir, ManifestName))) continue;
            var files = ReadManifest(dir);
            entries.Add(new BackupEntry(id, ParseTakenAt(id), files));
        }
        return entries.OrderBy(e => e.Id, StringComparer.Ordinal).ToList();
    }

    public void Restore(string gameDir, string id) {
        using var operation = Install.GameOperation.Acquire(gameDir);
        var restore = ReadRestoreFiles(gameDir, id);
        // Restoring is another installation: hide its currency marker until all of
        // the matching binaries have been restored, including during crash recovery.
        var gate = restore.Keys.FirstOrDefault(path => path.Equals("clientpatch/" + Install.InstallDetector.VersionSidecarName, StringComparison.OrdinalIgnoreCase))
            ?? restore.Keys.FirstOrDefault(path => Path.GetFileName(path).Equals("clientpatch-interop.json", StringComparison.OrdinalIgnoreCase))
            ?? restore.Keys.FirstOrDefault(path => path.Equals("clientpatch/bepinex.version", StringComparison.OrdinalIgnoreCase));
        Install.FileTransaction.Commit(gameDir, restore, currencyGate: gate);
    }

    public bool RestoreWithHandoff(string gameDir, string id) {
        var restore = ReadRestoreFiles(gameDir, id);
        var manager = Install.FileTransaction.Resolve(gameDir, "manager.exe");
        if (!restore.ContainsKey("manager.exe") || !string.Equals(Environment.ProcessPath, manager, StringComparison.OrdinalIgnoreCase)) {
            Restore(gameDir, id);
            return false;
        }
        Install.GameOperation.RequireStopped(gameDir);
        var stage = Path.Combine(Path.GetTempPath(), "clientpatch-rollback-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(stage);
        try {
            var executable = Path.Combine(stage, "manager.exe");
            File.Copy(manager, executable);
            var start = new System.Diagnostics.ProcessStartInfo(executable) {
                UseShellExecute = false, WorkingDirectory = stage, CreateNoWindow = true
            };
            foreach (var argument in new[] { "--restore-clientpatch-backup", Path.GetFullPath(gameDir), stage, id, Environment.ProcessId.ToString() })
                start.ArgumentList.Add(argument);
            using var helper = System.Diagnostics.Process.Start(start) ?? throw new IOException("Could not start the backup restorer.");
            // The helper and its diagnostics must survive until restoration is complete.
            return true;
        }
        catch { Install.FileTransaction.DeleteStage(stage); throw; }
    }

    static Dictionary<string, string?> ReadRestoreFiles(string gameDir, string id) {
        if (string.IsNullOrWhiteSpace(id) || Path.GetFileName(id) != id || id.Contains(':') || id is "." or "..")
            throw new ArgumentException("Invalid backup id.", nameof(id));
        var backupDir = Install.FileTransaction.Resolve(gameDir, Path.Combine(Install.InstallLayout.DataDirName, BackupRootName, id));
        if (!Directory.Exists(backupDir)) {
            throw new DirectoryNotFoundException($"Backup '{id}' not found under {gameDir}.");
        }
        var restore = new Dictionary<string, string?>(StringComparer.OrdinalIgnoreCase);
        var layout = Install.FileTransaction.Resolve(backupDir, ".layout");
        var legacy = !File.Exists(layout);
        if (!File.Exists(Install.FileTransaction.Resolve(backupDir, ManifestName)))
            throw new InvalidDataException("Backup manifest is missing; no files were restored.");
        var manifest = ReadManifest(backupDir);
        if (manifest.Count == 0) throw new InvalidDataException("Backup manifest is empty; no files were restored.");
        if ((legacy && manifest.Any(p => Normalize(p) != "clientpatch.toml")) || (!legacy && File.ReadAllText(layout) != "game-relative-v1"))
            throw new IOException("This backup uses ambiguous or unsupported paths and cannot be restored safely. Its files are preserved for manual recovery.");
        foreach (var rel in manifest) {
            var normalized = Normalize(rel);
            var src = Install.FileTransaction.Resolve(backupDir, normalized);
            var target = legacy ? "clientpatch/" + normalized : normalized;
            _ = Install.FileTransaction.Resolve(gameDir, target);
            restore[target] = File.Exists(src) ? src : null;
        }
        return restore;
    }

    /// <summary>Normalize to forward-slash relative form and reject anything that would
    /// escape the game directory (a feed asset name is caller-supplied data, I3).</summary>
    internal static string Normalize(string rel) {
        var n = rel.Replace('\\', '/');
        if (n.Length == 0 || Path.IsPathRooted(n) || n.Contains(':') ||
            n.Split('/').Any(seg => seg == ".."))
            throw new ArgumentException($"Unsafe relative path: '{rel}'.", nameof(rel));
        return n;
    }

    static void WriteManifest(string backupDir, IEnumerable<string> files) =>
        File.WriteAllLines(Path.Combine(backupDir, ManifestName), files);

    static IReadOnlyList<string> ReadManifest(string backupDir) {
        var path = Path.Combine(backupDir, ManifestName);
        if (!File.Exists(path)) {
            return Array.Empty<string>();
        }
        return File.ReadAllLines(path)
            .Where(l => l.Length > 0)
            .ToList();
    }

    static DateTimeOffset ParseTakenAt(string id) {
        var stamp = id;
        var dash = id.IndexOf('-');
        if (dash >= 0) {
            stamp = id[..dash];
        }
        if (DateTime.TryParseExact(stamp, IdFormat, CultureInfo.InvariantCulture,
                DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out var dt)) {
            return new DateTimeOffset(dt, TimeSpan.Zero);
        }
        return default;
    }
}
