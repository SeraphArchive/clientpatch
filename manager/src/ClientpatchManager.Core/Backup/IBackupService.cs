namespace ClientpatchManager.Core.Backup;

public record BackupEntry(string Id, DateTimeOffset TakenAt, IReadOnlyList<string> Files);

public interface IBackupService {
    /// <summary>All paths are relative to the game root, including clientpatch/ for owned data.</summary>
    BackupEntry Backup(string gameDir, IEnumerable<string> relativePaths);
    IReadOnlyList<BackupEntry> List(string gameDir);
    void Restore(string gameDir, string id);
    /// <summary>Returns true when a staged helper must finish after this manager exits.</summary>
    bool RestoreWithHandoff(string gameDir, string id) { Restore(gameDir, id); return false; }
}
