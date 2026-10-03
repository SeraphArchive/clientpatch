using System.IO.Compression;
using System.Reflection;
using System.Text.Json;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Feed;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Interop;
using ClientpatchManager.Core.Update;
using Mono.Cecil;

public sealed class TransactionAndInteropRegressionTests : IDisposable {
    readonly string root = Path.Combine(Path.GetTempPath(), "cpm-transaction-test-" + Guid.NewGuid());
    public TransactionAndInteropRegressionTests() => Directory.CreateDirectory(root);
    public void Dispose() => Directory.Delete(root, true);
    string Write(string relative, string text) { var path = Path.Combine(root, relative); Directory.CreateDirectory(Path.GetDirectoryName(path)!); File.WriteAllText(path, text); return path; }

    [Fact] public void Late_failure_restores_already_replaced_files_and_currency() {
        Write("a.dll", "old-a"); var b = Write("b.dll", "old-b"); Write("marker", "old-marker");
        var aNew = Write("staging/a", "new-a"); var bNew = Write("staging/b", "new-b"); var marker = Write("staging/marker", "new-marker");
        using var backup = new LockAfterPreflight(b); using var operation = GameOperation.Acquire(root);
        var error = Record.Exception(() => FileTransaction.Commit(root, new Dictionary<string,string?> { ["a.dll"] = aNew, ["b.dll"] = bNew, ["marker"] = marker }, backup, "marker"));
        Assert.True(error is IOException or UnauthorizedAccessException);
        Assert.Equal("old-a", File.ReadAllText(Path.Combine(root,"a.dll"))); Assert.Equal("old-marker", File.ReadAllText(Path.Combine(root,"marker")));
    }
    [Fact] public void Interrupted_transaction_is_recovered_before_next_commit() {
        Write("a.dll", "partial"); Write("clientpatch/.cpm-transaction-interrupted/before/a.dll", "original");
        Write("clientpatch/.cpm-transaction-interrupted/journal.json", JsonSerializer.Serialize(new { Originals = new[] { new { RelativePath = "a.dll", Exists = true } }, Gate = (string?)null }));
        var source = Write("staging/next", "next"); using var operation = GameOperation.Acquire(root);
        FileTransaction.Commit(root, new Dictionary<string,string?> { ["next.dll"] = source });
        Assert.Equal("original", File.ReadAllText(Path.Combine(root,"a.dll"))); Assert.Equal("next", File.ReadAllText(Path.Combine(root,"next.dll")));
    }
    [Fact] public void Operation_lock_rejects_a_second_writer() {
        using var first = GameOperation.Acquire(root); Assert.Throws<IOException>(() => GameOperation.Acquire(root));
    }
    [Fact] public void Failed_recovery_hides_currency_and_releases_the_operation_lock() {
        var live = Write("a.dll", "partial"); Write("marker", "partial-marker");
        Write("clientpatch/.cpm-transaction-interrupted/before/a.dll", "original");
        Write("clientpatch/.cpm-transaction-interrupted/before/marker", "original-marker");
        Write("clientpatch/.cpm-transaction-interrupted/journal.json", JsonSerializer.Serialize(new {
            Originals = new[] { new { RelativePath = "marker", Exists = true }, new { RelativePath = "a.dll", Exists = true } }, Gate = "marker"
        }));
        using (var held = new FileStream(live, FileMode.Open, FileAccess.ReadWrite, FileShare.Read)) {
            Assert.True(Record.Exception(() => GameOperation.Acquire(root)) is IOException or UnauthorizedAccessException);
            Assert.False(File.Exists(Path.Combine(root, "marker")));
        }
        using var operation = GameOperation.Acquire(root);
        Assert.Equal("original", File.ReadAllText(live));
        Assert.Equal("original-marker", File.ReadAllText(Path.Combine(root, "marker")));
    }
    [Fact] public void Incomplete_recovery_snapshot_does_not_partially_restore_live_files() {
        var live = Write("a.dll", "live-a"); var marker = Write("marker", "live-marker");
        Write("clientpatch/.cpm-transaction-interrupted/before/a.dll", "old-a");
        Write("clientpatch/.cpm-transaction-interrupted/before/marker", "old-marker");
        Write("clientpatch/.cpm-transaction-interrupted/journal.json", JsonSerializer.Serialize(new {
            Originals = new[] { new { RelativePath = "a.dll", Exists = true }, new { RelativePath = "missing.dll", Exists = true }, new { RelativePath = "marker", Exists = true } }, Gate = "marker"
        }));
        Assert.Throws<InvalidDataException>(() => GameOperation.Acquire(root));
        Assert.Equal("live-a", File.ReadAllText(live)); Assert.Equal("live-marker", File.ReadAllText(marker));
    }
    [Fact] public void Recovery_does_not_read_snapshot_files_through_a_junction() {
        var live = Write("a.dll", "live-a");
        var outside = Path.Combine(root, "outside"); Directory.CreateDirectory(outside);
        File.WriteAllText(Path.Combine(outside, "a.dll"), "external");
        var txn = Path.Combine(root, "clientpatch", ".cpm-transaction-linked"); Directory.CreateDirectory(txn);
        var link = Path.Combine(txn, "before"); CreateJunction(link, outside);
        File.WriteAllText(Path.Combine(txn, "journal.json"), JsonSerializer.Serialize(new {
            Originals = new[] { new { RelativePath = "a.dll", Exists = true } }, Gate = (string?)null
        }));
        try {
            Assert.Throws<IOException>(() => GameOperation.Acquire(root));
            Assert.Equal("live-a", File.ReadAllText(live));
        }
        finally { Directory.Delete(link); }
    }
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void Operation_and_staging_reject_data_directory_junctions_before_writing(bool dangling) {
        var game = Path.Combine(root, "game"); Directory.CreateDirectory(game);
        var outside = Path.Combine(root, "outside"); Directory.CreateDirectory(outside);
        var link = Path.Combine(game, "clientpatch");
        CreateJunction(link, outside);
        try {
            if (dangling) Directory.Delete(outside);
            Assert.Throws<IOException>(() => GameOperation.Acquire(game));
            Assert.Throws<IOException>(() => FileTransaction.NewStage(game));
            if (!dangling) Assert.Empty(Directory.GetFileSystemEntries(outside));
        }
        finally { Directory.Delete(link); }
    }
    [Fact] public void Plugin_scan_and_toggle_do_not_follow_a_directory_junction() {
        var bepinex = Path.Combine(root, "BepInEx"); Directory.CreateDirectory(Path.Combine(bepinex, "plugins"));
        var outside = Path.Combine(root, "outside"); Directory.CreateDirectory(outside);
        var plugin = Path.Combine(outside, "External.dll"); File.WriteAllText(plugin, "external");
        var link = Path.Combine(bepinex, "plugins", "linked");
        CreateJunction(link, outside);
        try {
            var service = new ClientpatchManager.Core.Plugins.PluginService();
            Assert.Empty(service.Scan(bepinex));
            Assert.Throws<IOException>(() => service.SetEnabled(bepinex, "linked/External.dll", false));
            Assert.Equal("external", File.ReadAllText(plugin));
            Assert.False(File.Exists(plugin + ".disabled"));
        }
        finally { Directory.Delete(link); }
    }
    [Fact] public void Rollback_of_a_different_installation_does_not_request_a_handoff() {
        var manager = Write("manager.exe", "old-manager"); var loader = Write("version.dll", "old-loader");
        var backups = new BackupService();
        var snapshot = backups.Backup(root, new[] { "manager.exe", "version.dll" });
        File.WriteAllText(manager, "new-manager"); File.WriteAllText(loader, "new-loader");
        Assert.False(backups.RestoreWithHandoff(root, snapshot.Id));
        Assert.Equal("old-manager", File.ReadAllText(manager)); Assert.Equal("old-loader", File.ReadAllText(loader));
    }
    [Fact] public void Rollback_rejects_a_backup_without_its_manifest() {
        var live = Write("version.dll", "live");
        Write("clientpatch/.clientpatch-backups/incomplete/.layout", "game-relative-v1");
        Write("clientpatch/.clientpatch-backups/incomplete/version.dll", "old");
        Assert.Throws<InvalidDataException>(() => new BackupService().RestoreWithHandoff(root, "incomplete"));
        Assert.Equal("live", File.ReadAllText(live));
    }
    static void CreateJunction(string link, string target) {
        var start = new System.Diagnostics.ProcessStartInfo("cmd.exe") { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in new[] { "/c", "mklink", "/J", link, target }) start.ArgumentList.Add(arg);
        using var process = System.Diagnostics.Process.Start(start)!; process.WaitForExit();
        Assert.True(process.ExitCode == 0, process.StandardError.ReadToEnd());
    }
    [Fact] public void Legacy_config_backup_has_an_unambiguous_restore_path() {
        Write("clientpatch/clientpatch.toml", "new");
        Write("clientpatch/.clientpatch-backups/legacy/.manifest", "clientpatch.toml");
        Write("clientpatch/.clientpatch-backups/legacy/clientpatch.toml", "old");
        new BackupService().Restore(root,"legacy");
        Assert.Equal("old",File.ReadAllText(Path.Combine(root,"clientpatch","clientpatch.toml")));
    }
    [Fact] public void Legacy_broken_dll_backup_never_deletes_the_live_dll() {
        var live = Write("version.dll", "good"); Write("clientpatch/.clientpatch-backups/legacy/.manifest", "version.dll");
        Assert.Throws<IOException>(() => new BackupService().Restore(root,"legacy")); Assert.Equal("good",File.ReadAllText(live));
    }
    [Fact] public void Empty_interop_is_rejected_without_touching_live_set() {
        var old = Write("BepInEx/interop/Assembly-CSharp.dll", "old"); Write("BepInEx/interop/clientpatch-interop.json", "old-marker");
        var staged = Path.Combine(root, "empty"); Directory.CreateDirectory(staged);
        var service = new InteropService(new ConcurrentOutput(), new EmptyCache());
        var error = Assert.Throws<TargetInvocationException>(() => typeof(InteropService).GetMethod("Install", BindingFlags.NonPublic | BindingFlags.Instance)!.Invoke(service, new object?[] { root, staged, "build", null }));
        Assert.IsType<InvalidDataException>(error.InnerException); Assert.Equal("old", File.ReadAllText(old));
    }
    [Fact] public async Task Concurrent_output_is_captured_without_lost_or_corrupt_lines() {
        var service = new InteropService(new ConcurrentOutput(), new EmptyCache());
        var results = new List<StepResult>();
        var task = (Task<bool>)typeof(InteropService).GetMethod("RunStepAsync", BindingFlags.NonPublic | BindingFlags.Instance)!.Invoke(service,
            new object?[] { InteropStep.Dump, "fake", "", root, new Progress<StepResult>(), results, CancellationToken.None, null, null })!;
        Assert.True(await task);
        Assert.Equal(2000 * (100 + Environment.NewLine.Length) - Environment.NewLine.Length, results.Single().Output.Length);
        Assert.Equal(2000, results.Single().Output.Split(Environment.NewLine).Length);
    }
    [Fact] public async Task Cached_tool_survives_release_api_outage() {
        Write("tools/Senbei/v1/.ready", "");
        var cache = new ToolCache(root, new Download("never"), new OfflineSource());
        Assert.Equal(Path.Combine(root,"tools","Senbei","v1"), await cache.EnsureAsync(InteropTool.Senbei, default));
    }
    [Fact] public void Release_asset_selection_rejects_other_platforms_and_ambiguity() {
        var payload = new ReleaseAsset("clientpatch-v1-win-x64.zip", "zip", 1); var checksum = new ReleaseAsset("clientpatch-v1-win-x64.zip.sha256", "hash", 64);
        Assert.Same(payload, ReleaseAssets.Select(new("clientpatch","1",Channel.Stable,new[] {checksum,payload})));
        Assert.Throws<InvalidOperationException>(() => ReleaseAssets.Select(new("clientpatch","1",Channel.Stable,new[] {checksum})));
        Assert.Throws<InvalidOperationException>(() => ReleaseAssets.Select(new("clientpatch","1",Channel.Stable,new[] {payload,payload})));
    }
    [Fact] public async Task Dll_and_version_sidecar_roll_back_together() {
        var original = Write("version.dll", "old-dll"); Write("clientpatch/clientpatch.version", "1.0.0");
        var download = Path.Combine(root,"release.dll");
        using (var dll = AssemblyDefinition.CreateAssembly(new AssemblyNameDefinition("release",new Version(2,0)),"release.dll",ModuleKind.Dll)) {
            dll.MainModule.Architecture = TargetArchitecture.AMD64; dll.Write(download);
        }
        var backups = new BackupService(); var installer = new UpdateInstaller(new Download(download), backups);
        var result = await installer.InstallReleaseAsync(root,new("version.dll","fake",1),"version.dll",null,"2.0.0",default);
        Assert.True(result.Ok, result.Error); Assert.Equal("2.0.0",File.ReadAllText(Path.Combine(root,"clientpatch","clientpatch.version")));
        backups.Restore(root,backups.List(root).Single().Id);
        Assert.Equal("old-dll",File.ReadAllText(original)); Assert.Equal("1.0.0",File.ReadAllText(Path.Combine(root,"clientpatch","clientpatch.version")));
    }
    [Fact] public async Task Corrupt_dll_release_is_rejected_before_backup_or_swap() {
        var original=Write("version.dll","old"); var text=Write("checksum","not a DLL"); var backups=new BackupService();
        var result=await new UpdateInstaller(new Download(text),backups).InstallReleaseAsync(root,new("version.dll","fake",1),"version.dll",null,"2",default);
        Assert.False(result.Ok); Assert.Equal("old",File.ReadAllText(original)); Assert.Empty(backups.List(root));
    }
    [Fact] public async Task Zip_rollback_removes_new_files_and_restores_overwrites() {
        Write("data/a", "old"); var archive=Path.Combine(root,"bundle.zip");
        using(var zip=ZipFile.Open(archive,ZipArchiveMode.Create)) foreach(var name in new[]{"data/a","data/new"}) { using var writer=new StreamWriter(zip.CreateEntry(name).Open()); writer.Write("new"); }
        var backup=new BackupService(); Assert.True((await new UpdateInstaller(new Download(archive),backup).InstallZipAsync(root,new("bundle.zip","fake",1),".",null,default)).Ok);
        backup.Restore(root,backup.List(root).Single().Id); Assert.Equal("old",File.ReadAllText(Path.Combine(root,"data","a"))); Assert.False(File.Exists(Path.Combine(root,"data","new")));
    }
    sealed class Download(string path) : IFileDownloader { public Task<string> DownloadToTempAsync(string url,CancellationToken ct) { var temp=Path.GetTempFileName(); File.Copy(path,temp,true); return Task.FromResult(temp); } }
    sealed class OfflineSource : IToolReleaseSource { public Task<ToolRelease> LatestAsync(InteropTool tool,CancellationToken ct)=>throw new HttpRequestException("offline"); }
    sealed class EmptyCache : IToolCache { public bool IsCached(InteropTool tool)=>false; public Task<string> EnsureAsync(InteropTool tool,CancellationToken ct)=>throw new NotSupportedException(); }
    sealed class ConcurrentOutput : IToolRunner { public Task<int> RunAsync(string exe,string args,string dir,Action<string> output,CancellationToken ct)=>Task.Run(()=> { Parallel.For(0,2,i=> { for(int n=0;n<1000;n++) output(new string((char)('a'+i),100)); }); return 0; }); }
    sealed class LockAfterPreflight(string file) : IBackupService, IDisposable {
        FileStream? held;
        public BackupEntry Backup(string dir,IEnumerable<string> paths) { held=new FileStream(file,FileMode.Open,FileAccess.ReadWrite,FileShare.None); return new("fake",DateTimeOffset.UtcNow,paths.ToArray()); }
        public IReadOnlyList<BackupEntry> List(string dir)=>Array.Empty<BackupEntry>(); public void Restore(string dir,string id)=>throw new NotSupportedException(); public void Dispose()=>held?.Dispose();
    }
}
