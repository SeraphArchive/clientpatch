//! Discard the official CDN maintenance result while Lilypad is enabled.
//!
//! Native evidence: GGLSequences.Prologue.CheckMaintenance0's completion calls
//! InMaintenance(2) before GGLInitialize; CheckMaintenance1 calls it with 1.
//! Both branches must remain open regardless of the downloaded announcement.
use crate::{il2cpp::Il2Cpp, logging};
use core::ffi::c_void;
use retour::GenericDetour;
use std::sync::OnceLock;

type InMaintenanceFn = unsafe extern "C" fn(*mut c_void, i32, *mut c_void) -> bool;
static IN_MAINTENANCE: OnceLock<GenericDetour<InMaintenanceFn>> = OnceLock::new();

pub fn apply(il2cpp: &Il2Cpp) -> anyhow::Result<()> {
    let cls = il2cpp.class("GGLSequences", "GGLSequences", "Prologue");
    anyhow::ensure!(!cls.is_null(), "GGLSequences.Prologue class not found");
    let method = il2cpp.method(cls, "InMaintenance", 1);
    anyhow::ensure!(!method.is_null(), "Prologue.InMaintenance not found");
    let entry = il2cpp.method_entry(method);
    anyhow::ensure!(!entry.is_null(), "Prologue.InMaintenance native entry is null");
    unsafe { install(core::mem::transmute::<*mut c_void, InMaintenanceFn>(entry))?; }
    logging::line("HOOK", "Prologue.InMaintenance: official maintenance ignored");
    Ok(())
}

unsafe fn install(target: InMaintenanceFn) -> anyhow::Result<()> {
    crate::hook::install(&IN_MAINTENANCE, target, in_maintenance_detour as InMaintenanceFn)
}

unsafe extern "C" fn in_maintenance_detour(
    _this: *mut c_void,
    _enabled_level: i32,
    _method: *mut c_void,
) -> bool {
    // No allocations or runtime calls: this FFI boundary cannot panic. Do not
    // mark the redirect LIVE; ignoring maintenance is not proof of a rewrite.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // A native fixture for the game's shared predicate. Exercise the patched
    // entry point, including its instance, argument and hidden MethodInfo ABI.
    #[inline(never)]
    unsafe extern "C" fn maintenance_fixture(
        this: *mut c_void,
        enabled_level: i32,
        _method: *mut c_void,
    ) -> bool {
        std::ptr::read_volatile(this.cast::<i32>()) >= enabled_level
    }

    #[test]
    fn discards_both_prologue_maintenance_levels_through_native_hook() {
        let target = std::hint::black_box(maintenance_fixture as InMaintenanceFn);
        let mut enabled = 2i32;
        let this = (&mut enabled as *mut i32).cast();
        let method = core::ptr::null_mut();
        unsafe {
            assert!(target(this, 2, method), "fixture must reproduce early maintenance");
            assert!(target(this, 1, method), "fixture must reproduce late maintenance");
            install(target).unwrap();
            install(target).unwrap();
            for level in [2, 1] {
                assert!(!target(this, level, method), "official maintenance level {level} blocked Lilypad");
            }
            std::ptr::write_volatile(this.cast::<i32>(), 0);
            assert!(!target(this, 1, method));
        }
    }
}
