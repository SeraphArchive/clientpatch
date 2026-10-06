//! Platform-host redirect: rewrite the official Gree gl-payment origin
//! to platform_base in the native GameLib config strings. Native detours by
//! exported name (GetProcAddress); only the gl-payment origin is touched —
//! Bandai Namco payment hosts are left at the real backend.
use crate::config::Lilypad;
use crate::modules::lilypad::url;

/// The official platform origin supplied (as an IL2CPP literal) into the native
/// GameLib config. The native DLLs contain no such literal themselves.
pub const GL_PAYMENT_ORIGIN: &str = "https://gl-payment.gree-apps.net";

/// Replace only the gl-payment origin with platform_base's origin. Boundary-aware
/// matching without JSON parsing stays robust to config-key renames and
/// scheme changes (https -> http for a local dev server).
pub fn rewrite_gl_payment_origin(input: &str, platform_base: &str) -> String {
    let dest = url::origin(platform_base);
    // Cover both schemes — HostSetting is https today, but a future build
    // (or a leftover http override) should still be rewritten.
    replace_origin(&replace_origin(input, GL_PAYMENT_ORIGIN, dest),
        "http://gl-payment.gree-apps.net", dest)
}

fn origin_boundary(rest: &str) -> bool {
    rest.chars().next().is_none_or(|c| {
        c.is_ascii_whitespace() || matches!(c, '/' | '?' | '#' | '"' | '\'' | ',' | '}' | ']')
    })
}

fn replace_origin(input: &str, official: &str, dest: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    for (start, _) in input.match_indices(official) {
        let end = start + official.len();
        if origin_boundary(&input[end..]) {
            output.push_str(&input[copied..start]);
            output.push_str(dest);
            copied = end;
        }
    }
    output.push_str(&input[copied..]);
    output
}

/// True when a GameLib config string still names the official payment host.
pub fn contains_official_gl_payment(input: &str) -> bool {
    [GL_PAYMENT_ORIGIN, "http://gl-payment.gree-apps.net"]
        .iter()
        .any(|official| input.match_indices(official)
            .any(|(start, _)| origin_boundary(&input[start + official.len()..])))
}

#[cfg(windows)]
mod hooks {
    use super::*;
    use crate::ffi::export;
    use crate::logging;
    use crate::wide;
    use retour::GenericDetour;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

    static PLATFORM_ORIGIN: OnceLock<String> = OnceLock::new();
    // Mirrors cfg.capture_md5 — gates debug logging of PAYMENT_Initialize args
    // (app_id / app_secret), used to recover the migration-password salt.
    static CAPTURE: AtomicBool = AtomicBool::new(false);

    // PAYMENT_Initialize(const wchar_t* appId, const wchar_t* appSecret, const wchar_t* configJson)
    type PaymentInitFn = unsafe extern "C" fn(*const u16, *const u16, *const u16) -> i32;
    // PAYMENT_SetConfig(const wchar_t* key, const wchar_t* value)
    type PaymentSetConfigFn = unsafe extern "C" fn(*const u16, *const u16) -> i32;

    // GenericDetour lives in a static so it stays installed for the process
    // lifetime. Install via hook::install (set THEN enable).
    static PAYMENT_INIT: OnceLock<GenericDetour<PaymentInitFn>> = OnceLock::new();
    static PAYMENT_SETCONFIG: OnceLock<GenericDetour<PaymentSetConfigFn>> = OnceLock::new();
    // PAYMENT_VerifyMigrationCode(const wchar_t* code, const wchar_t* password) — debug capture
    type PaymentVerifyMigFn = unsafe extern "C" fn(*const u16, *const u16);
    static PAYMENT_VERIFYMIG: OnceLock<GenericDetour<PaymentVerifyMigFn>> = OnceLock::new();

    pub fn apply(cfg: &Lilypad) -> anyhow::Result<()> {
        let _ = PLATFORM_ORIGIN.set(url::origin(&cfg.platform_base).to_string());
        CAPTURE.store(cfg.capture_md5, Ordering::SeqCst);

        // Never block the early-init pass: GGLInitialize / PAYMENT_Initialize
        // run at prologue, often during the old 8s IL2CPP settle. Arm a
        // LoadLibrary hook so we catch the wrapper the instant the game
        // loads it, plus a short poller as a backstop. Forcing LoadLibrary
        // ourselves would change init order (CrackProof-sensitive).
        crate::loadlib::add_post(on_dll_loaded);
        try_install_from_handle(unsafe {
            GetModuleHandleA(c"unity_gamelib_wrapper.dll".as_ptr() as *const u8)
        });
        spawn_wrapper_poller();
        Ok(())
    }

    fn on_dll_loaded(name: &str, module: HMODULE) {
        if crate::loadlib::path_contains(name, "unity_gamelib_wrapper") {
            try_install_from_handle(module);
        }
    }

    fn spawn_wrapper_poller() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            use windows_sys::Win32::System::Threading::Sleep;
            for _ in 0..400 {
                if PAYMENT_INIT.get().is_some() {
                    return;
                }
                let h =
                    unsafe { GetModuleHandleA(c"unity_gamelib_wrapper.dll".as_ptr() as *const u8) };
                if !h.is_null() {
                    try_install_from_handle(h);
                    return;
                }
                unsafe { Sleep(50) };
            }
            if PAYMENT_INIT.get().is_none() {
                crate::status::mark_failed(
                    "unity_gamelib_wrapper.dll never loaded; PAYMENT_Initialize not hooked",
                );
            }
        });
    }

    static INSTALL: Mutex<()> = Mutex::new(());

    fn already_initialized(module: HMODULE) -> bool {
        unsafe {
            let Some(p) = export(module, "PAYMENT_IsInitialized") else {
                return false;
            };
            let f: unsafe extern "C" fn() -> i32 = core::mem::transmute(p);
            f() != 0
        }
    }

    fn try_install_from_handle(module: HMODULE) {
        if module.is_null() || PAYMENT_INIT.get().is_some() {
            return;
        }
        let _guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
        if PAYMENT_INIT.get().is_some() {
            return;
        }
        let Some(init_p) = (unsafe { export(module, "PAYMENT_Initialize") }) else {
            return;
        };
        // Wrapper already in memory: if GameLib finished init against the
        // official host, hooking now cannot rewrite that first config.
        if already_initialized(module) {
            crate::status::mark_failed("PAYMENT_Initialize already ran before the hook was armed");
            // Still install so later SetConfig / re-init can be rewritten.
        }
        unsafe {
            let target: PaymentInitFn = core::mem::transmute(init_p);
            match crate::hook::install(&PAYMENT_INIT, target, payment_init_detour as PaymentInitFn)
            {
                Ok(()) => logging::line("HOOK", "PAYMENT_Initialize: enabled"),
                Err(e) => {
                    crate::status::mark_failed(&format!("PAYMENT_Initialize detour: {e}"));
                    return;
                }
            }
            if let Some(p) = export(module, "PAYMENT_SetConfig") {
                let target: PaymentSetConfigFn = core::mem::transmute(p);
                if crate::hook::install(
                    &PAYMENT_SETCONFIG,
                    target,
                    payment_setconfig_detour as PaymentSetConfigFn,
                )
                .is_ok()
                {
                    logging::line("HOOK", "PAYMENT_SetConfig: enabled");
                }
            } else {
                logging::line("WARN", "PAYMENT_SetConfig export not found");
            }
            if CAPTURE.load(Ordering::SeqCst) {
                if let Some(p) = export(module, "PAYMENT_VerifyMigrationCode") {
                    let target: PaymentVerifyMigFn = core::mem::transmute(p);
                    if crate::hook::install(
                        &PAYMENT_VERIFYMIG,
                        target,
                        payment_verifymig_detour as PaymentVerifyMigFn,
                    )
                    .is_ok()
                    {
                        logging::line("HOOK", "[capture_md5] PAYMENT_VerifyMigrationCode enabled");
                    }
                }
            }
        }
    }

    unsafe extern "C" fn payment_init_detour(
        app_id: *const u16,
        app_secret: *const u16,
        config_json: *const u16,
    ) -> i32 {
        let Some(detour) = PAYMENT_INIT.get() else {
            return 0;
        };
        crate::hook::no_panic_void(|| {
            if CAPTURE.load(Ordering::SeqCst) {
                let id = if app_id.is_null() {
                    String::new()
                } else {
                    wide::from_wide_nul(app_id)
                };
                let sec = if app_secret.is_null() {
                    String::new()
                } else {
                    wide::from_wide_nul(app_secret)
                };
                logging::line("PAYINIT", &format!("app_id={id:?} app_secret={sec:?}"));
            }
        });
        let started = crate::modules::diagnostics::native_init().then(std::time::Instant::now);
        if started.is_some() { logging::line("DIAG", "PAYMENT_Initialize enter"); }
        let result = match crate::hook::no_panic(None, || rewrite_wide(config_json)) {
            Some(buf) => {
                crate::hook::no_panic_void(|| crate::status::mark_live("PAYMENT_Initialize"));
                detour.call(app_id, app_secret, buf.as_ptr())
            }
            None => {
                crate::hook::no_panic_void(|| observe_init_payload(config_json));
                detour.call(app_id, app_secret, config_json)
            }
        };
        crate::hook::no_panic_void(|| {
            let Some(started) = started else { return; };
            let module = unsafe { GetModuleHandleA(c"unity_gamelib_wrapper.dll".as_ptr().cast()) };
            logging::line("DIAG", &format!(
                "PAYMENT_Initialize return={result} initialized={} elapsed_ms={}",
                already_initialized(module), started.elapsed().as_millis()));
            unsafe { crate::modules::diagnostics::observe_native_console(); }
        });
        result
    }

    // Debug capture: log the exact (code, password) C# hands to the native
    // migration verify. Reveals whether the password is plaintext or pre-hashed
    // (text + raw UTF-16 hex so binary/encoded content is visible).
    unsafe extern "C" fn payment_verifymig_detour(code: *const u16, password: *const u16) {
        crate::hook::no_panic_void(|| {
            log_wide("MIGCAP code", code);
            log_wide("MIGCAP password", password);
        });
        if let Some(d) = PAYMENT_VERIFYMIG.get() {
            d.call(code, password);
        }
    }

    fn log_wide(tag: &str, p: *const u16) {
        if p.is_null() {
            logging::line("MIGCAP", &format!("{tag}=<null>"));
            return;
        }
        let text = unsafe { wide::from_wide_nul(p) };
        let mut hexs = String::new();
        unsafe {
            let mut i = 0isize;
            while i < 512 && *p.offset(i) != 0 {
                hexs.push_str(&format!("{:04x} ", *p.offset(i)));
                i += 1;
            }
        }
        logging::line(
            "MIGCAP",
            &format!("{tag} text={text:?} u16hex={}", hexs.trim_end()),
        );
    }

    unsafe extern "C" fn payment_setconfig_detour(key: *const u16, value: *const u16) -> i32 {
        let Some(detour) = PAYMENT_SETCONFIG.get() else {
            return 0;
        };
        match crate::hook::no_panic(None, || rewrite_wide(value)) {
            Some(buf) => {
                crate::hook::no_panic_void(|| crate::status::mark_live("PAYMENT_SetConfig"));
                detour.call(key, buf.as_ptr())
            }
            None => detour.call(key, value),
        }
    }

    /// Read a wide arg, rewrite the gl-payment origin, and (only if it changed)
    /// return a fresh NUL-terminated UTF-16 buffer to pass to the original. The
    /// buffer is held on the detour's stack frame across the `.call()`, so it
    /// outlives the original's synchronous read of the string.
    fn rewrite_wide(ptr: *const u16) -> Option<Vec<u16>> {
        if ptr.is_null() {
            return None;
        }
        let original = unsafe { wide::from_wide_nul(ptr) };
        let origin = PLATFORM_ORIGIN.get()?;
        let rewritten = super::rewrite_gl_payment_origin(&original, origin);
        if rewritten == original {
            return None;
        }
        logging::line("REDIR", &format!("platform config rewritten to {origin}"));
        Some(wide::to_wide_nul(&rewritten))
    }

    fn observe_init_payload(ptr: *const u16) {
        if ptr.is_null() {
            crate::status::mark_failed("PAYMENT_Initialize config_json is null");
            return;
        }
        let original = unsafe { wide::from_wide_nul(ptr) };
        if super::contains_official_gl_payment(&original) {
            crate::status::mark_failed(
                "PAYMENT_Initialize still contains gl-payment.gree-apps.net",
            );
            return;
        }
        if let Some(origin) = PLATFORM_ORIGIN.get() {
            if original.contains(origin) {
                crate::status::mark_live("PAYMENT_Initialize already on platform_base");
                return;
            }
        }
        logging::line(
            "WARN",
            "PAYMENT_Initialize: no gl-payment origin in config; host unknown",
        );
    }
}

#[cfg(windows)]
pub use hooks::apply;

#[cfg(not(windows))]
pub fn apply(_cfg: &Lilypad) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_gl_payment_origin() {
        let cfg = r#"{"endpoint":"https://gl-payment.gree-apps.net/v1.0/x","bn":"https://bn-payment-ap.wrightflyer.net/v"}"#;
        let out = rewrite_gl_payment_origin(cfg, "http://127.0.0.1:8443");
        assert!(out.contains("http://127.0.0.1:8443/v1.0/x"));
        // Bandai Namco host untouched.
        assert!(out.contains("https://bn-payment-ap.wrightflyer.net/v"));
        assert!(!out.contains("gl-payment.gree-apps.net"));
    }

    #[test]
    fn strips_platform_base_trailing_slash() {
        let out = rewrite_gl_payment_origin("https://gl-payment.gree-apps.net/v1.0", "http://h:1/");
        assert_eq!(out, "http://h:1/v1.0");
    }

    #[test]
    fn noop_when_origin_absent() {
        let s = "no gree host here";
        assert_eq!(rewrite_gl_payment_origin(s, "http://h"), s);
    }

    #[test]
    fn rewrites_http_official_origin_too() {
        let out = rewrite_gl_payment_origin("http://gl-payment.gree-apps.net/v1.0", "http://h:1");
        assert_eq!(out, "http://h:1/v1.0");
    }

    #[test]
    fn detects_official_host() {
        assert!(contains_official_gl_payment(
            r#"{"endpoint":"https://gl-payment.gree-apps.net/v1.0"}"#
        ));
        assert!(!contains_official_gl_payment(
            r#"{"endpoint":"http://127.0.0.1:8443/v1.0"}"#
        ));
    }

    #[test]
    fn preserves_lookalike_payment_hosts_and_userinfo() {
        for endpoint in [
            "https://gl-payment.gree-apps.net.example.test/v1",
            "https://gl-payment.gree-apps.net@other.test/v1",
            "http://gl-payment.gree-apps.net-other.test/v1",
        ] {
            let config = format!(r#"{{"endpoint":"{endpoint}"}}"#);
            assert_eq!(rewrite_gl_payment_origin(&config, "http://local"), config);
            assert!(!contains_official_gl_payment(&config));
        }
    }
}
