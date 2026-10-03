using System.IO;
using ClientpatchManager.Core.Settings;
using Xunit;

namespace ClientpatchManager.Core.Settings;

public class SettingsStoreTests {
    [Fact] public void Round_trips_settings() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        var s = store.Load();                 // defaults on first run
        Assert.True(s.CheckOnLaunch);
        store.Save(s with { Language = "ja" });
        Assert.Equal("ja", new SettingsStore(dir).Load().Language);
    }

    [Fact] public void Corrupt_main_file_falls_back_to_bak() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default with { Language = "ko" });
        store.Save(ManagerSettings.Default with { Language = "ja" }); // main=ja, bak=ko
        File.WriteAllText(store.FilePath, "{ not json");
        Assert.Equal("ko", new SettingsStore(dir).Load().Language);
    }

    [Theory]
    [InlineData(ManagerSettings.LegacyFeedRepo)]
    [InlineData(ManagerSettings.PreviousFeedRepo)]
    public void Legacy_default_feed_repo_is_migrated_on_load(string previousRepo) {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default with { FeedRepo = previousRepo });
        var loaded = new SettingsStore(dir).Load();
        Assert.Equal(ManagerSettings.DefaultFeedRepo, loaded.FeedRepo);
        // Persisted: a second load sees the migrated value on disk.
        Assert.Equal(ManagerSettings.DefaultFeedRepo, new SettingsStore(dir).Load().FeedRepo);
    }

    [Fact] public void Custom_feed_repo_is_not_touched_on_load() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default with { FeedRepo = "someone/fork" });
        Assert.Equal("someone/fork", new SettingsStore(dir).Load().FeedRepo);
    }

    [Fact] public void Default_feed_migration_remains_available_when_main_file_cannot_be_replaced() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default with { FeedRepo = ManagerSettings.PreviousFeedRepo, Language = "ko" });
        using (var held = new FileStream(store.FilePath, FileMode.Open, FileAccess.Read, FileShare.Read)) {
            var settings = store.Load();
            Assert.Equal(ManagerSettings.DefaultFeedRepo, settings.FeedRepo);
            Assert.Equal("ko", settings.Language);
            Assert.Contains(ManagerSettings.PreviousFeedRepo, File.ReadAllText(store.FilePath));
        }
        Assert.Empty(Directory.GetFiles(dir, "*.tmp-*"));
    }

    [Fact] public void Saves_do_not_touch_another_instances_temporary_file() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default);
        var existingTemp = store.FilePath + ".tmp";
        File.WriteAllText(existingTemp, "another writer");
        using (var held = new FileStream(existingTemp, FileMode.Open, FileAccess.ReadWrite, FileShare.None))
            new SettingsStore(dir).Save(ManagerSettings.Default with { Language = "ja" });
        Assert.Equal("ja", store.Load().Language);
        Assert.Equal("another writer", File.ReadAllText(existingTemp));
        Assert.Empty(Directory.GetFiles(dir, "*.tmp-*"));
    }

    [Fact] public void Saving_after_corrupt_main_preserves_the_last_valid_backup() {
        var dir = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var store = new SettingsStore(dir);
        store.Save(ManagerSettings.Default with { Language = "ko" });
        store.Save(ManagerSettings.Default with { Language = "ja" });
        File.WriteAllText(store.FilePath, "corrupt");
        store.Save(store.Load() with { Language = "zh-Hans" });
        File.WriteAllText(store.FilePath, "corrupt again");
        Assert.Equal("ko", store.Load().Language);
    }
}
