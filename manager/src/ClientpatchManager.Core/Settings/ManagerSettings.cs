using ClientpatchManager.Core.Launch;

namespace ClientpatchManager.Core.Settings;

/// <summary>
/// Persisted manager preferences. Serialized to <c>settings.json</c> by
/// <see cref="SettingsStore"/>. Pure BCL type: no WPF/Windows dependency so it
/// stays unit-testable from Core.
/// </summary>
/// <param name="GameDir">Detected/selected game directory, or null if not yet chosen.</param>
/// <param name="LaunchMethod">How to launch the game (Steam or Direct).</param>
/// <param name="FeedRepo">GitHub release feed repo in <c>owner/repo</c> form.</param>
/// <param name="Language">UI culture name (e.g. "ja"); null means follow the system.</param>
/// <param name="Theme">UI theme: "system", "light", or "dark".</param>
/// <param name="CheckOnLaunch">Check for updates when the manager starts.</param>
/// <param name="BackupBeforeChanges">Take a backup before applying changes.</param>
public record ManagerSettings(
    string? GameDir,
    LaunchMethod LaunchMethod,
    string FeedRepo,
    string? Language,
    string Theme,
    bool CheckOnLaunch,
    bool BackupBeforeChanges)
{
    /// <summary>Default feed repository for clientpatch releases.</summary>
    public const string DefaultFeedRepo = "SeraphArchive/clientpatch";

    /// <summary>The pre-release default feed repo. It never shipped anything; a persisted
    /// value is re-pointed to <see cref="DefaultFeedRepo"/> when settings load.</summary>
    public const string LegacyFeedRepo = "hbr-tools/clientpatch";

    public const string PreviousFeedRepo = "karerin-dev/clientpatch";

    /// <summary>First-run defaults: Steam launch, system language/theme, checks and backups on.</summary>
    public static ManagerSettings Default { get; } = new(
        GameDir: null,
        LaunchMethod: LaunchMethod.Steam,
        FeedRepo: DefaultFeedRepo,
        Language: null,
        Theme: "system",
        CheckOnLaunch: true,
        BackupBeforeChanges: true);
}
