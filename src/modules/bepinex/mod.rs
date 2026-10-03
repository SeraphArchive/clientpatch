//! Module #4 — `bepinex`: clientpatch-driven BepInEx orchestration.
//!
//! Doorstop ships renamed (`doorstop.dll`) so it never autoloads; this module
//! is the only thing that loads it, and only when the setup is complete:
//!
//!   * `enable = false`                   → never loaded.
//!   * enabled, interop current           → `LoadLibrary(doorstop.dll)` at
//!     init_early (~30 ms) — same timing as a natural doorstop boot.
//!   * enabled, interop outdated/unknown  → doorstop deferred; at main-init the
//!     module drives a dump (or waits for the interopdump module's), then hands
//!     generation to the manager. The manager shows progress, restarts the game,
//!     and exits. No message box here.
//!   * enabled, payload missing           → hand the download to the manager the
//!     same way. The in-process curl/tar path remains only when no manager exe exists.
//!
//! "Outdated" is decided BEFORE IL2CPP is up by comparing the on-disk
//! GameAssembly.dll hash against `interop/latest.json` (written by interopdump
//! since 2026-09). A restart counter (`<root>/.clientpatch-restarts`) guards
//! against setup-restart loops; it is cleared on a Current boot.
use crate::config::Config;
use crate::module::{LoaderCtx, Module};
use std::path::Path;

#[derive(Default)]
pub struct BepInEx;

/// Cache maintenance is independent of module enablement and game currency.
#[cfg(windows)]
pub(crate) fn cleanup_on_startup(config: &Config, dll_dir: &Path) {
    worker::cleanup_on_startup(config, dll_dir);
}

impl Module for BepInEx {
    fn name(&self) -> &str {
        "bepinex"
    }

    #[cfg(windows)]
    fn init_early(&self, config: &Config, dll_dir: &Path) -> anyhow::Result<()> {
        worker::init_early(config, dll_dir)
    }

    #[cfg(windows)]
    fn init(&self, ctx: &LoaderCtx) -> anyhow::Result<()> {
        worker::init_late(ctx)
    }
}

// ---- pure status logic (unit-tested) ---------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteropStatus {
    Current,
    NeedsGenerate,
    NeedsDump,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestInfo {
    pub build_id: String,
    pub ga_file_hash: String,
}

/// Decide from the three inputs (latest.json, BepInEx marker, on-disk GA hash).
pub fn decide(
    latest: Option<&LatestInfo>,
    marker_build: Option<&str>,
    ga_file_hash: &str,
) -> InteropStatus {
    let Some(l) = latest else {
        return InteropStatus::NeedsDump;
    };
    // Old dumps predate the file-hash field: cannot prove currency -> redump.
    if l.ga_file_hash.is_empty() || l.ga_file_hash != ga_file_hash {
        return InteropStatus::NeedsDump;
    }
    if marker_build != Some(l.build_id.as_str()) {
        return InteropStatus::NeedsGenerate;
    }
    InteropStatus::Current
}

/// Parse `interop/latest.json` (tolerates the pre-fingerprint schema).
pub fn parse_latest(json: &str) -> Option<LatestInfo> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let build_id = v.get("build_id")?.as_str()?.to_string();
    let ga_file_hash = v
        .get("game_assembly_file_fingerprint")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(LatestInfo {
        build_id,
        ga_file_hash,
    })
}

/// Parse the BepInEx interop marker (`clientpatch-interop.json`) -> build_id.
pub fn parse_marker(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.get("build_id")?.as_str()?.to_string())
}

/// Marker filename inside `<root>\interop\`.
pub const MARKER_NAME: &str = "clientpatch-interop.json";

/// Preserve unrelated BepInEx settings while disabling its protected-input generator.
fn disable_builtin_generation(text: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut output = Vec::new();
    let mut in_il2cpp = false;
    let mut found_section = false;
    let mut found_key = false;
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_il2cpp && !found_key {
                output.push("UpdateInteropAssemblies = false".to_string());
            }
            in_il2cpp = trimmed == "[IL2CPP]";
            found_section |= in_il2cpp;
            found_key = false;
        }
        if in_il2cpp && trimmed.split_once('=').is_some_and(|(key, _)| key.trim() == "UpdateInteropAssemblies") {
            output.push("UpdateInteropAssemblies = false".to_string());
            found_key = true;
        } else {
            output.push(line.to_string());
        }
    }
    if !found_section {
        output.push("[IL2CPP]".to_string());
        output.push("UpdateInteropAssemblies = false".to_string());
    } else if in_il2cpp && !found_key {
        output.push("UpdateInteropAssemblies = false".to_string());
    }
    output.join(newline) + newline
}

/// What the manager should finish after clientpatch has closed the game.
/// clientpatch never shows a setup dialog; every unmet startup condition is one
/// of these jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupJob {
    /// Runtime dump is already on disk. Manager generates interop, then relaunches.
    Generate,
    /// BepInEx payload is missing. Manager downloads and extracts it, then relaunches.
    Payload,
    /// Files are already current. Manager only relaunches so plugins get a clean boot.
    Relaunch,
}

/// Arguments for the manager handoff. `None` when a generate job has no dump dir —
/// the manager cannot start that job, and the caller must not spawn it.
pub fn handoff_args(
    job: SetupJob,
    dump_dir: Option<&Path>,
    game_dir: &Path,
    launch_method: &str,
    payload_url: &str,
    doorstop: &str,
    bepinex_root: &str,
) -> Option<Vec<String>> {
    let mut args = Vec::new();
    match job {
        SetupJob::Generate => {
            let dump = dump_dir?;
            args.push("--generate-from-dump".to_string());
            args.push(dump.to_string_lossy().into_owned());
        }
        SetupJob::Payload => {
            args.push("--setup".to_string());
            args.push("payload".to_string());
            args.push("--payload-url".to_string());
            args.push(payload_url.to_string());
            args.push("--doorstop".to_string());
            args.push(doorstop.to_string());
        }
        SetupJob::Relaunch => {
            args.push("--setup".to_string());
            args.push("relaunch".to_string());
        }
    }
    args.push("--relaunch".to_string());
    args.push("--launch-method".to_string());
    args.push(launch_method.to_string());
    args.push("--game-dir".to_string());
    args.push(game_dir.to_string_lossy().into_owned());
    args.push("--bepinex-root".to_string());
    args.push(bepinex_root.to_string());
    Some(args)
}

#[cfg(windows)]
mod worker {
    use super::{
        decide, handoff_args, parse_latest, parse_marker, InteropStatus, LatestInfo, SetupJob,
        MARKER_NAME,
    };
    use crate::config::{Bepinex, Config};
    use crate::logging;
    use crate::module::LoaderCtx;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU8, Ordering};

    const ST_OK: u8 = 0;
    const ST_NEEDS_DUMP: u8 = 1;
    static STATE: AtomicU8 = AtomicU8::new(ST_OK);

    /// Cross-process setup mutex: two game processes must never run
    /// dump/generate/stage concurrently (one stages while the other's BepInEx
    /// resolves assemblies = chainloader FileNotFound, observed 2026-09-29).
    struct SetupGuard(windows_sys::Win32::Foundation::HANDLE);

    fn acquire_setup_guard() -> Option<SetupGuard> {
        acquire_setup_guard_wait(5 * 60 * 1000)
    }

    fn acquire_setup_guard_wait(timeout_ms: u32) -> Option<SetupGuard> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        const WAIT_OBJECT_0: u32 = 0;
        const WAIT_ABANDONED: u32 = 0x80;
        let name = crate::wide::to_wide_nul("Global\\clientpatch-bepinex-setup");
        let h = unsafe { CreateMutexW(core::ptr::null(), 0, name.as_ptr()) };
        if h.is_null() {
            return None;
        }
        let r = unsafe { WaitForSingleObject(h, timeout_ms) };
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            Some(SetupGuard(h))
        } else {
            unsafe {
                CloseHandle(h);
            }
            logging::line("WARN", "bepinex: setup mutex busy; setup deferred");
            None
        }
    }

    impl Drop for SetupGuard {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }

    pub(super) fn cleanup_on_startup(config: &Config, dll_dir: &Path) {
        let cfg = config.bepinex.clone().unwrap_or_default();
        let out_root = crate::modules::interopdump::resolve_out_dir(
            dll_dir,
            config
                .interopdump
                .as_ref()
                .map(|d| d.out_dir.as_str())
                .unwrap_or("interop"),
        );
        let interop = bepinex_interop_dir(dll_dir, &cfg);
        let data = crate::module::data_dir(dll_dir);
        if let Err(e) = std::thread::Builder::new()
            .name("interop-cleanup".into())
            .spawn(move || {
                // Share the native setup mutex and the manager's exclusive file lock:
                // generation/install must not lose a dump while it is consuming it.
                let Some(_guard) = acquire_setup_guard_wait(0) else {
                    return;
                };
                use std::os::windows::fs::OpenOptionsExt;
                let Ok(_manager_guard) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(data.join(".manager-operation.lock"))
                else {
                    return;
                };
                if crate::staging::pending(data.parent().unwrap_or(&data)) {
                    return;
                }
                match crate::modules::interopdump::cleanup::prune(&data, &out_root, &interop) {
                    Ok(0) => {}
                    Ok(n) => logging::line(
                        "INFO",
                        &format!("interopdump: removed {n} obsolete version entries"),
                    ),
                    Err(e) => logging::line(
                        "WARN",
                        &format!("interopdump: cache cleanup deferred: {e:#}"),
                    ),
                }
            })
        {
            logging::line("WARN", &format!("interopdump: cannot start cache cleanup: {e}"));
        }
    }

    pub fn init_early(config: &Config, dll_dir: &Path) -> anyhow::Result<()> {
        let Some(cfg) = config.bepinex.clone() else {
            return Ok(());
        };
        if !cfg.enable {
            logging::line(
                "INFO",
                "bepinex: present but not enabled (doorstop stays unloaded)",
            );
            return Ok(());
        }
        let manager_lock = crate::module::data_dir(dll_dir).join(".manager-operation.lock");
        if manager_lock.exists()
            && std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&manager_lock)
                .is_err()
        {
            logging::line(
                "WARN",
                "bepinex: manager setup is active; continuing without BepInEx",
            );
            return Ok(());
        }
        if crate::staging::pending(dll_dir) {
            let _guard = acquire_setup_guard_wait(0).ok_or_else(|| {
                anyhow::anyhow!("another native setup is active; BepInEx deferred")
            })?;
            crate::staging::recover_pending(dll_dir)?;
        }

        // 1. Payload present? The manager downloads and relaunches. The in-process
        // curl/tar path is only the fallback when no manager exe exists, and it
        // never raises a dialog.
        if !payload_present(dll_dir, &cfg) {
            if !cfg.auto_download {
                logging::line(
                    "ERR",
                    "bepinex: payload missing and auto_download=false; continuing without BepInEx",
                );
                return Ok(());
            }
            if manager_exe(dll_dir, &cfg).is_some() {
                handoff_to_manager(dll_dir, &cfg, config, SetupJob::Payload);
                return Ok(());
            }
            match download_payload(dll_dir, &cfg) {
                Ok(()) => {
                    logging::line("INFO", "bepinex: payload installed; restarting game");
                    restart(dll_dir, &cfg, "bepinex: payload installed; relaunching");
                    return Ok(()); // unreachable when restart proceeds
                }
                Err(e) => {
                    logging::line("ERR", &format!("bepinex: download failed: {e:#}"));
                    return Ok(());
                }
            }
        }

        ensure_bepinex_cfg(dll_dir, &cfg)?;

        // 2. Interop currency, decided pre-IL2CPP via the on-disk GA hash.
        let status = interop_status(dll_dir, &cfg, config);
        match status {
            InteropStatus::Current => {
                clear_restart_counter(dll_dir, &cfg);
                load_doorstop(dll_dir, &cfg);
                if !bepinex_interop_dir(dll_dir, &cfg)
                    .join("assembly-hash.txt")
                    .is_file()
                {
                    // Detached: PowerShell startup takes seconds and must never
                    // delay the doorstop race (observed: doorstop at +9.2s).
                    let (d, c) = (dll_dir.to_path_buf(), cfg.clone());
                    std::thread::spawn(move || write_assembly_hash(&d, &c));
                }
            }
            InteropStatus::NeedsGenerate => {
                if !cfg.auto_generate {
                    logging::line(
                        "WARN",
                        "bepinex: interop outdated and auto_generate=false; doorstop NOT loaded",
                    );
                    return Ok(());
                }
                let _guard = acquire_setup_guard()
                    .ok_or_else(|| anyhow::anyhow!("setup mutex unavailable"))?;
                // The other process may have finished setup while we waited.
                if interop_status(dll_dir, &cfg, config) == InteropStatus::Current {
                    clear_restart_counter(dll_dir, &cfg);
                    load_doorstop(dll_dir, &cfg);
                    return Ok(());
                }
                // Dump is current; only generation is missing. The manager generates,
                // shows progress, relaunches, and exits. The in-process script is
                // only the fallback when no manager exe exists.
                if manager_exe(dll_dir, &cfg).is_some() {
                    handoff_to_manager(dll_dir, &cfg, config, SetupJob::Generate);
                } else {
                    match generate_and_stage(dll_dir, &cfg, config) {
                        Ok(()) => {
                            logging::line("INFO", "bepinex: interop generated; restarting game");
                            restart(dll_dir, &cfg, "bepinex: interop generated; relaunching");
                        }
                        Err(e) => logging::line("ERR", &format!("bepinex: generate failed: {e:#}")),
                    }
                }
            }
            InteropStatus::NeedsDump => {
                STATE.store(ST_NEEDS_DUMP, Ordering::SeqCst);
                logging::line(
                    "INFO",
                    "bepinex: interop outdated/unknown; doorstop deferred until dump+generate",
                );
            }
        }
        Ok(())
    }

    pub fn init_late(ctx: &LoaderCtx) -> anyhow::Result<()> {
        if STATE.load(Ordering::SeqCst) != ST_NEEDS_DUMP {
            return Ok(());
        }
        let Some(cfg) = ctx.config.bepinex.clone() else {
            return Ok(());
        };
        let dll_dir = ctx.dll_dir;
        let _guard =
            acquire_setup_guard().ok_or_else(|| anyhow::anyhow!("setup mutex unavailable"))?;
        // The other process may have completed setup while we waited.
        if interop_status(dll_dir, &cfg, ctx.config) == InteropStatus::Current {
            clear_restart_counter(dll_dir, &cfg);
            load_doorstop(dll_dir, &cfg);
            return Ok(());
        }

        // Dump: drive it ourselves unless the interopdump module already did.
        if ctx.config.module_enabled("interopdump") {
            logging::line("INFO", "bepinex: waiting for interopdump module's dump");
            if !wait_for_dump(dll_dir, &cfg, ctx.config, 90) {
                logging::line("ERR", "bepinex: dump did not complete in time");
                return Ok(());
            }
        } else {
            logging::line("INFO", "bepinex: driving dump in-process");
            let out_dir = ctx
                .config
                .interopdump
                .as_ref()
                .map(|d| d.out_dir.as_str())
                .unwrap_or("interop");
            let out_root = crate::modules::interopdump::resolve_out_dir(dll_dir, out_dir);
            crate::modules::interopdump::dump_now(out_root, dll_dir.to_path_buf(), false, false);
        }

        // Re-evaluate after the dump: a cached dump may have proven currency
        // (e.g. legacy latest.json gaining the file hash) with nothing left to
        // generate — then a clean-boot restart is all that is needed.
        match interop_status(dll_dir, &cfg, ctx.config) {
            InteropStatus::Current => {
                logging::line(
                    "INFO",
                    "bepinex: dump proved currency; handing relaunch to the manager",
                );
                if manager_exe(dll_dir, &cfg).is_some() {
                    handoff_to_manager(dll_dir, &cfg, ctx.config, SetupJob::Relaunch);
                } else {
                    restart(dll_dir, &cfg, "bepinex: dump proved currency; relaunching");
                }
            }
            InteropStatus::NeedsGenerate => {
                if !cfg.auto_generate {
                    logging::line("WARN", "bepinex: auto_generate=false; doorstop NOT loaded");
                    return Ok(());
                }
                if manager_exe(dll_dir, &cfg).is_some() {
                    handoff_to_manager(dll_dir, &cfg, ctx.config, SetupJob::Generate);
                } else {
                    match generate_and_stage(dll_dir, &cfg, ctx.config) {
                        Ok(()) => {
                            logging::line("INFO", "bepinex: setup complete; restarting game");
                            restart(dll_dir, &cfg, "bepinex: interop generated; relaunching");
                        }
                        Err(e) => logging::line("ERR", &format!("bepinex: generate failed: {e:#}")),
                    }
                }
            }
            InteropStatus::NeedsDump => {
                logging::line("ERR", "bepinex: dump failed (see interopdump lines above)");
            }
        }
        Ok(())
    }

    // ---- status ------------------------------------------------------------

    fn interop_status(dll_dir: &Path, cfg: &Bepinex, config: &Config) -> InteropStatus {
        let out_dir = config
            .interopdump
            .as_ref()
            .map(|d| d.out_dir.as_str())
            .unwrap_or("interop");
        let out_root = crate::modules::interopdump::resolve_out_dir(dll_dir, out_dir);
        let latest = std::fs::read_to_string(out_root.join("latest.json"))
            .ok()
            .and_then(|s| parse_latest(&s));
        let marker = std::fs::read_to_string(bepinex_interop_dir(dll_dir, cfg).join(MARKER_NAME))
            .ok()
            .and_then(|s| parse_marker(&s));
        let ga_hash =
            crate::modules::interopdump::ga_fingerprint(&dll_dir.join("GameAssembly.dll"))
                .unwrap_or_default();
        let status = decide(latest.as_ref(), marker.as_deref(), &ga_hash);
        if status == InteropStatus::Current
            && ["Assembly-CSharp.dll", "UnityEngine.CoreModule.dll"]
                .iter()
                .any(|name| !bepinex_interop_dir(dll_dir, cfg).join(name).is_file())
        {
            InteropStatus::NeedsGenerate
        } else {
            status
        }
    }

    fn bepinex_interop_dir(dll_dir: &Path, cfg: &Bepinex) -> PathBuf {
        dll_dir.join(&cfg.root).join("interop")
    }

    fn payload_present(dll_dir: &Path, cfg: &Bepinex) -> bool {
        dll_dir.join(&cfg.doorstop).is_file()
            && dll_dir
                .join(&cfg.root)
                .join("core")
                .join("BepInEx.Unity.IL2CPP.dll")
                .is_file()
    }

    fn wait_for_dump(dll_dir: &Path, cfg: &Bepinex, config: &Config, timeout_s: u32) -> bool {
        for _ in 0..(timeout_s * 2) {
            if interop_status(dll_dir, cfg, config) != InteropStatus::NeedsDump {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        false
    }

    // ---- doorstop ----------------------------------------------------------

    fn load_doorstop(dll_dir: &Path, cfg: &Bepinex) {
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
        let path = dll_dir.join(&cfg.doorstop);
        let wide = crate::wide::to_wide_nul(&path.to_string_lossy());
        let h = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h.is_null() {
            logging::line(
                "ERR",
                &format!(
                    "bepinex: LoadLibrary({}) failed ({})",
                    path.display(),
                    std::io::Error::last_os_error()
                ),
            );
        } else {
            logging::line(
                "INFO",
                &format!("bepinex: doorstop loaded ({})", path.display()),
            );
        }
    }

    // ---- generation --------------------------------------------------------

    fn generate_and_stage(dll_dir: &Path, cfg: &Bepinex, config: &Config) -> anyhow::Result<()> {
        let script = dll_dir.join(&cfg.interopgen);
        if !script.is_file() {
            anyhow::bail!("interopgen wrapper not found at {}", script.display());
        }
        logging::line("INFO", "bepinex: generating interop assemblies (~1 min)...");
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &script.to_string_lossy(),
                "-GameDir",
                &dll_dir.to_string_lossy(),
                "-DumpRoot",
                &crate::modules::interopdump::resolve_out_dir(
                    dll_dir,
                    config
                        .interopdump
                        .as_ref()
                        .map(|d| d.out_dir.as_str())
                        .unwrap_or("interop"),
                )
                .to_string_lossy(),
            ])
            .status()?;
        if !status.success() {
            anyhow::bail!("interopgen exited with {status}");
        }

        // Stage into <root>\interop + write the currency marker.
        let out_dir = config
            .interopdump
            .as_ref()
            .map(|d| d.out_dir.as_str())
            .unwrap_or("interop");
        let out_root = crate::modules::interopdump::resolve_out_dir(dll_dir, out_dir);
        let latest: LatestInfo =
            parse_latest(&std::fs::read_to_string(out_root.join("latest.json"))?)
                .ok_or_else(|| anyhow::anyhow!("latest.json missing after generate"))?;
        let gen_dir = out_root.join(&latest.build_id).join("interop");
        let dst = bepinex_interop_dir(dll_dir, cfg);
        std::fs::create_dir_all(&dst)?;
        for name in ["Assembly-CSharp.dll", "UnityEngine.CoreModule.dll"] {
            if !gen_dir.join(name).is_file() {
                anyhow::bail!("generator did not produce {name}");
            }
        }
        let mut files = Vec::new();
        let mut names = std::collections::HashSet::new();
        for entry in std::fs::read_dir(&gen_dir)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|e| e == "dll" || e == "db")
            {
                names.insert(entry.file_name());
                files.push((dst.join(entry.file_name()), Some(entry.path())));
            }
        }
        for entry in std::fs::read_dir(&dst)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|e| e == "dll" || e == "db")
                && !names.contains(&entry.file_name())
            {
                files.push((entry.path(), None));
            }
        }
        let marker = gen_dir.join(MARKER_NAME);
        std::fs::write(
            &marker,
            serde_json::to_vec_pretty(&serde_json::json!({ "build_id": latest.build_id }))?,
        )?;
        let gate = dst.join(MARKER_NAME);
        files.push((gate.clone(), Some(marker)));
        crate::staging::commit(dll_dir, &files, Some(&gate))?;
        logging::line(
            "INFO",
            &format!(
                "bepinex: staged {} files into {}",
                names.len(),
                dst.display()
            ),
        );
        write_assembly_hash(dll_dir, cfg);
        Ok(())
    }

    /// Write `<root>\interop\assembly-hash.txt` — BepInEx's own staleness
    /// marker: MD5(GameAssembly.dll bytes ++ utf8(Il2CppInterop.Generator
    /// version) ++ utf8(Cpp2IL.Core version)), lowercase hex. Exact assembly
    /// versions need .NET reflection, so shell out (PowerShell is already a
    /// dependency of the generation step). Cosmetic; failure is non-fatal.
    fn write_assembly_hash(dll_dir: &Path, cfg: &Bepinex) {
        // NOTE: paths are interpolated — `powershell -Command` does not bind
        // trailing arguments to $args the way -File does.
        fn psq(s: &str) -> String {
            format!("'{}'", s.replace('\'', "''"))
        }
        let core = dll_dir.join(&cfg.root).join("core");
        let script = format!(
            "$md5=[System.Security.Cryptography.MD5]::Create();\
             $b=[IO.File]::ReadAllBytes({ga});\
             $g=[Reflection.AssemblyName]::GetAssemblyName({gen}).Version.ToString();\
             $c=[Reflection.AssemblyName]::GetAssemblyName({cpp}).Version.ToString();\
             $t=[Text.Encoding]::UTF8.GetBytes($g)+[Text.Encoding]::UTF8.GetBytes($c);\
             ($md5.ComputeHash($b+$t)|ForEach-Object{{$_.ToString('x2')}}) -join '' |\
             Out-File -NoNewline {out}",
            ga = psq(&dll_dir.join("GameAssembly.dll").to_string_lossy()),
            gen = psq(&core.join("Il2CppInterop.Generator.dll").to_string_lossy()),
            cpp = psq(&core.join("Cpp2IL.Core.dll").to_string_lossy()),
            out = psq(&bepinex_interop_dir(dll_dir, cfg)
                .join("assembly-hash.txt")
                .to_string_lossy()),
        );
        let r = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status();
        match r {
            Ok(s) if s.success() => logging::line("DBG", "bepinex: assembly-hash.txt written"),
            _ => logging::line("WARN", "bepinex: assembly-hash.txt write failed (cosmetic)"),
        }
    }

    // ---- config / hash / download ------------------------------------------

    pub(super) fn ensure_bepinex_cfg(dll_dir: &Path, cfg: &Bepinex) -> anyhow::Result<()> {
        let cfg_dir = dll_dir.join(&cfg.root).join("config");
        let path = cfg_dir.join("BepInEx.cfg");
        crate::staging::checked_path(dll_dir, &path)?;
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let text = super::disable_builtin_generation(&original);
        if text != original {
            let _guard = acquire_setup_guard_wait(0)
                .ok_or_else(|| anyhow::anyhow!("native setup is active; config update deferred"))?;
            let data = crate::module::data_dir(dll_dir);
            let lock_path = data.join(".manager-operation.lock");
            crate::staging::checked_path(dll_dir, &lock_path)?;
            use std::os::windows::fs::OpenOptionsExt;
            let _manager_guard = std::fs::OpenOptions::new().read(true).write(true)
                .create(true).truncate(false).share_mode(0).open(lock_path)?;
            // Preserve edits completed before we obtained the installation lock.
            crate::staging::checked_path(dll_dir, &path)?;
            let latest = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.into()),
            };
            let text = super::disable_builtin_generation(&latest);
            if text == latest {
                return Ok(());
            }
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
            let temporary = data.join(format!(".bepinex-config-{}-{nonce}", std::process::id()));
            crate::staging::checked_path(dll_dir, &temporary)?;
            let mut staged = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
            use std::io::Write;
            let result = (|| -> anyhow::Result<()> {
                staged.write_all(text.as_bytes())?;
                staged.sync_all()?;
                crate::staging::commit(dll_dir, &[(path, Some(temporary.clone()))], None)
            })();
            drop(staged);
            let _ = std::fs::remove_file(&temporary);
            result?;
        }
        Ok(())
    }

    fn download_payload(dll_dir: &Path, cfg: &Bepinex) -> anyhow::Result<()> {
        let _guard =
            acquire_setup_guard().ok_or_else(|| anyhow::anyhow!("setup mutex unavailable"))?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let stage = crate::module::data_dir(dll_dir)
            .join(format!(".native-payload-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&stage)?;
        let zip = stage.join("payload.zip");
        let extracted = stage.join("files");
        std::fs::create_dir_all(&extracted)?;
        logging::line("INFO", &format!("bepinex: downloading {} ...", cfg.url));
        run_tool(
            "curl.exe",
            &["-sSL", "--fail", "-o", &zip.to_string_lossy(), &cfg.url],
        )?;
        logging::line("INFO", "bepinex: extracting payload");
        run_tool(
            "tar.exe",
            &[
                "-xf",
                &zip.to_string_lossy(),
                "-C",
                &extracted.to_string_lossy(),
            ],
        )?;
        let _ = std::fs::remove_file(&zip);
        // The zip ships winhttp.dll (autoload hijack); rename so ONLY we load it.
        let winhttp = extracted.join("winhttp.dll");
        if winhttp.is_file() {
            if let Some(parent) = extracted.join(&cfg.doorstop).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&winhttp, extracted.join(&cfg.doorstop))?;
        }
        if cfg.root != "BepInEx" {
            let custom = extracted.join(&cfg.root);
            if let Some(parent) = custom.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(extracted.join("BepInEx"), &custom)?;
            let ini = extracted.join("doorstop_config.ini");
            if ini.is_file() {
                std::fs::write(
                    &ini,
                    std::fs::read_to_string(&ini)?
                        .replace("BepInEx/", &format!("{}/", cfg.root.replace('\\', "/")))
                        .replace("BepInEx\\", &format!("{}\\", cfg.root)),
                )?;
            }
        }
        if !payload_present(&extracted, cfg) {
            anyhow::bail!("payload incomplete after extract");
        }
        let mut files = Vec::new();
        fn collect(
            base: &Path,
            dir: &Path,
            game: &Path,
            files: &mut Vec<(PathBuf, Option<PathBuf>)>,
        ) -> anyhow::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    collect(base, &entry.path(), game, files)?;
                } else {
                    files.push((
                        game.join(entry.path().strip_prefix(base)?),
                        Some(entry.path()),
                    ));
                }
            }
            Ok(())
        }
        collect(&extracted, &extracted, dll_dir, &mut files)?;
        files.push((dll_dir.join("winhttp.dll"), None));
        crate::staging::commit(dll_dir, &files, None)?;
        let _ = std::fs::remove_dir_all(&stage);
        Ok(())
    }

    fn run_tool(exe: &str, args: &[&str]) -> anyhow::Result<()> {
        let status = Command::new(exe).args(args).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => anyhow::bail!("{exe} exited with {s}"),
            Err(e) => anyhow::bail!("{exe} spawn failed: {e} (needs Windows 10+ system {exe})"),
        }
    }

    // ---- restart -----------------------------------------------------------

    /// Resolve the manager exe. A configured path wins (absolute, or relative to the
    /// game dir). When unset, `manager.exe` beside the game is used if it is there.
    /// None means the in-process fallback, which does not raise a dialog.
    fn manager_exe(dll_dir: &Path, cfg: &Bepinex) -> Option<PathBuf> {
        let m = cfg.manager.trim();
        if !m.is_empty() {
            let p = Path::new(m);
            return Some(if p.is_absolute() {
                p.to_path_buf()
            } else {
                dll_dir.join(p)
            });
        }
        let beside = dll_dir.join("manager.exe");
        beside.is_file().then_some(beside)
    }

    /// The runtime dump dir the manager should generate from: `<out_root>/<build_id>/`,
    /// where interopdump wrote the decrypted GameAssembly.dll + global-metadata.dat.
    fn runtime_dump_dir(dll_dir: &Path, config: &Config) -> Option<PathBuf> {
        let out_dir = config
            .interopdump
            .as_ref()
            .map(|d| d.out_dir.as_str())
            .unwrap_or("interop");
        let out_root = crate::modules::interopdump::resolve_out_dir(dll_dir, out_dir);
        let latest = parse_latest(&std::fs::read_to_string(out_root.join("latest.json")).ok()?)?;
        Some(out_root.join(&latest.build_id))
    }

    /// Hand an unmet startup condition to the manager GUI, then HARD-terminate this
    /// game process. The manager shows its own progress window, finishes the job,
    /// relaunches the game, and exits. No dialog is raised here. The terminate is
    /// TerminateProcess (never ExitProcess): the latter runs DLL detach while game
    /// threads are inside our detours and AVs inside version.dll.
    fn handoff_to_manager(dll_dir: &Path, cfg: &Bepinex, config: &Config, job: SetupJob) {
        let Some(exe) = manager_exe(dll_dir, cfg) else {
            logging::line(
                "ERR",
                "bepinex: manager handoff requested but no manager exe",
            );
            return;
        };
        let dump_dir = runtime_dump_dir(dll_dir, config);
        let args = match handoff_args(
            job,
            dump_dir.as_deref(),
            dll_dir,
            &cfg.launch_method,
            &cfg.url,
            &cfg.doorstop,
            &cfg.root,
        ) {
            Some(a) => a,
            None => {
                logging::line(
                    "ERR",
                    "bepinex: manager handoff needs latest.json (dump missing)",
                );
                return;
            }
        };

        let counter = restart_counter_path(dll_dir, cfg);
        let n: u32 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if n >= cfg.max_restarts {
            logging::line(
                "ERR",
                &format!(
                    "bepinex: setup needed >{} restarts; giving up (doorstop NOT loaded)",
                    cfg.max_restarts
                ),
            );
            return;
        }
        let _ = std::fs::create_dir_all(dll_dir.join(&cfg.root));

        logging::line(
            "INFO",
            &format!(
                "bepinex: handing off {job:?} to manager {} ({})",
                exe.display(),
                args.join(" ")
            ),
        );

        let spawn = Command::new(exe)
            .args(&args)
            .args(["--parent-pid", &std::process::id().to_string()])
            .spawn();
        match spawn {
            Ok(_) => {
                // Count only a restart that actually started. A failed spawn must
                // not burn one of the few retries the loop guard allows.
                let _ = std::fs::write(&counter, (n + 1).to_string());
                logging::line("INFO", "bepinex: manager spawned; terminating this process");
                unsafe {
                    let h = windows_sys::Win32::System::Threading::GetCurrentProcess();
                    windows_sys::Win32::System::Threading::TerminateProcess(h, 0);
                }
            }
            Err(e) => logging::line("ERR", &format!("bepinex: manager spawn failed: {e}")),
        }
    }

    fn restart_counter_path(dll_dir: &Path, cfg: &Bepinex) -> PathBuf {
        dll_dir.join(&cfg.root).join(".clientpatch-restarts")
    }

    fn clear_restart_counter(dll_dir: &Path, cfg: &Bepinex) {
        let _ = std::fs::remove_file(restart_counter_path(dll_dir, cfg));
    }

    /// Spawn a fresh copy of the game and HARD-kill this one. The kill must be
    /// TerminateProcess: ExitProcess runs DLL detach/atexit while game threads
    /// are executing inside our detours, which AVs inside version.dll (and our
    /// own crashguard then shows a bogus crash window).
    fn restart(dll_dir: &Path, cfg: &Bepinex, reason: &str) {
        let counter = restart_counter_path(dll_dir, cfg);
        let n: u32 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if n >= cfg.max_restarts {
            logging::line(
                "ERR",
                &format!(
                    "bepinex: setup needed >{} restarts; giving up (doorstop NOT loaded)",
                    cfg.max_restarts
                ),
            );
            return;
        }
        let _ = std::fs::create_dir_all(dll_dir.join(&cfg.root));
        let exe = std::env::current_exe();
        let Ok(exe) = exe else {
            logging::line("ERR", "bepinex: cannot resolve own exe for restart");
            return;
        };
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        logging::line("INFO", reason);
        match Command::new(&exe).args(&args).spawn() {
            Ok(_) => {
                let _ = std::fs::write(&counter, (n + 1).to_string());
                logging::line("INFO", "bepinex: relaunched; terminating this process");
                unsafe {
                    let h = windows_sys::Win32::System::Threading::GetCurrentProcess();
                    windows_sys::Win32::System::Threading::TerminateProcess(h, 0);
                }
            }
            Err(e) => logging::line("ERR", &format!("bepinex: restart spawn failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "aaaa";
    #[cfg(windows)]
    #[test]
    fn config_updates_are_atomic_and_reject_linked_destinations() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = std::env::temp_dir().join(format!("cpm-bepinex-config-{}", std::process::id()));
        let cfg = crate::config::Bepinex::default();
        let config_dir = root.join(&cfg.root).join("config");
        let path = config_dir.join("BepInEx.cfg");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(crate::module::data_dir(&root)).unwrap();
        let original = "[IL2CPP]\nUpdateInteropAssemblies = true\nOther = keep\n";
        std::fs::write(&path, original).unwrap();
        worker::ensure_bepinex_cfg(&root, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), disable_builtin_generation(original));
        std::fs::write(&path, original).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
        assert!(worker::ensure_bepinex_cfg(&root, &cfg).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        drop(held);
        crate::staging::recover_pending(&root).unwrap();
        let outside = root.join("outside");
        std::fs::rename(&config_dir, &outside).unwrap();
        assert!(std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
            .arg(&config_dir).arg(&outside).output().unwrap().status.success());
        assert!(worker::ensure_bepinex_cfg(&root, &cfg).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("BepInEx.cfg")).unwrap(), original);
        std::fs::remove_dir(config_dir).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn disables_existing_generator_setting_and_preserves_other_sections() {
        let original = "[IL2CPP]\r\nUpdateInteropAssemblies = true\r\nOther = keep\r\n[Other]\r\nUpdateInteropAssemblies = true\r\n";
        let fixed = disable_builtin_generation(original);
        assert_eq!(fixed, original.replacen("UpdateInteropAssemblies = true", "UpdateInteropAssemblies = false", 1));
        assert_eq!(disable_builtin_generation(&fixed), fixed);
    }

    #[test]
    fn adds_missing_generator_setting_inside_its_section() {
        assert_eq!(disable_builtin_generation("[IL2CPP]\nOther = keep\n[Other]\nEnabled = true\n"),
            "[IL2CPP]\nOther = keep\nUpdateInteropAssemblies = false\n[Other]\nEnabled = true\n");
        assert_eq!(disable_builtin_generation("[Logging]\nEnabled = true\n"),
            "[Logging]\nEnabled = true\n[IL2CPP]\nUpdateInteropAssemblies = false\n");
    }
    fn latest(build: &str, hash: &str) -> LatestInfo {
        LatestInfo {
            build_id: build.into(),
            ga_file_hash: hash.into(),
        }
    }

    #[test]
    fn no_latest_means_needs_dump() {
        assert_eq!(decide(None, None, HASH), InteropStatus::NeedsDump);
        assert_eq!(decide(None, Some("b1"), HASH), InteropStatus::NeedsDump);
    }

    #[test]
    fn hash_mismatch_means_game_updated() {
        let l = latest("b1", "oldhash");
        assert_eq!(
            decide(Some(&l), Some("b1"), "newhash"),
            InteropStatus::NeedsDump
        );
    }

    #[test]
    fn missing_file_hash_means_legacy_dump_redump() {
        let l = latest("b1", "");
        assert_eq!(decide(Some(&l), Some("b1"), HASH), InteropStatus::NeedsDump);
    }

    #[test]
    fn marker_mismatch_means_needs_generate() {
        let l = latest("b1", HASH);
        assert_eq!(decide(Some(&l), None, HASH), InteropStatus::NeedsGenerate);
        assert_eq!(
            decide(Some(&l), Some("older"), HASH),
            InteropStatus::NeedsGenerate
        );
    }

    #[test]
    fn matching_everything_is_current() {
        let l = latest("b1", HASH);
        assert_eq!(decide(Some(&l), Some("b1"), HASH), InteropStatus::Current);
    }

    #[test]
    fn parses_latest_with_and_without_hash() {
        let l = parse_latest(r#"{"build_id":"b1","dir":"b1","metadata_version":31,"game_assembly_file_fingerprint":"ff"}"#).unwrap();
        assert_eq!(l, latest("b1", "ff"));
        let l = parse_latest(r#"{"build_id":"b1","dir":"b1","metadata_version":31}"#).unwrap();
        assert_eq!(l, latest("b1", ""));
        assert!(parse_latest("not json").is_none());
        assert!(parse_latest(r#"{"dir":"b1"}"#).is_none());
    }

    #[test]
    fn parses_marker() {
        assert_eq!(parse_marker(r#"{"build_id":"b1"}"#).as_deref(), Some("b1"));
        assert!(parse_marker("{}").is_none());
    }

    #[test]
    fn generate_handoff_names_the_dump_and_the_game() {
        let args = handoff_args(
            SetupJob::Generate,
            Some(Path::new(r"D:\game\interop\abc")),
            Path::new(r"D:\game"),
            "steam",
            "",
            "doorstop.dll",
            "BepInEx",
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "--generate-from-dump",
                r"D:\game\interop\abc",
                "--relaunch",
                "--launch-method",
                "steam",
                "--game-dir",
                r"D:\game",
                "--bepinex-root",
                "BepInEx",
            ]
        );
    }

    #[test]
    fn generate_handoff_refuses_without_a_dump() {
        assert!(handoff_args(
            SetupJob::Generate,
            None,
            Path::new(r"D:\game"),
            "steam",
            "",
            "doorstop.dll",
            "BepInEx",
        )
        .is_none());
    }

    #[test]
    fn payload_and_relaunch_handoffs_carry_their_job() {
        let payload = handoff_args(
            SetupJob::Payload,
            None,
            Path::new(r"D:\game"),
            "direct",
            "https://example/bepinex.zip",
            "doorstop.dll",
            "BepInEx",
        )
        .unwrap();
        assert_eq!(payload[0], "--setup");
        assert_eq!(payload[1], "payload");
        assert!(payload
            .windows(2)
            .any(|w| w == ["--payload-url", "https://example/bepinex.zip"]));
        assert!(payload
            .windows(2)
            .any(|w| w == ["--launch-method", "direct"]));

        let relaunch = handoff_args(
            SetupJob::Relaunch,
            None,
            Path::new(r"D:\game"),
            "steam",
            "",
            "doorstop.dll",
            "BepInEx",
        )
        .unwrap();
        assert_eq!(&relaunch[..2], ["--setup", "relaunch"]);
        assert!(!relaunch.iter().any(|a| a == "--generate-from-dump"));
    }
}
