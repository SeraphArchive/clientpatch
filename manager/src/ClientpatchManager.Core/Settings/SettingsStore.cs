using System.Text.Json;
using System.Text.Json.Serialization;

namespace ClientpatchManager.Core.Settings;

/// <summary>
/// Persists <see cref="ManagerSettings"/> to <c>&lt;dir&gt;/settings.json</c> via
/// System.Text.Json. <see cref="Save"/> is atomic (temp file + rename) and keeps the
/// previous file as <c>settings.json.bak</c>; <see cref="Load"/> falls back to that
/// backup when the main file is corrupt, and to defaults on first run.
/// Pure BCL: no WPF/Windows dependency.
/// </summary>
public sealed class SettingsStore
{
    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() },
    };

    private readonly string _path;
    private readonly string _backupPath;

    /// <param name="dir">
    /// Directory to hold <c>settings.json</c>. Typically
    /// <c>%APPDATA%/ClientpatchManager</c>; the directory is created on first save.
    /// </param>
    public SettingsStore(string dir)
    {
        ArgumentNullException.ThrowIfNull(dir);
        _path = Path.Combine(dir, "settings.json");
        _backupPath = _path + ".bak";
    }

    /// <summary>Full path to the backing JSON file.</summary>
    public string FilePath => _path;

    /// <summary>Load settings: main file, then the .bak fallback, then defaults.</summary>
    public ManagerSettings Load()
    {
        var loaded = TryRead(_path) ?? TryRead(_backupPath);
        if (loaded is null)
            return ManagerSettings.Default;

        // The pre-release default feed repo never shipped a release; re-point it once.
        if (string.Equals(loaded.FeedRepo, ManagerSettings.LegacyFeedRepo, StringComparison.OrdinalIgnoreCase)
            || string.Equals(loaded.FeedRepo, ManagerSettings.PreviousFeedRepo, StringComparison.OrdinalIgnoreCase))
        {
            loaded = loaded with { FeedRepo = ManagerSettings.DefaultFeedRepo };
            // Migration can be used in memory even when another manager or a
            // read-only profile prevents persistence. Explicit saves still report errors.
            try { Save(loaded); }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }

        return loaded;
    }

    /// <summary>Persist settings atomically, keeping the previous file as .bak.</summary>
    public void Save(ManagerSettings settings)
    {
        ArgumentNullException.ThrowIfNull(settings);
        var dir = Path.GetDirectoryName(_path);
        if (!string.IsNullOrEmpty(dir))
            Directory.CreateDirectory(dir);

        var json = JsonSerializer.Serialize(settings, Options);
        var tmp = _path + ".tmp-" + Guid.NewGuid().ToString("N");
        var backupTmp = tmp + ".bak";
        try {
            WriteNew(tmp, json);
            // Preserve a known-good fallback when the main file was corrupt.
            // Atomically replace the backup too, so readers never see partial JSON.
            if (TryRead(_path) is { } previous) {
                WriteNew(backupTmp, JsonSerializer.Serialize(previous, Options));
                File.Move(backupTmp, _backupPath, overwrite: true);
            }
            File.Move(tmp, _path, overwrite: true);
        }
        finally {
            Update.UpdateInstaller.DeleteTemp(tmp);
            Update.UpdateInstaller.DeleteTemp(backupTmp);
        }
    }

    private static void WriteNew(string path, string json)
    {
        using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        using var writer = new StreamWriter(stream, new System.Text.UTF8Encoding(false));
        writer.Write(json);
    }

    private ManagerSettings? TryRead(string path)
    {
        try
        {
            if (!File.Exists(path))
                return null;
            var json = File.ReadAllText(path);
            return JsonSerializer.Deserialize<ManagerSettings>(json, Options);
        }
        catch (JsonException)
        {
            return null;
        }
        catch (IOException)
        {
            return null;
        }
        catch (UnauthorizedAccessException)
        {
            return null;
        }
    }
}
