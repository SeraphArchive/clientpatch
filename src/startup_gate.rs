//! Hold Unity's GameAssembly load until the native early pass has finished.
//! The IAT gate is armed before the bootstrap worker starts, without loading a
//! DLL, allocating, or waiting under DllMain's loader lock.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_READWRITE,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, Sleep};

#[link(name = "ntdll")]
extern "system" {
    fn RtlIsThreadWithinLoaderCallout() -> u8;
}

static READY: AtomicBool = AtomicBool::new(false);
static OWNER: AtomicU32 = AtomicU32::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static MISSED: AtomicBool = AtomicBool::new(false);
static RESOLVER_SLOT: AtomicUsize = AtomicUsize::new(0);
static WAITED: AtomicBool = AtomicBool::new(false);

struct Import {
    slots: [AtomicUsize; 4],
    original: AtomicUsize,
}
impl Import {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicUsize::new(0) }; 4],
            original: AtomicUsize::new(0),
        }
    }
}
static LOAD_W: Import = Import::new();
static LOAD_A: Import = Import::new();
static LOAD_EX_W: Import = Import::new();
static LOAD_EX_A: Import = Import::new();

/// Only live, OS-mapped PE32+ modules are accepted. Bound every RVA to the image;
/// no file parsing, heap allocation, DLL loading, or logging in this traversal.
unsafe fn imports(module: HMODULE, mut visit: impl FnMut(*mut usize)) -> bool {
    if module.is_null() {
        return false;
    }
    let base = module.cast::<u8>();
    let mut info: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
    if VirtualQuery(base.cast(), &mut info, core::mem::size_of_val(&info)) == 0
        || info.RegionSize < 64
        || base.cast::<u16>().read_unaligned() != 0x5a4d
    {
        return false;
    }
    let nt = base.add(60).cast::<u32>().read_unaligned() as usize;
    if nt.checked_add(152).is_none_or(|end| end > info.RegionSize)
        || base.add(nt).cast::<u32>().read_unaligned() != 0x4550
        || base.add(nt + 24).cast::<u16>().read_unaligned() != 0x20b
    {
        return false;
    }
    let size = base.add(nt + 80).cast::<u32>().read_unaligned() as usize;
    let rva = base.add(nt + 144).cast::<u32>().read_unaligned() as usize;
    let len = base.add(nt + 148).cast::<u32>().read_unaligned() as usize;
    if rva == 0 || rva.checked_add(len).is_none_or(|end| end > size) {
        return false;
    }
    for offset in (0..len.saturating_sub(19)).step_by(20) {
        let desc = base.add(rva + offset);
        let name = desc.add(12).cast::<u32>().read_unaligned();
        if name == 0 {
            break;
        }
        let thunk = desc.add(16).cast::<u32>().read_unaligned() as usize;
        if thunk == 0 || thunk % core::mem::align_of::<usize>() != 0 {
            continue;
        }
        for offset in (thunk..size.saturating_sub(7)).step_by(8) {
            let slot = base.add(offset).cast::<usize>();
            if slot.read_volatile() == 0 {
                break;
            }
            visit(slot);
        }
    }
    true
}

unsafe fn replace(slot: *mut usize, expected: usize, value: usize) -> bool {
    let mut protection = 0;
    if VirtualProtect(
        slot.cast(),
        core::mem::size_of::<usize>(),
        PAGE_READWRITE,
        &mut protection,
    ) == 0
    {
        return false;
    }
    let cell = &*slot.cast::<AtomicUsize>();
    let changed = cell
        .compare_exchange(expected, value, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let mut unused = 0;
    VirtualProtect(
        slot.cast(),
        core::mem::size_of::<usize>(),
        protection,
        &mut unused,
    );
    changed
}

unsafe fn arm_module(module: HMODULE) -> bool {
    let kernel = GetModuleHandleW(windows_sys::w!("kernel32.dll"));
    if kernel.is_null() || module.is_null() {
        return false;
    }
    let hooks = [
        (
            &LOAD_W,
            b"LoadLibraryW\0".as_slice(),
            load_w as *const () as usize,
        ),
        (
            &LOAD_A,
            b"LoadLibraryA\0".as_slice(),
            load_a as *const () as usize,
        ),
        (
            &LOAD_EX_W,
            b"LoadLibraryExW\0".as_slice(),
            load_ex_w as *const () as usize,
        ),
        (
            &LOAD_EX_A,
            b"LoadLibraryExA\0".as_slice(),
            load_ex_a as *const () as usize,
        ),
    ];
    for (hook, name, _) in &hooks {
        hook.original.store(
            GetProcAddress(kernel, name.as_ptr()).map_or(0, |p| p as usize),
            Ordering::Release,
        );
    }
    let resolver = GetProcAddress(kernel, b"GetProcAddress\0".as_ptr()).map_or(0, |p| p as usize);
    let mut installed = false;
    let mut complete = true;
    imports(module, |slot| {
        let value = slot.read_volatile();
        if value == resolver && resolver != 0 {
            // Doorstop 4.5 hooks a single resolver import. Ambiguous layouts
            // cannot be claimed as fully guarded by this startup boundary.
            if RESOLVER_SLOT.swap(slot as usize, Ordering::AcqRel) != 0 {
                complete = false;
            }
        }
        for (hook, _, detour) in &hooks {
            if value != 0 && value == hook.original.load(Ordering::Acquire) {
                if let Some(saved) = hook
                    .slots
                    .iter()
                    .find(|saved| saved.load(Ordering::Acquire) == 0)
                {
                    if replace(slot, value, *detour) {
                        saved.store(slot as usize, Ordering::Release);
                        installed = true;
                    } else {
                        complete = false;
                    }
                } else {
                    complete = false;
                }
            }
        }
    });
    if installed && complete {
        TARGET.store(module as usize, Ordering::Release);
    }
    installed && complete
}

pub(crate) unsafe fn arm() {
    // Already loaded means this boundary cannot establish pre-init ordering.
    MISSED.store(
        !GetModuleHandleW(windows_sys::w!("GameAssembly.dll")).is_null(),
        Ordering::Release,
    );
    arm_module(GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")));
}

/// Called on the worker before LoadLibrary(Doorstop), never from DllMain.
pub(crate) fn before_doorstop() -> anyhow::Result<()> {
    anyhow::ensure!(
        TARGET.load(Ordering::Acquire) != 0 && RESOLVER_SLOT.load(Ordering::Acquire) != 0,
        "UnityPlayer startup gate unavailable; Doorstop was not loaded"
    );
    anyhow::ensure!(!MISSED.load(Ordering::Acquire)
        && unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")).is_null() },
        "GameAssembly passed the startup gate before Doorstop was ready; a clean relaunch is required");
    Ok(())
}

/// A successful LoadLibrary is insufficient: Doorstop may have disabled itself
/// or failed to find Unity's IAT. Verify the resolver's owner without invoking
/// it (Doorstop initializes its IL2CPP bindings on the first real lookup).
pub(crate) fn verify_doorstop(doorstop: HMODULE) -> anyhow::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    let slot = RESOLVER_SLOT.load(Ordering::Acquire) as *const AtomicUsize;
    anyhow::ensure!(
        !slot.is_null(),
        "UnityPlayer GetProcAddress import unavailable"
    );
    let address = unsafe { (*slot).load(Ordering::Acquire) };
    let mut owner = core::ptr::null_mut();
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut owner,
        )
    };
    anyhow::ensure!(
        found != 0 && owner == doorstop,
        "Doorstop DLL loaded but UnityPlayer GetProcAddress hook is not armed"
    );
    anyhow::ensure!(
        !MISSED.load(Ordering::Acquire),
        "startup gate was bypassed or timed out"
    );
    anyhow::ensure!(
        unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")).is_null() },
        "GameAssembly loaded through an unguarded path while Doorstop was starting"
    );
    Ok(())
}

unsafe fn is_game_assembly<T: Copy + Into<u32>>(name: *const T) -> bool {
    if name.is_null() {
        return false;
    }
    let mut matched = 0;
    let expected = b"gameassembly.dll";
    // Win32 paths are NUL-terminated. Match the entire final component, without
    // allocation, accepting either slash and ASCII case for A/W entry points.
    for i in 0..32768 {
        let c: u32 = (*name.add(i)).into();
        if c == 0 {
            return matched == expected.len();
        }
        if c == 47 || c == 92 {
            matched = 0;
            continue;
        }
        if matched < expected.len() && c < 128 && (c as u8).eq_ignore_ascii_case(&expected[matched])
        {
            matched += 1;
        } else {
            matched = expected.len() + 1;
        }
    }
    false
}

unsafe fn wait_for_early_pass() {
    wait_for_early_pass_ms(120_000);
}

unsafe fn wait_for_early_pass_ms(timeout_ms: u64) {
    if READY.load(Ordering::Acquire) || OWNER.load(Ordering::Acquire) == GetCurrentThreadId() {
        return;
    }
    // Waiting here from a DLL initializer would prevent the worker from loading
    // Doorstop. Preserve the native call and report the lost ordering instead.
    if RtlIsThreadWithinLoaderCallout() != 0 {
        MISSED.store(true, Ordering::Release);
        return;
    }
    WAITED.store(true, Ordering::Release);
    let start = GetTickCount64();
    while !READY.load(Ordering::Acquire) {
        if GetTickCount64().wrapping_sub(start) >= timeout_ms {
            MISSED.store(true, Ordering::Release);
            crate::hook::no_panic_void(|| {
                crate::logging::line_try("ERR", "bepinex: startup gate timed out; GameAssembly released without verified startup ordering")
            });
            release();
            break;
        }
        Sleep(1);
    }
}

unsafe extern "system" fn load_w(name: *const u16) -> HMODULE {
    let error = GetLastError();
    if is_game_assembly(name) {
        wait_for_early_pass();
    }
    SetLastError(error);
    let original: unsafe extern "system" fn(*const u16) -> HMODULE =
        core::mem::transmute(LOAD_W.original.load(Ordering::Acquire));
    original(name)
}
unsafe extern "system" fn load_a(name: *const u8) -> HMODULE {
    let error = GetLastError();
    if is_game_assembly(name) {
        wait_for_early_pass();
    }
    SetLastError(error);
    let original: unsafe extern "system" fn(*const u8) -> HMODULE =
        core::mem::transmute(LOAD_A.original.load(Ordering::Acquire));
    original(name)
}
unsafe extern "system" fn load_ex_w(
    name: *const u16,
    file: *mut core::ffi::c_void,
    flags: u32,
) -> HMODULE {
    let error = GetLastError();
    if flags & 0x62 == 0 && is_game_assembly(name) {
        wait_for_early_pass();
    }
    SetLastError(error);
    let original: unsafe extern "system" fn(*const u16, *mut core::ffi::c_void, u32) -> HMODULE =
        core::mem::transmute(LOAD_EX_W.original.load(Ordering::Acquire));
    original(name, file, flags)
}
unsafe extern "system" fn load_ex_a(
    name: *const u8,
    file: *mut core::ffi::c_void,
    flags: u32,
) -> HMODULE {
    let error = GetLastError();
    if flags & 0x62 == 0 && is_game_assembly(name) {
        wait_for_early_pass();
    }
    SetLastError(error);
    let original: unsafe extern "system" fn(*const u8, *mut core::ffi::c_void, u32) -> HMODULE =
        core::mem::transmute(LOAD_EX_A.original.load(Ordering::Acquire));
    original(name, file, flags)
}

pub(crate) struct EarlyPass;
impl EarlyPass {
    pub(crate) fn begin() -> Self {
        OWNER.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        Self
    }
}
impl Drop for EarlyPass {
    fn drop(&mut self) {
        release();
        if WAITED.load(Ordering::Acquire) && !MISSED.load(Ordering::Acquire) {
            crate::logging::line_try(
                "DBG",
                "GameAssembly startup gate released after native early hooks",
            );
        }
    }
}

pub(crate) fn release() {
    // Several waiting callers can time out together. Only one may change IAT
    // page protections; overlapping VirtualProtect/restore pairs are unsafe.
    if READY.swap(true, Ordering::AcqRel) {
        return;
    }
    // Do not overwrite a later hook installed by another component.
    for (hook, detour) in [
        (&LOAD_W, load_w as *const () as usize),
        (&LOAD_A, load_a as *const () as usize),
        (&LOAD_EX_W, load_ex_w as *const () as usize),
        (&LOAD_EX_A, load_ex_a as *const () as usize),
    ] {
        for saved in &hook.slots {
            let slot = saved.load(Ordering::Acquire) as *mut usize;
            if !slot.is_null() {
                unsafe {
                    replace(slot, detour, hook.original.load(Ordering::Acquire));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::LibraryLoader::{
        LoadLibraryA, LoadLibraryExA, LoadLibraryExW, LoadLibraryW,
    };

    static ORIGINAL_RESOLVER: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn fixture_init() -> isize {
        1
    }
    unsafe extern "system" fn fixture_resolver(
        module: HMODULE,
        name: *const u8,
    ) -> windows_sys::Win32::Foundation::FARPROC {
        if name as usize > 0xffff
            && std::ffi::CStr::from_ptr(name.cast()).to_bytes() == b"il2cpp_init"
        {
            return Some(fixture_init);
        }
        let original: unsafe extern "system" fn(
            HMODULE,
            *const u8,
        )
            -> windows_sys::Win32::Foundation::FARPROC =
            core::mem::transmute(ORIGINAL_RESOLVER.load(Ordering::Acquire));
        original(module, name)
    }

    unsafe fn load_fixture(variant: &str) -> HMODULE {
        match variant {
            "a" => LoadLibraryA(b"GameAssembly.dll\0".as_ptr()),
            "ex_w" => LoadLibraryExW(
                windows_sys::w!("GameAssembly.dll"),
                core::ptr::null_mut(),
                0,
            ),
            "ex_a" => LoadLibraryExA(b"GameAssembly.dll\0".as_ptr(), core::ptr::null_mut(), 0),
            _ => LoadLibraryW(windows_sys::w!("GameAssembly.dll")),
        }
    }

    /// The child uses the real PE IAT and LoadLibrary boundary, with the worker
    /// deliberately held until Unity has attempted its first GameAssembly load.
    #[test]
    fn delayed_worker_probe() {
        let Some(root) = std::env::var_os("CLIENTPATCH_STARTUP_PROBE") else {
            return;
        };
        let root = PathBuf::from(root);
        let mode = std::env::var("CLIENTPATCH_STARTUP_CASE").unwrap();
        unsafe {
            windows_sys::Win32::System::Environment::SetCurrentDirectoryW(
                crate::wide::to_wide_nul(&root.to_string_lossy()).as_ptr(),
            );
        }
        let module = unsafe { GetModuleHandleW(core::ptr::null()) };
        let preloaded = if mode == "late" {
            unsafe { load_fixture("w") }
        } else {
            core::ptr::null_mut()
        };
        assert!(unsafe { arm_module(module) });
        assert!(
            verify_doorstop(module).is_err(),
            "unhooked import must not count as Doorstop success"
        );
        if mode == "late" {
            assert!(!preloaded.is_null());
            assert!(before_doorstop().is_err());
            release();
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(preloaded);
            }
            return;
        }
        assert!(before_doorstop().is_ok());
        if mode == "disabled" {
            {
                let _early = EarlyPass::begin();
            } // also the config-error/panic Drop path
            assert!(READY.load(Ordering::Acquire));
            let h = unsafe { load_fixture("w") };
            assert!(!h.is_null());
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(h);
            }
            return;
        }
        if mode == "timeout" {
            unsafe {
                wait_for_early_pass_ms(10);
            }
            assert!(READY.load(Ordering::Acquire) && MISSED.load(Ordering::Acquire));
            assert!(before_doorstop().is_err());
            return;
        }
        // Unrelated loads and data/resource mappings must not depend on setup.
        unsafe {
            let unrelated = LoadLibraryW(windows_sys::w!("kernel32.dll"));
            assert!(!unrelated.is_null());
            windows_sys::Win32::Foundation::FreeLibrary(unrelated);
            let data = LoadLibraryExW(
                windows_sys::w!("GameAssembly.dll"),
                core::ptr::null_mut(),
                2,
            );
            assert!(!data.is_null());
            windows_sys::Win32::Foundation::FreeLibrary(data);
        }
        assert!(!READY.load(Ordering::Acquire));
        let module_address = module as usize;
        let real_doorstop = mode == "real_doorstop";
        let attempted = std::sync::Arc::new(AtomicBool::new(false));
        let worker_flag = attempted.clone();
        let worker = std::thread::spawn(move || {
            let _pass = EarlyPass::begin();
            while !worker_flag.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(100));
            before_doorstop().unwrap();
            let doorstop = if real_doorstop {
                unsafe { LoadLibraryW(windows_sys::w!("doorstop.dll")) }
            } else {
                let slot = RESOLVER_SLOT.load(Ordering::Acquire) as *mut usize;
                let original = unsafe { slot.read_volatile() };
                ORIGINAL_RESOLVER.store(original, Ordering::Release);
                assert!(unsafe { replace(slot, original, fixture_resolver as *const () as usize) });
                module_address as HMODULE
            };
            assert!(!doorstop.is_null());
            verify_doorstop(doorstop).unwrap();
        });
        let others = if mode == "concurrent" {
            (0..7)
                .map(|_| {
                    std::thread::spawn(|| {
                        let h = unsafe { load_fixture("ex_a") };
                        assert!(!h.is_null());
                        assert!(READY.load(Ordering::Acquire));
                        unsafe {
                            windows_sys::Win32::Foundation::FreeLibrary(h);
                        }
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        attempted.store(true, Ordering::Release);
        let start = Instant::now();
        let h = unsafe { load_fixture(&mode) };
        assert!(!h.is_null(), "fixture load failed");
        assert!(
            READY.load(Ordering::Acquire),
            "GameAssembly escaped before the native early pass completed"
        );
        assert!(
            start.elapsed() >= Duration::from_millis(90),
            "startup wait was bypassed"
        );
        worker.join().unwrap();
        for other in others {
            other.join().unwrap();
        }
        if !real_doorstop {
            let init = unsafe { GetProcAddress(h, b"il2cpp_init\0".as_ptr()) }.unwrap();
            assert_eq!(
                init as usize, fixture_init as *const () as usize,
                "runtime resolver missed Doorstop"
            );
        }
        unsafe {
            windows_sys::Win32::Foundation::FreeLibrary(h);
        }
    }

    fn probe(mode: &str) {
        let root =
            std::env::temp_dir().join(format!("clientpatch-startup-{}-{mode}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut system = [0u16; 32768];
        let n = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                system.as_mut_ptr(),
                system.len() as u32,
            )
        };
        let source =
            PathBuf::from(String::from_utf16_lossy(&system[..n as usize])).join("version.dll");
        std::fs::copy(source, root.join("GameAssembly.dll")).unwrap();
        let exe = if mode == "real_doorstop" {
            let payload =
                std::env::var_os("CLIENTPATCH_DOORSTOP_PROBE_DLL").expect("supply Doorstop DLL");
            std::fs::copy(payload, root.join("doorstop.dll")).unwrap();
            std::fs::write(root.join("stub.dll"), b"fixture: never executed").unwrap();
            std::fs::write(
                root.join("doorstop_config.ini"),
                b"[General]\nenabled = true\ntarget_assembly = stub.dll\n",
            )
            .unwrap();
            let exe = root.join("HeavenBurnsRed.exe");
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
            exe
        } else {
            std::env::current_exe().unwrap()
        };
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "startup_gate::tests::delayed_worker_probe",
                "--nocapture",
            ])
            .env("CLIENTPATCH_STARTUP_PROBE", &root)
            .env("CLIENTPATCH_STARTUP_CASE", mode)
            .env_remove("DOORSTOP_DISABLE")
            .env_remove("DOORSTOP_INITIALIZED")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("startup gate deadlocked");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn delayed_worker_holds_gameassembly() {
        probe("w");
    }
    #[test]
    fn ansi_load_waits_for_doorstop() {
        probe("a");
    }
    #[test]
    fn extended_wide_load_waits_for_doorstop() {
        probe("ex_w");
    }
    #[test]
    fn extended_ansi_load_waits_for_doorstop() {
        probe("ex_a");
    }
    #[test]
    fn concurrent_loads_wait_for_doorstop() {
        probe("concurrent");
    }
    #[test]
    fn disabled_or_failed_config_releases_gate() {
        probe("disabled");
    }
    #[test]
    fn timeout_releases_gate_without_claiming_success() {
        probe("timeout");
    }
    #[test]
    fn already_loaded_runtime_rejects_late_doorstop() {
        probe("late");
    }

    #[test]
    #[ignore = "requires CLIENTPATCH_DOORSTOP_PROBE_DLL; runs in an isolated fixture process"]
    fn real_doorstop_arms_before_runtime_load() {
        probe("real_doorstop");
    }

    #[test]
    fn only_exact_gameassembly_basename_waits() {
        for path in [
            "GameAssembly.dll",
            r"C:\fixture\GAMEASSEMBLY.DLL",
            "./GameAssembly.dll",
        ] {
            let wide = crate::wide::to_wide_nul(path);
            let ansi = std::ffi::CString::new(path).unwrap();
            assert!(unsafe { is_game_assembly(wide.as_ptr()) });
            assert!(unsafe { is_game_assembly(ansi.as_ptr().cast::<u8>()) });
        }
        for path in [
            "",
            "GameAssembly.dll.bak",
            "GameAssembly.dll/other.dll",
            "OtherGameAssembly.dll",
            "GameAssembly",
        ] {
            assert!(!unsafe { is_game_assembly(crate::wide::to_wide_nul(path).as_ptr()) });
        }
        assert!(!unsafe { is_game_assembly::<u16>(core::ptr::null()) });
    }
}
