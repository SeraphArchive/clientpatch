//! Shared `kernelbase!LoadLibrary*` detours so more than one module can watch
//! DLL loads. `platform` needs `unity_gamelib_wrapper`; `steam` needs
//! `steam_api64`. A second GenericDetour on the same export would fail, so
//! every watcher registers here.
//!
//! Callbacks run AFTER the original LoadLibrary returns (DllMain finished,
//! loader lock dropped). They must not call LoadLibrary themselves.
#![cfg(windows)]

use crate::ffi::export;
use crate::logging;
use crate::wide;
use retour::GenericDetour;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

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
    for dll in ["kernelbase.dll", "kernel32.dll"] {
        let h = unsafe { GetModuleHandleA(std::ffi::CString::new(dll).unwrap().as_ptr() as *const u8) };
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
    let hooks: Vec<PostHook> = POST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for hook in hooks {
        hook(name, h);
    }
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
    let h = d.call(name);
    let error = GetLastError();
    crate::hook::no_panic_void(|| notify(&name_wide(name), h));
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
    let h = d.call(name, file, flags);
    let error = GetLastError();
    // Skip LOAD_LIBRARY_AS_DATAFILE / AS_IMAGE_RESOURCE / AS_DATAFILE_EXCLUSIVE.
    if flags & 0x62 == 0 {
        crate::hook::no_panic_void(|| notify(&name_wide(name), h));
    }
    SetLastError(error);
    h
}

unsafe extern "system" fn load_a(name: *const u8) -> HMODULE {
    let Some(d) = LOAD_A.get() else {
        return core::ptr::null_mut();
    };
    let h = d.call(name);
    let error = GetLastError();
    crate::hook::no_panic_void(|| notify(&name_ansi(name), h));
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
    let h = d.call(name, file, flags);
    let error = GetLastError();
    if flags & 0x62 == 0 {
        crate::hook::no_panic_void(|| notify(&name_ansi(name), h));
    }
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
