using Octokit;

namespace ClientpatchManager.Core.Feed;

/// <summary>
/// Queries GitHub Releases via Octokit and maps them onto <see cref="ReleaseInfo"/>.
/// Never throws on network/rate-limit/API failures: those are wrapped into
/// <c>FeedResult(false, [], message)</c>. The endpoint is mutable so a Settings edit
/// takes effect without restarting the manager.
/// </summary>
public sealed class GitHubReleaseFeed : IReleaseFeed {
    private readonly IGitHubClient _client;
    // Reference assignment is atomic, so readers always see a consistent (owner, repo) pair.
    private volatile Endpoint _endpoint;

    private sealed record Endpoint(string Owner, string Repo);

    public GitHubReleaseFeed(IGitHubClient client, string owner, string repo) {
        _client = client ?? throw new ArgumentNullException(nameof(client));
        _endpoint = new Endpoint(owner ?? throw new ArgumentNullException(nameof(owner)),
                                 repo ?? throw new ArgumentNullException(nameof(repo)));
    }

    /// <summary>Point the feed at another repository (Settings "release feed" edits).</summary>
    public void SetEndpoint(string owner, string repo) {
        if (string.IsNullOrWhiteSpace(owner) || string.IsNullOrWhiteSpace(repo))
            throw new ArgumentException("owner and repo are required.");
        _endpoint = new Endpoint(owner, repo);
    }

    public async Task<FeedResult> QueryAsync(Channel channel, CancellationToken ct) {
        var endpoint = _endpoint;
        try {
            var releases = await _client.Repository.Release
                .GetAll(endpoint.Owner, endpoint.Repo)
                .ConfigureAwait(false);

            var mapped = new List<ReleaseInfo>();
            foreach (var release in releases) {
                ct.ThrowIfCancellationRequested();
                if (release.Draft) continue;

                var assets = new List<ReleaseAsset>(release.Assets?.Count ?? 0);
                if (release.Assets is not null) {
                    foreach (var a in release.Assets) {
                        assets.Add(new ReleaseAsset(a.Name, a.BrowserDownloadUrl, a.Size));
                    }
                }

                var info = ReleaseMapping.FromTag(release.TagName, release.Prerelease, assets);
                if (info is not null && info.Channel == channel) {
                    mapped.Add(info);
                }
            }

            return new FeedResult(true, mapped, null);
        }
        catch (OperationCanceledException) {
            // Cooperative cancellation propagates; not a feed failure.
            throw;
        }
        catch (RateLimitExceededException) {
            return new FeedResult(false, Array.Empty<ReleaseInfo>(), "GitHub API rate limit exceeded");
        }
        catch (ApiException ex) {
            return new FeedResult(false, Array.Empty<ReleaseInfo>(),
                $"GitHub API error: {ex.StatusCode} ({endpoint.Owner}/{endpoint.Repo})");
        }
        catch (HttpRequestException) {
            return new FeedResult(false, Array.Empty<ReleaseInfo>(), "Network error reaching the release feed");
        }
        catch (Exception) {
            // Backstop: never let an unexpected error escape the feed contract, and never
            // forward an exception message verbatim (it can carry request URLs or auth context).
            return new FeedResult(false, Array.Empty<ReleaseInfo>(), "Unexpected feed error");
        }
    }
}
