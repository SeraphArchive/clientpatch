using System.Diagnostics;

namespace ClientpatchManager.Core.Interop;

/// <summary>
/// Seam over process execution so the orchestration in <see cref="InteropService"/> can be
/// unit-tested with canned exit codes. Production wiring uses <see cref="ProcessToolRunner"/>.
/// </summary>
public interface IToolRunner {
    /// <summary>
    /// Runs <paramref name="exe"/> with <paramref name="args"/> in <paramref name="workingDir"/>,
    /// forwarding each captured stdout/stderr line to <paramref name="onOutput"/>, and returns the
    /// process exit code.
    /// </summary>
    Task<int> RunAsync(string exe, string args, string workingDir, Action<string> onOutput, CancellationToken ct);
}

/// <summary>
/// Real <see cref="IToolRunner"/> backed by <see cref="System.Diagnostics.Process"/>. Cross-platform
/// BCL; no WPF/Windows dependency. Merges stdout and stderr into the <c>onOutput</c> stream.
/// </summary>
public sealed class ProcessToolRunner : IToolRunner {
    public async Task<int> RunAsync(string exe, string args, string workingDir, Action<string> onOutput, CancellationToken ct) {
        var psi = new ProcessStartInfo {
            FileName = exe,
            Arguments = args,
            WorkingDirectory = workingDir,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            // Close stdin: upstream Il2CppDumper ends with Console.ReadKey when its config.json
            // has RequireAnyKey set, and a live stdin handle lets that prompt block the chain
            // forever instead of failing fast.
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        // Managed tools ship framework-dependent (Il2CppInterop.CLI is net6): let them run on
        // the .NET 10 runtime the manager itself requires, so players install nothing extra.
        psi.Environment["DOTNET_ROLL_FORWARD"] = "LatestMajor";

        using var proc = new Process { StartInfo = psi, EnableRaisingEvents = true };

        if (!proc.Start())
            throw new InvalidOperationException($"failed to start process: {exe}");

        proc.StandardInput.Close();
        var outputLock = new object();
        async Task PumpAsync(StreamReader reader) {
            try {
                while (await reader.ReadLineAsync(ct).ConfigureAwait(false) is { } line)
                    lock (outputLock) onOutput(line);
            }
            catch {
                try { if (!proc.HasExited) proc.Kill(entireProcessTree: true); } catch { /* process already gone */ }
                throw;
            }
        }
        var stdout = PumpAsync(proc.StandardOutput);
        var stderr = PumpAsync(proc.StandardError);

        try {
            await Task.WhenAll(proc.WaitForExitAsync(ct), stdout, stderr).ConfigureAwait(false);
        }
        catch (OperationCanceledException) {
            try { if (!proc.HasExited) proc.Kill(entireProcessTree: true); } catch { /* best-effort */ }
            throw;
        }

        return proc.ExitCode;
    }
}
