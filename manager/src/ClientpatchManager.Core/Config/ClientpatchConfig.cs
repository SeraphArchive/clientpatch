namespace ClientpatchManager.Core.Config;

public class ClientpatchConfig {
    /// <summary>[loader] log — the stem of the per-launch log files the proxy writes into
    /// clientpatch/logs (default clientpatch; the proxy keeps the 10 newest).</summary>
    public string? LoaderLog { get; set; }
    public LilypadSection Lilypad { get; set; } = new(false, "", "", "noop");
    public SteamSection Steam { get; set; } = new(false, "auto", "JP", "japanese");
    public RegRedirectSection RegRedirect { get; set; } = new(false, "clientpatch");
    public TitlebarSection Titlebar { get; set; } = new(false, " — clientpatch → {api_host}");
    public BepInExSection? BepInEx { get; set; }
    public InteropDumpSection? InteropDump { get; set; }

    public bool IsEnabled(string section) => section switch {
        "lilypad" => Lilypad.Enabled, "steam" => Steam.Enabled,
        "regredirect" => RegRedirect.Enabled, "titlebar" => Titlebar.Enabled,
        "bepinex" => BepInEx?.Enabled == true, "interopdump" => InteropDump?.Enabled == true,
        "lilypad.report" => Lilypad.Report.Enabled, _ => false
    };
}

public record LilypadSection(bool Enabled, string ApiBase, string PlatformBase, string Signing, string PublicKeyPath = "") {
    public ReportSection Report { get; init; } = new(true);
}
public record SteamSection(bool Enabled, string Mode, string Country, string UiLanguage);
public record RegRedirectSection(bool Enabled, string Suffix);
public record TitlebarSection(bool Enabled, string Template);
public record BepInExSection(bool Enabled, string Root = "BepInEx", string Doorstop = "doorstop.dll");
public record InteropDumpSection(bool Enabled, string OutDir = "interop");
public record ReportSection(bool Enabled, string Url = "");
