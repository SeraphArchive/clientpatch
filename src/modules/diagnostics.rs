//! Opt-in observations. Never change native results or record credentials.
use std::sync::atomic::{AtomicBool, Ordering};
use crate::{config::Config, module::Module};

static NATIVE_INIT: AtomicBool = AtomicBool::new(false);
static CRASH_CONTEXT: AtomicBool = AtomicBool::new(false);

pub struct Diagnostics;
impl Module for Diagnostics {
    fn name(&self) -> &str { "diagnostics" }
    fn init_early(&self, config: &Config, _: &std::path::Path) -> anyhow::Result<()> {
        if let Some(cfg) = &config.diagnostics {
            NATIVE_INIT.store(cfg.enable && cfg.native_init, Ordering::Relaxed);
            CRASH_CONTEXT.store(cfg.enable && cfg.crash_context, Ordering::Relaxed);
        }
        #[cfg(windows)]
        if native_init() { crate::loadlib::add_post(observe_module); }
        Ok(())
    }
}

pub fn native_init() -> bool { NATIVE_INIT.load(Ordering::Relaxed) }
pub fn crash_context() -> bool { CRASH_CONTEXT.load(Ordering::Relaxed) }

#[cfg(windows)]
fn observe_module(name: &str, module: windows_sys::Win32::Foundation::HMODULE) {
    let file = name.rsplit(['\\', '/']).next().unwrap_or("");
    if file.contains("gamelib") || file == "steam_api64.dll" {
        static SEEN: std::sync::Mutex<Vec<(String, usize)>> = std::sync::Mutex::new(Vec::new());
        let first = {
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            let key = (file.to_owned(), module as usize);
            if seen.contains(&key) { false } else { seen.push(key); true }
        };
        if first { crate::logging::line("DIAG", &format!("native module={file} base={module:p}")); }
    }
    if file == "steam_api64.dll" && !super::steam::stub_active() {
        unsafe { steam_observer::install(module); }
    }
}

#[cfg(windows)]
mod steam_observer {
    use std::sync::{Mutex, OnceLock};
    use retour::GenericDetour;
    type Init = unsafe extern "C" fn() -> u8;
    type Shutdown = unsafe extern "C" fn();
    static INIT: OnceLock<GenericDetour<Init>> = OnceLock::new();
    static SHUTDOWN: OnceLock<GenericDetour<Shutdown>> = OnceLock::new();
    static INSTALL: Mutex<()> = Mutex::new(());
    pub unsafe fn install(module: windows_sys::Win32::Foundation::HMODULE) {
        let _guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
        if INIT.get().is_none() {
            if let Some(p) = crate::ffi::export(module, "SteamAPI_Init") {
                let target: Init = core::mem::transmute(p);
                if let Err(e) = crate::hook::install(&INIT, target, init as Init) {
                    crate::logging::line("DIAG", &format!("SteamAPI_Init observation unavailable: {e}"));
                }
            }
        }
        if SHUTDOWN.get().is_none() {
            if let Some(p) = crate::ffi::export(module, "SteamAPI_Shutdown") {
                let target: Shutdown = core::mem::transmute(p);
                if let Err(e) = crate::hook::install(&SHUTDOWN, target, shutdown as Shutdown) {
                    crate::logging::line("DIAG", &format!("SteamAPI_Shutdown observation unavailable: {e}"));
                }
            }
        }
    }
    unsafe extern "C" fn init() -> u8 {
        let Some(detour) = INIT.get() else { return 0; };
        let started = std::time::Instant::now();
        crate::hook::no_panic_void(|| crate::logging::line("DIAG", "SteamAPI_Init enter"));
        let result = detour.call();
        crate::hook::no_panic_void(|| crate::logging::line("DIAG", &format!("SteamAPI_Init result={result} elapsed_ms={}", started.elapsed().as_millis())));
        result
    }
    unsafe extern "C" fn shutdown() {
        if let Some(detour) = SHUTDOWN.get() {
            crate::hook::no_panic_void(|| crate::logging::line("DIAG", "SteamAPI_Shutdown called"));
            detour.call();
        }
    }
}

#[cfg(windows)]
pub unsafe fn observe_native_console() {
    use windows_sys::Win32::System::Console::*;
    let handle = GetStdHandle(STD_OUTPUT_HANDLE);
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = core::mem::zeroed();
    if GetConsoleScreenBufferInfo(handle, &mut info) == 0 { return; }
    let row = (info.dwCursorPosition.Y - 100).max(0);
    let count = (info.dwSize.X as usize * (info.dwCursorPosition.Y - row + 1) as usize).min(32768);
    let mut buffer = vec![0u16; count];
    let mut read = 0;
    if ReadConsoleOutputCharacterW(handle, buffer.as_mut_ptr(), count as u32, COORD { X: 0, Y: row }, &mut read) == 0 { return; }
    let output = String::from_utf16_lossy(&buffer[..read as usize]);
    for marker in ["SteamAPI_Init() failed.", "SteamAPI_RestartAppIfNecessary() failed.", "Shop initialize Failed!", "Payment::initialize error!", "Steam initialized."] {
        if output.contains(marker) {
            crate::logging::line("DIAG", &format!("native console marker: {marker}"));
        }
    }
}
