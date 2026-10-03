namespace ClientpatchManager.Core.Feed;

public enum Channel { Stable, Beta }

public record ReleaseAsset(string Name, string DownloadUrl, long Size);

public record ReleaseInfo(string Component, string Version, Channel Channel, IReadOnlyList<ReleaseAsset> Assets);

public record FeedResult(bool Ok, IReadOnlyList<ReleaseInfo> Releases, string? Error);

/// <summary>
/// Pure, network-free mapping from a GitHub release tag to a <see cref="ReleaseInfo"/>.
/// Tag convention: clientpatch-vX.Y.Z, langpacks-vX.Y.Z, tools-vX.Y.Z, bepinex-&lt;build&gt;.
/// Unknown prefixes return null (ignored).
/// </summary>
public static class ReleaseMapping {
    // Components using the "-vX.Y.Z" version convention.
    private static readonly string[] VersionedComponents = { "clientpatch", "langpacks", "tools" };

    public static ReleaseInfo? FromTag(string tag, bool prerelease, IReadOnlyList<ReleaseAsset> assets) {
        if (string.IsNullOrWhiteSpace(tag)) return null;

        var channel = prerelease ? Channel.Beta : Channel.Stable;

        // bepinex has no "-v" prefix; version is whatever follows "bepinex-".
        const string bepinexPrefix = "bepinex-";
        if (tag.StartsWith(bepinexPrefix, StringComparison.Ordinal)) {
            var build = tag.Substring(bepinexPrefix.Length);
            if (build.Length == 0) return null;
            return new ReleaseInfo("bepinex", build, channel, assets);
        }

        foreach (var component in VersionedComponents) {
            var prefix = component + "-v";
            if (tag.StartsWith(prefix, StringComparison.Ordinal)) {
                var version = tag.Substring(prefix.Length);
                if (version.Length == 0) return null;
                return new ReleaseInfo(component, version, channel, assets);
            }
        }

        return null;
    }
}

/// <summary>
/// Picks the newest release by semantic version, including prerelease precedence.
/// Components with opaque build IDs retain the feed's newest-first order.
/// </summary>
public static class ReleaseSelection {
    public static ReleaseInfo? Latest(IEnumerable<ReleaseInfo> releases, string component) =>
        releases.Where(r => r.Component == component)
            .OrderByDescending(r => r.Version, SemanticVersionComparer.Instance)
            .FirstOrDefault();

    /// <summary>Null means a version cannot be compared safely. Build metadata is ignored.</summary>
    public static int? CompareVersions(string? x, string? y) {
        var xp = Parse(x);
        var yp = Parse(y);
        if (xp is null || yp is null) return null;
        for (var i = 0; i < 3; i++) {
            var order = CompareNumber(xp.Value.Core[i], yp.Value.Core[i]);
            if (order != 0) return order;
        }
        var a = xp.Value.Pre;
        var b = yp.Value.Pre;
        if (a.Length == 0 || b.Length == 0) return (a.Length == 0).CompareTo(b.Length == 0);
        for (var i = 0; i < Math.Min(a.Length, b.Length); i++) {
            var an = a[i].All(char.IsAsciiDigit);
            var bn = b[i].All(char.IsAsciiDigit);
            var order = an && bn ? CompareNumber(a[i], b[i])
                : an != bn ? (an ? -1 : 1) : string.CompareOrdinal(a[i], b[i]);
            if (order != 0) return order;
        }
        return a.Length.CompareTo(b.Length);
    }

    private static int CompareNumber(string a, string b) =>
        a.Length != b.Length ? a.Length.CompareTo(b.Length) : string.CompareOrdinal(a, b);

    private static (string[] Core, string[] Pre)? Parse(string? version) {
        if (version is null) return null;
        var match = System.Text.RegularExpressions.Regex.Match(version,
            @"\A(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\z");
        if (!match.Success) return null;
        var pre = match.Groups[4].Success ? match.Groups[4].Value.Split('.') : Array.Empty<string>();
        if (pre.Any(p => p.Length > 1 && p[0] == '0' && p.All(char.IsAsciiDigit))) return null;
        return (new[] { match.Groups[1].Value, match.Groups[2].Value, match.Groups[3].Value }, pre);
    }

    private sealed class SemanticVersionComparer : IComparer<string> {
        public static readonly SemanticVersionComparer Instance = new();
        public int Compare(string? x, string? y) {
            // A total ordering avoids inconsistent comparisons when malformed and valid tags mix.
            return CompareVersions(x, y) ?? (Parse(x) is not null).CompareTo(Parse(y) is not null);
        }
    }
}
