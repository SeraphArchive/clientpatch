using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Models;
using Xunit;

public class InstallDetectorTests {
    static string Tmp() { var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }

    [Fact] public void Missing_game_is_NotFound() {
        Assert.Equal(DeploymentState.NotFound, new InstallDetector().Detect(Tmp()).State);
    }
    [Fact] public void Game_without_proxy_is_GameOnly() {
        var d = Tmp(); File.WriteAllText(Path.Combine(d,"GameAssembly.dll"),"x");
        Assert.Equal(DeploymentState.GameOnly, new InstallDetector().Detect(d).State);
    }
    [Fact] public void Game_with_proxy_is_Deployed() {
        var d = Tmp();
        File.WriteAllText(Path.Combine(d,"GameAssembly.dll"),"x");
        File.WriteAllText(Path.Combine(d,"version.dll"),"x");
        Directory.CreateDirectory(Path.Combine(d, "clientpatch"));
        File.WriteAllText(Path.Combine(d,"clientpatch","clientpatch.toml"),"[loader]");
        Assert.Equal(DeploymentState.Deployed, new InstallDetector().Detect(d).State);
    }
    [Fact] public void Nonexistent_dir_is_NotFound_not_throw() {
        Assert.Equal(DeploymentState.NotFound, new InstallDetector().Detect("Q:/nope/x").State);
    }
    [Fact] public void Deployed_version_comes_from_the_manager_sidecar() {
        var d = Tmp();
        File.WriteAllText(Path.Combine(d,"GameAssembly.dll"),"x");
        File.WriteAllText(Path.Combine(d,"version.dll"),"x");
        File.WriteAllText(Path.Combine(d,"manager.exe"),"x");
        Directory.CreateDirectory(Path.Combine(d, "clientpatch"));
        File.WriteAllText(Path.Combine(d,"clientpatch",InstallDetector.VersionSidecarName),"1.4.2\n");
        var install = new InstallDetector().Detect(d);
        Assert.Equal("1.4.2", install.DeployedVersion);
    }
    [Theory]
    [InlineData("version.dll")]
    [InlineData("manager.exe")]
    public void Incomplete_bundle_does_not_report_the_shared_version(string presentFile) {
        var d = Tmp();
        File.WriteAllText(Path.Combine(d, "GameAssembly.dll"), "game");
        File.WriteAllText(Path.Combine(d, presentFile), "present");
        Directory.CreateDirectory(Path.Combine(d, "clientpatch"));
        File.WriteAllText(Path.Combine(d, "clientpatch", InstallDetector.VersionSidecarName), "1.4.2");
        Assert.Null(new InstallDetector().Detect(d).DeployedVersion);
    }
    [Fact] public void Config_beside_the_exe_is_ignored() {
        var d = Tmp();
        File.WriteAllText(Path.Combine(d,"GameAssembly.dll"),"x");
        File.WriteAllText(Path.Combine(d,"clientpatch.toml"),"not here");
        Assert.False(File.Exists(new InstallDetector().Detect(d).ConfigPath));
    }
    [Fact] public void Deployed_version_is_null_without_a_sidecar() {
        var d = Tmp();
        File.WriteAllText(Path.Combine(d,"GameAssembly.dll"),"x");
        File.WriteAllText(Path.Combine(d,"version.dll"),"x");
        Assert.Null(new InstallDetector().Detect(d).DeployedVersion);
    }
}
