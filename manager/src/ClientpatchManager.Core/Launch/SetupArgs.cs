namespace ClientpatchManager.Core.Launch;

/// <summary>What clientpatch asked this manager instance to finish before relaunching the game.</summary>
public enum SetupKind { Generate, Payload, Relaunch }

/// <summary>
/// One handoff from clientpatch. The game process has already exited; this instance does the
/// job, relaunches, and then exits itself.
/// </summary>
public sealed record SetupRequest(
    SetupKind Kind,
    string? DumpDir,
    string? GameDir,
    bool Relaunch,
    LaunchMethod LaunchMethod,
    string? PayloadUrl,
    string DoorstopName,
    string BepInExRoot,
    int? ParentPid = null);

/// <summary>
/// Parses the handoff command line clientpatch spawns:
/// <c>--generate-from-dump &lt;dumpDir&gt;</c> or <c>--setup payload|relaunch</c>, plus the shared
/// <c>--relaunch --launch-method steam|direct --game-dir &lt;dir&gt;</c> flags. Returns null for a
/// normal GUI launch.
/// </summary>
public static class SetupArgs {
    public const string SetupFlag = "--setup";
    public const string GenerateFromDumpFlag = "--generate-from-dump";
    public const string RelaunchFlag = "--relaunch";
    public const string LaunchMethodFlag = "--launch-method";
    public const string GameDirFlag = "--game-dir";
    public const string PayloadUrlFlag = "--payload-url";
    public const string DoorstopFlag = "--doorstop";
    public const string BepInExRootFlag = "--bepinex-root";

    public static SetupRequest? Parse(string[] args) {
        var handoff = false;
        var kind = SetupKind.Generate;
        string? dumpDir = null;
        var relaunch = false;
        var launchMethod = LaunchMethod.Steam;
        string? gameDir = null;
        string? payloadUrl = null;
        var doorstop = "doorstop.dll";
        var root = "BepInEx";
        int? parentPid = null;

        for (var i = 0; i < args.Length; i++) {
            var a = args[i];
            if (string.Equals(a, GenerateFromDumpFlag, StringComparison.OrdinalIgnoreCase)) {
                handoff = true;
                kind = SetupKind.Generate;
                if (NextValue(args, ref i) is { } dir)
                    dumpDir = dir;
            }
            else if (string.Equals(a, SetupFlag, StringComparison.OrdinalIgnoreCase)) {
                handoff = true;
                if (NextValue(args, ref i) is { } word)
                    kind = ParseKind(word);
            }
            else if (string.Equals(a, RelaunchFlag, StringComparison.OrdinalIgnoreCase)) {
                relaunch = true;
            }
            else if (string.Equals(a, LaunchMethodFlag, StringComparison.OrdinalIgnoreCase)) {
                if (NextValue(args, ref i) is { } method)
                    launchMethod = ParseLaunchMethod(method);
            }
            else if (string.Equals(a, GameDirFlag, StringComparison.OrdinalIgnoreCase)) {
                if (NextValue(args, ref i) is { } dir)
                    gameDir = dir;
            }
            else if (string.Equals(a, PayloadUrlFlag, StringComparison.OrdinalIgnoreCase)) {
                if (NextValue(args, ref i) is { } url)
                    payloadUrl = url;
            }
            else if (string.Equals(a, DoorstopFlag, StringComparison.OrdinalIgnoreCase)) {
                if (NextValue(args, ref i) is { } name)
                    doorstop = name;
            }
            else if (string.Equals(a, BepInExRootFlag, StringComparison.OrdinalIgnoreCase)) {
                if (NextValue(args, ref i) is { } name)
                    root = name;
            }
            else if (a.Equals("--parent-pid", StringComparison.OrdinalIgnoreCase) && NextValue(args, ref i) is { } pidText
                && int.TryParse(pidText, out var pid) && pid > 0) parentPid = pid;
        }

        if (!handoff)
            return null;
        return new SetupRequest(kind, dumpDir, gameDir, relaunch, launchMethod, payloadUrl, doorstop, root, parentPid);
    }

    /// <summary>The next token, unless it is another flag. Advances <paramref name="i"/> when consumed.</summary>
    private static string? NextValue(string[] args, ref int i) {
        if (i + 1 < args.Length && !args[i + 1].StartsWith("--", StringComparison.Ordinal))
            return args[++i];
        return null;
    }

    private static SetupKind ParseKind(string word) {
        if (string.Equals(word, "payload", StringComparison.OrdinalIgnoreCase))
            return SetupKind.Payload;
        if (string.Equals(word, "relaunch", StringComparison.OrdinalIgnoreCase))
            return SetupKind.Relaunch;
        return SetupKind.Generate;
    }

    public static LaunchMethod ParseLaunchMethod(string value) =>
        string.Equals(value, "direct", StringComparison.OrdinalIgnoreCase)
            ? LaunchMethod.Direct
            : LaunchMethod.Steam;
}
