using System.IO.Compression;
using System.Text.Json;
using ClientpatchManager.Core.Backup;

namespace ClientpatchManager.Core.Install;

/// <summary>Stages a complete file set, snapshots every old destination, and restores it on failure.
/// Pending journals survive process interruption and are recovered under the installation lock.</summary>
public static class FileTransaction {
    sealed record Original(string RelativePath, bool Exists);

    public static string Resolve(string root, string relative) {
        if (string.IsNullOrWhiteSpace(relative) || Path.IsPathRooted(relative) || relative.Contains(':') || relative.Replace('\\', '/').Split('/').Any(p => p == ".."))
            throw new ArgumentException($"Unsafe relative path: '{relative}'.");
        var fullRoot = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar);
        var dest = Path.GetFullPath(Path.Combine(fullRoot, relative));
        if (!dest.StartsWith(fullRoot + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException($"Path escapes destination: '{relative}'.");
        // Probe directly: Exists suppresses access errors and misses dangling links.
        // Include the destination itself so file links cannot become backup inputs.
        for (var part = dest; part is not null && part.Length > fullRoot.Length; part = Path.GetDirectoryName(part)) {
            try {
                if ((File.GetAttributes(part) & FileAttributes.ReparsePoint) != 0)
                    throw new IOException($"Destination contains a link: '{relative}'.");
            }
            catch (FileNotFoundException) { }
            catch (DirectoryNotFoundException) { }
        }
        return dest;
    }

    public static string NewStage(string gameDir) {
        var dir = Resolve(gameDir, Path.Combine(InstallLayout.DataDirName, ".cpm-stage-" + Guid.NewGuid().ToString("N")));
        Directory.CreateDirectory(dir); return dir;
    }

    public static void ExtractZip(string archive, string staging, CancellationToken ct = default, Func<string, bool>? includeEntry = null) {
        using var zip = ZipFile.OpenRead(archive);
        var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var entry in zip.Entries) {
            ct.ThrowIfCancellationRequested();
            var relative = entry.FullName.Replace('\\', '/');
            if (relative.Length == 0) continue;
            if (relative.TrimEnd('/') == ".") continue;
            string dest;
            try { dest = Resolve(staging, relative.TrimEnd('/')); }
            catch (ArgumentException ex) { throw new InvalidOperationException($"zip entry escapes destination: {relative}", ex); }
            if (includeEntry is not null && !includeEntry(relative)) continue;
            if (relative.EndsWith('/')) { Directory.CreateDirectory(dest); continue; }
            if (!paths.Add(dest)) throw new IOException($"Duplicate archive entry: {relative}");
            Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
            entry.ExtractToFile(dest);
        }
    }

    public static Dictionary<string, string?> FilesFrom(string staging, string relativeDir = ".") =>
        Directory.GetFiles(staging, "*", SearchOption.AllDirectories).ToDictionary(
            p => Path.Combine(relativeDir, Path.GetRelativePath(staging, p)), p => (string?)p, StringComparer.OrdinalIgnoreCase);

    /// <remarks>Caller holds GameOperation.Acquire for this entire call.</remarks>
    public static void Commit(string gameDir, IReadOnlyDictionary<string, string?> files, IBackupService? backup = null, string? currencyGate = null) {
        Recover(gameDir);
        GameOperation.RequireStopped(gameDir);
        var paths = files.Keys.Concat(currencyGate is null ? Array.Empty<string>() : new[] { currencyGate }).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
        // Validate all targets before a single live byte changes.
        foreach (var rel in paths) {
            var target = Resolve(gameDir, rel);
            if (Directory.Exists(target)) throw new IOException($"Expected a file, found a directory: {rel}");
            for (var p = Path.GetDirectoryName(target); p is not null; p = Path.GetDirectoryName(p))
                if (File.Exists(p)) throw new IOException($"Destination parent is a file: {p}");
            if (File.Exists(target)) { using var probe = new FileStream(target, FileMode.Open, FileAccess.ReadWrite, FileShare.None); }
        }
        var txn = Path.Combine(InstallLayout.DataDir(gameDir), ".cpm-transaction-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(txn);
        var originals = new List<Original>();
        var changed = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var journaled = false;
        try {
            foreach (var rel in paths) {
                var target = Resolve(gameDir, rel);
                var exists = File.Exists(target); originals.Add(new(rel, exists));
                if (exists) Copy(target, Resolve(Path.Combine(txn, "before"), rel));
                if (files.TryGetValue(rel, out var source) && source is not null)
                    Copy(source, Resolve(Path.Combine(txn, "after"), rel));
            }
            backup?.Backup(gameDir, paths);
            File.WriteAllText(Path.Combine(txn, "journal.json"), JsonSerializer.Serialize(new { Originals = originals, Gate = currencyGate }));
            journaled = true;
            if (currencyGate is not null) { var gatePath = Resolve(gameDir, currencyGate); if (File.Exists(gatePath)) File.Delete(gatePath); changed.Add(currencyGate); }
            foreach (var (rel, source) in files) {
                if (rel.Equals(currencyGate, StringComparison.OrdinalIgnoreCase)) continue;
                Put(gameDir, rel, source is null ? null : Resolve(Path.Combine(txn, "after"), rel)); changed.Add(rel);
            }
            if (currencyGate is not null && files.TryGetValue(currencyGate, out var gateSource) && gateSource is not null)
                Put(gameDir, currencyGate, Resolve(Path.Combine(txn, "after"), currencyGate));
            File.Delete(Path.Combine(txn, "journal.json")); journaled = false;
        }
        catch (Exception error) {
            try {
                Restore(gameDir, txn, originals.Where(o => changed.Contains(o.RelativePath)).ToList(), currencyGate);
                File.Delete(Path.Combine(txn, "journal.json")); journaled = false;
            }
            catch (Exception recovery) { throw new AggregateException($"Installation failed and recovery is pending in {txn}. Close the game and retry.", error, recovery); }
            throw;
        }
        finally { if (!journaled) DeleteStage(txn); }
    }

    static void Restore(string gameDir, string txn, IReadOnlyList<Original> originals, string? gate) {
        foreach (var item in originals.OrderBy(o => o.RelativePath.Equals(gate, StringComparison.OrdinalIgnoreCase)))
            Put(gameDir, item.RelativePath, item.Exists ? Resolve(Path.Combine(txn, "before"), item.RelativePath) : null);
    }

    internal static void Recover(string gameDir) {
        var data = Resolve(gameDir, InstallLayout.DataDirName);
        if (!Directory.Exists(data)) return;
        foreach (var txn in Directory.GetDirectories(data, ".cpm-transaction-*")) {
            var transactionRelative = Path.GetRelativePath(gameDir, txn);
            var journal = Resolve(gameDir, Path.Combine(transactionRelative, "journal.json"));
            if (!File.Exists(journal)) continue;
            GameOperation.RequireStopped(gameDir);
            using var doc = JsonDocument.Parse(File.ReadAllText(journal));
            var originals = doc.RootElement.GetProperty("Originals").Deserialize<List<Original>>()
                ?? throw new InvalidDataException("Recovery journal has no original file list.");
            var gate = doc.RootElement.GetProperty("Gate").GetString();
            // Validate the entire recovery set before deleting its marker or restoring
            // any files. A corrupt journal or linked snapshot must leave live data alone.
            var targets = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            _ = Resolve(gameDir, Path.Combine(transactionRelative, "before"));
            _ = Resolve(gameDir, Path.Combine(transactionRelative, "after"));
            foreach (var item in originals) {
                var target = Resolve(gameDir, item.RelativePath);
                if (!targets.Add(target) || Directory.Exists(target))
                    throw new InvalidDataException("Recovery journal has duplicate or non-file destinations.");
                var source = Resolve(gameDir, Path.Combine(transactionRelative, "before", item.RelativePath));
                if (item.Exists && !File.Exists(source))
                    throw new InvalidDataException($"Recovery snapshot is missing: {item.RelativePath}");
            }
            if (gate is not null) {
                var gatePath = Resolve(gameDir, gate);
                gate = originals.FirstOrDefault(item => Resolve(gameDir, item.RelativePath).Equals(gatePath, StringComparison.OrdinalIgnoreCase))?.RelativePath
                    ?? throw new InvalidDataException("Recovery marker is not part of the original file set.");
                if (File.Exists(gatePath)) File.Delete(gatePath);
            }
            Restore(gameDir, txn, originals, gate);
            File.Delete(journal); DeleteStage(txn);
        }
    }

    static void Put(string gameDir, string rel, string? source) {
        var target = Resolve(gameDir, rel);
        if (source is null) { if (File.Exists(target)) File.Delete(target); return; }
        Directory.CreateDirectory(Path.GetDirectoryName(target)!);
        var temp = target + ".cpm-new-" + Guid.NewGuid().ToString("N");
        try { File.Copy(source, temp); File.Move(temp, target, true); }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }
    static void Copy(string source, string dest) { Directory.CreateDirectory(Path.GetDirectoryName(dest)!); File.Copy(source, dest); }
    public static void DeleteStage(string path) { try { Directory.Delete(path, true); } catch (IOException) { } catch (UnauthorizedAccessException) { } }
}
