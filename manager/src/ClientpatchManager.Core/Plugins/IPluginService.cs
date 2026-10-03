namespace ClientpatchManager.Core.Plugins;

/// <summary>
/// A BepInEx plugin discovered under <c>plugins/</c>.
/// <paramref name="FileName"/> is the base <c>.dll</c> name (e.g. "A.dll") for BOTH
/// enabled and disabled entries; callers refer to a plugin by that name regardless of state.
/// </summary>
public record PluginInfo(string FileName, string DisplayName, bool Enabled);

public interface IPluginService {
    IReadOnlyList<PluginInfo> Scan(string bepinexRoot);
    void SetEnabled(string bepinexRoot, string fileName, bool enabled);
}
