namespace ClientpatchManager.Core.Interop;

/// <summary>The three interop tools bundled in the <c>tools-vX.Y.Z</c> release.</summary>
public enum InteropTool { Senbei, Il2CppDumper, Il2CppInteropCli }

/// <summary>
/// On-demand local cache for the interop tools. A tool is cached only when its
/// versioned directory holds a <c>.ready</c> marker (a partial extraction is not cached).
/// </summary>
public interface IToolCache {
    /// <summary>True iff the tool's version dir contains a <c>.ready</c> marker.</summary>
    bool IsCached(InteropTool tool);

    /// <summary>
    /// Returns the cached tool directory, downloading and extracting the <c>tools</c>
    /// bundle first if not already present.
    /// </summary>
    Task<string> EnsureAsync(InteropTool tool, CancellationToken ct);
}
