using System.IO.Compression;
using System.Security.Cryptography;
using ClientpatchManager.Core.Backup;
using ClientpatchManager.Core.Update;
using Mono.Cecil;

public sealed class ClientpatchBundleTests : IDisposable {
    readonly string root = Path.Combine(Path.GetTempPath(), "bundle-tests-" + Guid.NewGuid().ToString("N"));
    public ClientpatchBundleTests() => Directory.CreateDirectory(root);
    public void Dispose() => Directory.Delete(root, true);
    sealed class Download(string source) : IFileDownloader {
        public Task<string> DownloadToTempAsync(string url, CancellationToken ct) {
            var file = Path.GetTempFileName(); File.Copy(source, file, true); return Task.FromResult(file);
        }
    }
    string Bundle(bool missingManager = false) {
        var stage = Path.Combine(root, "bundle"); Directory.CreateDirectory(Path.Combine(stage, "clientpatch"));
        foreach (var (name, kind) in new[] { ("version.dll", ModuleKind.Dll), ("manager.exe", ModuleKind.Console) }) {
            if (missingManager && name == "manager.exe") continue;
            using var assembly = AssemblyDefinition.CreateAssembly(new AssemblyNameDefinition("fixture", new Version(2, 0)), name, kind);
            assembly.MainModule.Architecture = TargetArchitecture.AMD64; assembly.Write(Path.Combine(stage, name));
        }
        File.WriteAllText(Path.Combine(stage, "clientpatch", "clientpatch.example.toml"), "template");
        File.WriteAllText(Path.Combine(stage, "clientpatch", "clientpatch.version"), "2.0.0");
        var zip = Path.Combine(root, "bundle.zip"); ZipFile.CreateFromDirectory(stage, zip); return zip;
    }
    string Game() {
        var game = Path.Combine(root, "game"); Directory.CreateDirectory(Path.Combine(game, "clientpatch"));
        File.WriteAllText(Path.Combine(game, "version.dll"), "old dll");
        File.WriteAllText(Path.Combine(game, "manager.exe"), "old manager");
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.toml"), "user config");
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.version"), "1.0.0"); return game;
    }
    void AddExtras(string stage) {
        File.WriteAllText(Path.Combine(stage, "README.md"), "bundle readme");
        File.WriteAllText(Path.Combine(stage, "LICENSE"), "bundle license");
        File.WriteAllText(Path.Combine(stage, "manager.exe.config"), "unrelated sidecar");
        foreach (var dir in new[] { "licenses", "clientpatch-extra", "other" }) {
            Directory.CreateDirectory(Path.Combine(stage, dir));
            File.WriteAllText(Path.Combine(stage, dir, "version.dll"), "unrelated payload");
        }
        for (var i = 0; i < 100; i++) File.WriteAllText(Path.Combine(stage, $"extra-{i}.txt"), "extra");
        Directory.CreateDirectory(Path.Combine(stage, "clientpatch", "nested"));
        File.WriteAllText(Path.Combine(stage, "clientpatch", "nested", "payload.txt"), "included");
    }
    [Fact] public async Task Update_installs_only_allowed_paths_despite_many_extra_zip_entries() {
        var zip = Bundle(); var stage = Path.Combine(root, "bundle"); AddExtras(stage);
        File.Delete(zip); ZipFile.CreateFromDirectory(stage, zip);
        var game = Game(); File.WriteAllText(Path.Combine(game, "README.md"), "existing readme");
        var backups = new BackupService();
        var result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), null, "2.0.0", default);
        Assert.True(result.Ok, result.Error);
        Assert.Equal(new[] { "README.md", "manager.exe", "version.dll" }, Directory.GetFiles(game).Select(Path.GetFileName).OrderBy(n => n, StringComparer.Ordinal).ToArray());
        Assert.Equal("existing readme", File.ReadAllText(Path.Combine(game, "README.md")));
        Assert.Equal(new[] { "clientpatch" }, Directory.GetDirectories(game).Select(Path.GetFileName).ToArray());
        Assert.Equal("included", File.ReadAllText(Path.Combine(game, "clientpatch", "nested", "payload.txt")));
        Assert.Equal("user config", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.toml")));
        Assert.Equal("template", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.example.toml")));
        Assert.All(backups.List(game).Single().Files, path => Assert.True(path == "version.dll" || path == "manager.exe" || path.StartsWith("clientpatch/", StringComparison.Ordinal)));
    }
    [Fact] public void Prepared_handoff_also_ignores_unrelated_staged_files() {
        Bundle(); var stage = Path.Combine(root, "bundle"); AddExtras(stage); var game = Game();
        UpdateInstaller.ApplyClientpatchBundle(game, stage, "2.0.0", new BackupService());
        Assert.Equal(new[] { "manager.exe", "version.dll" }, Directory.GetFiles(game).Select(Path.GetFileName).OrderBy(n => n, StringComparer.Ordinal).ToArray());
        Assert.Equal(new[] { "clientpatch" }, Directory.GetDirectories(game).Select(Path.GetFileName).ToArray());
        Assert.Equal("included", File.ReadAllText(Path.Combine(game, "clientpatch", "nested", "payload.txt")));
    }
    [Fact] public async Task Missing_config_is_created_from_example_and_removed_on_rollback() {
        var zip = Bundle(); var stage = Path.Combine(root, "bundle");
        File.WriteAllText(Path.Combine(stage, "clientpatch", "clientpatch.toml"), "unwanted bundled config");
        File.Delete(zip); ZipFile.CreateFromDirectory(stage, zip);
        var game = Game(); var config = Path.Combine(game, "clientpatch", "clientpatch.toml"); File.Delete(config);
        var backups = new BackupService();
        var result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), null, "2.0.0", default);
        Assert.True(result.Ok, result.Error);
        Assert.Equal("template", File.ReadAllText(config));
        Assert.Equal("template", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.example.toml")));
        backups.Restore(game, backups.List(game).Single().Id);
        Assert.False(File.Exists(config));
        Assert.False(File.Exists(Path.Combine(game, "clientpatch", "clientpatch.example.toml")));
    }
    [Fact] public async Task Retry_recovers_interrupted_initial_config_before_planning_its_creation() {
        var zip = Bundle(); var game = Game();
        var transaction = Path.Combine(game, "clientpatch", ".cpm-transaction-interrupted");
        Directory.CreateDirectory(transaction);
        File.WriteAllText(Path.Combine(transaction, "journal.json"), System.Text.Json.JsonSerializer.Serialize(new {
            Originals = new[] { new { RelativePath = "clientpatch/clientpatch.toml", Exists = false } }, Gate = (string?)null
        }));
        // The interrupted initial installation had created this file before crashing.
        File.WriteAllText(Path.Combine(game, "clientpatch", "clientpatch.toml"), "partial template");
        var result = await new UpdateInstaller(new Download(zip), new BackupService()).InstallClientpatchReleaseAsync(
            game, new("bundle.zip", "fake", 1), null, "2.0.0", default);
        Assert.True(result.Ok, result.Error);
        Assert.Equal("template", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.toml")));
        Assert.False(Directory.Exists(transaction));
    }
    [Fact] public async Task Bundle_updates_both_files_preserves_config_and_rolls_back_together() {
        var zip = Bundle(); var game = Game(); var backups = new BackupService();
        var hash = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(zip)));
        var result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), hash, "2.0.0", default);
        Assert.True(result.Ok, result.Error); Assert.False(result.RestartRequired);
        Assert.Equal(File.ReadAllBytes(Path.Combine(root, "bundle", "manager.exe")), File.ReadAllBytes(Path.Combine(game, "manager.exe")));
        Assert.Equal(File.ReadAllBytes(Path.Combine(root, "bundle", "version.dll")), File.ReadAllBytes(Path.Combine(game, "version.dll")));
        Assert.Equal("user config", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.toml")));
        Assert.Equal("2.0.0", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.version")));
        backups.Restore(game, backups.List(game).Single().Id);
        Assert.Equal("old manager", File.ReadAllText(Path.Combine(game, "manager.exe")));
        Assert.Equal("old dll", File.ReadAllText(Path.Combine(game, "version.dll")));
        Assert.Equal("1.0.0", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.version")));
    }
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task Incomplete_bundle_or_wrong_checksum_leaves_installation_untouched(bool missingManager) {
        var zip = Bundle(missingManager); var game = Game(); var backups = new BackupService();
        var result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), missingManager ? null : new string('0', 64), "2.0.0", default);
        Assert.False(result.Ok); Assert.Empty(backups.List(game));
        Assert.Equal("old manager", File.ReadAllText(Path.Combine(game, "manager.exe")));
        Assert.Equal("old dll", File.ReadAllText(Path.Combine(game, "version.dll")));
        Assert.Equal("1.0.0", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.version")));
    }

    [Fact] public async Task Wrong_bundle_version_is_rejected_before_writes() {
        var zip = Bundle(); var game = Game(); var backups = new BackupService();
        var result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), null, "3.0.0", default);
        Assert.False(result.Ok); Assert.Empty(backups.List(game));
        Assert.Equal("old manager", File.ReadAllText(Path.Combine(game, "manager.exe")));
        Assert.Equal("old dll", File.ReadAllText(Path.Combine(game, "version.dll")));
    }

    [Fact] public async Task Locked_manager_cannot_leave_only_the_loader_updated() {
        var zip = Bundle(); var game = Game(); var backups = new BackupService();
        SwapResult result;
        using (var locked = new FileStream(Path.Combine(game, "manager.exe"), FileMode.Open, FileAccess.Read, FileShare.Read)) {
            result = await new UpdateInstaller(new Download(zip), backups).InstallClientpatchReleaseAsync(game, new("bundle.zip", "fake", 1), null, "2.0.0", default);
        }
        Assert.False(result.Ok); Assert.Empty(backups.List(game));
        Assert.Equal("old manager", File.ReadAllText(Path.Combine(game, "manager.exe")));
        Assert.Equal("old dll", File.ReadAllText(Path.Combine(game, "version.dll")));
        Assert.Equal("1.0.0", File.ReadAllText(Path.Combine(game, "clientpatch", "clientpatch.version")));
    }
}
