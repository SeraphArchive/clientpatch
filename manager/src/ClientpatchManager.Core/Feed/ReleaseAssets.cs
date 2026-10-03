namespace ClientpatchManager.Core.Feed;

/// <summary>Artifact selection is explicit; checksum and other-platform assets never become payloads.</summary>
public static class ReleaseAssets {
    public static ReleaseAsset Select(ReleaseInfo release) {
        var matches = release.Assets.Where(a => release.Component switch {
            "clientpatch" => a.Name.Equals($"clientpatch-v{release.Version}-win-x64.zip", StringComparison.OrdinalIgnoreCase),
            "bepinex" => a.Name.StartsWith("BepInEx-Unity.IL2CPP-win-x64-", StringComparison.OrdinalIgnoreCase) && a.Name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase),
            _ => false
        }).ToList();
        return matches.Count == 1 ? matches[0] : throw new InvalidOperationException($"Release {release.Component} {release.Version} must contain exactly one Windows x64 payload; found {matches.Count}.");
    }
}
