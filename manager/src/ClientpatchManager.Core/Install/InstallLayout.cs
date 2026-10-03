namespace ClientpatchManager.Core.Install;

/// <summary>
/// Where clientpatch's own files live inside a game install.
///
/// <c>version.dll</c>, <c>BepInEx/</c>, and the manager exe stay in the game directory.
/// Everything clientpatch itself creates — config, per-launch logs, langpacks, interop
/// dumps, keys, version sidecars — lives in <c>&lt;gameDir&gt;/clientpatch</c>. There is
/// no fallback to the game directory.
/// </summary>
public static class InstallLayout {
    /// <summary>Subdirectory of the game dir holding clientpatch's own files.</summary>
    public const string DataDirName = "clientpatch";

    /// <summary>Subdirectory of the data dir holding the per-launch log files.</summary>
    public const string LogsDirName = "logs";

    public const string ConfigFileName = "clientpatch.toml";
    public const string ExampleConfigFileName = "clientpatch.example.toml";

    /// <summary>Default stem for the per-launch log files ([loader] log's default).</summary>
    public const string LogStem = "clientpatch";

    /// <summary><c>&lt;gameDir&gt;/clientpatch</c>, created on demand by callers that write.</summary>
    public static string DataDir(string gameDir) => Path.Combine(gameDir, DataDirName);

    /// <summary><c>&lt;gameDir&gt;/clientpatch/logs</c>.</summary>
    public static string LogsDir(string gameDir) => Path.Combine(DataDir(gameDir), LogsDirName);

    /// <summary>
    /// A file clientpatch owns: <c>&lt;gameDir&gt;/clientpatch/&lt;relativePath&gt;</c>.
    /// A relative path keeps its subdirectories; an absolute one keeps only its file
    /// name, so config cannot point clientpatch's files outside its own directory.
    /// </summary>
    public static string Resolve(string gameDir, string relativePath) {
        var rel = Path.IsPathRooted(relativePath) ? Path.GetFileName(relativePath) : relativePath;
        return Path.Combine(DataDir(gameDir), rel);
    }

    /// <summary>
    /// The newest per-launch log file: the <c>&lt;stem&gt;_*.log</c> in
    /// <c>&lt;gameDir&gt;/clientpatch/logs</c> with the newest write time (name
    /// breaks ties, matching the proxy's timestamped names). Null when none exists.
    /// The stem comes from [loader] log; only its filename part is used.
    /// </summary>
    public static string? LatestLog(string gameDir, string? stem) {
        var dir = LogsDir(gameDir);
        var fileName = Path.GetFileName((stem ?? "").Trim());
        var prefix = (fileName.Length > 0 ? Path.GetFileNameWithoutExtension(fileName) : LogStem) + "_";
        if (!Directory.Exists(dir))
            return null;
        try {
            return Directory.GetFiles(dir, prefix + "*.log")
                .Select(p => new { Path = p, Stamp = File.GetLastWriteTimeUtc(p) })
                .OrderByDescending(f => f.Stamp)
                .ThenByDescending(f => f.Path, StringComparer.OrdinalIgnoreCase)
                .Select(f => f.Path)
                .FirstOrDefault();
        }
        catch (IOException) {
            return null;
        }
        catch (UnauthorizedAccessException) {
            return null;
        }
    }
}
