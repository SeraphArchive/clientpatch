using ClientpatchManager.Core.Launch;
using Xunit;

public class SteamLibraryLocatorTests {
    [Fact] public void Parses_library_paths_from_vdf() {
        var vdf = "\"libraryfolders\"\n{\n \"0\"\n {\n  \"path\"  \"D:\\\\SteamLibrary\"\n }\n}";
        var paths = SteamLibraryLocator.ParseLibraryFolders(vdf);
        Assert.Contains(@"D:\SteamLibrary", paths);
    }

    [Fact] public void Unescapes_quotes_and_backslashes_in_paths() {
        var vdf = """
            "libraryfolders"
            {
             "0"
             {
              "path"  "D:\\Games\\Steam \"Library\""
             }
            }
            """;
        var paths = SteamLibraryLocator.ParseLibraryFolders(vdf);
        Assert.Contains("D:\\Games\\Steam \"Library\"", paths);
    }

    [Fact] public void Parses_installdir_from_appmanifest() {
        var acf = """
            "AppState"
            {
             "appid"  "1973710"
             "installdir"  "HBR Custom"
            }
            """;
        Assert.Equal("HBR Custom", SteamLibraryLocator.ParseInstallDir(acf));
    }

    [Fact] public void FindHbrGameDir_honors_the_appmanifest_install_dir() {
        var root = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var declared = Path.Combine(root, "steamapps", "common", "HBR Custom");
        Directory.CreateDirectory(declared);
        File.WriteAllText(Path.Combine(root, "steamapps", "appmanifest_1973710.acf"),
            "\"AppState\"\n{\n \"installdir\"  \"HBR Custom\"\n}");

        Assert.Equal(declared, SteamLibraryLocator.FindHbrGameDir(new[] { root }));
    }

    [Fact] public void FindHbrGameDir_falls_back_to_the_default_folder_name() {
        var root = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName());
        var candidate = Path.Combine(root, "steamapps", "common", "HeavenBurnsRed");
        Directory.CreateDirectory(candidate);

        Assert.Equal(candidate, SteamLibraryLocator.FindHbrGameDir(new[] { root }));
    }
}
