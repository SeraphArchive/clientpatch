//! Native name redirection, shared by Win32, Unity/Mono IO and native SQLite.
//! Windows x64 OBJECT_ATTRIBUTES / UNICODE_STRING / FILE_RENAME_INFORMATION ABI.
use super::paths::{self, Mapping};
use core::ffi::c_void;
use retour::GenericDetour;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenCurrentUser, KEY_READ, KEY_WRITE,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_LocalAppDataLow, SHGetKnownFolderPath,
};

type Handle = *mut c_void;
type Status = i32;
const DENIED: Status = 0xC0000022u32 as i32;
const INVALID: Status = 0xC000000Du32 as i32;
const NOT_SUPPORTED: Status = 0xC00000BBu32 as i32;
const DONT_REPARSE: u32 = 0x1000;

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct ObjectAttributes {
    length: u32,
    root: Handle,
    name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_qos: *mut c_void,
}

type QueryObject = unsafe extern "system" fn(Handle, u32, *mut c_void, u32, *mut u32) -> Status;
type QueryKey = unsafe extern "system" fn(Handle, u32, *mut c_void, u32, *mut u32) -> Status;

struct State {
    files: Vec<Mapping>,
    registry: Vec<Mapping>,
    query_object: QueryObject,
    query_key: QueryKey,
}
static STATE: OnceLock<State> = OnceLock::new();

unsafe fn counted(s: *const UnicodeString) -> Result<Vec<u16>, Status> {
    if s.is_null() {
        return Ok(Vec::new());
    }
    let s = &*s;
    // NT object-name inputs are counted by Length. Native callers (including
    // Steam's named-pipe opens) can leave MaximumLength at zero; the kernel
    // accepts them. MaximumLength is not an input buffer validation boundary.
    if s.length % 2 != 0 || (s.length > 0 && s.buffer.is_null()) {
        return Err(INVALID);
    }
    if s.length == 0 {
        return Ok(Vec::new());
    }
    let result = std::slice::from_raw_parts(s.buffer, s.length as usize / 2).to_vec();
    if result.contains(&0) {
        return Err(INVALID);
    }
    Ok(result)
}

impl State {
    unsafe fn handle_name(&self, handle: Handle, registry: bool) -> Result<Vec<u16>, Status> {
        let mut required = 0;
        // Name queries do not open files or keys, so they cannot recurse into us.
        if registry {
            (self.query_key)(handle, 3, std::ptr::null_mut(), 0, &mut required);
        } else {
            (self.query_object)(handle, 1, std::ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 131072 {
            return Err(DENIED);
        }
        let mut buffer = vec![0u64; (required as usize + 7) / 8];
        let status = if registry {
            (self.query_key)(
                handle,
                3,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } else {
            (self.query_object)(
                handle,
                1,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        };
        if status < 0 {
            return Err(status);
        }
        if registry {
            let length = *(buffer.as_ptr() as *const u32) as usize;
            if length % 2 != 0 || length + 4 > buffer.len() * 8 {
                return Err(INVALID);
            }
            Ok(
                std::slice::from_raw_parts(
                    (buffer.as_ptr() as *const u8).add(4).cast(),
                    length / 2,
                )
                .to_vec(),
            )
        } else {
            counted(buffer.as_ptr().cast())
        }
    }

    unsafe fn absolute(
        &self,
        name: &[u16],
        root: Handle,
        registry: bool,
    ) -> Result<Vec<u16>, Status> {
        let full = if name.first() == Some(&(b'\\' as u16)) {
            name.to_vec()
        } else if root.is_null() {
            return Err(INVALID);
        } else {
            paths::join(&self.handle_name(root, registry)?, name)
        };
        if registry {
            Ok(full)
        } else {
            paths::normalize_file(&full).ok_or(INVALID)
        }
    }

    fn rewrite(&self, name: &[u16], registry: bool) -> Option<Vec<u16>> {
        let maps = if registry {
            &self.registry
        } else {
            &self.files
        };
        maps.iter().find_map(|m| m.rewrite(name))
    }

    fn protected_file(&self, name: &[u16]) -> bool {
        self.files
            .iter()
            .any(|m| paths::contains(&m.source, name) || paths::contains(&m.target, name))
    }
}

unsafe fn with_object(
    attrs: *const ObjectAttributes,
    registry: bool,
    call: impl FnOnce(*const ObjectAttributes) -> Status,
) -> Status {
    crate::hook::no_panic(DENIED, || {
        let Some(state) = STATE.get() else {
            return DENIED;
        };
        if attrs.is_null() {
            return call(attrs);
        }
        if (*attrs).length as usize != std::mem::size_of::<ObjectAttributes>() {
            return INVALID;
        }
        if (*attrs).name.is_null() {
            return call(attrs);
        }
        // Named-pipe relative opens are IPC, not filesystem profile access.
        // Steam opens a pipe using an empty name relative to a pipe handle;
        // NtQueryObject cannot resolve that handle as a filesystem root.
        if !registry && !(*attrs).root.is_null()
            && windows_sys::Win32::Storage::FileSystem::GetFileType((*attrs).root) == 3 {
            return call(attrs);
        }
        let name = match counted((*attrs).name)
            .and_then(|n| state.absolute(&n, (*attrs).root, registry))
        {
            Ok(n) => n,
            Err(e) => return e,
        };
        let mut name = match state.rewrite(&name, registry) {
            Some(n) => n,
            None if (if registry {
                &state.registry
            } else {
                &state.files
            })
            .iter()
            .any(|m| paths::contains(&m.target, &name)) =>
            {
                name
            }
            None => return call(attrs),
        };
        if name.len() > 32766 {
            return INVALID;
        }
        let mut unicode = UnicodeString {
            length: (name.len() * 2) as u16,
            maximum_length: (name.len() * 2) as u16,
            buffer: name.as_mut_ptr(),
        };
        let mut redirected = *attrs;
        redirected.name = &mut unicode;
        redirected.root = std::ptr::null_mut();
        redirected.attributes |= DONT_REPARSE;
        call(&redirected)
    })
}

macro_rules! object_hook {
    ($slot:ident, $ty:ident, $hook:ident, $key:literal, ($($arg:ident : $t:ty),*), $attrs:ident, $mapped:ident, ($($pass:expr),*)) => {
        type $ty = unsafe extern "system" fn($($t),*) -> Status;
        static $slot: OnceLock<GenericDetour<$ty>> = OnceLock::new();
        unsafe extern "system" fn $hook($($arg:$t),*) -> Status {
            let Some(d) = $slot.get() else { return DENIED; };
            with_object($attrs, $key, |$mapped| d.call($($pass),*))
        }
    };
}

// IO_STATUS_BLOCK, LARGE_INTEGER, EA buffers and allocation sizes are opaque:
// preserving their original pointers also preserves Windows' validation rules.
object_hook!(CREATE_FILE, CreateFile, create_file, false,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, io: *mut c_void, size: *const i64, flags: u32, share: u32, disposition: u32, options: u32, ea: *const c_void, ea_size: u32), attrs, mapped,
    (out, access, mapped, io, size, flags, share, disposition, options, ea, ea_size));
object_hook!(OPEN_FILE, OpenFile, open_file, false,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, io: *mut c_void, share: u32, options: u32), attrs, mapped,
    (out, access, mapped, io, share, options));
object_hook!(QUERY_ATTRIBUTES, QueryAttributes, query_attributes, false,
    (attrs: *const ObjectAttributes, info: *mut c_void), attrs, mapped, (mapped, info));
object_hook!(QUERY_FULL_ATTRIBUTES, QueryFullAttributes, query_full_attributes, false,
    (attrs: *const ObjectAttributes, info: *mut c_void), attrs, mapped, (mapped, info));
object_hook!(DELETE_FILE, DeleteFile, delete_file, false,
    (attrs: *const ObjectAttributes), attrs, mapped, (mapped));
object_hook!(OPEN_KEY, OpenKey, open_key, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes), attrs, mapped, (out, access, mapped));
object_hook!(OPEN_KEY_EX, OpenKeyEx, open_key_ex, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, options: u32), attrs, mapped, (out, access, mapped, options));
object_hook!(CREATE_KEY, CreateKey, create_key, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, title: u32, class: *const UnicodeString, options: u32, disposition: *mut u32), attrs, mapped,
    (out, access, mapped, title, class, options, disposition));
object_hook!(OPEN_KEY_TX, OpenKeyTx, open_key_tx, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, transaction: Handle), attrs, mapped,
    (out, access, mapped, transaction));
object_hook!(OPEN_KEY_TX_EX, OpenKeyTxEx, open_key_tx_ex, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, options: u32, transaction: Handle), attrs, mapped,
    (out, access, mapped, options, transaction));
object_hook!(CREATE_KEY_TX, CreateKeyTx, create_key_tx, true,
    (out: *mut Handle, access: u32, attrs: *const ObjectAttributes, title: u32, class: *const UnicodeString, options: u32, transaction: Handle, disposition: *mut u32), attrs, mapped,
    (out, access, mapped, title, class, options, transaction, disposition));

type SetInformation =
    unsafe extern "system" fn(Handle, *mut c_void, *const c_void, u32, u32) -> Status;
static SET_INFORMATION: OnceLock<GenericDetour<SetInformation>> = OnceLock::new();

unsafe extern "system" fn set_information(
    file: Handle,
    io: *mut c_void,
    info: *const c_void,
    length: u32,
    class: u32,
) -> Status {
    let Some(d) = SET_INFORMATION.get() else {
        return DENIED;
    };
    // FILE_RENAME_INFORMATION(10), bypass-access-check(56), Ex(65/66);
    // FILE_LINK_INFORMATION(11/57/72/73) uses the same x64 name layout.
    let link = matches!(class, 11 | 57 | 72 | 73);
    if !link && !matches!(class, 10 | 56 | 65 | 66) {
        return d.call(file, io, info, length, class);
    }
    crate::hook::no_panic(DENIED, || {
        let Some(state) = STATE.get() else {
            return DENIED;
        };
        if info.is_null() || length < 20 {
            return INVALID;
        }
        let bytes = info.cast::<u8>();
        let root = std::ptr::read_unaligned(bytes.add(8).cast::<Handle>());
        let size = std::ptr::read_unaligned(bytes.add(16).cast::<u32>()) as usize;
        if size % 2 != 0 || size > length as usize - 20 {
            return INVALID;
        }
        let name = std::slice::from_raw_parts(bytes.add(20).cast::<u16>(), size / 2);
        let source = match state.handle_name(file, false) {
            Ok(p) => p,
            Err(e) => return e,
        };
        if state
            .rewrite(&source, false)
            .is_some_and(|mapped| mapped != source)
        {
            // A file opened before isolation must not be moved out of the
            // original profile using a newly redirected destination.
            return DENIED;
        }
        let full = if name.first() != Some(&(b'\\' as u16)) && root.is_null() {
            let Some(end) = source.iter().rposition(|c| *c == b'\\' as u16) else {
                return INVALID;
            };
            paths::normalize_file(&paths::join(&source[..end], name)).ok_or(INVALID)
        } else {
            state.absolute(name, root, false)
        };
        let full = match full {
            Ok(p) => p,
            Err(e) => return e,
        };
        if link && (state.protected_file(&source) || state.protected_file(&full)) {
            return NOT_SUPPORTED;
        }
        let target = match state.rewrite(&full, false) {
            Some(n) => n,
            None if state.protected_file(&full) => full,
            None => return d.call(file, io, info, length, class),
        };
        if target.len() > 32766 {
            return INVALID;
        }
        // Pin the destination directory with reparse traversal disabled. A
        // path-only rename could otherwise follow a junction after redirection.
        let Some(end) = target.iter().rposition(|c| *c == b'\\' as u16) else {
            return INVALID;
        };
        let mut parent = target[..end].to_vec();
        let mut unicode = UnicodeString {
            length: (parent.len() * 2) as u16,
            maximum_length: (parent.len() * 2) as u16,
            buffer: parent.as_mut_ptr(),
        };
        let attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root: std::ptr::null_mut(),
            name: &mut unicode,
            attributes: 0x40 | DONT_REPARSE,
            security_descriptor: std::ptr::null_mut(),
            security_qos: std::ptr::null_mut(),
        };
        let Some(open) = OPEN_FILE.get() else {
            return DENIED;
        };
        let mut directory = std::ptr::null_mut();
        let mut status_block = [0usize; 2];
        let status = open.call(
            &mut directory,
            0x100080,
            &attrs,
            status_block.as_mut_ptr().cast(),
            7,
            0x21,
        );
        if status < 0 {
            return status;
        }
        struct Directory(Handle);
        impl Drop for Directory {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let directory = Directory(directory);
        let target = &target[end + 1..];
        let new_len = 20 + target.len() * 2;
        let mut buffer = vec![0u64; (new_len + 7) / 8];
        let dest = buffer.as_mut_ptr().cast::<u8>();
        std::ptr::copy_nonoverlapping(bytes, dest, 8); // replace flag / Ex flags and alignment
        std::ptr::write_unaligned(dest.add(8).cast::<Handle>(), directory.0);
        std::ptr::write_unaligned(dest.add(16).cast::<u32>(), (target.len() * 2) as u32);
        std::ptr::copy_nonoverlapping(target.as_ptr().cast::<u8>(), dest.add(20), target.len() * 2);
        d.call(file, io, dest.cast(), new_len as u32, class)
    })
}

unsafe fn symbol<T: Copy>(name: &str) -> anyhow::Result<T> {
    let module = crate::ffi::ensure_module("ntdll.dll");
    let ptr =
        crate::ffi::export(module, name).ok_or_else(|| anyhow::anyhow!("{name} unavailable"))?;
    Ok(std::mem::transmute_copy(&ptr))
}

fn known_folder(id: &windows_sys::core::GUID) -> anyhow::Result<PathBuf> {
    unsafe {
        let mut path = std::ptr::null_mut();
        let status = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut path);
        anyhow::ensure!(
            status >= 0 && !path.is_null(),
            "cannot resolve isolation storage root ({status:#x})"
        );
        let value = crate::wide::from_wide_nul(path);
        CoTaskMemFree(path.cast());
        Ok(PathBuf::from(value))
    }
}

fn ensure_directory(path: &Path) -> anyhow::Result<()> {
    let root = path
        .ancestors()
        .last()
        .ok_or_else(|| anyhow::anyhow!("missing volume root"))?;
    crate::staging::checked_path(root, path)?;
    std::fs::create_dir_all(path)?;
    crate::staging::checked_path(root, path)
}

fn file_mappings(source: &Path, target: &Path) -> anyhow::Result<Vec<Mapping>> {
    let source = source
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-Unicode storage root"))?;
    let target = target
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-Unicode profile root"))?;
    anyhow::ensure!(
        source.as_bytes().get(1) == Some(&b':') && target.as_bytes().get(1) == Some(&b':'),
        "isolation requires local drive storage"
    );
    fn device(path: &str) -> anyhow::Result<String> {
        let drive = crate::wide::to_wide_nul(&path[..2]);
        let mut buffer = vec![0u16; 32768];
        let n =
            unsafe { QueryDosDeviceW(drive.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
        anyhow::ensure!(n > 0, "cannot resolve storage volume");
        let device = unsafe { crate::wide::from_wide_nul(buffer.as_ptr()) };
        Ok(format!("{device}{}", &path[2..]))
    }
    let native_target = device(target)?;
    Ok(vec![
        Mapping::new(&format!(r"\??\{source}"), &native_target),
        Mapping::new(&device(source)?, &native_target),
        Mapping::new(&format!(r"\??\{target}"), &native_target),
    ])
}

pub(super) fn install(suffix: &str) -> anyhow::Result<()> {
    anyhow::ensure!(super::valid_suffix(suffix), "invalid isolation suffix");
    let mut state = State {
        files: Vec::new(),
        registry: Vec::new(),
        query_object: unsafe { symbol("NtQueryObject")? },
        query_key: unsafe { symbol("NtQueryKey")? },
    };
    let local = known_folder(&FOLDERID_LocalAppData)?;
    let low = known_folder(&FOLDERID_LocalAppDataLow)?;
    // A new namespace keeps legacy registry-only profiles recoverable. Never
    // import credentials or data from either legacy or official profiles.
    for source in [
        local.join("GameLib"),
        low.join("wfs").join("HeavenBurnsRed"),
    ] {
        let leaf = source.file_name().unwrap().to_string_lossy();
        let target = source.with_file_name(format!("{leaf}.{suffix}.isolated"));
        ensure_directory(&target)?;
        state.files.extend(file_mappings(&source, &target)?);
    }
    unsafe {
        let mut user = std::ptr::null_mut();
        anyhow::ensure!(
            RegOpenCurrentUser(KEY_READ | KEY_WRITE, &mut user) == 0,
            "cannot open user registry"
        );
        let user_name = state.handle_name(user, true);
        let target_subkey = format!(r"Software\wfs\HeavenBurnsRed.{suffix}.isolated");
        let mut target = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            user,
            crate::wide::to_wide_nul(&target_subkey).as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut target,
            std::ptr::null_mut(),
        );
        RegCloseKey(user);
        anyhow::ensure!(status == 0, "cannot create isolated registry ({status})");
        RegCloseKey(target);
        let user_name = String::from_utf16(
            &user_name.map_err(|s| anyhow::anyhow!("cannot resolve registry root ({s:#x})"))?,
        )?;
        state.registry.push(Mapping::new(
            &format!(r"{user_name}\Software\wfs\HeavenBurnsRed"),
            &format!(r"{user_name}\{target_subkey}"),
        ));
    }
    install_state(state)?;
    crate::logging::line("INFO", &format!("isolation active (profile={suffix}, registry + Unity persistent data + GameLib storage)"));
    Ok(())
}

fn install_state(state: State) -> anyhow::Result<()> {
    anyhow::ensure!(STATE.set(state).is_ok(), "isolation already initialized");
    unsafe {
        macro_rules! install {
            ($slot:ident, $ty:ident, $hook:ident, $name:literal) => {
                crate::hook::install(&$slot, symbol::<$ty>($name)?, $hook as $ty)?;
            };
        }
        install!(CREATE_FILE, CreateFile, create_file, "NtCreateFile");
        install!(OPEN_FILE, OpenFile, open_file, "NtOpenFile");
        install!(
            QUERY_ATTRIBUTES,
            QueryAttributes,
            query_attributes,
            "NtQueryAttributesFile"
        );
        install!(
            QUERY_FULL_ATTRIBUTES,
            QueryFullAttributes,
            query_full_attributes,
            "NtQueryFullAttributesFile"
        );
        install!(DELETE_FILE, DeleteFile, delete_file, "NtDeleteFile");
        install!(
            SET_INFORMATION,
            SetInformation,
            set_information,
            "NtSetInformationFile"
        );
        install!(OPEN_KEY, OpenKey, open_key, "NtOpenKey");
        install!(OPEN_KEY_EX, OpenKeyEx, open_key_ex, "NtOpenKeyEx");
        install!(CREATE_KEY, CreateKey, create_key, "NtCreateKey");
        install!(OPEN_KEY_TX, OpenKeyTx, open_key_tx, "NtOpenKeyTransacted");
        install!(
            OPEN_KEY_TX_EX,
            OpenKeyTxEx,
            open_key_tx_ex,
            "NtOpenKeyTransactedEx"
        );
        install!(
            CREATE_KEY_TX,
            CreateKeyTx,
            create_key_tx,
            "NtCreateKeyTransacted"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
