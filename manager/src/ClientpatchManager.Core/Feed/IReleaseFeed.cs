namespace ClientpatchManager.Core.Feed;

public interface IReleaseFeed {
    Task<FeedResult> QueryAsync(Channel channel, CancellationToken ct);
}
