using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using ClientpatchManager.Core.Install;
using ClientpatchManager.Core.Backup;
using Layout = ClientpatchManager.Core.Install.InstallLayout;

namespace ClientpatchManager.Core.Interop;

/// <summary>
/// Orchestrates the offline interop generation chain and reports whether the generated interop
/// is current for the installed build. See <c>manager/docs/interop-chain.md</c> for the verified
/// command sequence this automates.
/// </summary>
/// <remarks>
/// <para><b>The clientpatch currency contract</b> (src/modules/bepinex/mod.rs <c>decide</c>,
/// src/modules/interopdump/mod.rs): currency is decided from three inputs —
/// <c>&lt;gameDir&gt;/interop/latest.json</c> (written by interopdump at dump time; holds
/// <c>build_id</c> and <c>game_assembly_file_fingerprint</c>), the marker
/// <c>&lt;gameDir&gt;/BepInEx/interop/clientpatch-interop.json</c> (whose <c>build_id</c> must equal
/// latest.json's), and the on-disk <c>GameAssembly.dll</c> fingerprint
/// (sha256 of size_le64 ++ first 4 MiB). <c>build_id</c> is the first 8 bytes of the SHA-256 of the
/// DECRYPTED global-metadata.dat. The on-disk metadata is CrackProof-encrypted, so an offline build
/// id must be computed from senbei's decrypted output — never from the install file.</para>
/// <para><b>Stop-on-failure:</b> generation runs the steps in order and stops at the first non-zero
/// exit, returning the results collected so far.</para>
/// <para><b>Stage-then-promote:</b> generation writes into a scratch directory; the interop dir ends
/// up holding exactly the new set (new files placed via temp+rename, stale files removed, marker
/// written LAST), so a partial tree is never seen as "current".</para>
/// </remarks>
public sealed class InteropService : IInteropService {
    const string GameAssemblyName = "GameAssembly.dll";
    const string AuxPayloadName = "GameAssembly.dll._";
    const string MetadataRelPath = "HeavenBurnsRed_Data/il2cpp_data/Metadata/global-metadata.dat";

    /// <summary>BepInEx root dir name under the game dir (clientpatch cfg.root default, mod.rs).</summary>
    const string BepInexRootDir = "BepInEx";

    /// <summary>Interop subdir under the BepInEx root (clientpatch <c>bepinex_interop_dir</c>, mod.rs:351-353).</summary>
    const string InteropSubDir = "interop";

    /// <summary>Default interopdump out dir under the game dir (src/config.rs default_interop_out_dir).</summary>
    public const string DefaultDumpRootName = "interop";

    /// <summary>latest.json file name interopdump writes and bepinex reads (mod.rs).</summary>
    const string LatestName = "latest.json";

    /// <summary>
    /// Currency marker file name clientpatch reads (mod.rs:103 <c>MARKER_NAME</c>). The document is
    /// a JSON object whose <c>build_id</c> string field clientpatch's <c>parse_marker</c> reads.
    /// </summary>
    const string MarkerName = "clientpatch-interop.json";

    /// <summary>Bytes of the on-disk GameAssembly used for the build fingerprint (interopdump FINGERPRINT_PREFIX).</summary>
    const int FingerprintPrefix = 4 * 1024 * 1024;

    readonly IToolRunner _runner;
    readonly IToolCache _cache;
    readonly IToolPreparer _preparer;

    public InteropService(IToolRunner runner, IToolCache cache, IToolPreparer? preparer = null) {
        _runner = runner;
        _cache = cache;
        _preparer = preparer ?? new HbrDumperCompatibility();
    }

    /// <summary>The directory clientpatch loads interop from: <c>&lt;gameDir&gt;/BepInEx/interop</c>.</summary>
    static string InteropDir(string gameDir) {
        var path = Layout.Resolve(gameDir, Layout.ConfigFileName);
        var root = File.Exists(path) ? new Config.ConfigService().Load(File.ReadAllText(path)).BepInEx?.Root ?? BepInexRootDir : BepInexRootDir;
        return Path.Combine(FileTransaction.Resolve(gameDir, root), InteropSubDir);
    }

    /// <summary>
    /// The interopdump out root. Relative names resolve inside
    /// <c>&lt;gameDir&gt;/clientpatch</c>.
    /// </summary>
    static string DumpRoot(string gameDir, string? dumpRootName) =>
        Layout.Resolve(gameDir, dumpRootName ?? DefaultDumpRootName);

    /// <inheritdoc/>
    public InteropStatus CheckCurrency(string gameDir, string? dumpRootName = null) {
        // Mirror clientpatch's decide() exactly: latest.json must exist, its
        // game_assembly_file_fingerprint must match the on-disk GameAssembly.dll, and the
        // BepInEx marker's build_id must equal latest.json's build_id.
        var gaHash = GaFingerprint(Path.Combine(gameDir, GameAssemblyName));
        var latest = ReadLatest(Path.Combine(DumpRoot(gameDir, dumpRootName), LatestName));
        var markerBuild = ReadMarkerBuildId(Path.Combine(InteropDir(gameDir), MarkerName));

        var interopDir = InteropDir(gameDir);
        var count = Directory.Exists(interopDir)
            ? Directory.GetFiles(interopDir, "*.dll", SearchOption.TopDirectoryOnly).Length
            : 0;

        var current = File.Exists(Path.Combine(interopDir, "Assembly-CSharp.dll")) && File.Exists(Path.Combine(interopDir, "UnityEngine.CoreModule.dll"))
            && gaHash is not null && latest is not null
            && latest.Value.GaFileHash.Length > 0 && latest.Value.GaFileHash == gaHash
            && markerBuild is not null && markerBuild == latest.Value.BuildId;
        return new InteropStatus(current, latest?.BuildId ?? markerBuild, count);
    }

    /// <inheritdoc/>
    public async Task<IReadOnlyList<StepResult>> GenerateOfflineAsync(string gameDir, string? dumpRootName, IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null) {
        var results = new List<StepResult>();

        // Preflight: every input must exist before any tool runs (a three-green-steps run over
        // an empty directory is the failure this guards).
        var gaSrc = Path.Combine(gameDir, GameAssemblyName);
        var auxSrc = Path.Combine(gameDir, AuxPayloadName);
        var metaSrc = Path.Combine(gameDir, MetadataRelPath.Replace('/', Path.DirectorySeparatorChar));
        string? missing = !File.Exists(gaSrc) ? GameAssemblyName
            : !File.Exists(auxSrc) ? AuxPayloadName
            : !File.Exists(metaSrc) ? MetadataRelPath
            : null;
        if (missing is not null) {
            var fail = new StepResult(InteropStep.Unpack, false,
                $"required input '{missing}' not found under {gameDir}; check the game folder setting");
            progress.Report(fail);
            results.Add(fail);
            return results;
        }

        using var operation = GameOperation.Acquire(gameDir);
        GameOperation.RequireStopped(gameDir);
        var scratch = NewScratch(gameDir);
        var input = Path.Combine(scratch, "input");
        var unpack = Path.Combine(scratch, "unpack");
        var dumperOut = Path.Combine(scratch, "dumper-out");
        var interopOut = Path.Combine(scratch, "interop-out");
        Directory.CreateDirectory(input);
        Directory.CreateDirectory(unpack);
        Directory.CreateDirectory(dumperOut);
        Directory.CreateDirectory(interopOut);

        try {
            // --- Stage inputs (ruling #1: senbei needs the .dll._ aux payload beside the DLL) ---
            File.Copy(gaSrc, Path.Combine(input, GameAssemblyName), overwrite: true);
            File.Copy(auxSrc, Path.Combine(input, AuxPayloadName), overwrite: true);
            File.Copy(metaSrc, Path.Combine(input, "global-metadata.dat"), overwrite: true);
            var inputGaHash = GaFingerprint(Path.Combine(input, GameAssemblyName))
                ?? throw new IOException("Cannot fingerprint staged GameAssembly.dll.");

            // --- Step 1: senbei static unpack (runs over the whole input folder in one pass) ---
            var senbeiDir = await _cache.EnsureAsync(InteropTool.Senbei, ct).ConfigureAwait(false);
            var senbeiExe = FindTool(senbeiDir, "senbei.exe");
            var unpackArgs = $"\"{input}\" --out \"{unpack}\" --no-pause";
            if (!await RunStepAsync(InteropStep.Unpack, senbeiExe, unpackArgs, scratch, progress, results, ct, console: console).ConfigureAwait(false))
                return results;

            // --- Step 2: Il2CppDumper in NORMAL/index mode (ruling #2: no IsDumped flag).
            // Upstream's -win- zip ships a self-contained exe, so it runs directly. ---
            var dumperDir = await _cache.EnsureAsync(InteropTool.Il2CppDumper, ct).ConfigureAwait(false);
            dumperDir = _preparer.PrepareDumper(dumperDir, scratch);
            DisableDumperExitPrompt(dumperDir);
            var dumperExe = FindTool(dumperDir, "Il2CppDumper.exe");
            var unpackedDll = Path.Combine(unpack, "GameAssembly.unpack.dll");
            var unpackedMeta = Path.Combine(unpack, "global-metadata.unpack.dat");
            var dumpArgs = $"\"{unpackedDll}\" \"{unpackedMeta}\" \"{dumperOut}\"";
            // Upstream's exe ends with Console.ReadKey, which crashes AFTER the dump is done
            // under redirected IO — so the artifact, not the exit code, is the success signal
            // (same criterion as interopgen.ps1).
            if (!await RunStepAsync(InteropStep.Dump, dumperExe, dumpArgs, scratch, progress, results, ct,
                    Path.Combine(dumperOut, "DummyDll", "mscorlib.dll"), console: console).ConfigureAwait(false))
                return results;

            // --- Step 3: Il2CppInterop.CLI generate (into scratch staging) ---
            var cliDir = await _cache.EnsureAsync(InteropTool.Il2CppInteropCli, ct).ConfigureAwait(false);
            var cliDll = FindTool(cliDir, "Il2CppInterop.CLI.dll");
            var dummyDll = Path.Combine(dumperOut, "DummyDll");
            var genArgs = $"\"{cliDll}\" generate --input \"{dummyDll}\" --output \"{interopOut}\" --game-assembly \"{unpackedDll}\"";
            if (!await RunStepAsync(InteropStep.Generate, "dotnet", genArgs, scratch, progress, results, ct, console: console).ConfigureAwait(false))
                return results;

            // The build id MUST come from senbei's decrypted metadata: clientpatch hashes the
            // decrypted blob (interopdump/mod.rs:156-159), and the on-disk metadata is encrypted.
            var buildId = BuildIdFromMetadata(unpackedMeta)
                ?? throw new InvalidOperationException($"senbei did not produce decrypted metadata at {unpackedMeta}");
            var gaHash = GaFingerprint(gaSrc)
                ?? throw new InvalidOperationException($"cannot fingerprint {gaSrc}");
            if (gaHash != inputGaHash) throw new IOException("The game updated during generation. Retry with the updated installation.");
            var metadataVersion = ReadMetadataVersion(unpackedMeta);

            // Install assemblies + marker, then stage the dump set + latest.json so the offline
            // result is indistinguishable from a runtime dump to clientpatch's decide().
            var dumpFiles = StageOfflineDump(scratch, gameDir, dumpRootName, buildId, unpackedDll, unpackedMeta, gaHash, metadataVersion);
            Install(gameDir, interopOut, buildId, dumpFiles);

            return results;
        }
        catch (Exception ex) {
            ReportFailure(results, progress, InteropStep.Unpack, ex);
            throw;
        }
        finally {
            TryDeleteDir(scratch);
        }
    }

    /// <inheritdoc/>
    public async Task<IReadOnlyList<StepResult>> GenerateFromDumpAsync(string dumpDir, string? gameDir, IProgress<StepResult> progress, CancellationToken ct, IProgress<string>? console = null) {
        var results = new List<StepResult>();

        // Preflight: a bad dump dir must fail fast with its own message, not the dumper's.
        var dumpedDll = Path.Combine(dumpDir, GameAssemblyName);
        var dumpedMeta = Path.Combine(dumpDir, "global-metadata.dat");
        string? missing = !File.Exists(dumpedDll) ? GameAssemblyName
            : !File.Exists(dumpedMeta) ? "global-metadata.dat"
            : null;
        if (missing is not null) {
            var fail = new StepResult(InteropStep.Dump, false,
                $"required input '{missing}' not found in dump dir {dumpDir}");
            progress.Report(fail);
            results.Add(fail);
            return results;
        }

        // Stage on the same volume as the install target so the per-file rename is atomic; fall back
        // to the dump dir's volume when no gameDir is known (generation still runs, install is skipped).
        using var operation = GameOperation.Acquire(gameDir ?? dumpDir);
        GameOperation.RequireStopped(gameDir ?? dumpDir);
        var scratch = NewScratch(gameDir ?? dumpDir);
        var dumperOut = Path.Combine(scratch, "dumper-out");
        var interopOut = Path.Combine(scratch, "interop-out");
        Directory.CreateDirectory(dumperOut);
        Directory.CreateDirectory(interopOut);

        try {
            // Runtime-dump path: the injector already produced a decrypted memory image, so there
            // is no senbei Unpack step. Il2CppDumper runs in IsDumped mode (auto-detected from the
            // dumped ImageBase != 0x180000000). interopdump already wrote latest.json at dump time,
            // so this path only installs assemblies + the marker.
            var dumperDir = await _cache.EnsureAsync(InteropTool.Il2CppDumper, ct).ConfigureAwait(false);
            dumperDir = _preparer.PrepareDumper(dumperDir, scratch);
            DisableDumperExitPrompt(dumperDir);
            var dumperExe = FindTool(dumperDir, "Il2CppDumper.exe");
            var dumpArgs = $"\"{dumpedDll}\" \"{dumpedMeta}\" \"{dumperOut}\"";
            if (!await RunStepAsync(InteropStep.Dump, dumperExe, dumpArgs, scratch, progress, results, ct,
                    Path.Combine(dumperOut, "DummyDll", "mscorlib.dll"), console: console).ConfigureAwait(false))
                return results;

            var cliDir = await _cache.EnsureAsync(InteropTool.Il2CppInteropCli, ct).ConfigureAwait(false);
            var cliDll = FindTool(cliDir, "Il2CppInterop.CLI.dll");
            var dummyDll = Path.Combine(dumperOut, "DummyDll");
            var genArgs = $"\"{cliDll}\" generate --input \"{dummyDll}\" --output \"{interopOut}\" --game-assembly \"{dumpedDll}\"";
            if (!await RunStepAsync(InteropStep.Generate, "dotnet", genArgs, scratch, progress, results, ct, console: console).ConfigureAwait(false))
                return results;

            // The build id hashes the DECRYPTED global-metadata.dat in the dump dir — the same blob
            // clientpatch's interopdump hashed for latest.json, so the marker we write matches the id
            // clientpatch compares against.
            var buildId = BuildIdFromMetadata(dumpedMeta);
            if (buildId is not null && !string.IsNullOrWhiteSpace(gameDir))
                Install(gameDir!, interopOut, buildId);

            return results;
        }
        catch (Exception ex) {
            ReportFailure(results, progress, InteropStep.Dump, ex);
            throw;
        }
        finally {
            TryDeleteDir(scratch);
        }
    }

    static void ReportFailure(List<StepResult> results, IProgress<StepResult> progress, InteropStep first, Exception error) {
        var last = results.LastOrDefault();
        var step = last is null ? first : last.Step switch { InteropStep.Unpack => InteropStep.Dump, _ => InteropStep.Generate };
        progress.Report(new StepResult(step, false, error.Message));
    }

    /// <summary>Locates a tool's executable inside its extracted bundle (upstream zips nest
    /// differently: senbei under a versioned folder, the CLI under <c>net6.0/</c>).</summary>
    private static string FindTool(string dir, string fileName) =>
        Directory.EnumerateFiles(dir, fileName, SearchOption.AllDirectories).FirstOrDefault()
        ?? throw new InvalidOperationException($"tool '{fileName}' not found under {dir}");

    /// <summary>
    /// Turns off the exit prompt in every <c>config.json</c> shipped beside an upstream
    /// Il2CppDumper. Its <c>Main</c> reads that file from the exe directory and, with
    /// <c>RequireAnyKey</c> left at its upstream default, blocks on <c>Console.ReadKey</c> after a
    /// successful dump — which hangs the whole chain, since nothing ever presses a key. Only that
    /// one property is rewritten; a missing or unreadable config is left untouched (the dumper then
    /// keeps its own default and the run fails visibly instead of being patched into a crash).
    /// </summary>
    static void DisableDumperExitPrompt(string dumperDir) {
        foreach (var configPath in Directory.EnumerateFiles(dumperDir, "config.json", SearchOption.AllDirectories)) {
            try {
                using var doc = JsonDocument.Parse(File.ReadAllText(configPath));
                if (doc.RootElement.ValueKind != JsonValueKind.Object) continue;
                if (doc.RootElement.TryGetProperty("RequireAnyKey", out var flag)
                    && flag.ValueKind == JsonValueKind.False)
                    continue;

                using var stream = new MemoryStream();
                using (var writer = new Utf8JsonWriter(stream, new JsonWriterOptions { Indented = true })) {
                    writer.WriteStartObject();
                    foreach (var prop in doc.RootElement.EnumerateObject()) {
                        if (prop.NameEquals("RequireAnyKey"))
                            writer.WriteBoolean("RequireAnyKey", false);
                        else
                            prop.WriteTo(writer);
                    }
                    if (!doc.RootElement.TryGetProperty("RequireAnyKey", out _))
                        writer.WriteBoolean("RequireAnyKey", false);
                    writer.WriteEndObject();
                }
                File.WriteAllBytes(configPath, stream.ToArray());
            }
            catch (JsonException) { /* not the dumper's config; leave it */ }
            catch (IOException) { /* unreadable or locked; the run will surface the real failure */ }
            catch (UnauthorizedAccessException) { /* same */ }
        }
    }

    /// <summary>Runs one step through the runner, records/reports its result, and returns whether
    /// it succeeded. A nonzero exit code is forgiven when <paramref name="successArtifact"/> exists
    /// afterwards — upstream Il2CppDumper crashes on its exit-path <c>Console.ReadKey</c> under
    /// redirected IO, after the dump is complete.</summary>
    async Task<bool> RunStepAsync(InteropStep step, string exe, string args, string workingDir,
        IProgress<StepResult> progress, List<StepResult> results, CancellationToken ct,
        string? successArtifact = null, IProgress<string>? console = null) {
        var sb = new StringBuilder();
        var outputLock = new object();
        var code = await _runner.RunAsync(exe, args, workingDir,
            line => { lock (outputLock) { sb.AppendLine(line); console?.Report(line); } }, ct).ConfigureAwait(false);
        var ok = code == 0 || (successArtifact is not null && File.Exists(successArtifact));
        if (ok && code != 0)
            sb.AppendLine($"(exit code {code} after producing its output; continuing)");
        var result = new StepResult(step, ok, sb.ToString().TrimEnd());
        progress.Report(result);
        results.Add(result);
        return result.Ok;
    }

    /// <summary>
    /// Installs the staged interop assemblies into <c>&lt;gameDir&gt;/BepInEx/interop</c> so the
    /// directory ends up holding exactly the new set: new <c>*.dll</c>/<c>*.db</c> placed via
    /// temp+rename, stale ones removed, and the <c>clientpatch-interop.json</c> currency marker
    /// written LAST — a partial or mixed tree never carries the "current" marker. Throws (before
    /// the marker is written) if a stale file cannot be removed, e.g. while the game holds it open.
    /// </summary>
    void Install(string gameDir, string stagedInteropOut, string buildId, IReadOnlyDictionary<string, string?>? extraFiles = null) {
        foreach (var required in new[] { "Assembly-CSharp.dll", "UnityEngine.CoreModule.dll" })
            if (!File.Exists(Path.Combine(stagedInteropOut, required)) || new FileInfo(Path.Combine(stagedInteropOut, required)).Length == 0)
                throw new InvalidDataException($"Generator did not produce required assembly '{required}'.");
        foreach (var dll in Directory.GetFiles(stagedInteropOut, "*.dll"))
            _ = System.Reflection.AssemblyName.GetAssemblyName(dll);
        var dst = InteropDir(gameDir);
        var relative = Path.GetRelativePath(gameDir, dst);
        var files = new Dictionary<string, string?>(StringComparer.OrdinalIgnoreCase);
        if (extraFiles is not null) foreach (var pair in extraFiles) files.Add(pair.Key, pair.Value);
        foreach (var src in Directory.GetFiles(stagedInteropOut, "*")) {
            var ext = Path.GetExtension(src);
            if (ext.Equals(".dll", StringComparison.OrdinalIgnoreCase) || ext.Equals(".db", StringComparison.OrdinalIgnoreCase))
                files[Path.Combine(relative, Path.GetFileName(src))] = src;
        }
        if (Directory.Exists(dst)) foreach (var existing in Directory.GetFiles(dst, "*")) {
            var ext = Path.GetExtension(existing);
            var rel = Path.Combine(relative, Path.GetFileName(existing));
            if ((ext.Equals(".dll", StringComparison.OrdinalIgnoreCase) || ext.Equals(".db", StringComparison.OrdinalIgnoreCase)) && !files.ContainsKey(rel))
                files[rel] = null;
        }
        var marker = Path.Combine(stagedInteropOut, MarkerName);
        File.WriteAllText(marker, JsonSerializer.Serialize(new { build_id = buildId }));
        var gate = Path.Combine(relative, MarkerName);
        files[gate] = marker;
        FileTransaction.Commit(gameDir, files, new BackupService(), gate);
    }

    static IReadOnlyDictionary<string, string?> StageOfflineDump(string scratch, string gameDir, string? dumpRootName, string buildId,
        string unpackedDll, string unpackedMeta, string gaHash, uint metadataVersion) {
        var root = Path.Combine(scratch, "dump-publish");
        Directory.CreateDirectory(root);
        var manifest = Path.Combine(root, "manifest.json");
        File.WriteAllText(manifest, JsonSerializer.Serialize(new {
            build_id = buildId, clientpatch = "manager-offline", metadata_version = metadataVersion,
            metadata_size = new FileInfo(unpackedMeta).Length, game_assembly_size = new FileInfo(unpackedDll).Length,
            image_base = "0x0", handles_restored = 0, game_assembly_file_fingerprint = gaHash,
            dumped_unix = DateTimeOffset.UtcNow.ToUnixTimeSeconds()
        }));
        var done = Path.Combine(root, "DONE"); File.WriteAllText(done, buildId);
        var latest = Path.Combine(root, LatestName);
        File.WriteAllText(latest, JsonSerializer.Serialize(new {
            build_id = buildId, dir = buildId, metadata_version = metadataVersion, game_assembly_file_fingerprint = gaHash
        }));
        var dumpRelative = Path.GetRelativePath(gameDir, DumpRoot(gameDir, dumpRootName));
        var buildRelative = Path.Combine(dumpRelative, buildId);
        return new Dictionary<string, string?> {
            [Path.Combine(buildRelative, GameAssemblyName)] = unpackedDll,
            [Path.Combine(buildRelative, "global-metadata.dat")] = unpackedMeta,
            [Path.Combine(buildRelative, "manifest.json")] = manifest,
            [Path.Combine(buildRelative, "DONE")] = done,
            [Path.Combine(dumpRelative, LatestName)] = latest
        };
    }
    /// <summary>Reads build_id + game_assembly_file_fingerprint from latest.json (tolerates the
    /// pre-fingerprint schema, like clientpatch's parse_latest).</summary>
    static (string BuildId, string GaFileHash)? ReadLatest(string path) {
        try {
            if (!File.Exists(path)) return null;
            using var doc = JsonDocument.Parse(File.ReadAllText(path));
            if (!doc.RootElement.TryGetProperty("build_id", out var b) || b.ValueKind != JsonValueKind.String)
                return null;
            var ga = doc.RootElement.TryGetProperty("game_assembly_file_fingerprint", out var g)
                && g.ValueKind == JsonValueKind.String
                ? g.GetString() ?? ""
                : "";
            return (b.GetString()!, ga);
        }
        catch {
            return null;
        }
    }

    /// <summary>Reads the <c>build_id</c> field from a <c>clientpatch-interop.json</c> marker, or null.</summary>
    static string? ReadMarkerBuildId(string markerPath) {
        try {
            if (!File.Exists(markerPath)) return null;
            using var doc = JsonDocument.Parse(File.ReadAllText(markerPath));
            return doc.RootElement.TryGetProperty("build_id", out var v) && v.ValueKind == JsonValueKind.String
                ? v.GetString()
                : null;
        }
        catch {
            return null;
        }
    }

    /// <summary>build_id per clientpatch: first 8 bytes of the SHA-256 of the DECRYPTED
    /// global-metadata.dat, lowercase hex (16 chars). Null when the file is absent.</summary>
    static string? BuildIdFromMetadata(string decryptedMetadataPath) {
        if (!File.Exists(decryptedMetadataPath)) return null;
        using var stream = File.OpenRead(decryptedMetadataPath);
        using var sha = SHA256.Create();
        var hash = sha.ComputeHash(stream);
        return Convert.ToHexString(hash, 0, 8).ToLowerInvariant();
    }

    /// <summary>clientpatch's ga_fingerprint: sha256(file_size_le64 ++ first 4 MiB), full
    /// lowercase hex. Null when the file is absent.</summary>
    internal static string? GaFingerprint(string gameAssemblyPath) {
        try {
            if (!File.Exists(gameAssemblyPath)) return null;
            using var stream = File.OpenRead(gameAssemblyPath);
            var size = stream.Length;
            var prefix = new byte[(int)Math.Min(FingerprintPrefix, size)];
            var read = 0;
            while (read < prefix.Length) {
                var n = stream.Read(prefix, read, prefix.Length - read);
                if (n <= 0) break;
                read += n;
            }
            using var sha = SHA256.Create();
            sha.TransformBlock(BitConverter.GetBytes(size), 0, 8, null, 0);
            sha.TransformFinalBlock(prefix, 0, read);
            return Convert.ToHexString(sha.Hash!).ToLowerInvariant();
        }
        catch {
            return null;
        }
    }

    /// <summary>The IL2CPP metadata format version: u32 LE at offset 4 of the metadata header.</summary>
    static uint ReadMetadataVersion(string metadataPath) {
        using var stream = File.OpenRead(metadataPath);
        var buf = new byte[8];
        if (stream.Read(buf, 0, buf.Length) < buf.Length) return 0;
        return BitConverter.ToUInt32(buf, 4);
    }

    /// <summary>
    /// Creates a per-run scratch dir on the same volume as the install target (under the target's
    /// BepInEx root when a gameDir is known, else beside the given base), so staging and the per-file
    /// rename in <see cref="Install"/> stay same-volume. Never uses %TEMP% (which may be a different
    /// volume than the game dir).
    /// </summary>
    static string NewScratch(string baseDir) {
        var parent = Path.Combine(baseDir, BepInexRootDir);
        try { Directory.CreateDirectory(parent); }
        catch { parent = baseDir; }
        var dir = Path.Combine(parent, ".cpm-interop-scratch-" + Path.GetRandomFileName());
        Directory.CreateDirectory(dir);
        return dir;
    }

    static void TryDeleteDir(string dir) {
        try { if (Directory.Exists(dir)) Directory.Delete(dir, recursive: true); } catch { /* best-effort */ }
    }
}
