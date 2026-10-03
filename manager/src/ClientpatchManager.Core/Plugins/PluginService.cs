namespace ClientpatchManager.Core.Plugins;

/// <summary>
/// Enables/disables BepInEx plugins by toggling a <c>.disabled</c> filename suffix.
/// BepInEx only loads <c>*.dll</c>, so a suffixed file (<c>Foo.dll.disabled</c>) is ignored
/// and survives rescans. Scanning is recursive because BepInEx loads plugins from
/// subdirectories too (the common <c>plugins/&lt;Author-Name&gt;/Plugin.dll</c> layout).
/// Uses only System.IO; no WPF/Windows dependencies.
/// </summary>
public sealed class PluginService : IPluginService {
    const string DisabledSuffix = ".disabled";

    public IReadOnlyList<PluginInfo> Scan(string bepinexRoot) {
        var pluginsDir = Install.FileTransaction.Resolve(bepinexRoot, "plugins");
        if (!Directory.Exists(pluginsDir)) {
            return Array.Empty<PluginInfo>();
        }

        var byRelName = new Dictionary<string, PluginInfo>(StringComparer.OrdinalIgnoreCase);

        // Enabled: plugins/**/*.dll
        var enumeration = new EnumerationOptions { RecurseSubdirectories = true, AttributesToSkip = FileAttributes.ReparsePoint, IgnoreInaccessible = false };
        foreach (var path in Directory.EnumerateFiles(pluginsDir, "*.dll", enumeration)) {
            var rel = RelativeTo(pluginsDir, path);
            byRelName[rel] = new PluginInfo(rel, DisplayName(rel), Enabled: true);
        }

        // Disabled: plugins/**/*.dll.disabled, reported under its base .dll name.
        foreach (var path in Directory.EnumerateFiles(pluginsDir, "*.dll" + DisabledSuffix, enumeration)) {
            var rel = RelativeTo(pluginsDir, path);
            var baseName = rel[..^DisabledSuffix.Length];
            // If both A.dll and A.dll.disabled exist, prefer the enabled entry already recorded.
            if (!byRelName.ContainsKey(baseName)) {
                byRelName[baseName] = new PluginInfo(baseName, DisplayName(baseName), Enabled: false);
            }
        }

        return byRelName.Values
            .OrderBy(p => p.FileName, StringComparer.OrdinalIgnoreCase)
            .ToList();
    }

    public void SetEnabled(string bepinexRoot, string fileName, bool enabled) {
        if (string.IsNullOrWhiteSpace(fileName) || Path.IsPathRooted(fileName) ||
            fileName.Replace('\\', '/').Split('/').Any(seg => seg == ".."))
            throw new ArgumentException($"Unsafe plugin path: '{fileName}'.", nameof(fileName));

        var pluginsDir = Path.Combine(bepinexRoot, "plugins");
        var enabledPath = Install.FileTransaction.Resolve(bepinexRoot, Path.Combine("plugins", fileName));
        var disabledPath = Install.FileTransaction.Resolve(bepinexRoot, Path.Combine("plugins", fileName + DisabledSuffix));
        if (File.Exists(enabledPath) && File.Exists(disabledPath))
            throw new IOException($"Both '{fileName}' and its disabled copy exist. Resolve the duplicate before toggling.");

        if (enabled) {
            // Want A.dll. If it already exists, we're done. Otherwise rename A.dll.disabled -> A.dll.
            if (File.Exists(enabledPath)) {
                return;
            }
            if (File.Exists(disabledPath)) {
                File.Move(disabledPath, enabledPath);
                return;
            }
        } else {
            // Want A.dll.disabled. If it already exists, we're done. Otherwise rename A.dll -> A.dll.disabled.
            if (File.Exists(disabledPath)) {
                return;
            }
            if (File.Exists(enabledPath)) {
                File.Move(enabledPath, disabledPath);
                return;
            }
        }

        // Neither name exists: the toggle did nothing and the caller must know.
        throw new FileNotFoundException($"Plugin '{fileName}' not found under {pluginsDir} (enabled or disabled).");
    }

    static string RelativeTo(string pluginsDir, string path) =>
        Path.GetRelativePath(pluginsDir, path);

    static string DisplayName(string relName) =>
        Path.GetFileNameWithoutExtension(relName);
}
