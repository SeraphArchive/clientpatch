using System.Diagnostics;

namespace ClientpatchManager.Core.Install;

/// <summary>One writer per installation, across threads and manager processes.</summary>
public static class GameOperation {
    public static async Task WaitForStoppedAsync(string gameDir, CancellationToken ct) {
        while (true) {
            ct.ThrowIfCancellationRequested();
            try { RequireStopped(gameDir); return; }
            catch (IOException) { await Task.Delay(100, ct).ConfigureAwait(false); }
        }
    }
    public static IDisposable Acquire(string gameDir) {
        var lockPath = FileTransaction.Resolve(gameDir, Path.Combine(InstallLayout.DataDirName, ".manager-operation.lock"));
        Directory.CreateDirectory(Path.GetDirectoryName(lockPath)!);
        FileStream operation;
        try { operation = new FileStream(lockPath, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None); }
        catch (IOException ex) { throw new IOException("Another manager operation is using this installation. Wait for it to finish.", ex); }
        try {
            // Recover before callers inspect files to decide what the next mutation needs.
            FileTransaction.Recover(gameDir);
            return operation;
        }
        catch { operation.Dispose(); throw; }
    }

    public static void RequireStopped(string gameDir) {
        var exe = Path.GetFullPath(Path.Combine(gameDir, "HeavenBurnsRed.exe"));
        foreach (var process in Process.GetProcessesByName("HeavenBurnsRed")) {
            using (process) {
                try {
                    if (!process.HasExited && string.Equals(process.MainModule?.FileName, exe, StringComparison.OrdinalIgnoreCase))
                        throw new IOException("Close the game before changing this installation.");
                }
                catch (System.ComponentModel.Win32Exception ex) { throw new IOException("Cannot verify that the game is stopped. Close it before continuing.", ex); }
                catch (InvalidOperationException) { /* process exited while inspecting */ }
            }
        }
    }
}
