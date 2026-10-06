//! Shared `kernelbase!LoadLibrary*` detours so more than one module can watch
//! DLL loads. `platform` needs `unity_gamelib_wrapper`; `steam` needs
//! `steam_api64`. A second GenericDetour on the same export would fail, so
//! every watcher registers here.
//!
//! Notifications are delivered after the outermost LoadLibrary returns. Nested
//! loads inside DllMain or callbacks never synchronously reenter a watcher.
//! A LoadLibrary return inside another DLL's DllMain still owns the loader lock;
//! those notifications are transferred to a worker before invoking any watcher.
#![cfg(windows)]

use crate::ffi::export;
use crate::logging;
use crate::wide;
use retour::GenericDetour;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

#[link(name = "ntdll")]
extern "system" {
    fn RtlIsThreadWithinLoaderCallout() -> u8;
}

struct Event {
    name: String,
    module: usize,
}
static DEFERRED: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

struct LoadScope;
impl LoadScope {
    fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}
impl Drop for LoadScope {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
        crate::hook::no_panic_void(drain);
    }
}

fn start_worker() {
    DEFERRED.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Event>();
        std::thread::Builder::new()
            .name("native-load-notifications".into())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    crate::hook::no_panic_void(|| {
                        PENDING.with(|pending| pending.borrow_mut().push(event));
                        drain();
                    });
                }
            })
            .expect("cannot start native loader notification worker");
        sender
    });
}

fn drain() {
    if DEPTH.with(Cell::get) != 0 || DISPATCHING.with(Cell::get) {
        return;
    }
    let events = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if events.is_empty() {
        return;
    }
    if unsafe { RtlIsThreadWithinLoaderCallout() } != 0 {
        if let Some(sender) = DEFERRED.get() {
            for event in events {
                let _ = sender.send(event);
            }
        }
        return;
    }
    DISPATCHING.with(|flag| flag.set(true));
    struct DispatchGuard;
    impl Drop for DispatchGuard {
        fn drop(&mut self) {
            DISPATCHING.with(|flag| flag.set(false));
        }
    }
    let _guard = DispatchGuard;
    let mut seen = Vec::new();
    let mut batch = events;
    loop {
        for event in batch {
            // W/ExW/A wrappers and loads performed by a watcher can describe the
            // same DLL repeatedly. A watcher needs one notification per batch.
            if seen.contains(&event.module) {
                continue;
            }
            seen.push(event.module);
            deliver(event);
        }
        batch = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        if batch.is_empty() {
            break;
        }
    }
}

fn deliver(event: Event) {
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    };
    // A DLL loaded and freed inside DllMain may be gone by worker delivery.
    // Pin and validate it outside loader callouts before dereferencing exports.
    let mut module = core::ptr::null_mut();
    unsafe {
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            event.module as *const u16,
            &mut module,
        ) == 0
        {
            return;
        }
    }
    struct Pin(HMODULE);
    impl Drop for Pin {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(self.0);
            }
        }
    }
    let _pin = Pin(module);
    let mut path = [0u16; 32768];
    let count =
        unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
    if count == 0 || count >= path.len() {
        return;
    }
    let actual = String::from_utf16_lossy(&path[..count]).to_ascii_lowercase();
    let requested = event.name.rsplit(['\\', '/']).next().unwrap_or("");
    if !path_contains(&actual, requested.trim_end_matches(".dll")) {
        return;
    }
    let hooks: Vec<PostHook> = POST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for hook in hooks {
        hook(&actual, module);
    }
}

type LoadLibraryWFn = unsafe extern "system" fn(*const u16) -> HMODULE;
type LoadLibraryExWFn =
    unsafe extern "system" fn(*const u16, *mut core::ffi::c_void, u32) -> HMODULE;
type LoadLibraryAFn = unsafe extern "system" fn(*const u8) -> HMODULE;
type LoadLibraryExAFn =
    unsafe extern "system" fn(*const u8, *mut core::ffi::c_void, u32) -> HMODULE;

static LOAD_W: OnceLock<GenericDetour<LoadLibraryWFn>> = OnceLock::new();
static LOAD_EX_W: OnceLock<GenericDetour<LoadLibraryExWFn>> = OnceLock::new();
static LOAD_A: OnceLock<GenericDetour<LoadLibraryAFn>> = OnceLock::new();
static LOAD_EX_A: OnceLock<GenericDetour<LoadLibraryExAFn>> = OnceLock::new();
static ARMED: AtomicBool = AtomicBool::new(false);

type PostHook = fn(&str, HMODULE);
static POST: Mutex<Vec<PostHook>> = Mutex::new(Vec::new());

/// Register a callback invoked with the lowercased path/name after a
/// successful executable LoadLibrary. Idempotent install of the detours.
pub fn add_post(hook: PostHook) {
    {
        let mut g = POST.lock().unwrap_or_else(|e| e.into_inner());
        if g.iter().any(|h| *h as usize == hook as usize) {
            drop(g);
            install();
            return;
        }
        g.push(hook);
    }
    install();
}

/// Install the LoadLibrary detours if they are not already live.
pub fn install() {
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    start_worker();
    for dll in ["kernelbase.dll", "kernel32.dll"] {
        let h =
            unsafe { GetModuleHandleA(std::ffi::CString::new(dll).unwrap().as_ptr() as *const u8) };
        if h.is_null() {
            continue;
        }
        unsafe {
            if LOAD_W.get().is_none() {
                if let Some(p) = export(h, "LoadLibraryW") {
                    let t: LoadLibraryWFn = core::mem::transmute(p);
                    if crate::hook::install(&LOAD_W, t, load_w as LoadLibraryWFn).is_ok() {
                        logging::line("HOOK", &format!("LoadLibraryW ({dll}): shared watcher"));
                    }
                }
            }
            if LOAD_EX_W.get().is_none() {
                if let Some(p) = export(h, "LoadLibraryExW") {
                    let t: LoadLibraryExWFn = core::mem::transmute(p);
                    if crate::hook::install(&LOAD_EX_W, t, load_ex_w as LoadLibraryExWFn).is_ok() {
                        logging::line("HOOK", &format!("LoadLibraryExW ({dll}): shared watcher"));
                    }
                }
            }
            if LOAD_A.get().is_none() {
                if let Some(p) = export(h, "LoadLibraryA") {
                    let t: LoadLibraryAFn = core::mem::transmute(p);
                    let _ = crate::hook::install(&LOAD_A, t, load_a as LoadLibraryAFn);
                }
            }
            if LOAD_EX_A.get().is_none() {
                if let Some(p) = export(h, "LoadLibraryExA") {
                    let t: LoadLibraryExAFn = core::mem::transmute(p);
                    let _ = crate::hook::install(&LOAD_EX_A, t, load_ex_a as LoadLibraryExAFn);
                }
            }
        }
        if LOAD_W.get().is_some() && LOAD_EX_W.get().is_some() {
            break;
        }
    }
    if LOAD_W.get().is_none() && LOAD_EX_W.get().is_none() {
        logging::line("ERR", "loadlib: could not hook LoadLibrary*");
    }
}

fn notify(name: &str, h: HMODULE) {
    if h.is_null() || name.is_empty() {
        return;
    }
    PENDING.with(|pending| {
        pending.borrow_mut().push(Event {
            name: name.to_string(),
            module: h as usize,
        })
    });
}

fn name_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { wide::from_wide_nul(p) }.to_ascii_lowercase()
}

fn name_ansi(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p as *const i8) }
        .to_string_lossy()
        .to_ascii_lowercase()
}

unsafe extern "system" fn load_w(name: *const u16) -> HMODULE {
    let Some(d) = LOAD_W.get() else {
        return core::ptr::null_mut();
    };
    let scope = LoadScope::enter();
    let h = d.call(name);
    let error = GetLastError();
    crate::hook::no_panic_void(|| notify(&name_wide(name), h));
    drop(scope);
    SetLastError(error);
    h
}

unsafe extern "system" fn load_ex_w(
    name: *const u16,
    file: *mut core::ffi::c_void,
    flags: u32,
) -> HMODULE {
    let Some(d) = LOAD_EX_W.get() else {
        return core::ptr::null_mut();
    };
    let scope = LoadScope::enter();
    let h = d.call(name, file, flags);
    let error = GetLastError();
    // Skip LOAD_LIBRARY_AS_DATAFILE / AS_IMAGE_RESOURCE / AS_DATAFILE_EXCLUSIVE.
    if flags & 0x62 == 0 {
        crate::hook::no_panic_void(|| notify(&name_wide(name), h));
    }
    drop(scope);
    SetLastError(error);
    h
}

unsafe extern "system" fn load_a(name: *const u8) -> HMODULE {
    let Some(d) = LOAD_A.get() else {
        return core::ptr::null_mut();
    };
    let scope = LoadScope::enter();
    let h = d.call(name);
    let error = GetLastError();
    crate::hook::no_panic_void(|| notify(&name_ansi(name), h));
    drop(scope);
    SetLastError(error);
    h
}

unsafe extern "system" fn load_ex_a(
    name: *const u8,
    file: *mut core::ffi::c_void,
    flags: u32,
) -> HMODULE {
    let Some(d) = LOAD_EX_A.get() else {
        return core::ptr::null_mut();
    };
    let scope = LoadScope::enter();
    let h = d.call(name, file, flags);
    let error = GetLastError();
    if flags & 0x62 == 0 {
        crate::hook::no_panic_void(|| notify(&name_ansi(name), h));
    }
    drop(scope);
    SetLastError(error);
    h
}

/// True when `path` refers to a DLL whose base name matches `needle`
/// (case-insensitive; `needle` should already be lowercase).
pub fn path_contains(path: &str, needle: &str) -> bool {
    path.rsplit(['\\', '/']).next().is_some_and(|file| {
        file.eq_ignore_ascii_case(needle) || file.eq_ignore_ascii_case(&format!("{needle}.dll"))
    })
}

#[cfg(test)]
mod tests {
    use super::path_contains;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    static CALLBACK_LOCK: Mutex<()> = Mutex::new(());
    static REENTERED: AtomicBool = AtomicBool::new(false);

    fn reentrant_callback(name: &str, _: windows_sys::Win32::Foundation::HMODULE) {
        if !path_contains(name, "kernel32") {
            return;
        }
        let _guard = CALLBACK_LOCK.lock().unwrap();
        if !REENTERED.swap(true, Ordering::SeqCst) {
            // A watcher calls native DLL code (e.g. PAYMENT_IsInitialized),
            // which may itself resolve/load a DLL. The watcher owns its lock.
            unsafe {
                let module = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(
                    crate::wide::to_wide_nul("kernel32.dll").as_ptr(),
                );
                assert!(!module.is_null());
                windows_sys::Win32::Foundation::FreeLibrary(module);
            }
        }
    }

    #[test]
    fn native_loader_reentry_probe() {
        if std::env::var_os("CLIENTPATCH_LOADER_REENTRY_PROBE").is_none() {
            return;
        }
        super::add_post(reentrant_callback);
        unsafe {
            let module = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(
                crate::wide::to_wide_nul("kernel32.dll").as_ptr(),
            );
            assert!(!module.is_null());
            windows_sys::Win32::Foundation::FreeLibrary(module);
        }
        assert!(REENTERED.load(Ordering::SeqCst));
    }

    #[test]
    fn native_loader_reentry_finishes() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "loadlib::tests::native_loader_reentry_probe",
                "--nocapture",
            ])
            .env("CLIENTPATCH_LOADER_REENTRY_PROBE", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "loader probe failed: {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("LoadLibraryW deadlocked while a watcher reentered the native loader");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn matches_basename_and_full_path() {
        assert!(path_contains("steam_api64.dll", "steam_api64"));
        assert!(path_contains(
            r"e:\games\heavenburnsred\steam_api64.dll",
            "steam_api64"
        ));
        assert!(path_contains(
            "C:/x/unity_gamelib_wrapper.dll",
            "unity_gamelib_wrapper"
        ));
        assert!(!path_contains("kernel32.dll", "steam_api64"));
        assert!(!path_contains("D:/steam_api64/other.dll", "steam_api64"));
        assert!(!path_contains("steam_api64_backup.dll", "steam_api64"));
    }
}
