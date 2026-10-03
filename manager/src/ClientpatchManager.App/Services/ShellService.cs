using System.Diagnostics;
using System.IO;
using Microsoft.Win32;

namespace ClientpatchManager.App.Services;

/// <summary>
/// Thin wrapper over OS shell affordances used by pages: folder picking, opening a
/// folder in Explorer, and opening a file with its default handler. Isolated here so
/// view-models stay free of direct Win32/Process calls and can be swapped in tests.
/// </summary>
public sealed class ShellService
{
    /// <summary>Show a folder picker starting at <paramref name="initialDir"/>; null if cancelled.</summary>
    public string? PickFolder(string? initialDir)
    {
        var dlg = new OpenFolderDialog { Multiselect = false };
        if (!string.IsNullOrWhiteSpace(initialDir) && Directory.Exists(initialDir))
            dlg.InitialDirectory = initialDir;
        return dlg.ShowDialog() == true ? dlg.FolderName : null;
    }

    /// <summary>Open a folder in Explorer. No-op when the path is missing.</summary>
    public void OpenFolder(string? path)
    {
        if (string.IsNullOrWhiteSpace(path) || !Directory.Exists(path))
            return;
        Process.Start(new ProcessStartInfo(path) { UseShellExecute = true });
    }

    /// <summary>Open a file with its default handler. No-op when the file is missing.</summary>
    public void OpenFile(string? path)
    {
        if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
            return;
        Process.Start(new ProcessStartInfo(path) { UseShellExecute = true });
    }
}
