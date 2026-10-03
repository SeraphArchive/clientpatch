using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using ClientpatchManager.Core.Interop;
using Xunit;

public class InteropServiceTests {
    // ---- fakes -----------------------------------------------------------

    /// <summary>Simulates the three tools: senbei writes the decrypted unpack pair, the dumper
    /// writes a DummyDll dir, and the CLI writes interop assemblies. Exit codes are scripted.</summary>
    sealed class FakeRunner : IToolRunner {
        readonly int[] _codes; int _i;
        public int CallCount;
        /// <summary>Simulates upstream's Console.ReadKey crash: artifact produced, exit code 1.</summary>
        public bool DumperCrashesAfterWork;
        public FakeRunner(params int[] codes) { _codes = codes; }
        public Task<int> RunAsync(string exe, string args, string wd, Action<string> onOutput, CancellationToken ct) {
            CallCount++;
            var code = _codes[Math.Min(_i++, _codes.Length - 1)];
            var parts = args.Split('"');
            // Tools only produce their artifacts when they succeed (a failed dumper run leaves
            // no mscorlib.dll, which is what the service's artifact check looks for).
            if (code == 0 && Path.GetFileName(exe) == "senbei.exe") {
                // "<input>" --out "<unpack>" --no-pause
                var outDir = parts[3];
                Directory.CreateDirectory(outDir);
                File.WriteAllBytes(Path.Combine(outDir, "GameAssembly.unpack.dll"), DecryptedGaBytes);
                File.WriteAllBytes(Path.Combine(outDir, "global-metadata.unpack.dat"), DecryptedMetaBytes);
            }
            else if (Path.GetFileName(exe) == "Il2CppDumper.exe") {
                // "<dllpath>" "<meta>" "<outdir>" — the self-contained exe runs directly.
                if (code == 0 || DumperCrashesAfterWork) {
                    Directory.CreateDirectory(Path.Combine(parts[5], "DummyDll"));
                    File.WriteAllText(Path.Combine(parts[5], "DummyDll", "mscorlib.dll"), "dummy");
                }
                if (DumperCrashesAfterWork)
                    code = 1; // upstream's exit-path ReadKey crash, output already complete
            }
            else if (code == 0 && args.Contains("Il2CppInterop.CLI.dll")) {
                // "<cli>" generate --input "<in>" --output "<out>" --game-assembly "<ga>"
                Directory.CreateDirectory(parts[5]);
                WriteInterop(parts[5], "Assembly-CSharp.dll");
                WriteInterop(parts[5], "UnityEngine.CoreModule.dll");
            }
            onOutput($"ran {Path.GetFileName(exe)}");
            return Task.FromResult(code);
        }
    }

    // ---- independent re-implementations of the proxy's hashes (src/modules/interopdump/mod.rs) ---

    static readonly byte[] DecryptedMetaBytes = BuildMeta(version: 31);
    static readonly byte[] DecryptedGaBytes = Encoding.ASCII.GetBytes("decrypted-pe-image");

    static byte[] BuildMeta(uint version) {
        var b = new byte[64];
        b[0] = 0xAF; b[1] = 0x1B; b[2] = 0xB1; b[3] = 0xFA; // metadata magic
        BitConverter.GetBytes(version).CopyTo(b, 4);
        for (var i = 8; i < b.Length; i++) b[i] = (byte)i;
        return b;
    }

    /// <summary>build_id per interopdump: first 8 bytes of sha256(metadata), hex.</summary>
    static string BuildIdOf(byte[] meta) =>
        Convert.ToHexString(SHA256.HashData(meta), 0, 8).ToLowerInvariant();

    /// <summary>ga_fingerprint per interopdump: sha256(size_le64 ++ first 4 MiB), full hex.</summary>
    static string FingerprintOf(byte[] fileBytes) {
        using var sha = SHA256.Create();
        var prefix = fileBytes.Length > 4 * 1024 * 1024 ? fileBytes[..(4 * 1024 * 1024)] : fileBytes;
        sha.TransformBlock(BitConverter.GetBytes((long)fileBytes.Length), 0, 8, null, 0);
        sha.TransformFinalBlock(prefix, 0, prefix.Length);
        return Convert.ToHexString(sha.Hash!).ToLowerInvariant();
    }

    // ---- fixture ---------------------------------------------------------

    static string Tmp() { var d = Path.Combine(Path.GetTempPath(), Path.GetRandomFileName()); Directory.CreateDirectory(d); return d; }
    static void WriteInterop(string dir, string name) {
        using var assembly = Mono.Cecil.AssemblyDefinition.CreateAssembly(new Mono.Cecil.AssemblyNameDefinition(Path.GetFileNameWithoutExtension(name), new Version(1,0)), name, Mono.Cecil.ModuleKind.Dll);
        assembly.Write(Path.Combine(dir, name));
    }

    static readonly byte[] OnDiskGaBytes = Encoding.ASCII.GetBytes("protected-game-assembly-bytes");

    /// <summary>A game dir with every input the offline chain needs.</summary>
    static string SeedGameDir() {
        var gameDir = Tmp();
        File.WriteAllBytes(Path.Combine(gameDir, "GameAssembly.dll"), OnDiskGaBytes);
        File.WriteAllText(Path.Combine(gameDir, "GameAssembly.dll._"), "aux-payload");
        var metaDir = Path.Combine(gameDir, "HeavenBurnsRed_Data", "il2cpp_data", "Metadata");
        Directory.CreateDirectory(metaDir);
        File.WriteAllBytes(Path.Combine(metaDir, "global-metadata.dat"), Encoding.ASCII.GetBytes("ENCRYPTED-on-disk-metadata"));
        return gameDir;
    }

    static string WriteLatest(string gameDir, string buildId, string gaFingerprint) {
        var outRoot = Path.Combine(gameDir, "clientpatch", "interop");
        Directory.CreateDirectory(outRoot);
        var json = JsonSerializer.Serialize(new Dictionary<string, object> {
            ["build_id"] = buildId, ["dir"] = buildId,
            ["metadata_version"] = 31u, ["game_assembly_file_fingerprint"] = gaFingerprint,
        });
        var path = Path.Combine(outRoot, "latest.json");
        File.WriteAllText(path, json);
        return path;
    }

    static void WriteMarker(string gameDir, string buildId) {
        var dir = Path.Combine(gameDir, "BepInEx", "interop");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "Assembly-CSharp.dll"), "interop");
        File.WriteAllText(Path.Combine(dir, "UnityEngine.CoreModule.dll"), "interop");
        File.WriteAllText(Path.Combine(dir, "clientpatch-interop.json"),
            $"{{\"build_id\":\"{buildId}\"}}");
    }

    /// <summary>A cache that never downloads: each tool resolves to a ready temp dir with the
    /// executable where upstream zips put it (the CLI nests under net6.0/). The dumper dir also
    /// carries upstream's config.json, whose RequireAnyKey default is what stalls the chain.</summary>
    sealed class FakeCache : IToolCache {
        public string? DumperDir { get; private set; }
        public bool IsCached(InteropTool tool) => true;
        public Task<string> EnsureAsync(InteropTool tool, CancellationToken ct) {
            var dir = Tmp();
            var rel = tool switch {
                InteropTool.Senbei => "senbei-1.0.0-x86_64-pc-windows-msvc/senbei.exe",
                InteropTool.Il2CppDumper => "Il2CppDumper.exe",
                _ => Path.Combine("net6.0", "Il2CppInterop.CLI.dll"),
            };
            var path = Path.Combine(dir, rel);
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, "tool");
            if (tool == InteropTool.Il2CppDumper) {
                File.WriteAllText(Path.Combine(dir, "config.json"),
                    """{"DumpMethod":true,"GenerateDummyDll":true,"RequireAnyKey":true}""");
                DumperDir = dir;
            }
            return Task.FromResult(dir);
        }
    }

    static InteropService NewService(IToolRunner runner, FakeCache? cache = null) =>
        new(runner, cache ?? new FakeCache(), new FakePreparer());
    sealed class FakePreparer : IToolPreparer { public string PrepareDumper(string dir, string scratch) => dir; }

    // ---- tests -----------------------------------------------------------
    [Fact] public async Task Generation_and_currency_use_the_configured_bepinex_root() {
        var gameDir = SeedGameDir();
        var data = Path.Combine(gameDir, "clientpatch"); Directory.CreateDirectory(data);
        File.WriteAllText(Path.Combine(data, "clientpatch.toml"), "[loader]\nmodules=['bepinex']\n[bepinex]\nenabled=true\nroot='Mods'");
        var svc = NewService(new FakeRunner(0,0,0));
        await svc.GenerateOfflineAsync(gameDir,null,new Progress<StepResult>(),default);
        Assert.True(File.Exists(Path.Combine(gameDir,"Mods","interop","Assembly-CSharp.dll")));
        Assert.True(svc.CheckCurrency(gameDir).Current);
    }

    [Fact] public async Task Offline_generate_writes_proxy_shaped_outputs() {
        var gameDir = SeedGameDir();
        var svc = NewService(new FakeRunner(0, 0, 0));

        var res = await svc.GenerateOfflineAsync(gameDir, null, new Progress<StepResult>(), default);

        Assert.Equal(3, res.Count);
        Assert.All(res, r => Assert.True(r.Ok));

        // build_id comes from the DECRYPTED metadata, never the encrypted install file.
        var buildId = BuildIdOf(DecryptedMetaBytes);
        Assert.NotEqual(BuildIdOf(Encoding.ASCII.GetBytes("ENCRYPTED-on-disk-metadata")), buildId);

        // Marker at <gameDir>/BepInEx/interop/clientpatch-interop.json names that id.
        var marker = File.ReadAllText(Path.Combine(gameDir, "BepInEx", "interop", "clientpatch-interop.json"));
        Assert.Contains($"\"build_id\":\"{buildId}\"", marker.Replace(" ", ""));

        // latest.json at <gameDir>/clientpatch/interop names the same id and the GA fingerprint.
        var dumpRoot = Path.Combine(gameDir, "clientpatch", "interop");
        var latest = File.ReadAllText(Path.Combine(dumpRoot, "latest.json"));
        using var doc = JsonDocument.Parse(latest);
        Assert.Equal(buildId, doc.RootElement.GetProperty("build_id").GetString());
        Assert.Equal(FingerprintOf(OnDiskGaBytes),
            doc.RootElement.GetProperty("game_assembly_file_fingerprint").GetString());
        Assert.Equal(31u, doc.RootElement.GetProperty("metadata_version").GetUInt32());

        // The dump set mirrors interopdump's shape, DONE present.
        Assert.True(File.Exists(Path.Combine(dumpRoot, buildId, "DONE")));
        Assert.True(File.Exists(Path.Combine(dumpRoot, buildId, "GameAssembly.dll")));
        Assert.True(File.Exists(Path.Combine(dumpRoot, buildId, "global-metadata.dat")));

        // And the whole point: clientpatch's decide() inputs now line up.
        Assert.True(svc.CheckCurrency(gameDir).Current);
    }

    [Fact] public async Task Missing_inputs_fail_first_step_without_running_tools() {
        var runner = new FakeRunner(0, 0, 0);
        var svc = NewService(runner);

        var res = await svc.GenerateOfflineAsync(Tmp(), null, new Progress<StepResult>(), default);

        Assert.Single(res);
        Assert.Equal(InteropStep.Unpack, res[0].Step);
        Assert.False(res[0].Ok);
        Assert.Contains("GameAssembly.dll", res[0].Output);
        Assert.Equal(0, runner.CallCount);
    }

    [Fact] public async Task Dump_failure_stops_chain() {
        var svc = NewService(new FakeRunner(0, 1)); // unpack ok, dump fails
        var res = await svc.GenerateOfflineAsync(SeedGameDir(), null, new Progress<StepResult>(), default);
        Assert.Equal(2, res.Count);
        Assert.False(res[^1].Ok);
        Assert.Equal(InteropStep.Dump, res[^1].Step);
    }

    [Fact] public async Task Dumper_config_has_its_exit_prompt_disabled_before_it_runs() {
        // Upstream's config.json ships RequireAnyKey:true, so the exe prints "Press any key to
        // exit..." and blocks on Console.ReadKey after a successful dump. The chain must flip
        // that one flag (and nothing else) before launching it.
        var cache = new FakeCache();
        var svc = NewService(new FakeRunner(0, 0, 0), cache);

        var res = await svc.GenerateOfflineAsync(SeedGameDir(), null, new Progress<StepResult>(), default);
        Assert.All(res, r => Assert.True(r.Ok));

        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(cache.DumperDir!, "config.json")));
        Assert.False(doc.RootElement.GetProperty("RequireAnyKey").GetBoolean());
        Assert.True(doc.RootElement.GetProperty("DumpMethod").GetBoolean());
        Assert.True(doc.RootElement.GetProperty("GenerateDummyDll").GetBoolean());
    }

    [Fact] public async Task Dumper_exit_path_crash_is_forgiven_when_artifact_exists() {
        var svc = NewService(new FakeRunner(0, 0, 0) { DumperCrashesAfterWork = true });
        var res = await svc.GenerateOfflineAsync(SeedGameDir(), null, new Progress<StepResult>(), default);
        Assert.Equal(3, res.Count);
        Assert.All(res, r => Assert.True(r.Ok));
    }

    [Fact] public async Task Stale_interop_assemblies_are_removed_on_install() {
        var gameDir = SeedGameDir();
        var interopDir = Path.Combine(gameDir, "BepInEx", "interop");
        Directory.CreateDirectory(interopDir);
        File.WriteAllText(Path.Combine(interopDir, "Stale.From.Previous.Build.dll"), "old");

        var svc = NewService(new FakeRunner(0, 0, 0));
        var res = await svc.GenerateOfflineAsync(gameDir, null, new Progress<StepResult>(), default);
        Assert.All(res, r => Assert.True(r.Ok));

        Assert.False(File.Exists(Path.Combine(interopDir, "Stale.From.Previous.Build.dll")));
        Assert.True(File.Exists(Path.Combine(interopDir, "Assembly-CSharp.dll")));
        Assert.True(File.Exists(Path.Combine(interopDir, "UnityEngine.CoreModule.dll")));
    }

    [Fact] public void Currency_mirrors_clientpatch_decide() {
        var gameDir = SeedGameDir();
        var buildId = BuildIdOf(DecryptedMetaBytes);
        var svc = NewService(new FakeRunner());

        // Nothing generated yet -> not current.
        Assert.False(svc.CheckCurrency(gameDir).Current);

        // latest.json + marker agree and the fingerprint matches -> current.
        WriteLatest(gameDir, buildId, FingerprintOf(OnDiskGaBytes));
        WriteMarker(gameDir, buildId);
        Assert.True(svc.CheckCurrency(gameDir).Current);

        // Marker naming another build -> NeedsGenerate in proxy terms -> not current.
        WriteMarker(gameDir, "deadbeefdeadbeef");
        Assert.False(svc.CheckCurrency(gameDir).Current);
    }

    [Fact] public void Game_update_in_place_breaks_currency_via_fingerprint() {
        var gameDir = SeedGameDir();
        var buildId = BuildIdOf(DecryptedMetaBytes);
        WriteLatest(gameDir, buildId, FingerprintOf(OnDiskGaBytes));
        WriteMarker(gameDir, buildId);
        var svc = NewService(new FakeRunner());
        Assert.True(svc.CheckCurrency(gameDir).Current);

        // Steam updates the game: new GameAssembly.dll, same folder, metadata untouched.
        File.WriteAllBytes(Path.Combine(gameDir, "GameAssembly.dll"),
            Encoding.ASCII.GetBytes("protected-game-assembly-bytes-v2"));

        Assert.False(svc.CheckCurrency(gameDir).Current);
    }

    [Fact] public async Task From_dump_requires_dump_inputs() {
        var runner = new FakeRunner(0, 0);
        var svc = NewService(runner);
        var res = await svc.GenerateFromDumpAsync(Tmp(), null, new Progress<StepResult>(), default);
        Assert.Single(res);
        Assert.Equal(InteropStep.Dump, res[0].Step);
        Assert.False(res[0].Ok);
        Assert.Equal(0, runner.CallCount);
    }

    [Fact] public async Task From_dump_also_disables_the_dumper_exit_prompt() {
        var dumpDir = Tmp();
        File.WriteAllBytes(Path.Combine(dumpDir, "GameAssembly.dll"), DecryptedGaBytes);
        File.WriteAllBytes(Path.Combine(dumpDir, "global-metadata.dat"), DecryptedMetaBytes);
        var cache = new FakeCache();
        var svc = NewService(new FakeRunner(0, 0), cache);

        var res = await svc.GenerateFromDumpAsync(dumpDir, null, new Progress<StepResult>(), default);
        Assert.All(res, r => Assert.True(r.Ok));

        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(cache.DumperDir!, "config.json")));
        Assert.False(doc.RootElement.GetProperty("RequireAnyKey").GetBoolean());
    }
}
