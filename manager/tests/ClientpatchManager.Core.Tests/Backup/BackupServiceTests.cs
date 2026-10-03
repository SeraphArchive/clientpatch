using ClientpatchManager.Core.Backup;
using Xunit;

public class BackupServiceTests {
    static string Tmp() { var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }

    [Fact] public void Backup_then_restore_recovers_file() {
        var d = Tmp(); var f = Path.Combine(d, "clientpatch", "clientpatch.toml");
        Directory.CreateDirectory(Path.GetDirectoryName(f)!);
        File.WriteAllText(f, "good");
        var svc = new BackupService();
        var e = svc.Backup(d, new[]{ "clientpatch/clientpatch.toml" });
        File.WriteAllText(f, "broken");
        svc.Restore(d, e.Id);
        Assert.Equal("good", File.ReadAllText(f));
    }

    [Fact] public void List_returns_taken_backups() {
        var d = Tmp(); Directory.CreateDirectory(Path.Combine(d, "clientpatch"));
        File.WriteAllText(Path.Combine(d,"clientpatch","clientpatch.toml"), "x");
        var svc = new BackupService();
        svc.Backup(d, new[]{ "clientpatch/clientpatch.toml" });
        Assert.Single(svc.List(d));
    }

    [Fact] public void Restore_removes_a_file_the_mutation_created() {
        // The path did not exist at backup time, so rolling back must delete whatever appeared.
        var d = Tmp();
        var svc = new BackupService();
        var e = svc.Backup(d, new[]{ "clientpatch/langpacks/zhCN.json" });
        var created = Path.Combine(d, "clientpatch", "langpacks", "zhCN.json");
        Directory.CreateDirectory(Path.GetDirectoryName(created)!);
        File.WriteAllText(created, "new pack");
        svc.Restore(d, e.Id);
        Assert.False(File.Exists(created));
    }

    [Fact] public void Backup_rejects_paths_escaping_the_game_dir() {
        var d = Tmp();
        var svc = new BackupService();
        Assert.Throws<ArgumentException>(() => svc.Backup(d, new[]{ "../outside.txt" }));
        Assert.Throws<ArgumentException>(() => svc.Backup(d, new[]{ "C:/windows/win.ini" }));
        Assert.Throws<ArgumentException>(() => svc.Backup(d, new[]{ "sub/../../escape.txt" }));
    }
}
