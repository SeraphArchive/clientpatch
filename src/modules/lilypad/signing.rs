//! Response-signature handling. By-name IL2CPP only.
//!  - noop: force base SignatureProvider.Verify(headers, bytes) -> true.
//!  - rsa : return the configured key from Lily.SignatureProvider.ServerPublicKey().
use crate::config::Lilypad;
use crate::il2cpp::Il2Cpp;
use crate::logging;
use crate::modules::lilypad::keyconv;
use core::ffi::c_void;
use retour::GenericDetour;
use std::sync::OnceLock;

static SERVER_KEY: OnceLock<String> = OnceLock::new();
static IL2CPP: OnceLock<&'static Il2Cpp> = OnceLock::new();

type VerifyFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> bool;
type ServerPublicKeyFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;

// GenericDetour (stable Rust) kept alive in statics. These detours return a
// constant / fresh value and never call the original, but the hook must stay
// installed for the process lifetime — so the detour must not be dropped.
static VERIFY: OnceLock<GenericDetour<VerifyFn>> = OnceLock::new();
static SERVER_PUBLIC_KEY: OnceLock<GenericDetour<ServerPublicKeyFn>> = OnceLock::new();

pub fn apply(il2cpp: &'static Il2Cpp, cfg: &Lilypad, game_dir: &std::path::Path) -> anyhow::Result<()> {
    match cfg.signing.as_str() {
        "noop" => apply_noop(il2cpp),
        "rsa" => apply_rsa(il2cpp, cfg, game_dir),
        other => anyhow::bail!("unknown signing mode {other:?}"),
    }
}

/// noop: force base SignatureProvider.Verify(headers, bytes) -> true.
fn apply_noop(il2cpp: &Il2Cpp) -> anyhow::Result<()> {
    let cls = il2cpp.class("HTTPComm", "HTTPComm", "SignatureProvider");
    if cls.is_null() {
        anyhow::bail!("HTTPComm.SignatureProvider class not found");
    }
    let verify = il2cpp.method(cls, "Verify", 2);
    if verify.is_null() {
        anyhow::bail!("SignatureProvider.Verify(2) not found");
    }
    let entry = il2cpp.method_entry(verify);
    anyhow::ensure!(!entry.is_null(), "SignatureProvider.Verify native entry is null");
    unsafe {
        let target: VerifyFn = core::mem::transmute(entry);
        crate::hook::install(&VERIFY, target, verify_detour as VerifyFn)?;
    }
    logging::line("HOOK", "SignatureProvider.Verify -> true (noop)");
    Ok(())
}

unsafe extern "C" fn verify_detour(
    _this: *mut c_void,
    _headers: *mut c_void,
    _bytes: *mut c_void,
    _m: *mut c_void,
) -> bool {
    true
}

/// rsa: return the configured public key from Lily.SignatureProvider.ServerPublicKey().
fn apply_rsa(il2cpp: &'static Il2Cpp, cfg: &Lilypad, game_dir: &std::path::Path) -> anyhow::Result<()> {
    // A relative path keeps its subdirectory; an absolute one keeps only its
    // file name, so the key cannot be read from outside <game dir>/clientpatch.
    let p = std::path::Path::new(&cfg.server_public_key_pem);
    let rel = if p.is_absolute() {
        std::path::PathBuf::from(p.file_name().unwrap_or_default())
    } else {
        p.to_path_buf()
    };
    let key_path = crate::module::resolve_data_path(game_dir, &rel);
    let pem = std::fs::read_to_string(&key_path).map_err(|e| {
        anyhow::anyhow!(
            "read server_public_key_pem {:?}: {e}",
            key_path
        )
    })?;
    let key_str = keyconv::client_public_key_string(&pem)?;
    let _ = SERVER_KEY.set(key_str);
    let _ = IL2CPP.set(il2cpp);

    let cls = il2cpp.class("Assembly-CSharp", "Lily", "SignatureProvider");
    if cls.is_null() {
        anyhow::bail!("Lily.SignatureProvider class not found");
    }
    let m = il2cpp.method(cls, "ServerPublicKey", 0);
    if m.is_null() {
        anyhow::bail!("Lily.SignatureProvider.ServerPublicKey() not found");
    }
    let entry = il2cpp.method_entry(m);
    anyhow::ensure!(!entry.is_null(), "ServerPublicKey native entry is null");
    unsafe {
        let target: ServerPublicKeyFn = core::mem::transmute(entry);
        crate::hook::install(&SERVER_PUBLIC_KEY, target, server_pubkey_detour as ServerPublicKeyFn)?;
    }
    logging::line(
        "HOOK",
        "Lily.SignatureProvider.ServerPublicKey -> configured key (rsa)",
    );
    Ok(())
}

unsafe extern "C" fn server_pubkey_detour(_this: *mut c_void, _m: *mut c_void) -> *mut c_void {
    crate::hook::no_panic(core::ptr::null_mut(), || {
        if let (Some(il2cpp), Some(key)) = (IL2CPP.get(), SERVER_KEY.get()) {
            return il2cpp.new_string(key);
        }
        core::ptr::null_mut()
    })
}
