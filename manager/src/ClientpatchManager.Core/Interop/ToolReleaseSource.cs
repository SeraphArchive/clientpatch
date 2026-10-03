using Octokit;

namespace ClientpatchManager.Core.Interop;

/// <summary>One upstream tool release: a version tag and a downloadable asset URL.</summary>
public sealed record ToolRelease(string Version, string DownloadUrl, string? Sha256 = null);

/// <summary>
/// Resolves each interop tool's latest stable release from its OWN upstream repository.
/// The three tools are independent open-source projects with their own releases, so no
/// clientpatch release feed is involved in offline interop generation.
/// </summary>
public interface IToolReleaseSource {
    Task<ToolRelease> LatestAsync(InteropTool tool, CancellationToken ct);
}

/// <summary>GitHub Releases-backed <see cref="IToolReleaseSource"/> (Octokit). Never throws
/// raw API details; failures surface as sanitized <see cref="InvalidOperationException"/>.</summary>
public sealed class GitHubToolReleaseSource : IToolReleaseSource {
    // Pinned Il2CppDumper release from c01ns/Il2CppDumper; used without binary patches.
    public const string DumperVersion = "v39-support-hbr1";
    const string DumperUrl = "https://github.com/c01ns/Il2CppDumper/releases/download/v39-support/Il2CppDumper-win-x64-net8-v39.zip";
    const string DumperSha256 = "aac8b49b7c8c972852066e196db8f1fb4c02dfcfc1b3b46dc979c7a2bf83a808";
    private readonly IGitHubClient _client;

    public GitHubToolReleaseSource(IGitHubClient client) =>
        _client = client ?? throw new ArgumentNullException(nameof(client));

    /// <summary>Upstream repo + Windows asset selector per tool. Il2CppDumper uses the
    /// self-contained <c>-win-</c> zip (no .NET runtime needed on the target machine);
    /// senbei is a native binary; Il2CppInterop.CLI ships framework-dependent (net6) and
    /// is launched with <c>DOTNET_ROLL_FORWARD=LatestMajor</c>.</summary>
    internal static readonly IReadOnlyDictionary<InteropTool, (string Owner, string Repo, Func<string, bool> IsAsset)> Sources =
        new Dictionary<InteropTool, (string, string, Func<string, bool>)> {
            [InteropTool.Senbei] = ("Momoko-Ayase", "Senbei",
                n => n.EndsWith("-x86_64-pc-windows-msvc.zip", StringComparison.OrdinalIgnoreCase)),
            [InteropTool.Il2CppDumper] = ("c01ns", "Il2CppDumper",
                n => n.StartsWith("Il2CppDumper-win-", StringComparison.OrdinalIgnoreCase)
                     && n.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)),
            [InteropTool.Il2CppInteropCli] = ("BepInEx", "Il2CppInterop",
                n => n.StartsWith("Il2CppInterop.CLI.", StringComparison.OrdinalIgnoreCase)
                     && n.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)),
        };

    public async Task<ToolRelease> LatestAsync(InteropTool tool, CancellationToken ct) {
        ct.ThrowIfCancellationRequested();
        if (tool == InteropTool.Il2CppDumper) return new(DumperVersion, DumperUrl, DumperSha256);
        var (owner, repo, isAsset) = Sources[tool];
        try {
            var releases = await _client.Repository.Release.GetAll(owner, repo).ConfigureAwait(false);
            foreach (var release in releases) {
                ct.ThrowIfCancellationRequested();
                if (release.Prerelease)
                    continue;
                var asset = release.Assets.FirstOrDefault(a => isAsset(a.Name));
                if (asset is not null)
                    return new ToolRelease(release.TagName, asset.BrowserDownloadUrl);
            }
            throw new InvalidOperationException($"no suitable release asset in {owner}/{repo} for {tool}");
        }
        catch (OperationCanceledException) {
            throw;
        }
        catch (RateLimitExceededException) {
            throw new InvalidOperationException("tool release query failed: GitHub API rate limit exceeded");
        }
        catch (ApiException ex) {
            throw new InvalidOperationException($"tool release query failed: GitHub API error: {ex.StatusCode} ({owner}/{repo})");
        }
        catch (HttpRequestException) {
            throw new InvalidOperationException("tool release query failed: network error reaching GitHub");
        }
    }
}
