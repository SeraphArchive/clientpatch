using System.Diagnostics;
using ClientpatchManager.Core.Models;

namespace ClientpatchManager.Core.Launch;

/// <summary>
/// Launches Heaven Burns Red either through Steam (so Steam's overlay/DRM init
/// runs) or by starting the game exe directly from the install directory.
/// Launch failures surface as <see cref="InvalidOperationException"/> with the
/// underlying cause, never as a silent no-op.
/// </summary>
public sealed class Launcher : ILauncher {
    /// <summary>The Heaven Burns Red game executable (verified on the real install).</summary>
    public const string GameExeName = "HeavenBurnsRed.exe";

    public void Launch(GameInstall install, LaunchMethod method) {
        ArgumentNullException.ThrowIfNull(install);

        switch (method) {
            case LaunchMethod.Steam:
                LaunchViaSteam();
                break;
            case LaunchMethod.Direct:
                LaunchDirect(install);
                break;
            default:
                throw new ArgumentOutOfRangeException(nameof(method), method, "Unknown launch method.");
        }
    }

    private static void LaunchViaSteam() {
        var uri = $"steam://rungameid/{SteamLibraryLocator.HbrAppId}";
        var psi = new ProcessStartInfo(uri) { UseShellExecute = true };
        try {
            // Process.Start returns null for a URL even when the shell handled it, so a null
            // result here is success, not failure. A real failure throws.
            Process.Start(psi);
        }
        catch (Exception ex) when (ex is not InvalidOperationException) {
            // e.g. no handler registered for steam:// (Steam not installed).
            throw new InvalidOperationException($"could not launch via Steam ({ex.Message})", ex);
        }
    }

    private static void LaunchDirect(GameInstall install) {
        var exePath = Path.Combine(install.GameDir, GameExeName);
        if (!File.Exists(exePath))
            throw new FileNotFoundException($"Game executable not found: {exePath}", exePath);

        var psi = new ProcessStartInfo(exePath) {
            UseShellExecute = true,
            WorkingDirectory = install.GameDir,
        };
        try {
            Process.Start(psi);
        }
        catch (Exception ex) {
            throw new InvalidOperationException($"could not start {exePath} ({ex.Message})", ex);
        }
    }
}
