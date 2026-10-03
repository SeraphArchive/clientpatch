using System.Diagnostics;
using ClientpatchManager.Core.Models;

namespace ClientpatchManager.Core.Install;

/// <summary>
/// Pure file-system probing of a game folder. Never throws on a bad path:
/// an inaccessible or nonexistent directory yields a <see cref="DeploymentState.NotFound"/> install.
/// </summary>
public sealed class InstallDetector : IInstallDetector {
    private const string GameAssemblyName = "GameAssembly.dll";
    private const string VersionDllName = "version.dll";

    /// <summary>Sidecar the manager itself writes after installing a clientpatch update;
    /// the deployed version of a pre-existing install cannot be detected (the proxy does
    /// not stamp its version anywhere on disk) and reads as null -> "unknown".</summary>
    public const string VersionSidecarName = "clientpatch.version";

    public GameInstall Detect(string gameDir) {
        try {
            var gameAssemblyPath = Path.Combine(gameDir, GameAssemblyName);
            if (!File.Exists(gameAssemblyPath))
                return NotFound(gameDir);

            var versionDllPath = Path.Combine(gameDir, VersionDllName);
            var hasProxy = File.Exists(versionDllPath);

            var configPath = InstallLayout.Resolve(gameDir, InstallLayout.ConfigFileName);
            var hasConfig = File.Exists(configPath);

            var deployedVersion = ReadDeployedVersion(gameDir, hasProxy ? versionDllPath : null);

            var state = hasProxy ? DeploymentState.Deployed : DeploymentState.GameOnly;

            return new GameInstall(
                GameDir: gameDir,
                VersionDllPath: hasProxy ? versionDllPath : null,
                ConfigPath: hasConfig ? configPath : null,
                GameAssemblyPath: gameAssemblyPath,
                DeployedVersion: deployedVersion,
                State: state);
        }
        catch {
            // Bad/inaccessible path must not throw; report NotFound.
            return NotFound(gameDir);
        }
    }

    private static string? ReadDeployedVersion(string gameDir, string? versionDllPath) {
        // A shared version only describes the complete loader/manager bundle.
        if (versionDllPath is null || !File.Exists(Path.Combine(gameDir, "manager.exe"))) return null;
        // 1. The sidecar the manager writes after its own updates.
        try {
            var sidecar = InstallLayout.Resolve(gameDir, VersionSidecarName);
            if (File.Exists(sidecar)) {
                var text = File.ReadAllText(sidecar).Trim();
                if (text.Length > 0)
                    return text;
            }
        }
        catch { /* fall through to the PE check */ }

        // 2. PE version info on version.dll, if the build embeds it.
        try {
            if (versionDllPath is not null) {
                var info = FileVersionInfo.GetVersionInfo(versionDllPath);
                var v = info.ProductVersion ?? info.FileVersion;
                if (!string.IsNullOrWhiteSpace(v))
                    return v;
            }
        }
        catch { /* no version resource */ }

        return null;
    }

    private static GameInstall NotFound(string gameDir) =>
        new(gameDir, null, null, null, null, DeploymentState.NotFound);
}
