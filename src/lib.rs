//! clientpatch — an in-process mod loader for Heaven Burns Red, shipped as a
//! version.dll DLL-search-order proxy. Module #1 redirects the client to a
//! LilyPad private server. See docs/design.md.

pub mod config;
pub mod logging;
pub mod module;
pub mod modules;
pub mod process;
#[cfg(any(windows, test))]
mod staging;
pub mod status;
pub mod wide;

/// Release builds stamp the bundle version; local builds use the Cargo manifest.
pub const VERSION: &str = match option_env!("CLIENTPATCH_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(windows)]
mod bootstrap;
#[cfg(windows)]
mod crashguard;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod il2cpp;
#[cfg(windows)]
mod loadlib;
#[cfg(windows)]
mod version_proxy;

#[cfg(windows)]
mod entry {
    use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HMODULE, TRUE};
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    use windows_sys::Win32::System::LibraryLoader::{
        DisableThreadLibraryCalls, GetModuleFileNameW,
    };
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
    use windows_sys::Win32::System::Threading::CreateThread;

    /// DLL entry. Per the loader-lock rule: only detach thread notifications and
    /// spawn the init thread; never do heavy work here.
    #[no_mangle]
    pub extern "system" fn DllMain(
        hinst: HMODULE,
        reason: u32,
        _reserved: *mut core::ffi::c_void,
    ) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            unsafe {
                DisableThreadLibraryCalls(hinst);
                if !is_target_process() {
                    return TRUE;
                }
                let h = CreateThread(
                    core::ptr::null(),
                    0,
                    Some(super::bootstrap::init_thread),
                    core::ptr::null(),
                    0,
                    core::ptr::null_mut(),
                );
                // We never join the init thread; release our handle so it isn't
                // leaked for the process lifetime (the thread keeps running).
                if !h.is_null() {
                    CloseHandle(h);
                }
            }
        }
        TRUE
    }

    /// Kernel APIs + stack buffers only (no heap — loader-lock safe).
    unsafe fn is_target_process() -> bool {
        let mut buf = [0u16; 520];
        let n =
            GetModuleFileNameW(core::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) as usize;
        if n == 0 || n >= buf.len() {
            // Can't tell — fail open so the game still gets patched.
            return true;
        }
        let mut start = 0usize;
        for (i, &c) in buf.iter().take(n).enumerate() {
            if c == b'\\' as u16 || c == b'/' as u16 {
                start = i + 1;
            }
        }
        if !eq_ascii_ignore_case(&buf[start..n], "HeavenBurnsRed.exe") {
            return false;
        }
        let cmd = GetCommandLineW();
        if cmd.is_null() {
            return true;
        }
        !wide_has_cef_type(cmd)
    }

    fn eq_ascii_ignore_case(units: &[u16], ascii: &str) -> bool {
        let b = ascii.as_bytes();
        if units.len() != b.len() {
            return false;
        }
        units.iter().zip(b).all(|(&u, &a)| {
            if u >= 128 {
                return false;
            }
            (u as u8).eq_ignore_ascii_case(&a)
        })
    }

    unsafe fn wide_has_cef_type(p: *const u16) -> bool {
        let mut n = 0usize;
        while n < 32768 && *p.add(n) != 0 {
            n += 1;
        }
        let slice = core::slice::from_raw_parts(p, n);
        let needle_eq: &[u16] = &[
            0x2d,
            0x2d,
            b't' as u16,
            b'y' as u16,
            b'p' as u16,
            b'e' as u16,
            b'=' as u16,
        ];
        let needle_sp: &[u16] = &[
            0x2d,
            0x2d,
            b't' as u16,
            b'y' as u16,
            b'p' as u16,
            b'e' as u16,
            b' ' as u16,
        ];
        if slice.len() < needle_eq.len() {
            return false;
        }
        for w in slice.windows(needle_eq.len()) {
            let mut hit_eq = true;
            let mut hit_sp = true;
            for i in 0..needle_eq.len() {
                let c = w[i];
                let lo = if c >= b'A' as u16 && c <= b'Z' as u16 {
                    c + 32
                } else {
                    c
                };
                if lo != needle_eq[i] {
                    hit_eq = false;
                }
                if lo != needle_sp[i] {
                    hit_sp = false;
                }
            }
            if hit_eq || hit_sp {
                return true;
            }
        }
        false
    }
}
