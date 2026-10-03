//! `clientpatch.toml` schema, parsing, and validation. Pure logic.
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub loader: Loader,
    pub lilypad: Option<Lilypad>,
    pub regredirect: Option<RegRedirect>,
    pub titlebar: Option<Titlebar>,
    pub steam: Option<Steam>,
    pub interopdump: Option<InteropDump>,
    pub bepinex: Option<Bepinex>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Loader {
    #[serde(default = "default_log")]
    pub log: String,
    /// After init, deliberately AV so the crash window + UnityCrashHandler
    /// block can be verified. Also honoured via env `CLIENTPATCH_CRASH_TEST=1`.
    /// Off by default — never leave this on for a real session.
    #[serde(default)]
    pub crash_test: bool,
}

impl Default for Loader {
    fn default() -> Self {
        Loader {
            log: default_log(),
            crash_test: false,
        }
    }
}

fn default_log() -> String {
    "clientpatch.log".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lilypad {
    #[serde(default)]
    pub report: Report,
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub platform_base: String,
    #[serde(default = "default_signing")]
    pub signing: String,
    #[serde(default)]
    pub server_public_key_pem: String,
    #[serde(default)]
    pub neutralize_debugger_flag: bool,
    /// Debug-only: hook MD5 HashCore by name at runtime and log small inputs, to
    /// recover the 引き継ぎ migration-password pre-image (password + salt). Off by
    /// default; set `capture_md5 = true` under [lilypad] for one capture run.
    #[serde(default)]
    pub capture_md5: bool,
}

fn default_signing() -> String {
    "noop".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegRedirect {
    #[serde(default)]
    pub enable: bool,
    /// Sibling key is HKCU\Software\wfs\HeavenBurnsRed.<suffix>.
    #[serde(default = "default_reg_suffix")]
    pub suffix: String,
}

impl Default for RegRedirect {
    fn default() -> Self {
        RegRedirect {
            enable: false,
            suffix: default_reg_suffix(),
        }
    }
}

fn default_reg_suffix() -> String {
    "clientpatch".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Titlebar {
    #[serde(default)]
    pub enable: bool,
    /// Appended to the game window title; "{api_host}" is substituted at runtime.
    #[serde(default = "default_titlebar_template")]
    pub template: String,
}

impl Default for Titlebar {
    fn default() -> Self {
        Titlebar {
            enable: false,
            template: default_titlebar_template(),
        }
    }
}

fn default_titlebar_template() -> String {
    " — clientpatch → {api_host}".to_string()
}

fn default_true() -> bool {
    true
}

/// Version-report settings (module #1a): at launch, report the installed
/// build's program + asset version/hash to LilyPad so the server can echo the
/// correct assetVersion/assetHash on /api/app/start without config hand-editing.
#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Optional override; when empty, defaults to
    /// `{lilypad.api_base}/api/clientpatch/version`.
    #[serde(default)]
    pub url: String,
}

impl Default for Report {
    fn default() -> Self {
        Report {
            enable: true,
            url: String::new(),
        }
    }
}

/// Steam launch policy (skip relaunch / full stub / auto).
#[derive(Debug, Clone, Deserialize)]
pub struct Steam {
    #[serde(default)]
    pub enable: bool,
    /// `off` | `skip_restart` | `stub` | `auto`
    #[serde(default = "default_steam_mode")]
    pub mode: String,
    /// Stub only: `ISteamUtils::GetIPCountry`. HBR's title lock wants `JP`.
    #[serde(default = "default_ip_country")]
    pub ip_country: String,
    /// Stub only: `GetSteamUILanguage` / `GetCurrentGameLanguage`.
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
}

impl Default for Steam {
    fn default() -> Self {
        Steam {
            enable: false,
            mode: default_steam_mode(),
            ip_country: default_ip_country(),
            ui_language: default_ui_language(),
        }
    }
}

fn default_steam_mode() -> String {
    "auto".to_string()
}

fn default_ip_country() -> String {
    "JP".to_string()
}

fn default_ui_language() -> String {
    "japanese".to_string()
}

/// Module #3 — runtime IL2CPP dump for local interop generation. Once per game
/// build, carves the decrypted `GameAssembly.dll` image + `global-metadata.dat`
/// out of the live process into `<out_dir>/<build_id>/`, so a Cpp2IL/Il2CppInterop
/// generator can run against them (the on-disk files are protected and unreadable).
#[derive(Debug, Clone, Deserialize)]
pub struct InteropDump {
    #[serde(default)]
    pub enable: bool,
    /// Output root; relative paths resolve against `<game dir>/clientpatch`.
    /// Defaults to `interop`.
    #[serde(default = "default_interop_out_dir")]
    pub out_dir: String,
    /// Re-dump even when `<build_id>/DONE` already exists.
    #[serde(default)]
    pub force: bool,
    /// Rewrite Unity 6 runtime type/generic-param handles in the carved image
    /// back to static metadata indices. Default OFF: Il2CppDumper
    /// consumes the raw handle form (its `IsDumped` mode derives the metadata
    /// blob base from the handles and converts them itself). Enable only for
    /// Cpp2IL-compatible output, which expects the static index form.
    #[serde(default)]
    pub restore_handles: bool,
}

impl Default for InteropDump {
    fn default() -> Self {
        InteropDump {
            enable: false,
            out_dir: default_interop_out_dir(),
            force: false,
            restore_handles: false,
        }
    }
}

fn default_interop_out_dir() -> String {
    "interop".to_string()
}

/// Module #4 — BepInEx orchestration. clientpatch becomes the mod platform
/// bootstrapper: doorstop ships renamed (no autoload), and clientpatch loads it
/// only when enabled + the interop assemblies are current, generating them
/// first (and restarting the game once) otherwise.
#[derive(Debug, Clone, Deserialize)]
pub struct Bepinex {
    /// Master switch. When false, doorstop.dll is never loaded.
    #[serde(default)]
    pub enable: bool,
    /// BepInEx root directory, relative to the game dir.
    #[serde(default = "default_bepinex_root")]
    pub root: String,
    /// Doorstop proxy DLL filename in the game dir (renamed winhttp.dll).
    #[serde(default = "default_bepinex_doorstop")]
    pub doorstop: String,
    /// Pinned BepInEx IL2CPP zip URL for auto-download.
    #[serde(default = "default_bepinex_url")]
    pub url: String,
    /// Download + extract BepInEx when the payload is missing, then restart.
    #[serde(default = "default_true")]
    pub auto_download: bool,
    /// Generate interop assemblies (via `interopgen`) when outdated.
    #[serde(default = "default_true")]
    pub auto_generate: bool,
    /// Path to the interopgen PowerShell wrapper, inside `<game dir>/clientpatch`.
    #[serde(default = "default_bepinex_interopgen")]
    pub interopgen: String,
    /// Optional path to the clientpatch manager exe (absolute, or relative to the game dir).
    /// When set, or when `manager.exe` sits beside the game, every unmet startup condition
    /// (missing payload, missing interop, clean-boot relaunch) is handed to that exe.
    /// It shows progress, finishes the job, relaunches the game, and exits.
    /// Empty and no `manager.exe` beside the game => silent in-process fallback.
    #[serde(default)]
    pub manager: String,
    /// Relaunch method the manager should use after generating (`steam` or `direct`).
    #[serde(default = "default_bepinex_launch_method")]
    pub launch_method: String,
    /// Restart-loop guard: give up after this many consecutive setup restarts.
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

impl Default for Bepinex {
    fn default() -> Self {
        Bepinex {
            enable: false,
            root: default_bepinex_root(),
            doorstop: default_bepinex_doorstop(),
            url: default_bepinex_url(),
            auto_download: true,
            auto_generate: true,
            interopgen: default_bepinex_interopgen(),
            manager: String::new(),
            launch_method: default_bepinex_launch_method(),
            max_restarts: default_max_restarts(),
        }
    }
}

fn default_bepinex_root() -> String {
    "BepInEx".to_string()
}
fn default_bepinex_doorstop() -> String {
    "doorstop.dll".to_string()
}
fn default_bepinex_url() -> String {
    "https://builds.bepinex.dev/projects/bepinex_be/788/BepInEx-Unity.IL2CPP-win-x64-6.0.0-be.788%2B5b766a3.zip".to_string()
}
fn default_bepinex_interopgen() -> String {
    "clientpatch\\interopgen.ps1".to_string()
}
fn default_bepinex_launch_method() -> String {
    "steam".to_string()
}
fn default_max_restarts() -> u32 {
    2
}

impl Config {
    pub fn version_report_enabled(&self) -> bool {
        self.lilypad
            .as_ref()
            .is_some_and(|lp| lp.enable && lp.report.enable)
    }
    pub fn module_enabled(&self, name: &str) -> bool {
        match name {
            "lilypad" => self.lilypad.as_ref().is_some_and(|m| m.enable),
            "steam" => self.steam.as_ref().is_some_and(|m| m.enable),
            "regredirect" => self.regredirect.as_ref().is_some_and(|m| m.enable),
            "titlebar" => self.titlebar.as_ref().is_some_and(|m| m.enable),
            "interopdump" => self.interopdump.as_ref().is_some_and(|m| m.enable),
            "bepinex" => self.bepinex.as_ref().is_some_and(|m| m.enable),
            _ => false,
        }
    }
    /// Parse + validate a TOML string.
    pub fn parse(s: &str) -> anyhow::Result<Config> {
        let mut value: toml::Value = toml::from_str(s)?;
        normalize_enable(&mut value)?;
        let mut cfg: Config = value.try_into()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Read and parse a config file. A missing file is an error the caller
    /// treats as "load nothing" (the proxy still forwards exports).
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let s = std::fs::read_to_string(path)?;
        Config::parse(&s)
    }

    fn validate(&mut self) -> anyhow::Result<()> {
        if let Some(lp) = self.lilypad.as_mut().filter(|lp| lp.enable) {
            lp.api_base = lp.api_base.trim().to_string();
            lp.platform_base = lp.platform_base.trim().to_string();
            if lp.api_base.is_empty() {
                anyhow::bail!("lilypad.api_base is empty");
            }
            if lp.platform_base.is_empty() {
                anyhow::bail!("lilypad.platform_base is empty");
            }
            match lp.signing.as_str() {
                "noop" => {}
                "rsa" => {
                    if lp.server_public_key_pem.trim().is_empty() {
                        anyhow::bail!("lilypad.signing = \"rsa\" requires server_public_key_pem");
                    }
                }
                other => {
                    anyhow::bail!("lilypad.signing must be \"noop\" or \"rsa\", got {other:?}")
                }
            }
        }
        if let Some(rr) = self.regredirect.as_ref().filter(|rr| rr.enable) {
            if rr.suffix.trim().is_empty() {
                anyhow::bail!("regredirect.suffix is empty");
            }
            if rr.suffix.contains('\\') {
                anyhow::bail!("regredirect.suffix must not contain a backslash");
            }
        }
        if let Some(st) = self.steam.as_mut().filter(|st| st.enable) {
            st.mode = st.mode.trim().to_string();
            crate::modules::steam::SteamMode::parse(&st.mode)?;
            st.ip_country = st.ip_country.trim().to_string();
            st.ui_language = st.ui_language.trim().to_string();
            if st.ip_country.is_empty() {
                anyhow::bail!("steam.ip_country is empty");
            }
            if st.ui_language.is_empty() {
                anyhow::bail!("steam.ui_language is empty");
            }
        }
        if let Some(d) = self.interopdump.as_mut().filter(|d| d.enable) {
            d.out_dir = d.out_dir.trim().to_string();
            if d.out_dir.is_empty() {
                anyhow::bail!("interopdump.out_dir is empty");
            }
        }
        if let Some(b) = &mut self.bepinex {
            b.root = b.root.trim().to_string();
            if b.root.is_empty() {
                anyhow::bail!("bepinex.root is empty");
            }
            b.doorstop = b.doorstop.trim().to_string();
            if b.doorstop.is_empty() {
                anyhow::bail!("bepinex.doorstop is empty");
            }
            if b.doorstop.eq_ignore_ascii_case("winhttp.dll") {
                anyhow::bail!("bepinex.doorstop must have a non-autoload filename");
            }
            for (name, path) in [("root", &b.root), ("doorstop", &b.doorstop)] {
                if std::path::Path::new(path).is_absolute()
                    || path.contains(':')
                    || path.split(['\\', '/']).any(|segment| segment == "..")
                {
                    anyhow::bail!("bepinex.{name} must stay inside the game directory");
                }
            }
            b.interopgen = b.interopgen.trim().to_string();
            if b.interopgen.is_empty() {
                anyhow::bail!("bepinex.interopgen is empty");
            }
            b.manager = b.manager.trim().to_string();
            b.launch_method = b.launch_method.trim().to_lowercase();
            if b.launch_method.is_empty() {
                b.launch_method = default_bepinex_launch_method();
            }
            if b.launch_method != "steam" && b.launch_method != "direct" {
                anyhow::bail!(
                    "bepinex.launch_method must be \"steam\" or \"direct\" (got \"{}\")",
                    b.launch_method
                );
            }
            if b.enable && b.auto_download && b.url.trim().is_empty() {
                anyhow::bail!("bepinex.url is empty but auto_download is on");
            }
        }
        Ok(())
    }
}

/// Read legacy switches once; runtime activation uses only each section's enable.
fn normalize_enable(value: &mut toml::Value) -> anyhow::Result<()> {
    let root = value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config must be a table"))?;
    if let Some(report) = root.remove("report") {
        let parent = root.entry("lilypad").or_insert_with(|| {
            let mut table = toml::map::Map::new();
            table.insert("enable".into(), toml::Value::Boolean(false));
            toml::Value::Table(table)
        });
        if let Some(parent) = parent.as_table_mut() {
            parent.entry("report").or_insert(report);
        }
    }
    let legacy = root
        .get("loader")
        .and_then(|v| v.get("modules"))
        .map(|v| {
            let items = v
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("loader.modules must be an array of strings"))?;
            items
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        anyhow::anyhow!("loader.modules must be an array of strings")
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?;
    if let Some(loader) = root.get_mut("loader").and_then(toml::Value::as_table_mut) {
        loader.remove("modules");
    }
    for name in [
        "lilypad",
        "steam",
        "regredirect",
        "titlebar",
        "interopdump",
        "bepinex",
    ] {
        if !root.contains_key(name)
            && name != "lilypad"
            && name != "bepinex"
            && legacy
                .as_ref()
                .is_some_and(|items| items.iter().any(|item| item == name))
        {
            root.insert(name.into(), toml::Value::Table(Default::default()));
        }
        let Some(section) = root.get_mut(name).and_then(toml::Value::as_table_mut) else {
            continue;
        };
        let old = section.remove("enabled");
        if section.contains_key("enable") {
            continue;
        }
        let enabled = if let Some(items) = &legacy {
            let listed = items.iter().any(|item| item == name);
            if name == "bepinex" {
                let old = old
                    .map(|v| {
                        v.as_bool()
                            .ok_or_else(|| anyhow::anyhow!("bepinex.enabled must be a boolean"))
                    })
                    .transpose()?
                    .unwrap_or(false);
                toml::Value::Boolean(listed && old)
            } else {
                toml::Value::Boolean(listed)
            }
        } else {
            old.unwrap_or(toml::Value::Boolean(false))
        };
        section.insert("enable".into(), enabled);
    }
    if let Some(report) = root
        .get_mut("lilypad")
        .and_then(|parent| parent.get_mut("report"))
        .and_then(toml::Value::as_table_mut)
    {
        let old = report.remove("enabled");
        report
            .entry("enable")
            .or_insert_with(|| old.unwrap_or(toml::Value::Boolean(true)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOOP: &str = r#"
[loader]
modules = ["lilypad"]
log = "clientpatch.log"

[lilypad]
api_base = "http://127.0.0.1:8443/"
platform_base = "http://127.0.0.1:8443"
signing = "noop"
"#;

    #[test]
    fn parses_noop_config() {
        let c = Config::parse(NOOP).unwrap();
        assert!(c.module_enabled("lilypad"));
        assert_eq!(c.loader.log, "clientpatch.log");
        assert!(!c.loader.crash_test);
        let lp = c.lilypad.unwrap();
        assert_eq!(lp.api_base, "http://127.0.0.1:8443/");
        assert_eq!(lp.platform_base, "http://127.0.0.1:8443");
        assert_eq!(lp.signing, "noop");
        assert!(!lp.neutralize_debugger_flag);
    }

    #[test]
    fn signing_defaults_to_noop_and_log_defaults() {
        let c = Config::parse(
            "[lilypad]\nenable=true\napi_base=\"http://h/\"\nplatform_base=\"http://h\"\n",
        )
        .unwrap();
        assert_eq!(c.lilypad.unwrap().signing, "noop");
        assert_eq!(c.loader.log, "clientpatch.log");
        assert!(!c.loader.crash_test);
    }

    #[test]
    fn crash_test_flag_parses() {
        let c = Config::parse("[loader]\ncrash_test = true\n").unwrap();
        assert!(c.loader.crash_test);
    }

    #[test]
    fn rsa_without_key_is_rejected() {
        let s = "[lilypad]\nenable=true\napi_base=\"http://h/\"\nplatform_base=\"http://h\"\nsigning=\"rsa\"\n";
        assert!(Config::parse(s).is_err());
    }

    #[test]
    fn rsa_with_key_ok() {
        let s = "[lilypad]\nenable=true\napi_base=\"http://h/\"\nplatform_base=\"http://h\"\nsigning=\"rsa\"\nserver_public_key_pem=\"key.pem\"\n";
        assert_eq!(Config::parse(s).unwrap().lilypad.unwrap().signing, "rsa");
    }

    #[test]
    fn unknown_signing_is_rejected() {
        let s = "[lilypad]\nenable=true\napi_base=\"http://h/\"\nplatform_base=\"http://h\"\nsigning=\"hmac\"\n";
        assert!(Config::parse(s).is_err());
    }

    #[test]
    fn empty_api_base_is_rejected() {
        let s = "[lilypad]\nenable=true\napi_base=\"\"\nplatform_base=\"http://h\"\n";
        assert!(Config::parse(s).is_err());
    }

    #[test]
    fn missing_lilypad_table_is_ok() {
        // A loader with no module tables is valid (loads nothing).
        let c = Config::parse("[loader]\nmodules=[]\n").unwrap();
        assert!(c.lilypad.is_none());
    }

    #[test]
    fn parses_regredirect_and_titlebar() {
        let s = r#"
[loader]
modules = ["regredirect", "titlebar"]
[regredirect]
suffix = "myserver"
[titlebar]
template = " [PATCH]"
"#;
        let c = Config::parse(s).unwrap();
        assert_eq!(c.regredirect.unwrap().suffix, "myserver");
        assert_eq!(c.titlebar.unwrap().template, " [PATCH]");
    }

    #[test]
    fn empty_tables_use_defaults() {
        let c = Config::parse("[regredirect]\nenable=true\n[titlebar]\n").unwrap();
        assert_eq!(c.regredirect.unwrap().suffix, "clientpatch");
        assert_eq!(c.titlebar.unwrap().template, " — clientpatch → {api_host}");
    }

    #[test]
    fn absent_new_tables_are_none() {
        let c = Config::parse("[loader]\nmodules=[]\n").unwrap();
        assert!(c.regredirect.is_none());
        assert!(c.titlebar.is_none());
    }

    #[test]
    fn empty_regredirect_suffix_is_rejected() {
        assert!(Config::parse("[regredirect]\nenable=true\nsuffix=\"\"\n").is_err());
    }

    #[test]
    fn backslash_regredirect_suffix_is_rejected() {
        assert!(Config::parse("[regredirect]\nenable=true\nsuffix=\"a\\\\b\"\n").is_err());
    }

    #[test]
    fn whitespace_is_trimmed_from_urls() {
        let s = "[lilypad]\nenable=true\napi_base=\" http://h/ \"\nplatform_base=\" http://h \"\n";
        let c = Config::parse(s).unwrap();
        let lp = c.lilypad.unwrap();
        assert_eq!(lp.api_base, "http://h/");
        assert_eq!(lp.platform_base, "http://h");
    }

    #[test]
    fn steam_defaults() {
        let c = Config::parse("[steam]\nenable=true\n").unwrap();
        let st = c.steam.unwrap();
        assert_eq!(st.mode, "auto");
        assert_eq!(st.ip_country, "JP");
        assert_eq!(st.ui_language, "japanese");
    }

    #[test]
    fn steam_modes_parse() {
        for m in ["off", "skip_restart", "stub", "auto"] {
            let s = format!("[steam]\nenable=true\nmode=\"{m}\"\n");
            assert_eq!(Config::parse(&s).unwrap().steam.unwrap().mode, m);
        }
    }

    #[test]
    fn steam_unknown_mode_rejected() {
        assert!(Config::parse("[steam]\nenable=true\nmode=\"maybe\"\n").is_err());
    }

    #[test]
    fn steam_absent_is_none() {
        let c = Config::parse("[loader]\nmodules=[]\n").unwrap();
        assert!(c.steam.is_none());
    }

    #[test]
    fn shipped_toml_enables_steam_auto() {
        let c = Config::parse(include_str!("../clientpatch.toml")).unwrap();
        assert!(c.module_enabled("steam"));
        let st = c.steam.expect("shipped toml has [steam]");
        assert_eq!(st.mode, "auto");
        assert_eq!(st.ip_country, "JP");
        assert_eq!(st.ui_language, "japanese");
    }

    #[test]
    fn interopdump_defaults() {
        let c = Config::parse("[interopdump]\nenable=true\n").unwrap();
        let d = c.interopdump.unwrap();
        assert_eq!(d.out_dir, "interop");
        assert!(!d.force);
    }

    #[test]
    fn interopdump_absent_is_none() {
        let c = Config::parse("[loader]\nmodules=[]\n").unwrap();
        assert!(c.interopdump.is_none());
    }

    #[test]
    fn interopdump_empty_out_dir_is_rejected() {
        assert!(Config::parse("[interopdump]\nenable=true\nout_dir=\"\"\n").is_err());
    }

    #[test]
    fn interopdump_parses() {
        let s = r#"
[loader]
modules = ["interopdump"]
[interopdump]
out_dir = "interop_cache"
force = true
"#;
        let c = Config::parse(s).unwrap();
        let d = c.interopdump.unwrap();
        assert_eq!(d.out_dir, "interop_cache");
        assert!(d.force);
    }

    #[test]
    fn bepinex_defaults() {
        let c = Config::parse("[bepinex]\n").unwrap();
        let b = c.bepinex.unwrap();
        assert!(!b.enable);
        assert_eq!(b.root, "BepInEx");
        assert_eq!(b.doorstop, "doorstop.dll");
        assert!(b.auto_download && b.auto_generate);
        assert_eq!(b.max_restarts, 2);
        // No manager configured by default => PowerShell interopgen fallback.
        assert!(b.manager.is_empty());
        assert_eq!(b.launch_method, "steam");
    }

    #[test]
    fn bepinex_manager_and_launch_method_parse() {
        let s = "[bepinex]\nmanager = \"cpm\\\\manager.exe\"\nlaunch_method = \"Direct\"\n";
        let b = Config::parse(s).unwrap().bepinex.unwrap();
        assert_eq!(b.manager, "cpm\\manager.exe");
        assert_eq!(b.launch_method, "direct"); // normalized to lowercase
    }

    #[test]
    fn bepinex_bad_launch_method_rejected() {
        assert!(Config::parse("[bepinex]\nlaunch_method = \"epic\"\n").is_err());
        // Empty falls back to the default rather than erroring.
        let b = Config::parse("[bepinex]\nlaunch_method = \"\"\n")
            .unwrap()
            .bepinex
            .unwrap();
        assert_eq!(b.launch_method, "steam");
    }

    #[test]
    fn bepinex_absent_is_none() {
        let c = Config::parse("[loader]\nmodules=[]\n").unwrap();
        assert!(c.bepinex.is_none());
    }

    #[test]
    fn bepinex_enabled_requires_url_when_auto_download() {
        let s = "[bepinex]\nenabled = true\nurl = \"\"\n";
        assert!(Config::parse(s).is_err());
        let s = "[bepinex]\nenabled = true\nauto_download = false\nurl = \"\"\n";
        assert!(Config::parse(s).is_ok());
    }

    #[test]
    fn bepinex_empty_fields_rejected() {
        assert!(Config::parse("[bepinex]\nroot = \"\"\n").is_err());
        assert!(Config::parse("[bepinex]\ndoorstop = \"\"\n").is_err());
        assert!(Config::parse("[bepinex]\ninteropgen = \"\"\n").is_err());
    }

    #[test]
    fn section_switches_override_legacy_flags() {
        let cfg = Config::parse(
            r#"
[loader]
modules = ["steam", "interopdump"]
[steam]
enable = false
[interopdump]
enable = false
[bepinex]
enable = true
enabled = false
[report]
enable = false
enabled = true
"#,
        )
        .unwrap();
        assert!(!cfg.module_enabled("steam"));
        assert!(!cfg.module_enabled("interopdump"));
        assert!(cfg.module_enabled("bepinex"));
        assert!(!cfg.lilypad.as_ref().unwrap().report.enable);
    }

    #[test]
    fn disabled_sections_need_no_active_settings_and_default_off() {
        let cfg =
            Config::parse("[lilypad]\nenable=false\n[steam]\n[interopdump]\n[bepinex]\n").unwrap();
        for name in ["lilypad", "steam", "interopdump", "bepinex"] {
            assert!(!cfg.module_enabled(name));
        }
    }

    #[test]
    fn switches_require_booleans() {
        for name in [
            "lilypad",
            "steam",
            "regredirect",
            "titlebar",
            "interopdump",
            "bepinex",
            "lilypad.report",
        ] {
            assert!(Config::parse(&format!("[{name}]\nenable='true'\n")).is_err());
        }
    }

    #[test]
    fn shipped_switches_keep_optional_modules_explicitly_off() {
        let cfg = Config::parse(include_str!("../clientpatch.toml")).unwrap();
        for name in ["lilypad", "steam", "regredirect", "titlebar"] {
            assert!(cfg.module_enabled(name));
        }
        assert!(!cfg.interopdump.unwrap().enable);
        assert!(!cfg.bepinex.unwrap().enable);
        assert!(cfg.lilypad.as_ref().unwrap().report.enable);
    }

    #[test]
    fn nested_report_takes_priority_over_legacy_report() {
        let cfg = Config::parse("[lilypad]\nenable=false\n[lilypad.report]\nenable=true\nurl='http://nested/'\n[report]\nenable=false\nurl='http://legacy/'\n").unwrap();
        let lp = cfg.lilypad.as_ref().unwrap();
        assert!(lp.report.enable);
        assert_eq!(lp.report.url, "http://nested/");
        assert!(!cfg.version_report_enabled());
    }

    #[test]
    fn report_requires_both_parent_and_child_switches() {
        for (parent, child, expected) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let cfg = Config::parse(&format!("[lilypad]\nenable={parent}\napi_base='http://api/'\nplatform_base='http://platform'\n[lilypad.report]\nenable={child}\n")).unwrap();
            assert_eq!(cfg.version_report_enabled(), expected);
        }
    }
}
