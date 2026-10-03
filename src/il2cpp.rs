//! IL2CPP runtime binding — resolves `il2cpp_*` exports from GameAssembly.dll and
//! wraps the operations the redirect needs. Ported from version/dllmain.cpp
//! (read-only proxy) and extended for write/redirect use. By-name only.
#![allow(non_snake_case)]

use crate::logging;
use crate::wide;
use core::ffi::c_void;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Threading::Sleep;

type DomainGet = unsafe extern "C" fn() -> *mut c_void;
type ThreadAttach = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type DomainGetAssemblies = unsafe extern "C" fn(*mut c_void, *mut usize) -> *mut *mut c_void;
type AssemblyGetImage = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type ImageGetName = unsafe extern "C" fn(*mut c_void) -> *const i8;
type ClassFromName = unsafe extern "C" fn(*mut c_void, *const i8, *const i8) -> *mut c_void;
type ClassGetMethod = unsafe extern "C" fn(*mut c_void, *const i8, i32) -> *mut c_void;
type ClassGetField = unsafe extern "C" fn(*mut c_void, *const i8) -> *mut c_void;
type FieldGetOffset = unsafe extern "C" fn(*mut c_void) -> u32;
type FieldStaticGet = unsafe extern "C" fn(*mut c_void, *mut c_void);
type FieldStaticSet = unsafe extern "C" fn(*mut c_void, *mut c_void);
type StringNew = unsafe extern "C" fn(*const i8) -> *mut c_void;
type StringChars = unsafe extern "C" fn(*mut c_void) -> *mut u16;
type StringLength = unsafe extern "C" fn(*mut c_void) -> i32;
type RuntimeInvoke =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void, *mut *mut c_void) -> *mut c_void;
type ObjectGetClass = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type ObjectNew = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type ClassGetName = unsafe extern "C" fn(*mut c_void) -> *const i8;
type GchandleNew = unsafe extern "C" fn(*mut c_void, i32) -> u32;
type GetCorlib = unsafe extern "C" fn() -> *mut c_void;
type ClassFromType = unsafe extern "C" fn(*const c_void) -> *mut c_void;

pub struct Il2Cpp {
    domain_get: DomainGet,
    thread_attach: ThreadAttach,
    domain_get_assemblies: DomainGetAssemblies,
    assembly_get_image: AssemblyGetImage,
    image_get_name: ImageGetName,
    class_from_name: ClassFromName,
    class_get_method: ClassGetMethod,
    class_get_field: ClassGetField,
    field_get_offset: FieldGetOffset,
    field_static_get: FieldStaticGet,
    field_static_set: FieldStaticSet,
    string_new: StringNew,
    string_chars: StringChars,
    string_length: StringLength,
    runtime_invoke: RuntimeInvoke,
    object_get_class: ObjectGetClass,
    object_new: ObjectNew,
    class_get_name: ClassGetName,
    gchandle_new: GchandleNew,
    get_corlib: GetCorlib,
    class_from_type: ClassFromType,
}

/// GetProcAddress + transmute helper. Returns None (logged) on the first miss.
unsafe fn sym<T>(ga: HMODULE, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    match GetProcAddress(ga, c.as_ptr() as *const u8) {
        Some(f) => Some(core::mem::transmute_copy::<_, T>(&f)),
        None => {
            logging::line("ERR", &format!("il2cpp export missing: {name}"));
            None
        }
    }
}

impl Il2Cpp {
    /// Wait (bounded) for GameAssembly.dll and resolve all needed exports.
    pub fn load() -> Option<Il2Cpp> {
        let ga = unsafe {
            let mut h: HMODULE = core::ptr::null_mut();
            for _ in 0..600 {
                h = GetModuleHandleA(c"GameAssembly.dll".as_ptr() as *const u8);
                if !h.is_null() {
                    break;
                }
                Sleep(100);
            }
            if h.is_null() {
                logging::line("ERR", "GameAssembly.dll not loaded after ~60s");
                return None;
            }
            h
        };
        unsafe {
            Some(Il2Cpp {
                domain_get: sym(ga, "il2cpp_domain_get")?,
                thread_attach: sym(ga, "il2cpp_thread_attach")?,
                domain_get_assemblies: sym(ga, "il2cpp_domain_get_assemblies")?,
                assembly_get_image: sym(ga, "il2cpp_assembly_get_image")?,
                image_get_name: sym(ga, "il2cpp_image_get_name")?,
                class_from_name: sym(ga, "il2cpp_class_from_name")?,
                class_get_method: sym(ga, "il2cpp_class_get_method_from_name")?,
                class_get_field: sym(ga, "il2cpp_class_get_field_from_name")?,
                field_get_offset: sym(ga, "il2cpp_field_get_offset")?,
                field_static_get: sym(ga, "il2cpp_field_static_get_value")?,
                field_static_set: sym(ga, "il2cpp_field_static_set_value")?,
                string_new: sym(ga, "il2cpp_string_new")?,
                string_chars: sym(ga, "il2cpp_string_chars")?,
                string_length: sym(ga, "il2cpp_string_length")?,
                runtime_invoke: sym(ga, "il2cpp_runtime_invoke")?,
                object_get_class: sym(ga, "il2cpp_object_get_class")?,
                object_new: sym(ga, "il2cpp_object_new")?,
                class_get_name: sym(ga, "il2cpp_class_get_name")?,
                gchandle_new: sym(ga, "il2cpp_gchandle_new")?,
                get_corlib: sym(ga, "il2cpp_get_corlib")?,
                class_from_type: sym(ga, "il2cpp_class_from_type")?,
            })
        }
    }

    pub fn thread_attach(&self) {
        unsafe {
            let d = (self.domain_get)();
            if !d.is_null() {
                (self.thread_attach)(d);
            }
        }
    }

    /// Cheap when the calling thread is already attached (TLS check). Detours
    /// can run on UWR/worker threads that never went through bootstrap attach.
    fn ensure_attached(&self) {
        self.thread_attach();
    }

    pub fn domain_ready(&self) -> bool {
        unsafe { !(self.domain_get)().is_null() }
    }

    /// Non-enumerating check that the core library is loaded — `il2cpp_get_corlib`
    /// returns a global set partway through `il2cpp_init`, so it is safe to poll
    /// even on a not-yet-fully-initialized runtime (unlike any enumeration API).
    pub fn corlib_ready(&self) -> bool {
        unsafe { !(self.get_corlib)().is_null() }
    }

    /// Assembly count only — does not walk the array. `il2cpp_domain_get_assemblies`
    /// returns `(ptr, n)` from a global that is appended during `il2cpp_init`;
    /// reading `n` is the cheap "has init progressed?" probe. Walking the array
    /// (`find_image`) is what AVs mid-init, so callers must wait for `n` to
    /// plateau before enumerating.
    pub fn assembly_count(&self) -> usize {
        unsafe {
            let domain = (self.domain_get)();
            if domain.is_null() {
                return 0;
            }
            let mut n: usize = 0;
            let assemblies = (self.domain_get_assemblies)(domain, &mut n);
            if assemblies.is_null() || n > 100_000 {
                0
            } else {
                n
            }
        }
    }

    pub fn find_image(&self, wanted: &str) -> *mut c_void {
        unsafe {
            let domain = (self.domain_get)();
            if domain.is_null() {
                return core::ptr::null_mut();
            }
            let mut n: usize = 0;
            let assemblies = (self.domain_get_assemblies)(domain, &mut n);
            // Breadcrumb the FIRST enumeration: if we survive the call we learn
            // the count; if the loop below crashes, this is the last line.
            {
                static LOGGED: AtomicBool = AtomicBool::new(false);
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    logging::line(
                        "DBG",
                        &format!("find_image[1st]: get_assemblies n={n} arr={assemblies:p}"),
                    );
                }
            }
            // A null array, a zero count, or an absurd count means the runtime
            // has not registered assemblies yet (or returned garbage); treat it
            // as "not ready" rather than enumerating into invalid memory.
            if assemblies.is_null() || n == 0 || n > 100_000 {
                return core::ptr::null_mut();
            }
            for i in 0..n {
                let asm = *assemblies.add(i);
                if asm.is_null() {
                    continue;
                }
                let img = (self.assembly_get_image)(asm);
                if img.is_null() {
                    continue;
                }
                let name = (self.image_get_name)(img);
                if !name.is_null() {
                    let s = std::ffi::CStr::from_ptr(name).to_string_lossy();
                    // Match the assembly name EXACTLY (with or without the `.dll`
                    // suffix). Substring matching is wrong: "HTTPComm" also matches
                    // "HTTPCommGenerated", and "Assembly-CSharp" also matches
                    // "Assembly-CSharp-firstpass" — returning the wrong image and a
                    // spurious "class not found".
                    let name_str: &str = &s;
                    if name_str == wanted || name_str.strip_suffix(".dll") == Some(wanted) {
                        return img;
                    }
                }
            }
            core::ptr::null_mut()
        }
    }

    pub fn class(&self, image: &str, ns: &str, name: &str) -> *mut c_void {
        let img = self.find_image(image);
        if img.is_null() {
            return core::ptr::null_mut();
        }
        let ns = CString::new(ns).unwrap();
        let nm = CString::new(name).unwrap();
        unsafe { (self.class_from_name)(img, ns.as_ptr(), nm.as_ptr()) }
    }

    /// Resolve a class from the corlib (mscorlib) image directly — avoids guessing
    /// the assembly name for BCL types like `System.Security.Cryptography.*`.
    pub fn class_corlib(&self, ns: &str, name: &str) -> *mut c_void {
        let img = unsafe { (self.get_corlib)() };
        if img.is_null() {
            return core::ptr::null_mut();
        }
        let ns = CString::new(ns).unwrap();
        let nm = CString::new(name).unwrap();
        unsafe { (self.class_from_name)(img, ns.as_ptr(), nm.as_ptr()) }
    }

    pub fn method(&self, class: *mut c_void, name: &str, argc: i32) -> *mut c_void {
        if class.is_null() {
            return core::ptr::null_mut();
        }
        let nm = CString::new(name).unwrap();
        unsafe { (self.class_get_method)(class, nm.as_ptr(), argc) }
    }

    /// MethodInfo.methodPointer is the first field -> native entry.
    pub fn method_entry(&self, method: *mut c_void) -> *mut c_void {
        if method.is_null() {
            return core::ptr::null_mut();
        }
        unsafe { *(method as *mut *mut c_void) }
    }

    pub fn field(&self, class: *mut c_void, name: &str) -> *mut c_void {
        if class.is_null() {
            return core::ptr::null_mut();
        }
        let nm = CString::new(name).unwrap();
        unsafe { (self.class_get_field)(class, nm.as_ptr()) }
    }

    pub fn field_offset(&self, field: *mut c_void) -> u32 {
        if field.is_null() {
            return 0;
        }
        unsafe { (self.field_get_offset)(field) }
    }

    /// Read an instance pointer field (`obj + offset`). Null obj -> null.
    pub fn read_ptr_field(&self, obj: *mut c_void, offset: u32) -> *mut c_void {
        if obj.is_null() {
            return core::ptr::null_mut();
        }
        unsafe { *((obj as *const u8).add(offset as usize) as *const *mut c_void) }
    }

    /// Read an instance `int16` field (`obj + offset`) — e.g. a `LanguageCode`
    /// enum backing field. Null obj -> 0.
    pub fn read_i16_field(&self, obj: *mut c_void, offset: u32) -> i16 {
        if obj.is_null() {
            return 0;
        }
        unsafe { *((obj as *const u8).add(offset as usize) as *const i16) }
    }

    /// Read an instance `int32` field (`obj + offset`). Null obj -> 0.
    pub fn read_i32_field(&self, obj: *mut c_void, offset: u32) -> i32 {
        if obj.is_null() {
            return 0;
        }
        unsafe { *((obj as *const u8).add(offset as usize) as *const i32) }
    }

    /// Write an instance pointer field (`obj + offset`). The IL2CPP GC (Boehm) is
    /// conservative and non-moving, so a direct store is safe as long as `val` is
    /// reachable (the field itself roots it after the store).
    pub fn write_ptr_field(&self, obj: *mut c_void, offset: u32, val: *mut c_void) {
        if obj.is_null() {
            return;
        }
        unsafe { *((obj as *mut u8).add(offset as usize) as *mut *mut c_void) = val };
    }

    /// Write an instance `int32` field (`obj + offset`) — e.g. an enum value.
    pub fn write_i32_field(&self, obj: *mut c_void, offset: u32, val: i32) {
        if obj.is_null() {
            return;
        }
        unsafe { *((obj as *mut u8).add(offset as usize) as *mut i32) = val };
    }

    pub fn new_string(&self, s: &str) -> *mut c_void {
        self.ensure_attached();
        let c = CString::new(s).unwrap_or_default();
        unsafe { (self.string_new)(c.as_ptr()) }
    }

    pub fn string_to_rust(&self, s: *mut c_void) -> String {
        if s.is_null() {
            return String::new();
        }
        unsafe {
            let len = (self.string_length)(s);
            let chars = (self.string_chars)(s);
            if len <= 0 || chars.is_null() {
                return String::new();
            }
            let slice = core::slice::from_raw_parts(chars, len as usize);
            wide::utf16_to_string(slice)
        }
    }

    /// Length (`max_length`) of an Il2CppArray. On 64-bit the header is
    /// { Il2CppObject(0x10), bounds*(0x8), max_length(0x8) }, so the count is at
    /// offset 0x18 and element data begins at 0x20.
    pub fn array_len(&self, arr: *mut c_void) -> usize {
        if arr.is_null() {
            return 0;
        }
        unsafe { *((arr as *const u8).add(0x18) as *const usize) }
    }

    /// Copy `count` bytes starting at element `start` from a managed `byte[]`
    /// (element data at offset 0x20). Bounds-clamped to the array length.
    pub fn array_bytes(&self, arr: *mut c_void, start: usize, count: usize) -> Vec<u8> {
        if arr.is_null() || count == 0 {
            return Vec::new();
        }
        let len = self.array_len(arr);
        let end = start.saturating_add(count).min(len);
        if start >= end {
            return Vec::new();
        }
        unsafe {
            let data = (arr as *const u8).add(0x20).add(start);
            core::slice::from_raw_parts(data, end - start).to_vec()
        }
    }

    pub fn static_set_string(&self, field: *mut c_void, s: &str) {
        if field.is_null() {
            return;
        }
        self.ensure_attached();
        let managed = self.new_string(s);
        unsafe { (self.field_static_set)(field, managed) }
    }

    pub fn static_set_bool(&self, field: *mut c_void, v: bool) {
        if field.is_null() {
            return;
        }
        self.ensure_attached();
        let mut b: u8 = v as u8;
        unsafe { (self.field_static_set)(field, &mut b as *mut u8 as *mut c_void) }
    }

    /// Read a static field's stored pointer value (e.g. the BaseUriForRouting dict).
    pub fn static_get_ptr(&self, field: *mut c_void) -> *mut c_void {
        if field.is_null() {
            return core::ptr::null_mut();
        }
        self.ensure_attached();
        let mut out: *mut c_void = core::ptr::null_mut();
        unsafe { (self.field_static_get)(field, &mut out as *mut *mut c_void as *mut c_void) };
        out
    }

    /// Store an object pointer into a (reference-type) static field. Unlike a value
    /// type, `il2cpp_field_static_set_value` takes the object pointer directly as the
    /// value for reference fields (same as `static_set_string`).
    pub fn static_set_object(&self, field: *mut c_void, obj: *mut c_void) {
        if field.is_null() {
            return;
        }
        self.ensure_attached();
        unsafe { (self.field_static_set)(field, obj) }
    }

    /// The `Il2CppType*` of a field. Native `FieldInfo` layout is
    /// `{ name@0x0, type@0x8, parent@0x10, offset@0x18 }` (verified against
    /// `il2cpp_field_get_flags`, which dereferences `field+0x8`). `il2cpp_field_get_type`
    /// is not exported by this build, so read the slot directly.
    pub fn field_type(&self, field: *mut c_void) -> *const c_void {
        if field.is_null() {
            return core::ptr::null();
        }
        unsafe { *((field as *const *const c_void).add(1)) }
    }

    /// Resolve the `Il2CppClass*` for an `Il2CppType*` (e.g. a generic instantiation
    /// like `Func<string,string>` obtained from a field's type).
    pub fn class_from_type(&self, ty: *const c_void) -> *mut c_void {
        if ty.is_null() {
            return core::ptr::null_mut();
        }
        unsafe { (self.class_from_type)(ty) }
    }

    pub fn invoke(
        &self,
        method: *mut c_void,
        this: *mut c_void,
        args: &mut [*mut c_void],
    ) -> *mut c_void {
        self.invoke_raw(method, this, args).0
    }

    /// Like `invoke`, but `None` if the method is null or a managed exception
    /// was thrown. Use on ctor / write-back paths so a failed construct is not
    /// stored as a live object.
    pub fn invoke_ok(
        &self,
        method: *mut c_void,
        this: *mut c_void,
        args: &mut [*mut c_void],
    ) -> Option<*mut c_void> {
        let (ret, exc) = self.invoke_raw(method, this, args);
        if method.is_null() || exc {
            None
        } else {
            Some(ret)
        }
    }

    fn invoke_raw(
        &self,
        method: *mut c_void,
        this: *mut c_void,
        args: &mut [*mut c_void],
    ) -> (*mut c_void, bool) {
        if method.is_null() {
            return (core::ptr::null_mut(), true);
        }
        self.ensure_attached();
        let mut exc: *mut c_void = core::ptr::null_mut();
        let ret = unsafe { (self.runtime_invoke)(method, this, args.as_mut_ptr(), &mut exc) };
        if !exc.is_null() {
            let msg = self.exception_to_string(exc);
            logging::line("ERR", &format!("il2cpp exception: {msg}"));
            return (ret, true);
        }
        (ret, false)
    }

    /// Best-effort `Exception.ToString()`. Returns "<exception>" on any failure.
    fn exception_to_string(&self, exc: *mut c_void) -> String {
        unsafe {
            let obj_cls = (self.object_get_class)(exc);
            if obj_cls.is_null() {
                return "<exception>".to_string();
            }
            let to_string = (self.class_get_method)(obj_cls, c"ToString".as_ptr(), 0);
            if to_string.is_null() {
                return "<exception>".to_string();
            }
            let mut nested_exception = core::ptr::null_mut();
            let ret = (self.runtime_invoke)(to_string, exc, core::ptr::null_mut(), &mut nested_exception);
            if !nested_exception.is_null() || ret.is_null() {
                return "<exception>".to_string();
            }
            self.string_to_rust(ret)
        }
    }

    pub fn object_class(&self, obj: *mut c_void) -> *mut c_void {
        if obj.is_null() {
            return core::ptr::null_mut();
        }
        unsafe { (self.object_get_class)(obj) }
    }

    pub fn object_new(&self, class: *mut c_void) -> *mut c_void {
        if class.is_null() {
            return core::ptr::null_mut();
        }
        self.ensure_attached();
        unsafe { (self.object_new)(class) }
    }

    /// The IL2CPP class name of an object (e.g. "Sprite", "Texture2D"), for logging.
    pub fn type_name(&self, obj: *mut c_void) -> String {
        if obj.is_null() {
            return "<null>".to_string();
        }
        unsafe {
            let cls = (self.object_get_class)(obj);
            if cls.is_null() {
                return "<?>".to_string();
            }
            let n = (self.class_get_name)(cls);
            if n.is_null() {
                return "<?>".to_string();
            }
            std::ffi::CStr::from_ptr(n).to_string_lossy().into_owned()
        }
    }

    /// Permanently root a managed object against the GC (Boehm). `HideAndDontSave`
    /// only blocks `Resources.UnloadUnusedAssets`, NOT the GC, so runtime-created
    /// textures/sprites must be gchandle-rooted or a mid-build GC frees them.
    pub fn gc_root(&self, obj: *mut c_void) {
        if !obj.is_null() {
            self.ensure_attached();
            unsafe { (self.gchandle_new)(obj, 0) };
        }
    }
}
