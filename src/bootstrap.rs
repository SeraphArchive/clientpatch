//! Init thread: load config -> bind IL2CPP -> attach -> wait for HTTPComm image
//! -> run the enabled modules. Never blocks the loader lock (runs on its own thread).
//!
//! `version.dll` sits in the game dir, but every file this loader reads or writes
//! (config, per-launch logs, langpacks, dumps) lives in `<game dir>/clientpatch`.
use crate::config::Config;
use crate::il2cpp::Il2Cpp;
use crate::logging;
use crate::module::{self, LoaderCtx};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Threading::Sleep;

pub unsafe extern "system" fn init_thread(param: *mut core::ffi::c_void) -> u32 {
    // A panic here unwinds off a raw Win32 thread, which is undefined and skips
    // the crash dialog. Fence it; the body stays exactly init_thread_inner.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| init_thread_inner(param))) {
        Ok(rc) => rc,
        Err(_) => {
            logging::line_try("ERR", "init thread panicked; modules not loaded");
            0
        }
    }
}

unsafe fn init_thread_inner(_param: *mut core::ffi::c_void) -> u32 {
    let dir = dll_dir().unwrap_or_else(|| PathBuf::from("."));
    // Everything clientpatch owns lives under <game dir>/clientpatch. The game
    // dir itself holds only version.dll, BepInEx/, and the manager exe.
    let data = crate::module::data_dir(&dir);

    // Always-on debug aids (no config): a console window mirroring the log, and a
    // crash guard that shows a crash window + blocks UnityCrashHandler64.
    logging::enable_console();

    // Config first, so its `log` key can pick the log filename.
    let cfg = match Config::load(&crate::module::resolve_data_path(&dir, Path::new("clientpatch.toml"))) {
        Ok(c) => c,
        Err(e) => {
            // No/invalid config: still init a log and load nothing.
            logging::init(&data, "clientpatch.log");
            crate::crashguard::install();
            logging::line(
                "WARN",
                &format!("config not loaded ({e}); loading no modules"),
            );
            return 0;
        }
    };
    // `loader.log` is the per-launch log file's stem (the filename part only;
    // any directory in the value is ignored). Every launch writes a fresh
    // `<stem>_<timestamp>.log` into `<game dir>/clientpatch/logs/`.
    let configured = Path::new(&cfg.loader.log);
    let log_name = configured
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    logging::init(&data, &log_name);
    crate::crashguard::install();
    logging::line("INFO", "clientpatch starting");

    // Early pass: modules that must hook native APIs before the game first uses
    // them (e.g. regredirect must beat Unity's first PlayerPrefs access). These
    // need no IL2CPP, so run them now — long before the runtime-ready gate.
    //
    // bepinex goes FIRST even though it is last in the registry: doorstop must
    // be loaded before UnityPlayer resolves il2cpp_init (doorstop IAT-hooks
    // that GetProcAddress call; miss the window and BepInEx never boots), so
    // its early pass cannot wait behind the other modules' hook installs.
    let modules = module::registry();
    for m in &modules {
        if m.name() == "bepinex" && cfg.module_enabled(m.name()) {
            if let Err(e) = m.init_early(&cfg, &dir) {
                logging::line("ERR", &format!("module '{}' early init failed: {e}", m.name()));
            }
        }
    }
    for m in &modules {
        if m.name() != "bepinex" && cfg.module_enabled(m.name()) {
            if let Err(e) = m.init_early(&cfg, &dir) {
                logging::line("ERR", &format!("module '{}' early init failed: {e}", m.name()));
            }
        }
    }

    // Leak the runtime binding: module hooks dereference it for the whole
    // process, long after this thread returns.
    let il2cpp: &'static Il2Cpp = match Il2Cpp::load() {
        Some(x) => Box::leak(Box::new(x)),
        None => {
            logging::line("ERR", "IL2CPP bind failed; aborting");
            return 0;
        }
    };

    logging::line(
        "DBG",
        &format!("il2cpp exports bound (pid={})", std::process::id()),
    );

    // The IL2CPP runtime initializes on the main thread via il2cpp_init, which
    // registers EVERY assembly. Calling any domain/assembly enumeration API
    // before that finishes walks a half-built list and crashes (AV in
    // il2cpp_assembly_get_image / reading the assembly array; see Player.log).
    // `il2cpp_get_corlib` goes non-null PARTWAY through init (right after
    // mscorlib loads) — necessary but not sufficient. There is no exported
    // "init complete" flag. A fixed 8s settle was both too slow (missed
    // InitializeLanguage / PAYMENT_Initialize) and sometimes too short.
    let mut corlib_ms = 0u32;
    for i in 0..1200u32 {
        if il2cpp.corlib_ready() {
            corlib_ms = i * 100;
            break;
        }
        Sleep(100);
    }
    if !il2cpp.corlib_ready() {
        logging::line("ERR", "il2cpp core (corlib) not ready after ~120s; aborting");
        return 0;
    }
    logging::line(
        "DBG",
        &format!("corlib ready (~{corlib_ms}ms); waiting for assembly list to plateau"),
    );

    // `il2cpp_get_corlib` goes non-null PARTWAY through init. Walking the
    // assembly array before il2cpp_init finishes AVs. A fixed 8s sleep was both
    // too slow (InitializeLanguage / GGLInitialize often run during it) and
    // sometimes too short (slow disks). Wait until the assembly count stops
    // growing — reading `n` only, never walking the array.
    let mut last = 0usize;
    let mut stable_ticks = 0u32;
    let mut waited_ms = 0u32;
    let mut plateaued = false;
    loop {
        let n = il2cpp.assembly_count();
        // Don't treat a mid-init stall at n<50 as "done" — walking then AVs.
        // Historical full set is 184; 50 is past corlib+Unity and before most game images.
        if n >= 50 && n == last {
            stable_ticks += 1;
            // 20 × 50ms = 1s unchanged.
            if stable_ticks >= 20 {
                plateaued = true;
                break;
            }
        } else {
            stable_ticks = 0;
            last = n;
        }
        if waited_ms >= 15_000 {
            logging::line(
                "WARN",
                &format!("assembly count still changing after 15s (n={n}); not enumerating"),
            );
            break;
        }
        Sleep(50);
        waited_ms += 50;
    }
    if !plateaued {
        crate::status::mark_failed("IL2CPP assembly list did not plateau; IL2CPP hooks skipped");
        logging::line(
            "ERR",
            "assembly list did not plateau; skipping IL2CPP modules (native hooks remain)",
        );
        crate::crashguard::reassert();
        crate::modules::titlebar::start_claim_loop();
        crate::modules::titlebar::refresh();
        return 0;
    }
    logging::line(
        "DBG",
        &format!("assembly plateau n={last} after +{waited_ms}ms; attaching thread"),
    );

    il2cpp.thread_attach();
    logging::line("DBG", "thread attached; scanning for Domain / HTTPComm images");

    // Language hooks must install before InitializeFirst calls InitializeLanguage
    // (isInit then blocks any later append). Domain.dll is the only image those
    // hooks need; don't wait for HTTPComm first.
    let mut domain_ready = false;
    for _ in 0..600 {
        if !il2cpp.find_image("Domain").is_null() {
            domain_ready = true;
            break;
        }
        Sleep(50);
    }
    if !domain_ready {
        logging::line("WARN", "Domain image not registered after ~30s; language early-init skipped");
    }

    let ctx = LoaderCtx {
        il2cpp,
        config: &cfg,
        dll_dir: &dir,
    };
    if domain_ready {
        for m in module::registry() {
            if cfg.module_enabled(m.name()) {
                if let Err(e) = m.init_il2cpp_early(&ctx) {
                    logging::line(
                        "ERR",
                        &format!("module '{}' il2cpp-early init failed: {e}", m.name()),
                    );
                }
            }
        }
    }

    let mut ready = false;
    for _ in 0..600 {
        if !il2cpp.find_image("HTTPComm").is_null() {
            ready = true;
            break;
        }
        Sleep(50);
    }
    if !ready {
        logging::line("ERR", "HTTPComm image not registered after ~30s; aborting");
        return 0;
    }
    logging::line("INFO", "IL2CPP ready; HTTPComm registered; thread attached");

    for m in module::registry() {
        if cfg.module_enabled(m.name()) {
            match m.init(&ctx) {
                Ok(()) => logging::line("INFO", &format!("module '{}' initialized", m.name())),
                Err(e) => logging::line("ERR", &format!("module '{}' failed: {e}", m.name())),
            }
        } else {
            logging::line(
                "INFO",
                &format!("module '{}' present but not enabled", m.name()),
            );
        }
    }
    logging::line("INFO", "clientpatch init complete");
    // UnityPlayer registers its own VEH + unhandled filter during startup;
    // put ours first again so the crash window still wins.
    crate::crashguard::reassert();
    // Game window exists by now; re-apply the current status (LIVE may have
    // fired while TARGET_HWND was still the console / unset).
    crate::modules::titlebar::start_claim_loop();
    crate::modules::titlebar::refresh();

    let crash_test = cfg.loader.crash_test
        || std::env::var("CLIENTPATCH_CRASH_TEST")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    if crash_test {
        crate::crashguard::schedule_test_crash();
    }
    // All hook installation passes are finished. Cache scanning and deletion
    // run on their own thread and never delay the hook startup window.
    crate::modules::bepinex::cleanup_on_startup(&cfg, &dir);
    0
}

fn dll_dir() -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    unsafe {
        let mut hmod: HMODULE = core::ptr::null_mut();
        let anchor = dll_dir as *const () as *const u16;
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            anchor,
            &mut hmod,
        ) == 0
        {
            return None;
        }
        // Grow the buffer until the path fits: GetModuleFileNameW returns the
        // buffer size (not the true length) and truncates when the path is too
        // long, so a fixed 260-wchar buffer would silently resolve the wrong
        // directory for a deep install path — and then config/log fail to load
        // and the whole patch goes inert. Cap at the extended-length maximum.
        let mut buf = vec![0u16; 260];
        loop {
            let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32) as usize;
            if n == 0 {
                return None;
            }
            if n < buf.len() {
                let path = String::from_utf16_lossy(&buf[..n]);
                return PathBuf::from(path).parent().map(|p| p.to_path_buf());
            }
            // Returned == buffer size: truncated. Grow and retry.
            if buf.len() >= 32768 {
                return None;
            }
            buf.resize((buf.len() * 2).min(32768), 0);
        }
    }
}
