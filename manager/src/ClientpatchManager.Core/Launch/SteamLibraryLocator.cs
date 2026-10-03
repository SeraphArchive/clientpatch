using System.Text.RegularExpressions;

namespace ClientpatchManager.Core.Launch;

/// <summary>
/// Parses Steam's <c>libraryfolders.vdf</c> and probes library roots for the
/// Heaven Burns Red install. <see cref="ParseLibraryFolders"/> and
/// <see cref="ParseInstallDir"/> are pure string parsing (the unit-tested seams);
/// <see cref="FindHbrGameDir"/> touches the filesystem. No registry APIs are used
/// here — Steam root discovery is left to the App layer.
/// </summary>
public static class SteamLibraryLocator {
    /// <summary>Heaven Burns Red on Steam (store.steampowered.com/app/1973710,
    /// appmanifest_1973710.acf, name ヘブンバーンズレッド). 2117300 is a different app.</summary>
    public const string HbrAppId = "1973710";

    private const string HbrFolderName = "HeavenBurnsRed";

    // Matches the escaped library path in VDF entries.
    private static readonly Regex PathEntry = new(
        "\"path\"\\s*\"((?:[^\"\\\\]|\\\\.)*)\"",
        RegexOptions.Compiled | RegexOptions.IgnoreCase);

    // Matches the appmanifest entry:  "installdir"    "HeavenBurnsRed"
    private static readonly Regex InstallDirEntry = new(
        "\"installdir\"\\s*\"((?:[^\"\\\\]|\\\\.)*)\"",
        RegexOptions.Compiled | RegexOptions.IgnoreCase);

    /// <summary>
    /// Extracts the <c>"path"</c> values from a <c>libraryfolders.vdf</c> body,
    /// un-escaping VDF's backslash escapes (<c>\\</c> and <c>\"</c>).
    /// </summary>
    public static IReadOnlyList<string> ParseLibraryFolders(string vdfText) {
        if (string.IsNullOrEmpty(vdfText))
            return [];

        var results = new List<string>();
        foreach (Match m in PathEntry.Matches(vdfText)) {
            var unescaped = Unescape(m.Groups[1].Value);
            if (unescaped.Length > 0)
                results.Add(unescaped);
        }
        return results;
    }

    /// <summary>Extracts the <c>"installdir"</c> value from an <c>appmanifest_*.acf</c> body.</summary>
    public static string? ParseInstallDir(string acfText) {
        if (string.IsNullOrEmpty(acfText))
            return null;
        var m = InstallDirEntry.Match(acfText);
        return m.Success ? Unescape(m.Groups[1].Value) : null;
    }

    /// <summary>
    /// Probes each library root for the HBR install: first the folder its
    /// <c>appmanifest_1973710.acf</c> declares (handles non-default install dir names),
    /// then the default <c>&lt;root&gt;\steamapps\common\HeavenBurnsRed</c>.
    /// Returns the first that exists, or <c>null</c> if none do.
    /// </summary>
    public static string? FindHbrGameDir(IEnumerable<string> libraryRoots) {
        foreach (var root in libraryRoots) {
            if (string.IsNullOrWhiteSpace(root))
                continue;
            try {
                var manifestPath = Path.Combine(root, "steamapps", $"appmanifest_{HbrAppId}.acf");
                if (File.Exists(manifestPath)) {
                    var installDir = ParseInstallDir(File.ReadAllText(manifestPath));
                    if (!string.IsNullOrWhiteSpace(installDir)) {
                        var declared = Path.Combine(root, "steamapps", "common", installDir);
                        if (Directory.Exists(declared))
                            return declared;
                    }
                }

                var candidate = Path.Combine(root, "steamapps", "common", HbrFolderName);
                if (Directory.Exists(candidate))
                    return candidate;
            }
            catch {
                // Ignore inaccessible roots and keep probing.
            }
        }
        return null;
    }

    /// <summary>VDF string unescape: <c>\\</c> -&gt; <c>\</c>, <c>\"</c> -&gt; <c>"</c>.</summary>
    private static string Unescape(string raw) {
        var sb = new System.Text.StringBuilder(raw.Length);
        for (var i = 0; i < raw.Length; i++) {
            if (raw[i] == '\\' && i + 1 < raw.Length && (raw[i + 1] == '\\' || raw[i + 1] == '"')) {
                sb.Append(raw[i + 1]);
                i++;
            }
            else {
                sb.Append(raw[i]);
            }
        }
        return sb.ToString();
    }
}
