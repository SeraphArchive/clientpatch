using ClientpatchManager.Core.Feed;
using Xunit;

namespace ClientpatchManager.Core.Tests.Feed;

public class FeedMappingTests {
    static IReadOnlyList<ReleaseAsset> A() => new[]{ new ReleaseAsset("version.dll","http://x/version.dll",10) };

    [Fact] public void Maps_clientpatch_stable_tag() {
        var r = ReleaseMapping.FromTag("clientpatch-v1.5.0", false, A());
        Assert.Equal("clientpatch", r!.Component);
        Assert.Equal("1.5.0", r.Version);
        Assert.Equal(Channel.Stable, r.Channel);
    }
    [Fact] public void Prerelease_is_beta() {
        var r = ReleaseMapping.FromTag("clientpatch-v1.6.0", true, A());
        Assert.Equal(Channel.Beta, r!.Channel);
    }
    [Fact] public void Unknown_tag_is_ignored() {
        Assert.Null(ReleaseMapping.FromTag("random-tag", false, A()));
    }
}
