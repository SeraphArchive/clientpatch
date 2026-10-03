using ClientpatchManager.Core.Install;
using Xunit;

public class InstallLayoutTests {
    static string Tmp() { var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }

    [Fact] public void LatestLog_is_null_without_a_logs_dir() {
        Assert.Null(InstallLayout.LatestLog(Tmp(), null));
    }
    [Fact] public void LatestLog_picks_the_newest_file_for_the_stem() {
        var d = Tmp();
        var logs = InstallLayout.LogsDir(d);
        Directory.CreateDirectory(logs);
        var old = Path.Combine(logs, "clientpatch_20260101-000000.log");
        var newer = Path.Combine(logs, "clientpatch_20260102-000000.log");
        File.WriteAllText(old, "a");
        Thread.Sleep(20); // distinct write times
        File.WriteAllText(newer, "b");
        Assert.Equal(newer, InstallLayout.LatestLog(d, null));
    }
    [Fact] public void LatestLog_matches_only_the_configured_stem() {
        var d = Tmp();
        var logs = InstallLayout.LogsDir(d);
        Directory.CreateDirectory(logs);
        var other = Path.Combine(logs, "other_20260103-000000.log");
        var mine = Path.Combine(logs, "clientpatch_20260101-000000.log");
        File.WriteAllText(other, "a");
        Thread.Sleep(20);
        File.WriteAllText(mine, "b");
        Assert.Equal(mine, InstallLayout.LatestLog(d, null));
        Assert.Equal(other, InstallLayout.LatestLog(d, "other.log"));
    }
    [Fact] public void LatestLog_ignores_a_directory_in_the_configured_stem() {
        var d = Tmp();
        var logs = InstallLayout.LogsDir(d);
        Directory.CreateDirectory(logs);
        var mine = Path.Combine(logs, "clientpatch_20260101-000000.log");
        File.WriteAllText(mine, "a");
        Assert.Equal(mine, InstallLayout.LatestLog(d, @"..\..\clientpatch.log"));
    }
}
