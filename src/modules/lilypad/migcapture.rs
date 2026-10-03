//! Debug capture (temporary, gated by `capture_md5`): recover the 引き継ぎ
//! migration-password pre-image.
//!
//! v2: the migration digest is MD5 but was MISSED in v1 because the v1 input
//! filter (≤128 bytes) dropped it — the pre-image is the password plus the 96-hex
//! app secret (and maybe appId/code), which exceeds 128 bytes. So v2:
//!   * raises the input cap to MAX_LOG,
//!   * logs the `this` pointer on both hooks for correlation,
//!   * ALSO hooks HashFinal to log the OUTPUT digest, so the call whose output ==
//!     the observed wire digest can be matched directly and its HashCore input
//!     (same `this`) read off as the exact pre-image.
//!     If no HashFinal output ever equals the digest, the hash is NOT
//!     MD5CryptoServiceProvider (custom/native) and we escalate to native hooking.
use crate::il2cpp::Il2Cpp;
use crate::logging;
use core::ffi::c_void;
use retour::GenericDetour;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

static IL2CPP: OnceLock<&'static Il2Cpp> = OnceLock::new();

// protected override void HashCore(byte[] array, int ibStart, int cbSize)
type HashCoreFn = unsafe extern "C" fn(*mut c_void, *mut c_void, i32, i32, *mut c_void);
static HASHCORE: OnceLock<GenericDetour<HashCoreFn>> = OnceLock::new();
// protected override byte[] HashFinal()
type HashFinalFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
static HASHFINAL: OnceLock<GenericDetour<HashFinalFn>> = OnceLock::new();

/// Pre-image is small (password + 96-hex secret + ids < ~256B); cap well above that
/// but below typical streamed asset-hash chunks.
const MAX_LOG: usize = 1024;
const MAX_LINES: u32 = 8000;
static LINES: AtomicU32 = AtomicU32::new(0);

pub fn apply(il2cpp: &'static Il2Cpp) -> anyhow::Result<()> {
    let _ = IL2CPP.set(il2cpp);

    let cls = {
        let c = il2cpp.class_corlib("System.Security.Cryptography", "MD5CryptoServiceProvider");
        if c.is_null() {
            il2cpp.class(
                "mscorlib",
                "System.Security.Cryptography",
                "MD5CryptoServiceProvider",
            )
        } else {
            c
        }
    };
    if cls.is_null() {
        logging::line(
            "WARN",
            "[capture_md5] MD5CryptoServiceProvider class not found",
        );
        return Ok(());
    }

    if let Some(m) = nonnull(il2cpp.method(cls, "HashCore", 3)) {
        let entry = il2cpp.method_entry(m);
        if !entry.is_null() {
            unsafe {
                let target: HashCoreFn = core::mem::transmute(entry);
                if crate::hook::install(&HASHCORE, target, hashcore_detour as HashCoreFn).is_ok() {
                    logging::line("HOOK", "[capture_md5] MD5.HashCore enabled");
                }
            }
        }
    } else {
        logging::line("WARN", "[capture_md5] HashCore(byte[],int,int) not found");
    }

    if let Some(m) = nonnull(il2cpp.method(cls, "HashFinal", 0)) {
        let entry = il2cpp.method_entry(m);
        if !entry.is_null() {
            unsafe {
                let target: HashFinalFn = core::mem::transmute(entry);
                if crate::hook::install(&HASHFINAL, target, hashfinal_detour as HashFinalFn).is_ok()
                {
                    logging::line("HOOK", "[capture_md5] MD5.HashFinal enabled");
                }
            }
        }
    } else {
        logging::line("WARN", "[capture_md5] HashFinal() not found");
    }

    logging::line(
        "HOOK",
        "[capture_md5] do ONE 引き継ぎ; match HashFinal out=<digest> to its HashCore this=",
    );
    Ok(())
}

fn nonnull(p: *mut c_void) -> Option<*mut c_void> {
    if p.is_null() {
        None
    } else {
        Some(p)
    }
}

unsafe extern "C" fn hashcore_detour(
    this: *mut c_void,
    array: *mut c_void,
    ib_start: i32,
    cb_size: i32,
    method: *mut c_void,
) {
    crate::hook::no_panic_void(|| {
        if let Some(il2cpp) = IL2CPP.get() {
            let start = ib_start.max(0) as usize;
            let count = cb_size.max(0) as usize;
            if count > 0 && count <= MAX_LOG && LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
                let bytes = il2cpp.array_bytes(array, start, count);
                logging::line(
                    "MD5CAP",
                    &format!(
                        "HashCore this={:p} len={} hex={} ascii={}",
                        this,
                        bytes.len(),
                        hex(&bytes),
                        ascii(&bytes)
                    ),
                );
            }
        }
    });
    if let Some(d) = HASHCORE.get() {
        d.call(this, array, ib_start, cb_size, method);
    }
}

unsafe extern "C" fn hashfinal_detour(this: *mut c_void, method: *mut c_void) -> *mut c_void {
    // The original must run even if our state is not ready yet: returning null
    // here makes the caller's digest read fault.
    let Some(d) = HASHFINAL.get() else {
        return core::ptr::null_mut();
    };
    let ret = d.call(this, method);
    crate::hook::no_panic_void(|| {
        if let Some(il2cpp) = IL2CPP.get() {
            if LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
                let out = il2cpp.array_bytes(ret, 0, 16);
                logging::line(
                    "MD5CAP",
                    &format!("HashFinal this={this:p} out={}", hex(&out)),
                );
            }
        }
    });
    ret
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn ascii(b: &[u8]) -> String {
    b.iter()
        .map(|&x| {
            if (0x20..0x7f).contains(&x) {
                x as char
            } else {
                '.'
            }
        })
        .collect()
}
