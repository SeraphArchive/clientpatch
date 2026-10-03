using ClientpatchManager.Core.Feed;
using Xunit;

public class ReleaseSelectionTests {
    static ReleaseInfo R(string component, string version) =>
        new(component, version, Channel.Stable, Array.Empty<ReleaseAsset>());

    [Fact] public void Latest_compares_dotted_versions_numerically() {
        var releases = new[] { R("clientpatch", "1.9.0"), R("clientpatch", "1.10.0"), R("clientpatch", "1.8.3") };
        Assert.Equal("1.10.0", ReleaseSelection.Latest(releases, "clientpatch")!.Version);
    }

    [Fact] public void Latest_ignores_other_components() {
        var releases = new[] { R("langpacks", "9.9.9"), R("clientpatch", "1.0.0") };
        Assert.Equal("1.0.0", ReleaseSelection.Latest(releases, "clientpatch")!.Version);
    }

    [Fact] public void Latest_keeps_feed_order_for_non_numeric_versions() {
        // bepinex build ids are not semver; GitHub returns newest first, so the first one wins.
        var releases = new[] { R("bepinex", "be.790"), R("bepinex", "be.788") };
        Assert.Equal("be.790", ReleaseSelection.Latest(releases, "bepinex")!.Version);
    }

    [Fact] public void Latest_returns_null_when_component_absent() {
        Assert.Null(ReleaseSelection.Latest(new[] { R("tools", "1.0.0") }, "clientpatch"));
    }

    [Fact] public void Latest_orders_prereleases_even_when_older_release_was_republished() {
        var releases = new[] { R("clientpatch", "2.0.0-beta.9"), R("clientpatch", "2.0.0-beta.10"), R("clientpatch", "1.9.0") };
        Assert.Equal("2.0.0-beta.10", ReleaseSelection.Latest(releases, "clientpatch")!.Version);
    }

    [Theory]
    [InlineData("1.0.0-alpha", "1.0.0-alpha.1")]
    [InlineData("1.0.0-alpha.1", "1.0.0-alpha.beta")]
    [InlineData("1.0.0-beta.2", "1.0.0-beta.11")]
    [InlineData("1.0.0-rc.1", "1.0.0")]
    [InlineData("1.0.0", "10.0.0")]
    public void Compares_semantic_precedence(string older, string newer) {
        Assert.True(ReleaseSelection.CompareVersions(older, newer) < 0);
        Assert.True(ReleaseSelection.CompareVersions(newer, older) > 0);
    }

    [Fact] public void Metadata_does_not_change_version_precedence() =>
        Assert.Equal(0, ReleaseSelection.CompareVersions("1.0.0+one", "1.0.0+two"));

    [Theory]
    [InlineData("1.0.0-beta.01")]
    [InlineData("1.0.0-")]
    [InlineData("01.0.0")]
    [InlineData("unknown")]
    public void Malformed_versions_are_not_comparable(string version) =>
        Assert.Null(ReleaseSelection.CompareVersions(version, "1.0.0"));
}
