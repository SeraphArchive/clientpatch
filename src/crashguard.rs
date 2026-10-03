//! Always-on crash window + UnityCrashHandler64 suppression (no config).
//!
//! Two jobs:
//!   1. Never let `UnityCrashHandler64.exe` spawn (Steam restores that exe
//!      every update). Hook CreateProcessW/A on kernelbase — kernel32 is a
//!      jmp forwarder that retour cannot patch reliably.
//!   2. Show a crash window with the exception + recent clientpatch log, then
//!      terminate. Unity installs a *vectored* handler that swallows the
//!      crash before `SetUnhandledExceptionFilter` ever runs, so UEF alone
//!      (even `reassert()`'d) is not enough. We register a first VEH for
//!      fatal codes and also hook `SetUnhandledExceptionFilter` so Unity
//!      cannot replace our last-chance filter.
use crate::ffi::{ensure_module, export};
use crate::logging;
use crate::wide;
use core::ffi::c_void;
use retour::GenericDetour;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleA, GetModuleHandleExW, GetProcAddress,
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, MB_SYSTEMMODAL,
};

/// True if an application name or command line refers to Unity's crash reporter.
pub fn is_unity_crash_handler(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("unitycrashhandler")
}

// VEH runs BEFORE every SEH / VEH Unity and CrackProof installed. We only
// swallow codes that are never used as anti-tamper traps. CrackProof in
// GameAssembly regularly raises PRIVILEGED_INSTRUCTION (0xC0000096) and
// similar "ud2 / cli / hlt" traps that ITS OWN later handler recovers
// from — claiming those as fatal is what the first VEH pass just did.
// ACCESS_VIOLATION is deliberately NOT here: a first-chance one is routine
// (guard-page stack growth, IsBadReadPtr probes, a plugin catching its own),
// and terminating from the VEH means the handler that would have recovered
// never runs. A genuine unhandled one still reaches crash_filter below.
const FATAL: &[u32] = &[
    0xC000_00FD, // STACK_OVERFLOW
];

fn is_fatal(code: u32) -> bool {
    FATAL.contains(&code)
}

fn exception_name(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "ACCESS_VIOLATION",
        0xC000_00FD => "STACK_OVERFLOW",
        0xC000_001D => "ILLEGAL_INSTRUCTION",
        0xC000_0094 => "INTEGER_DIVIDE_BY_ZERO",
        0xC000_0096 => "PRIVILEGED_INSTRUCTION",
        0xC000_0006 => "IN_PAGE_ERROR",
        0xC000_0409 => "STACK_BUFFER_OVERRUN",
        0xC000_0374 => "HEAP_CORRUPTION",
        0x8000_0003 => "BREAKPOINT",
        _ => "exception",
    }
}

pub fn install() {
    install_create_process();
    install_unhandled_filter_hook();
    install_vectored();
    reassert();
    logging::line(
        "INFO",
        "crashguard: crash window on; UnityCrashHandler64 blocked",
    );
}

/// Re-assert our unhandled filter + put our VEH first again. UnityPlayer
/// (and later subsystems) install their own after startup.
pub fn reassert() {
    unsafe { SetUnhandledExceptionFilter(Some(crash_filter)) };
    // Remove + re-add first so we stay ahead of anything Unity registered.
    reinstall_vectored();
}

/// After init is done: spawn a thread that null-derefs in ~3s so the crash
/// window + CreateProcess block can be verified without a real game crash.
pub fn schedule_test_crash() {
    logging::line(
        "WARN",
        "crashguard: CLIENTPATCH_CRASH_TEST — deliberate AV in ~3s",
    );
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        logging::line("WARN", "crashguard: firing test ACCESS_VIOLATION now");
        unsafe {
            let p = core::ptr::null_mut::<u32>();
            core::ptr::write_volatile(p, 0xDEAD);
        }
    });
}

// --- CreateProcess block ---

type CreateProcessWFn = unsafe extern "system" fn(
    *const u16,
    *mut u16,
    *const c_void,
    *const c_void,
    i32,
    u32,
    *const c_void,
    *const u16,
    *const c_void,
    *mut c_void,
) -> i32;
type CreateProcessAFn = unsafe extern "system" fn(
    *const u8,
    *mut u8,
    *const c_void,
    *const c_void,
    i32,
    u32,
    *const c_void,
    *const u8,
    *const c_void,
    *mut c_void,
) -> i32;

static CPW: OnceLock<GenericDetour<CreateProcessWFn>> = OnceLock::new();
static CPA: OnceLock<GenericDetour<CreateProcessAFn>> = OnceLock::new();

fn install_create_process() {
    for dll in ["kernelbase.dll", "kernel32.dll"] {
        let h = ensure_module(dll);
        if h.is_null() {
            continue;
        }
        unsafe {
            if CPW.get().is_none() {
                if let Some(p) = export(h, "CreateProcessW") {
                    let t: CreateProcessWFn = core::mem::transmute(p);
                    if crate::hook::install(&CPW, t, cpw_detour as CreateProcessWFn).is_ok() {
                        logging::line("HOOK", &format!("CreateProcessW ({dll}): crash-handler block"));
                    }
                }
            }
            if CPA.get().is_none() {
                if let Some(p) = export(h, "CreateProcessA") {
                    let t: CreateProcessAFn = core::mem::transmute(p);
                    if crate::hook::install(&CPA, t, cpa_detour as CreateProcessAFn).is_ok() {
                        logging::line("HOOK", &format!("CreateProcessA ({dll}): crash-handler block"));
                    }
                }
            }
        }
        if CPW.get().is_some() {
            break;
        }
    }
    if CPW.get().is_none() {
        logging::line("ERR", "crashguard: could not hook CreateProcessW");
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn cpw_detour(
    app: *const u16,
    cmd: *mut u16,
    pa: *const c_void,
    ta: *const c_void,
    inherit: i32,
    flags: u32,
    env: *const c_void,
    cwd: *const u16,
    si: *const c_void,
    pi: *mut c_void,
) -> i32 {
    let Some(d) = CPW.get() else {
        return 0;
    };
    let block = crate::hook::no_panic(false, || {
        let app_s = if app.is_null() {
            String::new()
        } else {
            wide::from_wide_nul(app)
        };
        let cmd_s = if cmd.is_null() {
            String::new()
        } else {
            wide::from_wide_nul(cmd)
        };
        is_unity_crash_handler(&app_s) || is_unity_crash_handler(&cmd_s)
    });
    if block {
        logging::line_try("INFO", "crashguard: blocked UnityCrashHandler64 spawn");
        return 0;
    }
    d.call(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn cpa_detour(
    app: *const u8,
    cmd: *mut u8,
    pa: *const c_void,
    ta: *const c_void,
    inherit: i32,
    flags: u32,
    env: *const c_void,
    cwd: *const u8,
    si: *const c_void,
    pi: *mut c_void,
) -> i32 {
    let Some(d) = CPA.get() else {
        return 0;
    };
    let block = crate::hook::no_panic(false, || {
        let app_s = ansi_to_string(app);
        let cmd_s = ansi_to_string(cmd);
        is_unity_crash_handler(&app_s) || is_unity_crash_handler(&cmd_s)
    });
    if block {
        logging::line_try("INFO", "crashguard: blocked UnityCrashHandler64 spawn");
        return 0;
    }
    d.call(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi)
}

fn ansi_to_string(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p as *const i8) }
        .to_string_lossy()
        .into_owned()
}

// --- SetUnhandledExceptionFilter stay-on-top ---

type SuefFn = unsafe extern "system" fn(
    Option<unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32>,
) -> Option<unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32>;
static SUEF: OnceLock<GenericDetour<SuefFn>> = OnceLock::new();

fn install_unhandled_filter_hook() {
    for dll in ["kernelbase.dll", "kernel32.dll"] {
        let h = unsafe { GetModuleHandleA(std::ffi::CString::new(dll).unwrap().as_ptr() as *const u8) };
        if h.is_null() {
            continue;
        }
        unsafe {
            if let Some(p) = export(h, "SetUnhandledExceptionFilter") {
                let t: SuefFn = core::mem::transmute(p);
                if crate::hook::install(&SUEF, t, suef_detour as SuefFn).is_ok() {
                    logging::line(
                        "HOOK",
                        &format!("SetUnhandledExceptionFilter ({dll}): stay-on-top"),
                    );
                    return;
                }
            }
        }
    }
    logging::line("WARN", "crashguard: could not hook SetUnhandledExceptionFilter");
}

unsafe extern "system" fn suef_detour(
    _incoming: Option<unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32>,
) -> Option<unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32> {
    // Unity (and anyone else) thinks they installed a filter. We keep ours.
    if let Some(d) = SUEF.get() {
        return d.call(Some(crash_filter));
    }
    Some(crash_filter)
}

// --- Vectored handler (beats Unity's VEH) ---

type AddVehFn = unsafe extern "system" fn(u32, *const c_void) -> *mut c_void;
type RemoveVehFn = unsafe extern "system" fn(*mut c_void) -> i32;

static ADD_VEH: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static REMOVE_VEH: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static VEH_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

fn veh_procs() -> Option<(AddVehFn, RemoveVehFn)> {
    let add = ADD_VEH.load(Ordering::SeqCst);
    let rem = REMOVE_VEH.load(Ordering::SeqCst);
    if add.is_null() || rem.is_null() {
        let k32: HMODULE = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr() as *const u8) };
        if k32.is_null() {
            return None;
        }
        unsafe {
            let a = GetProcAddress(k32, c"AddVectoredExceptionHandler".as_ptr() as *const u8)?;
            let r = GetProcAddress(k32, c"RemoveVectoredExceptionHandler".as_ptr() as *const u8)?;
            ADD_VEH.store(a as *mut c_void, Ordering::SeqCst);
            REMOVE_VEH.store(r as *mut c_void, Ordering::SeqCst);
            Some((
                core::mem::transmute_copy(&a),
                core::mem::transmute_copy(&r),
            ))
        }
    } else {
        unsafe {
            Some((
                core::mem::transmute_copy(&add),
                core::mem::transmute_copy(&rem),
            ))
        }
    }
}

fn install_vectored() {
    reinstall_vectored();
}

fn reinstall_vectored() {
    let Some((add, remove)) = veh_procs() else {
        logging::line("WARN", "crashguard: AddVectoredExceptionHandler not found");
        return;
    };
    unsafe {
        let prev = VEH_HANDLE.swap(core::ptr::null_mut(), Ordering::SeqCst);
        if !prev.is_null() {
            let _ = remove(prev);
        }
        // First=1: call us before anyone else (including UnityPlayer).
        let h = add(1, veh_handler as *const c_void);
        VEH_HANDLE.store(h, Ordering::SeqCst);
    }
}

unsafe extern "system" fn veh_handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let code = exception_code(info);
    if !is_fatal(code) {
        return 0; // EXCEPTION_CONTINUE_SEARCH
    }
    handle_crash(info);
    1 // EXCEPTION_EXECUTE_HANDLER (unreached after TerminateProcess)
}

unsafe extern "system" fn crash_filter(info: *const EXCEPTION_POINTERS) -> i32 {
    handle_crash(info);
    1
}

fn exception_code(info: *const EXCEPTION_POINTERS) -> u32 {
    unsafe {
        if info.is_null() || (*info).ExceptionRecord.is_null() {
            0
        } else {
            (*(*info).ExceptionRecord).ExceptionCode as u32
        }
    }
}

fn exception_addr(info: *const EXCEPTION_POINTERS) -> *mut c_void {
    unsafe {
        if info.is_null() || (*info).ExceptionRecord.is_null() {
            core::ptr::null_mut()
        } else {
            (*(*info).ExceptionRecord).ExceptionAddress
        }
    }
}

fn module_at(addr: *mut c_void) -> String {
    if addr.is_null() {
        return String::new();
    }
    unsafe {
        let mut h: HMODULE = core::ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            addr as *const u16,
            &mut h,
        ) == 0
            || h.is_null()
        {
            return String::new();
        }
        let mut buf = [0u16; 520];
        let n = GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) as usize;
        if n == 0 {
            return String::new();
        }
        let path = wide::utf16_to_string(&buf[..n.min(buf.len())]);
        path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string()
    }
}

fn handle_crash(info: *const EXCEPTION_POINTERS) {
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::SeqCst) {
        unsafe { TerminateProcess(GetCurrentProcess(), exception_code(info)) };
        return;
    }
    let code = exception_code(info);
    let addr = exception_addr(info);
    let name = exception_name(code);
    let module = module_at(addr);
    // Snapshot the status before any log call: mark_failed logs while holding the
    // reason lock, so reading it after logging here inverts the lock order.
    let redirect = crate::status::state();
    let reason = crate::status::reason();
    logging::line_try(
        "CRASH",
        &format!("0x{code:08X} {name} at {addr:p} module={module} status={redirect:?}"),
    );

    // Stack overflow: MessageBox needs stack. Just die.
    if code == 0xC000_00FD {
        unsafe { TerminateProcess(GetCurrentProcess(), code) };
        return;
    }

    let reason_line = if reason.is_empty() {
        String::new()
    } else {
        format!("reason = {reason}\n")
    };
    let body = format!(
        "clientpatch caught a crash.\n\n\
         code = 0x{code:08X} ({name})\n\
         address = {addr:p}\n\
         module = {module}\n\
         redirect = {redirect:?}\n\
         {reason_line}\
         The game will close after you press OK.\n\n\
         \u{2500}\u{2500} recent clientpatch log \u{2500}\u{2500}\n{}",
        logging::recent()
    );
    let wbody: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    let wtitle: Vec<u16> = "clientpatch \u{2014} crash"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            core::ptr::null_mut(),
            wbody.as_ptr(),
            wtitle.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SYSTEMMODAL,
        );
        TerminateProcess(GetCurrentProcess(), code);
    }
}

#[cfg(test)]
mod tests {
    use super::is_unity_crash_handler;

    #[test]
    fn matches_crash_handler_exe_and_cmdline() {
        assert!(is_unity_crash_handler(
            r"E:\SteamLibrary\steamapps\common\HeavenBurnsRed\UnityCrashHandler64.exe"
        ));
        assert!(is_unity_crash_handler(
            r#""C:\game\UnityCrashHandler64.exe" -parentpid 1234"#
        ));
        assert!(is_unity_crash_handler("unitycrashhandler32.exe"));
        assert!(!is_unity_crash_handler("HeavenBurnsRed.exe"));
        assert!(!is_unity_crash_handler(""));
    }
}
