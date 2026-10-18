using ClientpatchManager.Core.Config;

public class OfficialServerWarningTests {
    [Theory]
    [InlineData("")]
    [InlineData("[lilypad]")]
    [InlineData("[lilypad]\nenable=false")]
    [InlineData("[lilypad]\nenabled=false")]
    public void Missing_or_disabled_redirection_warns(string text) =>
        Assert.True(new ConfigService().Load(text).UsesOfficialServers);

    [Theory]
    [InlineData("api_base", "https://heaven-burns-red.wfs.games/", true)]
    [InlineData("api_base", "https://api-ap.heaven-burns-red.wfs.games/api/", true)]
    [InlineData("api_base", " HTTPS://API.HEAVEN-BURNS-RED.WFS.GAMES.:443/?x=1 ", true)]
    [InlineData("api_base", "https://user:pass@api.heaven-burns-red.wfs.games/", true)]
    [InlineData("platform_base", "https://gree-apps.net/", true)]
    [InlineData("platform_base", "https://gl-payment.jp.gree-apps.net:443/#fragment", true)]
    [InlineData("platform_base", " HTTPS://GL-PAYMENT.GREE-APPS.NET./ ", true)]
    [InlineData("api_base", "https://api.heaven-burns-red.wfs.games.example.org/", false)]
    [InlineData("api_base", "https://notheaven-burns-red.wfs.games/", false)]
    [InlineData("platform_base", "https://notgree-apps.net/", false)]
    [InlineData("platform_base", "https://gree-apps.net.example.org/", false)]
    [InlineData("api_base", "https://example.org/heaven-burns-red.wfs.games", false)]
    [InlineData("platform_base", "https://example.org/?host=gree-apps.net", false)]
    [InlineData("api_base", "https://api.heaven-burns-red.wfs.games@example.org/", false)]
    [InlineData("platform_base", "https://gree-apps.net@example.org/", false)]
    [InlineData("api_base", "http://127.0.0.1:8443/", false)]
    [InlineData("platform_base", "http://[::1]:8443/", false)]
    public void Either_endpoint_matches_its_official_host_only(string key, string endpoint, bool expected) {
        var text = "[lilypad]\nenable=true\napi_base='http://localhost/'\nplatform_base='http://localhost/'";
        text = text.Replace($"{key}='http://localhost/'", $"{key}='{endpoint}'");
        var service = new ConfigService();
        // Official endpoints remain valid and saveable; this is only a warning.
        var config = service.Load(text);
        Assert.Equal(expected, config.UsesOfficialServers);
        Assert.Equal(expected, service.Load(service.ApplyToText(text, config)).UsesOfficialServers);
    }
}
