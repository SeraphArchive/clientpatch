using ClientpatchManager.Core.Interop;

public sealed class HbrDumperCompatibilityTests {
    [Fact] public void PrepareDumper_preserves_tool_bytes_and_isolates_configuration() {
        var root = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"));
        var cache = Path.Combine(root, "cache");
        var scratch = Path.Combine(root, "scratch");
        Directory.CreateDirectory(cache);
        var binary = new byte[] { 0x4d, 0x5a, 0, 1, 2, 3 };
        File.WriteAllBytes(Path.Combine(cache, "Il2CppDumper.dll"), binary);
        File.WriteAllText(Path.Combine(cache, "config.json"), "{\"RequireAnyKey\":true}");
        try {
            var copy = new HbrDumperCompatibility().PrepareDumper(cache, scratch);
            Assert.Equal(binary, File.ReadAllBytes(Path.Combine(copy, "Il2CppDumper.dll")));
            Assert.Equal(binary, File.ReadAllBytes(Path.Combine(cache, "Il2CppDumper.dll")));
            File.WriteAllText(Path.Combine(copy, "config.json"), "{\"RequireAnyKey\":false}");
            Assert.Equal("{\"RequireAnyKey\":true}", File.ReadAllText(Path.Combine(cache, "config.json")));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact] public void PrepareDumper_rejects_bundle_without_dumper() {
        var root = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"));
        var cache = Path.Combine(root, "cache");
        Directory.CreateDirectory(cache);
        try {
            Assert.Throws<InvalidDataException>(() =>
                new HbrDumperCompatibility().PrepareDumper(cache, Path.Combine(root, "scratch")));
        }
        finally { Directory.Delete(root, true); }
    }
}
