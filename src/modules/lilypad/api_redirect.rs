//! Game-API host redirect. By-name IL2CPP only.
//!
//! Strategy (timing-robust against the game assigning BaseUri during startup):
//!  1. set the static BaseUri field to api_base at init,
//!  2. neutralize BaseUriForRouting so a per-route host can't bypass us,
//!  3. hook the non-generic PrepareHeaders funnel to re-assert BaseUri and
//!     rewrite the in-flight request's m_uri host.
//!
//! UWRServerRequest.Post is generic: name resolution returns an open-generic
//! stub that is not safely hookable. PrepareHeaders is the non-generic funnel;
//! because m_uri may already be built by then, rewrite it directly using the
//! field offset resolved by name. See docs/design.md.
use crate::config::Lilypad;
use crate::il2cpp::Il2Cpp;
use crate::logging;
use crate::modules::lilypad::url;
use core::ffi::c_void;
use retour::GenericDetour;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::OnceLock;

// Shared with the PrepareHeaders detour (the detour fn is a plain extern "C"
// pointer and can't capture, so cross-call state lives in module statics).
static BASE_URI_FIELD: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static MURI_OFFSET: AtomicU32 = AtomicU32::new(0);
static API_ORIGIN: OnceLock<String> = OnceLock::new();
static IL2CPP: OnceLock<&'static Il2Cpp> = OnceLock::new();
// Cached `Dictionary<string,string>.set_Item` MethodInfo, resolved lazily from
// the first request's headers dictionary (all requests share the class).
static HEADER_SET_ITEM: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

// GenericDetour (stable Rust) stored in a static so the hook stays installed and
// the detour fn can reach the trampoline. Set before enable.
type PrepareHeadersFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
static PREPARE_HEADERS: OnceLock<GenericDetour<PrepareHeadersFn>> = OnceLock::new();

// UnityWebRequest-level safety net. This catches requests whose URL was built
// before PrepareHeaders was hooked, and any API request path that bypasses the
// UWRServerRequest PrepareHeaders method.
type InternalSetUrlFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
type SendWebRequestFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
static INTERNAL_SET_URL: OnceLock<GenericDetour<InternalSetUrlFn>> = OnceLock::new();
static SEND_WEB_REQUEST: OnceLock<GenericDetour<SendWebRequestFn>> = OnceLock::new();
static UNITY_GET_URL_METHOD: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static UNITY_INTERNAL_SET_URL_METHOD: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

pub fn apply(il2cpp: &'static Il2Cpp, cfg: &Lilypad) -> anyhow::Result<()> {
    let _ = IL2CPP.set(il2cpp);
    let _ = API_ORIGIN.set(url::origin(&cfg.api_base).to_string());

    // UWRServerRequest lives in the HTTPCommGenerated assembly, NOT HTTPComm
    // (which holds SignatureProvider/SystemMonitor). Both image names contain
    // "HTTPComm", so the image must be named exactly.
    let cls = il2cpp.class("HTTPCommGenerated", "HTTPComm", "UWRServerRequest");
    if cls.is_null() {
        anyhow::bail!("UWRServerRequest class not found");
    }

    // (1) Set the static BaseUri field to api_base now.
    let base_uri = il2cpp.field(cls, "BaseUri");
    if base_uri.is_null() {
        anyhow::bail!("UWRServerRequest.BaseUri field not found");
    }
    il2cpp.static_set_string(base_uri, &cfg.api_base);
    BASE_URI_FIELD.store(base_uri, Ordering::SeqCst);
    logging::line("INFO", &format!("BaseUri set to {}", cfg.api_base));

    // (2) Neutralize BaseUriForRouting (clear the dictionary so a per-route host
    //     override cannot bypass the redirect).
    let routing_field = il2cpp.field(cls, "BaseUriForRouting");
    if !routing_field.is_null() {
        let dict = il2cpp.static_get_ptr(routing_field);
        if !dict.is_null() {
            let dcls = il2cpp.object_class(dict);
            let clear = il2cpp.method(dcls, "Clear", 0);
            if !clear.is_null() {
                il2cpp.invoke(clear, dict, &mut []);
                logging::line("INFO", "BaseUriForRouting cleared");
            } else {
                logging::line("WARN", "BaseUriForRouting.Clear not found; left as-is");
            }
        }
    }

    // Resolve m_uri instance-field offset by name (for the per-request rewrite).
    let muri_field = il2cpp.field(cls, "m_uri");
    if !muri_field.is_null() {
        MURI_OFFSET.store(il2cpp.field_offset(muri_field), Ordering::SeqCst);
    } else {
        logging::line("WARN", "m_uri field not found; per-request URI rewrite disabled");
    }

    // (3) Hook the non-generic PrepareHeaders funnel.
    let prepare = il2cpp.method(cls, "PrepareHeaders", 1);
    if prepare.is_null() {
        logging::line(
            "WARN",
            "PrepareHeaders not found; relying on UnityWebRequest URL guard",
        );
    } else {
        let entry = il2cpp.method_entry(prepare);
        anyhow::ensure!(!entry.is_null(), "PrepareHeaders native entry is null");
        unsafe {
            let target: PrepareHeadersFn = core::mem::transmute(entry);
            crate::hook::install(&PREPARE_HEADERS, target, prepare_headers_detour as PrepareHeadersFn)?;
        }
        logging::line("HOOK", "PrepareHeaders: enabled");
    }

    match install_unity_url_guard(il2cpp) {
        Ok(()) => logging::line("HOOK", "UnityWebRequest URL guard: enabled"),
        Err(e) => logging::line(
            "WARN",
            &format!("UnityWebRequest URL guard not installed ({e}); relying on PrepareHeaders"),
        ),
    }
    Ok(())
}

/// Runs in every request's send path. Re-assert the static BaseUri and rewrite
/// this request's m_uri host so no stale host can leak.
unsafe extern "C" fn prepare_headers_detour(
    this: *mut c_void,
    request_headers: *mut c_void,
    method: *mut c_void,
) -> *mut c_void {
    let Some(detour) = PREPARE_HEADERS.get() else {
        return core::ptr::null_mut();
    };
    crate::hook::no_panic_void(|| {
        if let (Some(il2cpp), Some(origin)) = (IL2CPP.get(), API_ORIGIN.get()) {
            let f = BASE_URI_FIELD.load(Ordering::SeqCst);
            if !f.is_null() {
                il2cpp.static_set_string(f, &format!("{origin}/"));
            }
            let off = MURI_OFFSET.load(Ordering::SeqCst);
            if off != 0 && !this.is_null() {
                rewrite_muri_host(il2cpp, this, off, origin);
            }
        }
    });
    let result = detour.call(this, request_headers, method);
    crate::hook::no_panic_void(|| {
        if let Some(il2cpp) = IL2CPP.get() {
            inject_header(il2cpp, result, "x-lilypad-session", super::report::session_id());
        }
        if let (Some(il2cpp), Some(cid)) = (IL2CPP.get(), super::report::client_id()) {
            inject_header(il2cpp, result, "x-clientpatch-id", cid);
        }
    });
    result
}

/// Add `name: value` to a `Dictionary<string,string>` (the headers dictionary
/// `PrepareHeaders` returns). Uses the indexer setter `set_Item`, which
/// overwrites, so a repeat call is harmless. Non-fatal on any miss.
fn inject_header(il2cpp: &Il2Cpp, headers: *mut c_void, name: &str, value: &str) {
    if headers.is_null() {
        return;
    }
    let mut set_item = HEADER_SET_ITEM.load(Ordering::SeqCst);
    if set_item.is_null() {
        let cls = il2cpp.object_class(headers);
        set_item = il2cpp.method(cls, "set_Item", 2);
        if set_item.is_null() {
            logging::line(
                "WARN",
                "Dictionary.set_Item not found; x-clientpatch-id header not injected",
            );
            return;
        }
        HEADER_SET_ITEM.store(set_item, Ordering::SeqCst);
    }
    let key = il2cpp.new_string(name);
    let val = il2cpp.new_string(value);
    il2cpp.invoke(set_item, headers, &mut [key, val]);
}

/// Replace `self.m_uri` with a Uri pointing at our origin + the original path.
fn rewrite_muri_host(il2cpp: &Il2Cpp, this: *mut c_void, offset: u32, origin: &str) {
    unsafe {
        let slot = (this as *mut u8).add(offset as usize) as *mut *mut c_void;
        let uri_obj = *slot;
        if uri_obj.is_null() {
            return;
        }
        // path = uri.get_PathAndQuery()
        let uri_cls = il2cpp.object_class(uri_obj);
        let get_paq = il2cpp.method(uri_cls, "get_PathAndQuery", 0);
        if get_paq.is_null() {
            return;
        }
        let path_str = il2cpp.invoke(get_paq, uri_obj, &mut []);
        let path = il2cpp.string_to_rust(path_str);
        let new_url = url::rebuild_url(origin, &path);
        // Allocate the constructor argument FIRST, then the bare object, so no
        // GC-triggering allocation sits between object_new and .ctor.  A bare
        // (unconstructed) Il2CppObject is not safe to finalise or relocate; if
        // il2cpp_string_new (inside new_string) triggers a GC between object_new
        // and invoke(.ctor), Boehm's conservative stack scan *should* keep the
        // pointer alive, but reordering removes the window entirely.
        let ctor = il2cpp.method(uri_cls, ".ctor", 1);
        if ctor.is_null() {
            return;
        }
        let arg = il2cpp.new_string(&new_url);
        let new_obj = il2cpp.object_new(uri_cls);
        if new_obj.is_null() {
            return;
        }
        if il2cpp.invoke_ok(ctor, new_obj, &mut [arg]).is_none() {
            return;
        }
        *slot = new_obj;
        crate::status::mark_live("UWRServerRequest.m_uri");
        logging::line("REDIR", &new_url);
    }
}

fn install_unity_url_guard(il2cpp: &'static Il2Cpp) -> anyhow::Result<()> {
    let cls = il2cpp.class(
        "UnityEngine.UnityWebRequestModule",
        "UnityEngine.Networking",
        "UnityWebRequest",
    );
    if cls.is_null() {
        anyhow::bail!("UnityWebRequest class not found");
    }

    let get_url = {
        let m = il2cpp.method(cls, "GetUrl", 0);
        if m.is_null() {
            il2cpp.method(cls, "get_url", 0)
        } else {
            m
        }
    };
    if get_url.is_null() {
        anyhow::bail!("UnityWebRequest.GetUrl/get_url not found");
    }
    UNITY_GET_URL_METHOD.store(get_url, Ordering::SeqCst);

    let set_url = il2cpp.method(cls, "InternalSetUrl", 1);
    if set_url.is_null() {
        anyhow::bail!("UnityWebRequest.InternalSetUrl not found");
    }
    UNITY_INTERNAL_SET_URL_METHOD.store(set_url, Ordering::SeqCst);

    let send = il2cpp.method(cls, "SendWebRequest", 0);
    if send.is_null() {
        anyhow::bail!("UnityWebRequest.SendWebRequest not found");
    }

    unsafe {
        let set_entry = il2cpp.method_entry(set_url);
        if set_entry.is_null() {
            anyhow::bail!("UnityWebRequest.InternalSetUrl entry is null");
        }
        let set_target: InternalSetUrlFn = core::mem::transmute(set_entry);
        crate::hook::install(&INTERNAL_SET_URL, set_target, internal_set_url_detour as InternalSetUrlFn)?;

        let send_entry = il2cpp.method_entry(send);
        if send_entry.is_null() {
            anyhow::bail!("UnityWebRequest.SendWebRequest entry is null");
        }
        let send_target: SendWebRequestFn = core::mem::transmute(send_entry);
        crate::hook::install(&SEND_WEB_REQUEST, send_target, send_web_request_detour as SendWebRequestFn)?;
    }

    Ok(())
}

unsafe extern "C" fn internal_set_url_detour(
    this: *mut c_void,
    url: *mut c_void,
    method: *mut c_void,
) {
    let Some(detour) = INTERNAL_SET_URL.get() else {
        return;
    };
    let call_url = crate::hook::no_panic(url, || {
        let mut call_url = url;
        if let (Some(il2cpp), Some(origin)) = (IL2CPP.get(), API_ORIGIN.get()) {
            let current = il2cpp.string_to_rust(url);
            if let Some(rewritten) = url::rewrite_official_api_url(&current, origin) {
                call_url = il2cpp.new_string(&rewritten);
                logging::line("REDIR", &rewritten);
                crate::status::mark_live("UnityWebRequest.InternalSetUrl");
            } else if url::is_our_origin(&current, origin) {
                crate::status::mark_live("UnityWebRequest already on api_base");
            }
        }
        call_url
    });
    detour.call(this, call_url, method);
}

unsafe extern "C" fn send_web_request_detour(
    this: *mut c_void,
    method: *mut c_void,
) -> *mut c_void {
    let Some(detour) = SEND_WEB_REQUEST.get() else {
        return core::ptr::null_mut();
    };
    crate::hook::no_panic_void(|| {
        if let (Some(il2cpp), Some(origin)) = (IL2CPP.get(), API_ORIGIN.get()) {
            force_unity_request_url(il2cpp, this, origin);
        }
    });
    detour.call(this, method)
}

fn force_unity_request_url(il2cpp: &Il2Cpp, this: *mut c_void, origin: &str) {
    if this.is_null() {
        return;
    }

    let get_url = UNITY_GET_URL_METHOD.load(Ordering::SeqCst);
    let set_url = UNITY_INTERNAL_SET_URL_METHOD.load(Ordering::SeqCst);
    if get_url.is_null() || set_url.is_null() {
        return;
    }

    let current = il2cpp.invoke(get_url, this, &mut []);
    let current = il2cpp.string_to_rust(current);
    if let Some(rewritten) = url::rewrite_official_api_url(&current, origin) {
        let arg = il2cpp.new_string(&rewritten);
        if il2cpp.invoke_ok(set_url, this, &mut [arg]).is_none() {
            return;
        }
        logging::line("REDIR", &rewritten);
        crate::status::mark_live("UnityWebRequest.SendWebRequest");
    } else if url::is_our_origin(&current, origin) {
        crate::status::mark_live("UnityWebRequest already on api_base");
    }
}

/// Optional anti-tamper telemetry suppressor. Off by default.
pub fn neutralize_debugger(il2cpp: &Il2Cpp) {
    let cls = il2cpp.class("HTTPComm", "HTTPComm", "SystemMonitor");
    if cls.is_null() {
        logging::line("WARN", "SystemMonitor class not found");
        return;
    }
    let field = il2cpp.field(cls, "isDebuggerAttached");
    if field.is_null() {
        logging::line("WARN", "SystemMonitor.isDebuggerAttached field not found");
        return;
    }
    il2cpp.static_set_bool(field, false);
    logging::line("INFO", "SystemMonitor.isDebuggerAttached forced false");
}
