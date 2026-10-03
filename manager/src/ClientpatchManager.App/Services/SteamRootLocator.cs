using System.IO;
using Microsoft.Win32;

namespace ClientpatchManager.App.Services;

/// <summary>
/// App-layer discovery of the Steam install root from the Windows registry, kept out
/// of Core (which stays registry-free). Core's <c>SteamLibraryLocator</c> parses the
/// library folders once we hand it the root's <c>libraryfolders.vdf</c>.
/// </summary>
public static class SteamRootLocator
{
    /// <summary>
    /// Reads <c>HKCU\Software\Valve\Steam\SteamPath</c> (falling back to the 64-bit
    /// HKLM WOW6432Node key). Returns null when Steam is not installed or unreadable.
    /// </summary>
    public static string? FindSteamRoot()
    {
        try
        {
            if (Registry.GetValue(@"HKEY_CURRENT_USER\Software\Valve\Steam", "SteamPath", null) is string p
                && !string.IsNullOrWhiteSpace(p))
                return p;

            if (Registry.GetValue(@"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath", null) is string ip
                && !string.IsNullOrWhiteSpace(ip))
                return ip;
        }
        catch
        {
            // Registry access can throw on locked-down machines; treat as "not found".
        }
        return null;
    }

    /// <summary>
    /// Reads the library roots from the Steam root's <c>steamapps/libraryfolders.vdf</c>.
    /// Returns the Steam root plus every parsed library path (deduplicated), or an empty
    /// list when nothing is found.
    /// </summary>
    public static IReadOnlyList<string> LibraryRoots(string steamRoot)
    {
        var roots = new List<string> { steamRoot };
        try
        {
            var vdf = Path.Combine(steamRoot, "steamapps", "libraryfolders.vdf");
            if (File.Exists(vdf))
            {
                var text = File.ReadAllText(vdf);
                foreach (var r in Core.Launch.SteamLibraryLocator.ParseLibraryFolders(text))
                    if (!roots.Contains(r))
                        roots.Add(r);
            }
        }
        catch
        {
            // Best-effort; return whatever we have.
        }
        return roots;
    }
}
