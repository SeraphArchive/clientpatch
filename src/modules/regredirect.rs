//! Module "regredirect" — isolate the client's per-user registry state so a
//! third-party server cannot corrupt the official login. Redirects the
//! HKCU\Software\wfs\HeavenBurnsRed subtree to a sibling key (default
//! `…\HeavenBurnsRed.clientpatch`) by rewriting the subkey path at the registry
//! open/create/delete-key funnels. See docs/design.md.
use crate::module::Module;

const COMPANY: &str = "wfs";
const PRODUCT: &str = "HeavenBurnsRed";

/// If `subkey` contains the product segment `<COMPANY>\<PRODUCT>` (optionally
/// followed by deeper subkeys), return it with that segment rewritten to
/// `<COMPANY>\<PRODUCT>.<suffix>`. Returns `None` when there is no match (the
/// call is then forwarded unchanged).
///
/// - Case-insensitive (registry paths are case-insensitive).
/// - Segment-aware: matches only when `PRODUCT` is bounded by `\` or end, so
///   `HeavenBurnsRedExtra` is NOT rewritten and `HeavenBurnsRed\Sub` IS.
/// - Independent of the root `HKEY` (the match is purely on the path string).
pub fn rewrite_subkey(subkey: &str, suffix: &str) -> Option<String> {
    let needle = format!("{COMPANY}\\{PRODUCT}");
    let pos = subkey.to_ascii_lowercase().find(&needle.to_ascii_lowercase())?;
    // Boundary before the company segment: start-of-string or a '\'.
    if pos != 0 && subkey.as_bytes()[pos - 1] != b'\\' {
        return None;
    }
    let end = pos + needle.len();
    // Boundary after the product segment: end-of-string or a '\'.
    if end < subkey.len() && subkey.as_bytes()[end] != b'\\' {
        return None;
    }
    let mut out = String::with_capacity(subkey.len() + 1 + suffix.len());
    out.push_str(&subkey[..end]);
    out.push('.');
    out.push_str(suffix);
    out.push_str(&subkey[end..]);
    Some(out)
}

#[derive(Default)]
pub struct RegRedirect;

impl Module for RegRedirect {
    fn name(&self) -> &str {
        "regredirect"
    }

    #[cfg(windows)]
    fn init_early(&self, config: &crate::config::Config, _dll_dir: &std::path::Path) -> anyhow::Result<()> {
        let cfg = config.regredirect.clone().unwrap_or_default();
        hooks::apply(&cfg.suffix)
    }
}

#[cfg(windows)]
mod hooks {
    use super::rewrite_subkey;
    use crate::ffi::{ensure_module, export};
    use crate::logging;
    use crate::wide;
    use core::ffi::{c_char, c_void};
    use retour::GenericDetour;
    use std::ffi::{CStr, CString};
    use std::sync::OnceLock;

    static SUFFIX: OnceLock<String> = OnceLock::new();

    // Hand-written Win32 ABI fn types (HKEY = *mut c_void, LPCWSTR = *const u16,
    // REGSAM/flags/LSTATUS = u32). Resolved by GetProcAddress + transmute, so no
    // windows-sys Registry feature is required.
    type RegOpenKeyExWFn =
        unsafe extern "system" fn(*mut c_void, *const u16, u32, u32, *mut *mut c_void) -> u32;
    type RegCreateKeyExWFn = unsafe extern "system" fn(
        *mut c_void,
        *const u16,
        u32,
        *const u16,
        u32,
        u32,
        *const c_void,
        *mut *mut c_void,
        *mut u32,
    ) -> u32;
    type RegDeleteKeyExWFn = unsafe extern "system" fn(*mut c_void, *const u16, u32, u32) -> u32;
    type RegDeleteKeyWFn = unsafe extern "system" fn(*mut c_void, *const u16) -> u32;
    type RegOpenKeyWFn =
        unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> u32;
    type RegCreateKeyWFn =
        unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> u32;
    type RegDeleteTreeWFn = unsafe extern "system" fn(*mut c_void, *const u16) -> u32;

    type RegOpenKeyExAFn =
        unsafe extern "system" fn(*mut c_void, *const c_char, u32, u32, *mut *mut c_void) -> u32;
    type RegCreateKeyExAFn = unsafe extern "system" fn(
        *mut c_void,
        *const c_char,
        u32,
        *const c_char,
        u32,
        u32,
        *const c_void,
        *mut *mut c_void,
        *mut u32,
    ) -> u32;
    type RegDeleteKeyExAFn =
        unsafe extern "system" fn(*mut c_void, *const c_char, u32, u32) -> u32;
    type RegDeleteKeyAFn = unsafe extern "system" fn(*mut c_void, *const c_char) -> u32;
    type RegOpenKeyAFn =
        unsafe extern "system" fn(*mut c_void, *const c_char, *mut *mut c_void) -> u32;
    type RegCreateKeyAFn =
        unsafe extern "system" fn(*mut c_void, *const c_char, *mut *mut c_void) -> u32;
    type RegDeleteTreeAFn = unsafe extern "system" fn(*mut c_void, *const c_char) -> u32;

    static OPEN_EX_W: OnceLock<GenericDetour<RegOpenKeyExWFn>> = OnceLock::new();
    static CREATE_EX_W: OnceLock<GenericDetour<RegCreateKeyExWFn>> = OnceLock::new();
    static DELETE_EX_W: OnceLock<GenericDetour<RegDeleteKeyExWFn>> = OnceLock::new();
    static DELETE_W: OnceLock<GenericDetour<RegDeleteKeyWFn>> = OnceLock::new();
    static OPEN_W: OnceLock<GenericDetour<RegOpenKeyWFn>> = OnceLock::new();
    static CREATE_W: OnceLock<GenericDetour<RegCreateKeyWFn>> = OnceLock::new();
    static DELETE_TREE_W: OnceLock<GenericDetour<RegDeleteTreeWFn>> = OnceLock::new();

    static OPEN_EX_A: OnceLock<GenericDetour<RegOpenKeyExAFn>> = OnceLock::new();
    static CREATE_EX_A: OnceLock<GenericDetour<RegCreateKeyExAFn>> = OnceLock::new();
    static DELETE_EX_A: OnceLock<GenericDetour<RegDeleteKeyExAFn>> = OnceLock::new();
    static DELETE_A: OnceLock<GenericDetour<RegDeleteKeyAFn>> = OnceLock::new();
    static OPEN_A: OnceLock<GenericDetour<RegOpenKeyAFn>> = OnceLock::new();
    static CREATE_A: OnceLock<GenericDetour<RegCreateKeyAFn>> = OnceLock::new();
    static DELETE_TREE_A: OnceLock<GenericDetour<RegDeleteTreeAFn>> = OnceLock::new();

    pub fn apply(suffix: &str) -> anyhow::Result<()> {
        let _ = SUFFIX.set(suffix.to_string());
        let module = ensure_module("advapi32.dll");
        if module.is_null() {
            anyhow::bail!("advapi32.dll not available");
        }
        let mut installed = 0usize;
        unsafe {
            macro_rules! install {
                ($cell:ident, $name:literal, $ty:ty, $detour:ident) => {
                    match export(module, $name) {
                        Some(p) => {
                            let target: $ty = core::mem::transmute(p);
                            match crate::hook::install(&$cell, target, $detour as $ty) {
                                Ok(()) => {
                                    installed += 1;
                                    logging::line("HOOK", &format!("{}: enabled", $name));
                                }
                                Err(e) => logging::line(
                                    "ERR",
                                    &format!("{}: detour failed: {e}", $name),
                                ),
                            }
                        }
                        None => logging::line("WARN", &format!("{} export not found", $name)),
                    }
                };
            }

            install!(OPEN_EX_W, "RegOpenKeyExW", RegOpenKeyExWFn, open_ex_w_detour);
            install!(
                CREATE_EX_W,
                "RegCreateKeyExW",
                RegCreateKeyExWFn,
                create_ex_w_detour
            );
            install!(
                DELETE_EX_W,
                "RegDeleteKeyExW",
                RegDeleteKeyExWFn,
                delete_ex_w_detour
            );
            install!(DELETE_W, "RegDeleteKeyW", RegDeleteKeyWFn, delete_w_detour);
            install!(OPEN_W, "RegOpenKeyW", RegOpenKeyWFn, open_w_detour);
            install!(CREATE_W, "RegCreateKeyW", RegCreateKeyWFn, create_w_detour);
            install!(
                DELETE_TREE_W,
                "RegDeleteTreeW",
                RegDeleteTreeWFn,
                delete_tree_w_detour
            );

            install!(OPEN_EX_A, "RegOpenKeyExA", RegOpenKeyExAFn, open_ex_a_detour);
            install!(
                CREATE_EX_A,
                "RegCreateKeyExA",
                RegCreateKeyExAFn,
                create_ex_a_detour
            );
            install!(
                DELETE_EX_A,
                "RegDeleteKeyExA",
                RegDeleteKeyExAFn,
                delete_ex_a_detour
            );
            install!(DELETE_A, "RegDeleteKeyA", RegDeleteKeyAFn, delete_a_detour);
            install!(OPEN_A, "RegOpenKeyA", RegOpenKeyAFn, open_a_detour);
            install!(CREATE_A, "RegCreateKeyA", RegCreateKeyAFn, create_a_detour);
            install!(
                DELETE_TREE_A,
                "RegDeleteTreeA",
                RegDeleteTreeAFn,
                delete_tree_a_detour
            );
        }
        if installed == 0 {
            anyhow::bail!("no advapi32 registry hooks installed");
        }
        logging::line(
            "INFO",
            &format!(
                "regredirect active (suffix={:?}, hooks={installed})",
                SUFFIX.get().map(String::as_str).unwrap_or("")
            ),
        );
        Ok(())
    }

    /// Read `subkey`, rewrite if it targets the product key, and return a fresh
    /// NUL-terminated UTF-16 buffer (held on the detour's stack frame across
    /// `.call()`), or `None` to forward the original pointer unchanged.
    fn rewritten_wide(subkey: *const u16) -> Option<Vec<u16>> {
        if subkey.is_null() {
            return None;
        }
        let s = unsafe { wide::from_wide_nul(subkey) };
        let suffix = SUFFIX.get()?;
        let new = rewrite_subkey(&s, suffix)?;
        logging::line("REDIR", &format!("registry W {s} -> {new}"));
        Some(wide::to_wide_nul(&new))
    }

    fn rewritten_ansi(subkey: *const c_char) -> Option<CString> {
        if subkey.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(subkey) }
            .to_string_lossy()
            .into_owned();
        let suffix = SUFFIX.get()?;
        let new = rewrite_subkey(&s, suffix)?;
        logging::line("REDIR", &format!("registry A {s} -> {new}"));
        CString::new(new).ok()
    }

    unsafe extern "system" fn open_ex_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
        opts: u32,
        sam: u32,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = OPEN_EX_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), opts, sam, out),
            None => d.call(hkey, subkey, opts, sam, out),
        }
    }

    unsafe extern "system" fn create_ex_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
        reserved: u32,
        class: *const u16,
        dwopts: u32,
        sam: u32,
        sa: *const c_void,
        out: *mut *mut c_void,
        disp: *mut u32,
    ) -> u32 {
        let Some(d) = CREATE_EX_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), reserved, class, dwopts, sam, sa, out, disp),
            None => d.call(hkey, subkey, reserved, class, dwopts, sam, sa, out, disp),
        }
    }

    unsafe extern "system" fn delete_ex_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
        sam: u32,
        reserved: u32,
    ) -> u32 {
        let Some(d) = DELETE_EX_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), sam, reserved),
            None => d.call(hkey, subkey, sam, reserved),
        }
    }

    unsafe extern "system" fn delete_w_detour(hkey: *mut c_void, subkey: *const u16) -> u32 {
        let Some(d) = DELETE_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr()),
            None => d.call(hkey, subkey),
        }
    }

    unsafe extern "system" fn open_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = OPEN_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), out),
            None => d.call(hkey, subkey, out),
        }
    }

    unsafe extern "system" fn create_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = CREATE_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), out),
            None => d.call(hkey, subkey, out),
        }
    }

    unsafe extern "system" fn delete_tree_w_detour(
        hkey: *mut c_void,
        subkey: *const u16,
    ) -> u32 {
        let Some(d) = DELETE_TREE_W.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_wide(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr()),
            None => d.call(hkey, subkey),
        }
    }

    unsafe extern "system" fn open_ex_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
        opts: u32,
        sam: u32,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = OPEN_EX_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), opts, sam, out),
            None => d.call(hkey, subkey, opts, sam, out),
        }
    }

    unsafe extern "system" fn create_ex_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
        reserved: u32,
        class: *const c_char,
        dwopts: u32,
        sam: u32,
        sa: *const c_void,
        out: *mut *mut c_void,
        disp: *mut u32,
    ) -> u32 {
        let Some(d) = CREATE_EX_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), reserved, class, dwopts, sam, sa, out, disp),
            None => d.call(hkey, subkey, reserved, class, dwopts, sam, sa, out, disp),
        }
    }

    unsafe extern "system" fn delete_ex_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
        sam: u32,
        reserved: u32,
    ) -> u32 {
        let Some(d) = DELETE_EX_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), sam, reserved),
            None => d.call(hkey, subkey, sam, reserved),
        }
    }

    unsafe extern "system" fn delete_a_detour(hkey: *mut c_void, subkey: *const c_char) -> u32 {
        let Some(d) = DELETE_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr()),
            None => d.call(hkey, subkey),
        }
    }

    unsafe extern "system" fn open_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = OPEN_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), out),
            None => d.call(hkey, subkey, out),
        }
    }

    unsafe extern "system" fn create_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
        out: *mut *mut c_void,
    ) -> u32 {
        let Some(d) = CREATE_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr(), out),
            None => d.call(hkey, subkey, out),
        }
    }

    unsafe extern "system" fn delete_tree_a_detour(
        hkey: *mut c_void,
        subkey: *const c_char,
    ) -> u32 {
        let Some(d) = DELETE_TREE_A.get() else {
            return 6;
        };
        match crate::hook::no_panic(None, || rewritten_ansi(subkey)) {
            Some(buf) => d.call(hkey, buf.as_ptr()),
            None => d.call(hkey, subkey),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite_subkey;

    #[test]
    fn rewrites_exact_product_key() {
        assert_eq!(
            rewrite_subkey(r"Software\wfs\HeavenBurnsRed", "clientpatch").as_deref(),
            Some(r"Software\wfs\HeavenBurnsRed.clientpatch")
        );
    }

    #[test]
    fn rewrites_deeper_subkey() {
        assert_eq!(
            rewrite_subkey(r"Software\wfs\HeavenBurnsRed\Sub\Key", "cp").as_deref(),
            Some(r"Software\wfs\HeavenBurnsRed.cp\Sub\Key")
        );
    }

    #[test]
    fn does_not_rewrite_sibling_with_shared_prefix() {
        assert_eq!(rewrite_subkey(r"Software\wfs\HeavenBurnsRedExtra", "cp"), None);
    }

    #[test]
    fn case_insensitive_match_preserves_original_casing() {
        assert_eq!(
            rewrite_subkey(r"SOFTWARE\WFS\HEAVENBURNSRED", "cp").as_deref(),
            Some(r"SOFTWARE\WFS\HEAVENBURNSRED.cp")
        );
    }

    #[test]
    fn no_match_or_empty_returns_none() {
        assert_eq!(rewrite_subkey(r"Software\Microsoft\Windows", "cp"), None);
        assert_eq!(rewrite_subkey("", "cp"), None);
    }

    #[test]
    fn does_not_rewrite_company_prefix() {
        assert_eq!(rewrite_subkey(r"Software\mywfs\HeavenBurnsRed", "cp"), None);
    }
}
