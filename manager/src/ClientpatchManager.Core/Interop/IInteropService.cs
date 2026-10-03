namespace ClientpatchManager.Core.Interop;

/// <summary>
/// Whether the generated interop assemblies match the installed game build.
/// <paramref name="Current"/> is true when the on-disk <c>GameAssembly.dll</c> fingerprint
/// matches a completed interop marker; <paramref name="BuildId"/> is the marker's build id
/// (null when nothing is generated); <paramref name="AssemblyCount"/> is the number of
/// interop <c>*.dll</c> assemblies present for that build.
/// </summary>
public record InteropStatus(bool Current, string? BuildId, int AssemblyCount);

/// <summary>
/// Ordered stages shown while interop is prepared. Unpack/Dump/Generate are the offline chain.
/// Install and Relaunch are the runtime handoff only (payload download, then starting the game).
/// </summary>
public enum InteropStep { Unpack, Dump, Generate, Install, Relaunch }

/// <summary>Outcome of one <see cref="InteropStep"/>: the step, whether it succeeded, and its captured output.</summary>
public record StepResult(InteropStep Step, bool Ok, string Output);

/// <summary>
/// Checks interop currency for an installed build and orchestrates the offline generation
/// chain (senbei unpack -&gt; Il2CppDumper -&gt; Il2CppInterop.CLI), stopping on the first
/// failing step and only promoting the output after the final step succeeds.
/// </summary>
public interface IInteropService {
    /// <summary>Mirrors clientpatch's currency decision: latest.json exists, its
    /// game_assembly_file_fingerprint matches the on-disk GameAssembly.dll, and the
    /// BepInEx marker's build_id equals latest.json's. <paramref name="dumpRootName"/> is
    /// the configured [interopdump] out_dir (null = the default "interop").</summary>
    InteropStatus CheckCurrency(string gameDir, string? dumpRootName = null);

    /// <summary>
    /// Runs the offline chain against an installed game directory. Returns the step results
    /// collected so far, stopping immediately at the first non-zero exit. On success the
    /// build id comes from senbei's DECRYPTED metadata (matching clientpatch), and the
    /// interop dir, marker, dump set, and latest.json are all written. When
    /// <paramref name="console"/> is supplied, every tool output line is forwarded as it
    /// arrives (the step results still carry each step's full text).
    /// </summary>
    Task<IReadOnlyList<StepResult>> GenerateOfflineAsync(string gameDir, string? dumpRootName, IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null);

    /// <summary>
    /// Runs the runtime-dump chain: the dumper consumes an already-dumped memory image
    /// (IsDumped mode) from <paramref name="dumpDir"/>, then Il2CppInterop.CLI generates.
    /// When <paramref name="gameDir"/> is non-null, the generated assemblies are installed into
    /// <c>&lt;gameDir&gt;/BepInEx/interop</c> with the <c>clientpatch-interop.json</c> currency
    /// marker clientpatch reads; when null, generation runs but no install is performed.
    /// <paramref name="console"/> receives tool output lines live, like the offline path.
    /// </summary>
    Task<IReadOnlyList<StepResult>> GenerateFromDumpAsync(string dumpDir, string? gameDir, IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null);
}
