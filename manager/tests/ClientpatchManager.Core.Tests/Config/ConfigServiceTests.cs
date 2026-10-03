using Xunit;
using ClientpatchManager.Core.Config;

public class ConfigServiceTests {
    const string Sample = """
        # top comment
        [loader]
        log = "clientpatch.log"
        [lilypad]
        enable = true # inline
        api_base = "http://127.0.0.1:8443/"
        platform_base = "http://127.0.0.1:8443"
        server_public_key_pem = "keys/public.pem"
        signing = "noop"
        """;

    [Fact] public void Load_reads_values() {
        var c = new ConfigService().Load(Sample);
        Assert.True(c.Lilypad.Enabled);
        Assert.Equal("noop", c.Lilypad.Signing);
    }

    [Fact] public void ApplyToText_preserves_comments_and_order() {
        var svc = new ConfigService();
        var c = svc.Load(Sample);
        c.Lilypad = c.Lilypad with { Signing = "rsa" };
        var outText = svc.ApplyToText(Sample, c);
        Assert.Contains("# top comment", outText);
        Assert.Contains("# inline", outText);
        Assert.Contains("signing = \"rsa\"", outText);
        Assert.True(outText.IndexOf("[loader]") < outText.IndexOf("[lilypad]"));
    }

    [Fact] public void ApplyToText_writes_module_enable_preserving_inline_comment() {
        var svc = new ConfigService();
        var c = svc.Load(Sample);
        c.Steam = c.Steam with { Enabled = true };
        var outText = svc.ApplyToText(Sample, c);
        Assert.Contains("[lilypad]", outText);
        Assert.Contains("[steam]", outText);
        // Per-section flags remain re-parseable.
        Assert.Equal("", svc.RawValidate(outText));
        Assert.True(svc.Load(outText).Lilypad.Enabled);
        Assert.True(svc.Load(outText).Steam.Enabled);
        Assert.Contains("# inline", outText);
        Assert.Contains("# top comment", outText);
        Assert.True(outText.IndexOf("[loader]") < outText.IndexOf("[lilypad]"));
    }

    [Fact] public void RawValidate_reports_syntax_error() {
        Assert.NotEqual("", new ConfigService().RawValidate("[loader"));
        Assert.Equal("", new ConfigService().RawValidate(Sample));
    }

    [Fact] public void Load_throws_on_invalid_toml() {
        Assert.Throws<FormatException>(() => new ConfigService().Load("[loader"));
    }

    // ---- the Rust schema (src/config.rs) ---------------------------------

    const string RustShaped = """
        [loader]
        modules = ["lilypad", "steam"]
        log = "cp.log"

        [interopdump]
        out_dir = "interop"
        """;

    [Fact] public void Load_reads_the_loader_and_interop_keys() {
        var c = new ConfigService().Load(RustShaped);
        Assert.Equal("cp.log", c.LoaderLog);
        Assert.Equal("interop", c.InteropDump!.OutDir);
    }

    [Fact] public void Legacy_modules_list_is_read_for_compatibility() {
        var c = new ConfigService().Load("[loader]\nmodules = [\"steam\"]");
        Assert.True(c.Steam.Enabled);
        Assert.False(c.Lilypad.Enabled);
        Assert.False(c.Titlebar.Enabled);
        Assert.False(c.RegRedirect.Enabled);
    }

    [Fact] public void ApplyToText_rejects_empty_required_values() {
        var svc = new ConfigService();
        var c = svc.Load(Sample);
        c.Lilypad = c.Lilypad with { ApiBase = "" }; // api_base must be non-empty for the proxy
        Assert.Throws<FormatException>(() => svc.ApplyToText(Sample, c));
    }

    [Fact] public void ApplyToText_escapes_strings_into_valid_toml() {
        var svc = new ConfigService();
        const string withTemplate = "[loader]\nmodules = [\"titlebar\"]\n[titlebar]\ntemplate = \"x\"";
        var c = svc.Load(withTemplate);
        c.Titlebar = c.Titlebar with { Template = "line1\nline2 \"quoted\"" };
        var outText = svc.ApplyToText(withTemplate, c);
        Assert.Equal("", svc.RawValidate(outText));
        Assert.Contains("template = \"line1\\nline2 \\\"quoted\\\"\"", outText);
    }

    [Fact] public void New_switches_override_legacy_flags_and_save_only_canonical_keys() {
        const string text = "[loader]\nmodules=['steam','interopdump']\n[steam]\nenable=false\n[interopdump]\nenable=false\n[bepinex]\nenable=true\nenabled=false\n[report]\nenable=false\nenabled=true\n";
        var service = new ConfigService(); var cfg = service.Load(text);
        Assert.False(cfg.Steam.Enabled); Assert.False(cfg.InteropDump!.Enabled);
        Assert.True(cfg.BepInEx!.Enabled); Assert.False(cfg.Lilypad.Report.Enabled);
        var saved = service.ApplyToText(text, cfg);
        Assert.DoesNotContain("modules", saved); Assert.DoesNotContain("enabled", saved);
        var roundtrip = service.Load(saved);
        Assert.False(roundtrip.Steam.Enabled); Assert.True(roundtrip.BepInEx!.Enabled); Assert.False(roundtrip.Lilypad.Report.Enabled);
    }

    [Fact] public void Form_roundtrip_persists_all_seven_switches_and_preserves_other_settings() {
        const string text = "# preserved\n[steam]\nenable=false\n[bepinex]\nenable=false\nauto_generate=false\n[interopdump]\nenable=false\nforce=true\n[report]\nenable=true\nurl='http://report/'\n";
        var service = new ConfigService(); var cfg = service.Load(text);
        cfg.Lilypad = new(true, "http://api/", "http://platform", "noop") { Report = cfg.Lilypad.Report };
        cfg.Steam = cfg.Steam with { Enabled = true };
        cfg.Titlebar = cfg.Titlebar with { Enabled = true };
        cfg.RegRedirect = cfg.RegRedirect with { Enabled = true };
        cfg.BepInEx = cfg.BepInEx! with { Enabled = true };
        cfg.InteropDump = cfg.InteropDump! with { Enabled = true };
        cfg.Lilypad = cfg.Lilypad with { Report = cfg.Lilypad.Report with { Enabled = false } };
        var saved = service.ApplyToText(text, cfg); var loaded = service.Load(saved);
        foreach (var section in new[] { "lilypad", "steam", "titlebar", "regredirect", "bepinex", "interopdump" }) Assert.True(loaded.IsEnabled(section), section + ": " + saved);
        Assert.False(loaded.Lilypad.Report.Enabled); Assert.Equal("http://report/", loaded.Lilypad.Report.Url);
        Assert.Contains("# preserved", saved); Assert.Contains("auto_generate=false", saved); Assert.Contains("force=true", saved);
    }

    [Theory]
    [InlineData("steam.enable = false\nsteam.mode = 'auto'\n")]
    [InlineData("steam = { enable = false, mode = 'auto' } # retained\n")]
    public void Dotted_and_inline_sections_support_switch_edits(string text) {
        var service = new ConfigService(); var cfg = service.Load(text); cfg.Steam = cfg.Steam with { Enabled = true };
        var saved = service.ApplyToText(text, cfg);
        Assert.True(service.Load(saved).Steam.Enabled, saved);
        if (text.Contains("# retained")) Assert.Contains("# retained", saved);
    }

    [Theory]
    [InlineData("['steam']\n'enable' = false # retained\n")]
    [InlineData("\"steam\" . 'enable' = false # retained\n")]
    [InlineData("'steam' = { 'enable' = false } # retained\n")]
    public void Quoted_keys_and_spaced_dotted_keys_preserve_form_edits(string text) {
        var service = new ConfigService(); var cfg = service.Load(text);
        cfg.Steam = cfg.Steam with { Enabled = true };
        var saved = service.ApplyToText(text, cfg);
        Assert.True(service.Load(saved).Steam.Enabled);
        Assert.Contains("# retained", saved);
    }

    [Fact] public void Quoted_literal_dots_do_not_alias_nested_report_keys() {
        const string text = "['lilypad.report']\nenable=false # unrelated\n";
        var service = new ConfigService(); var cfg = service.Load(text);
        var saved = service.ApplyToText(text, cfg);
        Assert.Contains("['lilypad.report']\nenable=false # unrelated", saved);
        Assert.True(service.Load(saved).Lilypad.Report.Enabled);
    }

    [Theory]
    [InlineData("lilypad")]
    [InlineData("steam")]
    [InlineData("regredirect")]
    [InlineData("titlebar")]
    [InlineData("interopdump")]
    [InlineData("bepinex")]
    [InlineData("lilypad.report")]
    public void Switches_reject_non_boolean_values(string section) {
        Assert.NotEmpty(new ConfigService().RawValidate($"[{section}]\nenable='true'\n"));
    }

    [Fact] public void Disabled_lilypad_needs_no_server_settings() {
        Assert.False(new ConfigService().Load("[lilypad]\nenable=false").Lilypad.Enabled);
    }

    [Fact] public void Shipped_example_has_matching_defaults_and_survives_form_save() {
        var text = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "clientpatch.example.toml"));
        var service = new ConfigService(); var cfg = service.Load(text);
        foreach (var name in new[] { "steam", "lilypad", "regredirect", "titlebar", "lilypad.report" }) Assert.True(cfg.IsEnabled(name));
        Assert.NotNull(cfg.InteropDump); Assert.NotNull(cfg.BepInEx);
        Assert.False(cfg.InteropDump.Enabled); Assert.False(cfg.BepInEx.Enabled);
        Assert.Equal(text, service.ApplyToText(text, cfg));
    }

    [Fact] public void Nested_report_overrides_old_table_and_preserves_its_settings_on_save() {
        const string text = "[lilypad]\nenable=false\n[lilypad.report]\nenable=true # kept\nurl='http://nested/'\n[report]\nenable=false\nurl='http://old/'\n";
        var service = new ConfigService(); var cfg = service.Load(text);
        Assert.False(cfg.Lilypad.Enabled); Assert.True(cfg.Lilypad.Report.Enabled);
        Assert.Equal("http://nested/", cfg.Lilypad.Report.Url);
        var saved = service.ApplyToText(text, cfg);
        Assert.DoesNotContain("[report]", saved); Assert.Contains("[lilypad.report]", saved); Assert.Contains("# kept", saved);
        Assert.False(service.Load(saved).Lilypad.Enabled); Assert.Equal("http://nested/", service.Load(saved).Lilypad.Report.Url);
    }

    [Theory]
    [InlineData("[lilypad.report]\nenable=false\nurl='http://report/'\n")]
    [InlineData("[report]\nenabled=false\nurl='http://report/'\n")]
    [InlineData("lilypad = { enable = false, report = { enabled = false, url = 'http://report/' } }\n")]
    [InlineData("[lilypad]\nenable=false\nreport.enable=false\nreport.url='http://report/'\n")]
    public void Report_subsection_roundtrip_supports_nested_and_legacy_forms(string text) {
        var service = new ConfigService(); var cfg = service.Load(text);
        Assert.False(cfg.Lilypad.Report.Enabled);
        cfg.Lilypad = cfg.Lilypad with { Report = cfg.Lilypad.Report with { Enabled = true } };
        var saved = service.ApplyToText(text, cfg); var loaded = service.Load(saved);
        Assert.True(loaded.Lilypad.Report.Enabled); Assert.False(loaded.Lilypad.Enabled);
        Assert.Equal("http://report/", loaded.Lilypad.Report.Url);
        Assert.DoesNotContain("[report]", saved); Assert.DoesNotContain("enabled", saved);
    }
}
