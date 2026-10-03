using ClientpatchManager.Core.Update;
using ClientpatchManager.Core.Backup;
using Xunit;

public class UpdateInstallerTests {
    static string Tmp(){ var d=Path.Combine(Path.GetTempPath(),Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }
    sealed class FakeDl : IFileDownloader {
        readonly string _content; public FakeDl(string c){_content=c;}
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct){
            var p=Path.GetTempFileName(); File.WriteAllText(p,_content); return Task.FromResult(p);
        }
    }
    static string Sha(string s){ using var h=System.Security.Cryptography.SHA256.Create();
        return Convert.ToHexString(h.ComputeHash(System.Text.Encoding.UTF8.GetBytes(s))).ToLowerInvariant(); }

    [Fact] public async Task Good_download_swaps_and_backs_up() {
        var d=Tmp(); var dest=Path.Combine(d,"version.dll"); File.WriteAllText(dest,"old");
        var inst=new UpdateInstaller(new FakeDl("new"), new BackupService());
        var r=await inst.InstallAsync(d, new("version.dll","http://x",3), "version.dll", Sha("new"), default);
        Assert.True(r.Ok);
        Assert.Equal("new", File.ReadAllText(dest));
        Assert.Single(new BackupService().List(d));
    }
    [Fact] public async Task Hash_mismatch_refuses_swap() {
        var d=Tmp(); var dest=Path.Combine(d,"version.dll"); File.WriteAllText(dest,"old");
        var inst=new UpdateInstaller(new FakeDl("tampered"), new BackupService());
        var r=await inst.InstallAsync(d, new("version.dll","http://x",3), "version.dll", Sha("expected"), default);
        Assert.False(r.Ok);
        Assert.Equal("old", File.ReadAllText(dest));
    }

    [Fact] public async Task Failed_swap_keeps_the_live_file_and_cleans_staging() {
        // Failure injection: the destination path is a directory, so the final rename of the
        // staged file must fail. The original destination and no staging leftovers must remain.
        var d=Tmp();
        var dest=Path.Combine(d,"version.dll");
        Directory.CreateDirectory(dest); // a directory where the file should be
        var inst=new UpdateInstaller(new FakeDl("new"), new BackupService());
        var r=await inst.InstallAsync(d, new("version.dll","http://x",3), "version.dll", Sha("new"), default);
        Assert.False(r.Ok);
        Assert.True(Directory.Exists(dest));                       // untouched
        Assert.False(File.Exists(dest + ".cpm-new"));              // staging cleaned
        Assert.False(File.Exists(dest + ".cpm-old"));              // no moved-aside leftover
    }
}
