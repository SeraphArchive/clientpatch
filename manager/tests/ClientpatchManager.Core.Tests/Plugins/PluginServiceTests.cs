using ClientpatchManager.Core.Plugins;
using Xunit;

public class PluginServiceTests {
    static string Root(){ var d=Path.Combine(Path.GetTempPath(),Path.GetRandomFileName());
        Directory.CreateDirectory(Path.Combine(d,"plugins")); return d; }

    [Fact] public void Scan_reports_enabled_and_disabled() {
        var r=Root();
        File.WriteAllText(Path.Combine(r,"plugins","A.dll"),"x");
        File.WriteAllText(Path.Combine(r,"plugins","B.dll.disabled"),"x");
        var list=new PluginService().Scan(r);
        Assert.Contains(list, p=>p.FileName=="A.dll" && p.Enabled);
        Assert.Contains(list, p=>p.FileName=="B.dll" && !p.Enabled);
    }

    [Fact] public void SetEnabled_false_then_true_round_trips() {
        var r=Root(); File.WriteAllText(Path.Combine(r,"plugins","A.dll"),"x");
        var svc=new PluginService();
        svc.SetEnabled(r,"A.dll",false);
        Assert.True(File.Exists(Path.Combine(r,"plugins","A.dll.disabled")));
        svc.SetEnabled(r,"A.dll",true);
        Assert.True(File.Exists(Path.Combine(r,"plugins","A.dll")));
    }

    [Fact] public void Scan_finds_plugins_in_subdirectories() {
        var r=Root();
        var sub=Path.Combine(r,"plugins","Author-Pack");
        Directory.CreateDirectory(sub);
        File.WriteAllText(Path.Combine(sub,"My.Plugin.dll"),"x");
        var list=new PluginService().Scan(r);
        var entry=Assert.Single(list);
        Assert.Equal("My.Plugin", entry.DisplayName); // full base name, not truncated at the first dot
        Assert.Equal("Author-Pack/My.Plugin.dll", entry.FileName.Replace('\\','/'));
    }

    [Fact] public void SetEnabled_round_trips_in_subdirectory() {
        var r=Root();
        var sub=Path.Combine(r,"plugins","Author-Pack");
        Directory.CreateDirectory(sub);
        File.WriteAllText(Path.Combine(sub,"My.Plugin.dll"),"x");
        var svc=new PluginService();
        var rel="Author-Pack"+Path.DirectorySeparatorChar+"My.Plugin.dll";
        svc.SetEnabled(r,rel,false);
        Assert.True(File.Exists(Path.Combine(sub,"My.Plugin.dll.disabled")));
        svc.SetEnabled(r,rel,true);
        Assert.True(File.Exists(Path.Combine(sub,"My.Plugin.dll")));
    }

    [Fact] public void SetEnabled_throws_when_plugin_is_missing() {
        var r=Root();
        Assert.Throws<FileNotFoundException>(()=>new PluginService().SetEnabled(r,"Nope.dll",false));
    }

    [Fact] public void SetEnabled_rejects_traversal() {
        var r=Root();
        Assert.Throws<ArgumentException>(()=>new PluginService().SetEnabled(r,"../evil.dll",false));
    }
}
