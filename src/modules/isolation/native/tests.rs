use super::*;
use std::process::Command;
use windows_sys::Win32::System::Registry::*;

unsafe fn open_named_pipe_directory_with_input_length() {
    let mut path: Vec<u16> = r"\Device\NamedPipe\".encode_utf16().collect();
    let mut name = UnicodeString { length: (path.len()*2) as u16, maximum_length: 0, buffer: path.as_mut_ptr() };
    let attrs = ObjectAttributes { length: std::mem::size_of::<ObjectAttributes>() as u32, root: std::ptr::null_mut(), name: &mut name, attributes: 0x40, security_descriptor: std::ptr::null_mut(), security_qos: std::ptr::null_mut() };
    let mut file = std::ptr::null_mut();
    let mut io = [0usize; 2];
    let open: OpenFile = symbol("NtOpenFile").unwrap();
    assert_eq!(open(&mut file,0x100080,&attrs,io.as_mut_ptr().cast(),3,0x20),0,"input MaximumLength must not reject a native named-pipe open");
    CloseHandle(file);
}

unsafe fn anonymous_pipe_roundtrip() {
    #[link(name = "kernel32")]
    extern "system" { fn CreatePipe(read: *mut Handle, write: *mut Handle, security: *const c_void, size: u32) -> i32; }
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    assert_ne!(CreatePipe(&mut read, &mut write, std::ptr::null(), 0), 0,
        "anonymous IPC uses an empty name relative to a pipe handle");
    let sent = [42u8];
    let mut received = [0u8];
    use std::{io::{Read, Write}, os::windows::io::FromRawHandle};
    let mut writer = std::fs::File::from_raw_handle(write);
    let mut reader = std::fs::File::from_raw_handle(read);
    writer.write_all(&sent).unwrap();
    reader.read_exact(&mut received).unwrap();
    assert_eq!(received, sent);
}

/// Local compatibility probe: real Steam client required, no game storage mapped.
#[test]
#[ignore = "requires an explicitly supplied Steam API DLL and running Steam client"]
fn steam_client_probe() {
    let dll = std::env::var("CLIENTPATCH_STEAM_PROBE_DLL").expect("supply Steam API DLL");
    if std::env::var_os("CLIENTPATCH_STEAM_PROBE_ISOLATION").is_some() {
        let state = State {
            files: Vec::new(), registry: Vec::new(),
            query_object: unsafe { symbol("NtQueryObject").unwrap() },
            query_key: unsafe { symbol("NtQueryKey").unwrap() },
        };
        install_state(state).unwrap();
    }
    unsafe {
        let module = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(crate::wide::to_wide_nul(&dll).as_ptr());
        assert!(!module.is_null());
        let init: unsafe extern "C" fn() -> u8 = core::mem::transmute(crate::ffi::export(module,"SteamAPI_Init").unwrap());
        let shutdown: unsafe extern "C" fn() = core::mem::transmute(crate::ffi::export(module,"SteamAPI_Shutdown").unwrap());
        let start = std::time::Instant::now();
        let result = init();
        eprintln!("SteamAPI_Init result={result} elapsed_ms={}", start.elapsed().as_millis());
        shutdown();
        assert_ne!(result, 0, "native Steam initialization failed");
    }
}

unsafe fn key(root: HKEY, name: &str) -> HKEY {
    let mut out = std::ptr::null_mut();
    assert_eq!(
        RegCreateKeyExW(
            root,
            crate::wide::to_wide_nul(name).as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            std::ptr::null(),
            &mut out,
            std::ptr::null_mut()
        ),
        0
    );
    out
}
unsafe fn set(key: HKEY, value: u32) {
    assert_eq!(
        RegSetValueExW(
            key,
            crate::wide::to_wide_nul("identity").as_ptr(),
            0,
            REG_DWORD,
            (&value as *const u32).cast(),
            4
        ),
        0
    );
}
unsafe fn get(key: HKEY) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4;
    let status = RegQueryValueExW(
        key,
        crate::wide::to_wide_nul("identity").as_ptr(),
        std::ptr::null(),
        std::ptr::null_mut(),
        (&mut value as *mut u32).cast(),
        &mut size,
    );
    if status == 2 {
        None
    } else {
        assert_eq!(status, 0);
        Some(value)
    }
}

/// Runs only in a disposable child: process-global NT hooks must never affect
/// concurrent tests, the real game, or a developer's actual registry/data roots.
#[test]
fn native_profile_probe() {
    let Some(root) = std::env::var_os("CLIENTPATCH_ISOLATION_PROBE") else {
        return;
    };
    let root = PathBuf::from(root);
    let profile = std::env::var("CLIENTPATCH_ISOLATION_TEST_PROFILE").unwrap();
    let reg = std::env::var("CLIENTPATCH_ISOLATION_TEST_REGISTRY").unwrap();
    let reopening = std::env::var_os("CLIENTPATCH_ISOLATION_TEST_REOPEN").is_some();
    let source = root.join("official");
    let target = root.join(&profile);
    let mut state = State {
        files: file_mappings(&source, &target).unwrap(),
        registry: Vec::new(),
        query_object: unsafe { symbol("NtQueryObject").unwrap() },
        query_key: unsafe { symbol("NtQueryKey").unwrap() },
    };
    unsafe {
        // Parent handles deliberately predate installation. Relative key opens
        // must resolve their object name rather than matching only lpSubKey.
        let parent = key(HKEY_CURRENT_USER, &reg);
        let parent_name = state.handle_name(parent, true).unwrap();
        let parent_name = String::from_utf16(&parent_name).unwrap();
        state.registry.push(Mapping::new(
            &format!(r"{parent_name}\official"),
            &format!(r"{parent_name}\{profile}"),
        ));
        let directory = windows_sys::Win32::Storage::FileSystem::CreateFileW(
            crate::wide::to_wide_nul(root.to_str().unwrap()).as_ptr(),
            0x80,
            7,
            std::ptr::null(),
            3,
            0x02000000,
            std::ptr::null_mut(),
        );
        assert_ne!(
            directory,
            windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
        );
        open_named_pipe_directory_with_input_length();
        install_state(state).unwrap();
        open_named_pipe_directory_with_input_length();
        anonymous_pipe_roundtrip();
        let isolated_key = key(parent, "official");
        assert_eq!(
            get(isolated_key),
            if reopening { Some(22) } else { None },
            "profile identity is not independent"
        );
        set(isolated_key, 22);
        RegCloseKey(isolated_key);
        let reopened = key(parent, "official");
        assert_eq!(get(reopened), Some(22));
        RegCloseKey(reopened);
        let disposable = key(parent, r"official\temporary");
        set(disposable, 33);
        RegCloseKey(disposable);
        assert_eq!(
            RegDeleteTreeW(
                parent,
                crate::wide::to_wide_nul(r"official\temporary").as_ptr()
            ),
            0
        );
        RegCloseKey(parent);
        // Real NtCreateFile with a parent directory handle acquired before
        // installation, rather than another absolute Win32 open.
        let mut relative: Vec<u16> = r"official\native-relative".encode_utf16().collect();
        let mut name = UnicodeString {
            length: (relative.len() * 2) as u16,
            maximum_length: (relative.len() * 2) as u16,
            buffer: relative.as_mut_ptr(),
        };
        let attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root: directory,
            name: &mut name,
            attributes: 0x40,
            security_descriptor: std::ptr::null_mut(),
            security_qos: std::ptr::null_mut(),
        };
        let mut file = std::ptr::null_mut();
        let mut io = [0usize; 2];
        let create: CreateFile = symbol("NtCreateFile").unwrap();
        assert_eq!(
            create(
                &mut file,
                0x100002,
                &attrs,
                io.as_mut_ptr().cast(),
                std::ptr::null(),
                0,
                7,
                3,
                0x60,
                std::ptr::null(),
                0
            ),
            0
        );
        CloseHandle(file);
        CloseHandle(directory);
    }
    assert!(source.join("native-relative").exists());
    if reopening {
        assert_eq!(
            std::fs::read(source.join("auth.db")).unwrap(),
            b"replacement"
        );
        assert!(!source.join("official-only").exists());
        return;
    }
    assert!(
        !source.join("auth.db").exists(),
        "official SDK identity leaked"
    );
    assert!(!source.join("_bcdb0_").exists(), "official save leaked");
    assert!(
        std::fs::read(source.join("bridge/auth.db")).is_err(),
        "junction leaked official identity"
    );
    assert!(std::fs::write(source.join("bridge/auth.db"), b"wrong").is_err());
    std::fs::write(root.join("blocked-stage"), b"wrong").unwrap();
    assert!(std::fs::rename(root.join("blocked-stage"), source.join("bridge/auth.db")).is_err());
    std::fs::write(source.join("auth.db"), b"private credential").unwrap();
    assert_eq!(
        std::fs::read(source.join("auth.db")).unwrap(),
        b"private credential"
    );
    std::fs::create_dir_all(source.join("_bcdb1_")).unwrap();
    std::fs::write(source.join("_bcdb1_/record"), b"private save").unwrap();
    std::fs::rename(source.join("_bcdb1_"), source.join("_bcdb0_")).unwrap();
    assert_eq!(
        std::fs::read(source.join("_bcdb0_/record")).unwrap(),
        b"private save"
    );
    // SQLite companions, replace-existing rename, and rename from an unrelated
    // staging directory all have to select the same profile.
    for suffix in ["-journal", "-wal", "-shm"] {
        let path = source.join(format!("auth.db{suffix}"));
        std::fs::write(&path, b"journal").unwrap();
        assert!(path.metadata().unwrap().is_file());
        std::fs::remove_file(path).unwrap();
    }
    std::fs::write(root.join("stage"), b"replacement").unwrap();
    std::fs::rename(root.join("stage"), source.join("auth.db")).unwrap();
    assert_eq!(
        std::fs::read(source.join("auth.db")).unwrap(),
        b"replacement"
    );
    let entries: Vec<_> = std::fs::read_dir(&source)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(entries.iter().any(|e| e == "auth.db"));
    assert!(!entries.iter().any(|e| e == "official-only"));
    assert!(std::fs::hard_link(source.join("auth.db"), root.join("escaped-link")).is_err());
    std::fs::remove_dir_all(source.join("_bcdb0_")).unwrap();
    assert!(!source.join("_bcdb0_").exists());
}

#[test]
fn native_profiles_preserve_official_and_each_other() {
    let root = std::env::temp_dir().join(format!(
        "clientpatch-isolation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let reg = format!(
        r"Software\clientpatch-isolation-tests\{}",
        root.file_name().unwrap().to_string_lossy()
    );
    let source = root.join("official");
    std::fs::create_dir_all(source.join("_bcdb0_")).unwrap();
    std::fs::write(source.join("auth.db"), b"official credential").unwrap();
    std::fs::write(source.join("_bcdb0_/record"), b"official save").unwrap();
    std::fs::write(source.join("official-only"), b"keep").unwrap();
    unsafe {
        let original = key(HKEY_CURRENT_USER, &format!(r"{reg}\official"));
        set(original, 11);
        RegCloseKey(original);
    }
    for (profile, reopening) in [
        ("private-a", false),
        ("private-b", false),
        ("private-a", true),
    ] {
        std::fs::create_dir_all(root.join(profile)).unwrap();
        let junction = Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(root.join(profile).join("bridge"))
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            junction.status.success(),
            "junction fixture: {}",
            String::from_utf8_lossy(&junction.stderr)
        );
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "modules::isolation::native::tests::native_profile_probe",
                "--nocapture",
            ])
            .env("CLIENTPATCH_ISOLATION_PROBE", &root)
            .env("CLIENTPATCH_ISOLATION_TEST_PROFILE", profile)
            .env("CLIENTPATCH_ISOLATION_TEST_REGISTRY", &reg);
        if reopening {
            command.env("CLIENTPATCH_ISOLATION_TEST_REOPEN", "1");
        } else {
            command.env_remove("CLIENTPATCH_ISOLATION_TEST_REOPEN");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(root.join(profile).join("auth.db")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            std::fs::read(source.join("auth.db")).unwrap(),
            b"official credential"
        );
        assert!(!source.join("native-relative").exists());
        assert!(root.join(profile).join("native-relative").exists());
        assert_eq!(
            std::fs::read(source.join("_bcdb0_/record")).unwrap(),
            b"official save"
        );
        // Remove only the junction entry before recursively cleaning fixtures.
        std::fs::remove_dir(root.join(profile).join("bridge")).unwrap();
        unsafe {
            let original = key(HKEY_CURRENT_USER, &format!(r"{reg}\official"));
            assert_eq!(get(original), Some(11));
            RegCloseKey(original);
            let private = key(HKEY_CURRENT_USER, &format!(r"{reg}\{profile}"));
            assert_eq!(get(private), Some(22));
            RegCloseKey(private);
        }
    }
    // This process installed no hooks; its original view is still intact.
    std::fs::remove_dir_all(&root).unwrap();
    unsafe {
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, crate::wide::to_wide_nul(&reg).as_ptr()),
            0
        );
    }
}
