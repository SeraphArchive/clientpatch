//! Shared Win32 FFI helpers for the native hook modules: resolve an export by
//! name (for `transmute` to a fn pointer) and ensure a DLL is loaded. Used by
//! every `#[cfg(windows)] mod hooks` block.
use core::ffi::c_void;
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};

/// `GetProcAddress` + cast to a raw pointer, ready to `transmute` into a fn type.
///
/// # Safety
/// `module` must be a valid module handle; the returned pointer is only sound to
/// call after transmuting to the export's true signature.
pub(crate) unsafe fn export(module: HMODULE, name: &str) -> Option<*const c_void> {
    let c = std::ffi::CString::new(name).ok()?;
    GetProcAddress(module, c.as_ptr() as *const u8).map(|f| f as *const c_void)
}

/// Handle for an already-loaded DLL, loading it if necessary. Returns null only
/// if the DLL is neither loaded nor loadable.
pub(crate) fn ensure_module(name: &str) -> HMODULE {
    let c = std::ffi::CString::new(name).unwrap();
    unsafe {
        let h = GetModuleHandleA(c.as_ptr() as *const u8);
        if !h.is_null() {
            return h;
        }
        LoadLibraryA(c.as_ptr() as *const u8)
    }
}
