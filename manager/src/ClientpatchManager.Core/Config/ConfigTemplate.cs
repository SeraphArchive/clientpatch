using ClientpatchManager.Core.Install;

namespace ClientpatchManager.Core.Config;

/// <summary>Create the user's configuration from the shipped example without replacing it.</summary>
public static class ConfigTemplate {
    public static bool Ensure(string? gameDir) {
        if (string.IsNullOrWhiteSpace(gameDir)) return false;
        var config = FileTransaction.Resolve(gameDir, $"clientpatch/{InstallLayout.ConfigFileName}");
        var example = FileTransaction.Resolve(gameDir, $"clientpatch/{InstallLayout.ExampleConfigFileName}");
        if (File.Exists(config) || !File.Exists(example)) return false;
        using var operation = GameOperation.Acquire(gameDir);
        if (File.Exists(config)) return false;
        GameOperation.RequireStopped(gameDir);
        var temp = config + ".cpm-new-" + Guid.NewGuid().ToString("N");
        try {
            File.Copy(example, temp);
            // An external writer may have created the config after our check.
            // Moving without overwrite keeps that writer's file intact.
            try { File.Move(temp, config, overwrite: false); }
            catch (IOException) when (File.Exists(config)) { return false; }
            return true;
        } finally { if (File.Exists(temp)) File.Delete(temp); }
    }
}
