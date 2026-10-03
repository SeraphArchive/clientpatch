//! Module "titlebar" — when the patch is active, append a status-aware suffix to
//! the game window title. The suffix only names the API host after a rewrite has
//! actually fired; until then it says pending, and a GameLib miss says HOOK FAIL.
//! Sticky `SetWindowTextW` hook + a claim loop for the Unity window.
//! See docs/design.md.
use crate::module::Module;
use crate::status::RedirectState;

/// Extract host[:port] from a base URL: "http://127.0.0.1:8443/" -> "127.0.0.1:8443".
pub fn api_host(api_base: &str) -> String {
    let no_scheme = api_base
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(api_base);
    no_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// Render the configured suffix by substituting "{api_host}". Falls back to the
/// literal "clientpatch" when no host is available (e.g. lilypad not configured).
pub fn render_suffix(template: &str, api_host: Option<&str>) -> String {
    template.replace("{api_host}", api_host.unwrap_or("clientpatch"))
}

pub const PENDING_SUFFIX: &str = " — clientpatch (pending)";
pub const FAIL_SUFFIX: &str = " — clientpatch HOOK FAIL";

/// Suffix for the current redirect state. The configured template (which names
/// the host) is used only after a rewrite has been observed.
pub fn status_suffix(template: &str, api_host: Option<&str>, state: RedirectState) -> String {
    match state {
        RedirectState::Pending => PENDING_SUFFIX.to_string(),
        RedirectState::Live => render_suffix(template, api_host),
        RedirectState::Failed => FAIL_SUFFIX.to_string(),
    }
}

/// Strip a previously applied clientpatch suffix so we can replace it when the
/// state changes (pending → live / fail).
pub fn strip_status_suffix(title: &str, live_suffix: &str) -> String {
    for marker in [FAIL_SUFFIX, PENDING_SUFFIX, live_suffix] {
        if !marker.is_empty() && title.ends_with(marker) {
            return title[..title.len() - marker.len()].to_string();
        }
    }
    if let Some(i) = title.find(" — clientpatch") {
        return title[..i].to_string();
    }
    title.to_string()
}

/// The Unity standalone player window class. The one-shot finder logs the actual
/// class so this can be confirmed/adjusted for the shipped Unity 6 build.
const UNITY_WND_CLASS: &str = "UnityWndClass";

/// Whether a window class name is the Unity player window.
pub fn is_unity_window_name(class: &str) -> bool {
    class == UNITY_WND_CLASS
}

/// Re-apply the current suffix to the Unity window. Called from `status` on
/// Pending→Live/Fail. No-op until `init_early` has run.
pub fn refresh() {
    #[cfg(windows)]
    hooks::refresh();
}

/// Keep claiming the Unity window for a while. AllocConsole creates a visible
/// top-level window first; the game window appears later and can be recreated.
pub fn start_claim_loop() {
    #[cfg(windows)]
    hooks::start_claim_loop();
}

#[derive(Default)]
pub struct Titlebar;

impl Module for Titlebar {
    fn name(&self) -> &str {
        "titlebar"
    }

    #[cfg(windows)]
    fn init_early(&self, config: &crate::config::Config, _dll_dir: &std::path::Path) -> anyhow::Result<()> {
        let tb = config.titlebar.clone().unwrap_or_default();
        let host = config.lilypad.as_ref().map(|lp| api_host(&lp.api_base));
        hooks::apply(tb.template, host)
    }
}

#[cfg(windows)]
mod hooks {
    use super::{is_unity_window_name, render_suffix};
    use crate::ffi::{ensure_module, export};
    use crate::logging;
    use crate::wide;
    use retour::GenericDetour;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{BOOL, FALSE, HWND, LPARAM, TRUE};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, SetWindowTextW, GW_OWNER,
    };

    static TEMPLATE: OnceLock<String> = OnceLock::new();
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
    static CLAIM_STARTED: AtomicBool = AtomicBool::new(false);

    type SetWindowTextWFn = unsafe extern "system" fn(HWND, *const u16) -> BOOL;
    static SETTITLE: OnceLock<GenericDetour<SetWindowTextWFn>> = OnceLock::new();

    fn current_suffix() -> String {
        super::status_suffix(
            TEMPLATE.get().map(String::as_str).unwrap_or(""),
            HOST.get().and_then(|h| h.as_deref()),
            crate::status::state(),
        )
    }

    pub fn apply(template: String, host: Option<String>) -> anyhow::Result<()> {
        let _ = TEMPLATE.set(template);
        let _ = HOST.set(host);
        crate::status::set_on_change(refresh);

        let user32 = ensure_module("user32.dll");
        if user32.is_null() {
            anyhow::bail!("user32.dll not available");
        }
        unsafe {
            if let Some(p) = export(user32, "SetWindowTextW") {
                let target: SetWindowTextWFn = core::mem::transmute(p);
                crate::hook::install(&SETTITLE, target, set_title_detour as SetWindowTextWFn)?;
                logging::line("HOOK", "SetWindowTextW: enabled");
            } else {
                logging::line("WARN", "SetWindowTextW export not found");
            }
        }

        start_claim_loop();
        Ok(())
    }

    /// Keep looking for `UnityWndClass` — never claim the AllocConsole window
    /// (or any other first-visible HWND). The game window is created later and
    /// Unity can recreate it.
    pub fn start_claim_loop() {
        if CLAIM_STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            for i in 0..600 {
                let _ = apply_once();
                if i == 599 && TARGET_HWND.load(Ordering::Relaxed) == 0 {
                    logging::line("WARN", "titlebar: no UnityWndClass for this PID after ~60s");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    /// Strip any previous clientpatch mark and append the suffix for the
    /// current redirect state. `None` = already current, pass through.
    fn suffixed(current: &str) -> Option<Vec<u16>> {
        let suffix = current_suffix();
        if suffix.is_empty() {
            return None;
        }
        let live = render_suffix(
            TEMPLATE.get().map(String::as_str).unwrap_or(""),
            HOST.get().and_then(|h| h.as_deref()),
        );
        let base = super::strip_status_suffix(current, &live);
        let next = format!("{base}{suffix}");
        if next == current {
            return None;
        }
        Some(wide::to_wide_nul(&next))
    }

    pub fn refresh() {
        // Always re-find UnityWndClass. A cached HWND may be the console
        // (claimed before this fix) or a destroyed splash window.
        if apply_once() {
            return;
        }
        let hwnd = TARGET_HWND.load(Ordering::Relaxed);
        if hwnd != 0 && is_unity_window(hwnd as HWND) {
            apply_to(hwnd as HWND);
        }
    }

    unsafe extern "system" fn set_title_detour(hwnd: HWND, text: *const u16) -> BOOL {
        let Some(d) = SETTITLE.get() else {
            return FALSE;
        };
        let rewritten = crate::hook::no_panic(None, || {
            if is_unity_window(hwnd) {
                TARGET_HWND.store(hwnd as isize, Ordering::Relaxed);
                let current = wide::from_wide_nul(text);
                return suffixed(&current);
            }
            None
        });
        match rewritten {
            Some(buf) => d.call(hwnd, buf.as_ptr()),
            None => d.call(hwnd, text),
        }
    }

    fn window_class(hwnd: HWND) -> String {
        if hwnd.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return String::new();
        }
        wide::utf16_to_string(&buf[..n as usize])
    }

    fn is_unity_window(hwnd: HWND) -> bool {
        is_unity_window_name(&window_class(hwnd))
    }

    struct Found {
        hwnd: HWND,
    }

    fn apply_once() -> bool {
        unsafe {
            let mut found = Found {
                hwnd: core::ptr::null_mut(),
            };
            EnumWindows(Some(enum_proc), &mut found as *mut Found as LPARAM);
            let hwnd = found.hwnd;
            if hwnd.is_null() {
                return false;
            }
            TARGET_HWND.store(hwnd as isize, Ordering::Relaxed);
            apply_to(hwnd);
            true
        }
    }

    fn apply_to(hwnd: HWND) {
        if hwnd.is_null() || !is_unity_window(hwnd) {
            return;
        }
        unsafe {
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut tbuf = vec![0u16; len + 1];
            let got = GetWindowTextW(hwnd, tbuf.as_mut_ptr(), tbuf.len() as i32).max(0) as usize;
            let current = wide::utf16_to_string(&tbuf[..got]);
            let cls = window_class(hwnd);
            if let Some(buf) = suffixed(&current) {
                SetWindowTextW(hwnd, buf.as_ptr());
                logging::line(
                    "INFO",
                    &format!(
                        "titlebar set (class={cls:?}, state={:?}, was={current:?})",
                        crate::status::state()
                    ),
                );
            }
        }
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = &mut *(lparam as *mut Found);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != GetCurrentProcessId() {
            return TRUE;
        }
        if !GetWindow(hwnd, GW_OWNER).is_null() || IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        // Unity only — the console (`AllocConsole`) is the first visible
        // top-level window of this PID and must never be claimed.
        if is_unity_window(hwnd) {
            found.hwnd = hwnd;
            return FALSE;
        }
        TRUE
    }
}

#[cfg(test)]
mod tests {
    use super::{api_host, render_suffix};

    #[test]
    fn api_host_extracts_host_port() {
        assert_eq!(api_host("http://127.0.0.1:8443/"), "127.0.0.1:8443");
        assert_eq!(api_host("https://lily.example.com/api/x"), "lily.example.com");
        assert_eq!(api_host("https://h"), "h");
        assert_eq!(api_host("http://h:1/?q=1"), "h:1");
    }

    #[test]
    fn render_substitutes_api_host() {
        assert_eq!(
            render_suffix(" — clientpatch → {api_host}", Some("127.0.0.1:8443")),
            " — clientpatch → 127.0.0.1:8443"
        );
    }

    #[test]
    fn render_falls_back_when_no_host() {
        assert_eq!(render_suffix(" — {api_host}", None), " — clientpatch");
    }

    #[test]
    fn render_static_template_unchanged() {
        assert_eq!(render_suffix(" [CLIENTPATCHED]", Some("h")), " [CLIENTPATCHED]");
    }

    #[test]
    fn recognizes_unity_window_class() {
        assert!(super::is_unity_window_name("UnityWndClass"));
        assert!(!super::is_unity_window_name("Chrome_WidgetWin_1"));
        assert!(!super::is_unity_window_name("ConsoleWindowClass"));
    }

    #[test]
    fn pending_suffix_does_not_name_the_host() {
        assert_eq!(
            super::status_suffix(
                " — clientpatch → {api_host}",
                Some("127.0.0.1:8443"),
                super::RedirectState::Pending
            ),
            super::PENDING_SUFFIX
        );
    }

    #[test]
    fn live_suffix_uses_the_template() {
        assert_eq!(
            super::status_suffix(
                " — clientpatch → {api_host}",
                Some("127.0.0.1:8443"),
                super::RedirectState::Live
            ),
            " — clientpatch → 127.0.0.1:8443"
        );
    }

    #[test]
    fn fail_suffix_is_unambiguous() {
        assert_eq!(
            super::status_suffix(" — {api_host}", Some("h"), super::RedirectState::Failed),
            super::FAIL_SUFFIX
        );
    }

    #[test]
    fn strip_replaces_pending_with_live() {
        let live = " — clientpatch → 127.0.0.1:8443";
        assert_eq!(
            super::strip_status_suffix("HeavenBurnsRed — clientpatch (pending)", live),
            "HeavenBurnsRed"
        );
        assert_eq!(
            super::strip_status_suffix("HeavenBurnsRed — clientpatch HOOK FAIL", live),
            "HeavenBurnsRed"
        );
        assert_eq!(
            super::strip_status_suffix("HeavenBurnsRed — clientpatch → 127.0.0.1:8443", live),
            "HeavenBurnsRed"
        );
    }
}
