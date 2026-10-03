//! Steam launch policy: skip the title-screen relaunch, or fully stub steam_api.
//!
//! `mode`:
//! - `skip_restart` — `SteamAPI_RestartAppIfNecessary` always false; real Steam used
//! - `stub` — never talk to steamclient; in-process dummy (ticket/country/language)
//! - `auto` — Steam client running → skip_restart, else stub
//! - `off` — vanilla
use crate::config::Config;
use crate::module::Module;

#[cfg(windows)]
mod detect;
#[cfg(windows)]
mod stub;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamMode {
    Off,
    SkipRestart,
    Stub,
    Auto,
}

impl SteamMode {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "none" => Ok(Self::Off),
            "skip_restart" | "skip-restart" | "skip" => Ok(Self::SkipRestart),
            "stub" | "emu" | "offline" => Ok(Self::Stub),
            "auto" => Ok(Self::Auto),
            other => anyhow::bail!(
                "steam.mode must be off|skip_restart|stub|auto, got {other:?}"
            ),
        }
    }

    pub fn resolve(self, steam_running: bool) -> Resolved {
        match self {
            Self::Off => Resolved::Off,
            Self::SkipRestart => Resolved::SkipRestart,
            Self::Stub => Resolved::Stub,
            Self::Auto => {
                if steam_running {
                    Resolved::SkipRestart
                } else {
                    Resolved::Stub
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    Off,
    SkipRestart,
    Stub,
}

#[derive(Default)]
pub struct Steam;

impl Module for Steam {
    fn name(&self) -> &str {
        "steam"
    }

    #[cfg(windows)]
    fn init_early(
        &self,
        config: &Config,
        _dll_dir: &std::path::Path,
    ) -> anyhow::Result<()> {
        hooks::apply(config)
    }
}

#[cfg(windows)]
mod hooks {
    use super::{Resolved, SteamMode};
    use crate::config::Config;
    use crate::ffi::export;
    use crate::loadlib;
    use crate::logging;
    use retour::{GenericDetour, RawDetour};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

    const MODE_OFF: u8 = 0;
    const MODE_SKIP: u8 = 1;
    const MODE_STUB: u8 = 2;
    static MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
    static EXPORTS_HOOKED: AtomicBool = AtomicBool::new(false);
    /// steam_api64 handle; GPA rewrite is restricted to this module.
    static STEAM_MOD: std::sync::atomic::AtomicPtr<core::ffi::c_void> =
        std::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

    type Farproc = Option<unsafe extern "system" fn()>;
    type GetProcFn = unsafe extern "system" fn(HMODULE, *const u8) -> Farproc;

    static GETPROC: OnceLock<GenericDetour<GetProcFn>> = OnceLock::new();

    pub fn apply(config: &Config) -> anyhow::Result<()> {
        let cfg = config.steam.clone().unwrap_or_default();
        let mode = SteamMode::parse(&cfg.mode)?;
        if mode == SteamMode::Off {
            logging::line("INFO", "steam: mode=off");
            return Ok(());
        }
        let running = super::detect::client_is_running();
        let resolved = mode.resolve(running);
        logging::line(
            "INFO",
            &format!(
                "steam: mode={:?} steam_client={} -> {:?}",
                mode,
                running,
                resolved
            ),
        );
        match resolved {
            Resolved::Off => return Ok(()),
            Resolved::SkipRestart => MODE.store(MODE_SKIP, Ordering::SeqCst),
            Resolved::Stub => {
                MODE.store(MODE_STUB, Ordering::SeqCst);
                super::stub::configure(&cfg.ip_country, &cfg.ui_language);
                // GPA hook is only needed in stub mode (Steamworks.NET name
                // redirects). skip_restart patches the export body; GPA would
                // return the already-jmp'd address. Avoid kernelbase GPA on the
                // common Steam-running path (CrackProof A04/A08).
                install_getproc();
            }
        }
        loadlib::add_post(on_loaded);
        // steam_api64 may already be in the process (unusual this early).
        let h = unsafe { GetModuleHandleA(c"steam_api64.dll".as_ptr() as *const u8) };
        if !h.is_null() {
            on_loaded("steam_api64.dll", h);
        }
        Ok(())
    }

    fn on_loaded(name: &str, module: HMODULE) {
        if !loadlib::path_contains(name, "steam_api64") {
            return;
        }
        STEAM_MOD.store(module, Ordering::SeqCst);
        if EXPORTS_HOOKED.swap(true, Ordering::SeqCst) {
            return;
        }
        match MODE.load(Ordering::SeqCst) {
            MODE_SKIP => {
                // GameLib binds this via a static IAT. C# Steamworks.NET
                // GetProcAddress returns the already-patched export body —
                // no kernelbase GPA hook on this path.
                hook_export(
                    module,
                    "SteamAPI_RestartAppIfNecessary",
                    skip_restart as *const (),
                );
            }
            MODE_STUB => hook_gamelib_iat(module),
            _ => {
                EXPORTS_HOOKED.store(false, Ordering::SeqCst);
            }
        }
    }

    /// `cpp_gamelib_steam.dll` statically imports these 9 from steam_api64
    /// (confirmed PE IAT; not delay-load). GetProcAddress covers Steamworks.NET
    /// only — the export bodies must be patched or GameLib still talks to the
    /// real steamclient (or, after a stub Init, gets NULL interfaces → 102002).
    fn hook_gamelib_iat(module: HMODULE) {
        use super::stub;
        let hooks: &[(&str, *const ())] = &[
            ("SteamAPI_Init", stub::SteamAPI_Init as _),
            ("SteamAPI_Shutdown", stub::SteamAPI_Shutdown as _),
            (
                "SteamAPI_RestartAppIfNecessary",
                stub::SteamAPI_RestartAppIfNecessary as _,
            ),
            (
                "SteamAPI_RegisterCallback",
                stub::SteamAPI_RegisterCallback as _,
            ),
            (
                "SteamAPI_UnregisterCallback",
                stub::SteamAPI_UnregisterCallback as _,
            ),
            ("SteamAPI_RunCallbacks", stub::SteamAPI_RunCallbacks as _),
            ("SteamAPI_GetHSteamUser", stub::SteamAPI_GetHSteamUser as _),
            (
                "SteamInternal_ContextInit",
                stub::SteamInternal_ContextInit as _,
            ),
            (
                "SteamInternal_FindOrCreateUserInterface",
                stub::SteamInternal_FindOrCreateUserInterface as _,
            ),
        ];
        for &(name, detour) in hooks {
            hook_export(module, name, detour);
        }
    }

    unsafe extern "C" fn skip_restart(_appid: u32) -> u8 {
        0
    }

    /// Resolve a steam_api64 export WITHOUT going through our GetProcAddress
    /// hook (which would hand back the stub/skip pointer and we'd detour ourselves).
    fn raw_export(module: HMODULE, name: &str) -> Option<*const core::ffi::c_void> {
        let c = std::ffi::CString::new(name).ok()?;
        if let Some(dt) = GETPROC.get() {
            return unsafe { dt.call(module, c.as_ptr() as *const u8) }
                .map(|f| f as *const core::ffi::c_void);
        }
        unsafe { export(module, name) }
    }

    fn hook_export(module: HMODULE, name: &str, detour: *const ()) {
        let Some(p) = raw_export(module, name) else {
            logging::line("WARN", &format!("steam: {name} not in steam_api64"));
            return;
        };
        unsafe {
            match RawDetour::new(p as *const (), detour) {
                Ok(d) => match d.enable() {
                    Ok(()) => {
                        let _ = Box::leak(Box::new(d));
                        logging::line("HOOK", &format!("steam_api64!{name}"));
                    }
                    Err(e) => logging::line("ERR", &format!("{name} enable: {e:?}")),
                },
                Err(e) => logging::line("ERR", &format!("{name} detour: {e:?}")),
            }
        }
    }

    fn install_getproc() {
        if GETPROC.get().is_some() {
            return;
        }
        for dll in ["kernelbase.dll", "kernel32.dll"] {
            let h = crate::ffi::ensure_module(dll);
            if h.is_null() {
                continue;
            }
            let Some(p) = (unsafe { export(h, "GetProcAddress") }) else {
                continue;
            };
            unsafe {
                let target: GetProcFn =
                    core::mem::transmute::<*const core::ffi::c_void, GetProcFn>(p);
                match crate::hook::install(&GETPROC, target, getproc_detour as GetProcFn) {
                    Ok(()) => {
                        logging::line("HOOK", &format!("GetProcAddress ({dll}): steam exports"));
                        return;
                    }
                    Err(e) => logging::line("ERR", &format!("GetProcAddress detour: {e}")),
                }
            }
        }
    }

    fn as_farproc(p: *const core::ffi::c_void) -> Farproc {
        if p.is_null() {
            None
        } else {
            Some(unsafe {
                core::mem::transmute::<*const core::ffi::c_void, unsafe extern "system" fn()>(p)
            })
        }
    }

    unsafe extern "system" fn getproc_detour(module: HMODULE, name: *const u8) -> Farproc {
        let d = GETPROC.get()?;
        let orig = d.call(module, name);
        let steam = STEAM_MOD.load(Ordering::SeqCst);
        if steam.is_null() || !std::ptr::eq(module, steam) {
            return orig;
        }
        if name.is_null() || (name as usize) < 0x10000 {
            return orig;
        }
        crate::hook::no_panic(orig, || {
            let bytes = match std::ffi::CStr::from_ptr(name as *const i8).to_bytes() {
                b if b.starts_with(b"SteamAPI") || b.starts_with(b"SteamInternal") => b,
                _ => return orig,
            };
            match MODE.load(Ordering::SeqCst) {
                MODE_STUB => super::stub::resolve(bytes).map(as_farproc).unwrap_or(orig),
                _ => orig,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Resolved, SteamMode};

    #[test]
    fn parse_aliases() {
        assert_eq!(SteamMode::parse("skip_restart").unwrap(), SteamMode::SkipRestart);
        assert_eq!(SteamMode::parse("SKIP").unwrap(), SteamMode::SkipRestart);
        assert_eq!(SteamMode::parse("stub").unwrap(), SteamMode::Stub);
        assert_eq!(SteamMode::parse("offline").unwrap(), SteamMode::Stub);
        assert_eq!(SteamMode::parse("auto").unwrap(), SteamMode::Auto);
        assert_eq!(SteamMode::parse("off").unwrap(), SteamMode::Off);
        assert!(SteamMode::parse("maybe").is_err());
    }

    #[test]
    fn auto_follows_client() {
        assert_eq!(
            SteamMode::Auto.resolve(true),
            Resolved::SkipRestart
        );
        assert_eq!(SteamMode::Auto.resolve(false), Resolved::Stub);
        assert_eq!(SteamMode::Stub.resolve(true), Resolved::Stub);
        assert_eq!(SteamMode::SkipRestart.resolve(false), Resolved::SkipRestart);
    }
}
