//! Bindings to the AOSP bionic linker, retargeted to the host by
//! `mcpelauncher-linker` and wrapped in `native/shim.cpp`.
//!
//! This crate is deliberately thin: it exposes the linker's operations and
//! nothing else. Symbol-table policy — what Cordial provides for each Android
//! library — lives in `cordial-runtime`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    extern "C" {
        pub fn cordial_linker_init();
        pub fn cordial_linker_load_library(
            name: *const c_char,
            names: *const *const c_char,
            addrs: *const *mut c_void,
            n: usize,
        ) -> *mut c_void;
        pub fn cordial_linker_update_ld_library_path(path: *const c_char);
        pub fn cordial_linker_dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        pub fn cordial_linker_dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        pub fn cordial_linker_dlerror() -> *const c_char;
        pub fn cordial_linker_get_library_base(handle: *mut c_void) -> usize;
        pub fn cordial_linker_get_library_code_region(
            handle: *mut c_void,
            base: *mut usize,
            size: *mut usize,
        );
    }
}

/// `RTLD_NOW` — resolve every relocation at load time. Cordial always uses this:
/// a lazy load would report success and then fail later on an unrelated call.
pub const RTLD_NOW: c_int = 2;
pub const RTLD_LAZY: c_int = 1;

/// A library loaded by, or registered with, the bionic linker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Library(*mut c_void);

// The linker keeps its own global state under its own lock; a handle is just an
// index into it. Sending one between threads is no less safe than using it.
unsafe impl Send for Library {}

impl Library {
    pub fn as_ptr(self) -> *mut c_void {
        self.0
    }

    /// Base address the object was mapped at.
    pub fn base(self) -> usize {
        unsafe { ffi::cordial_linker_get_library_base(self.0) }
    }

    /// Address and length of the executable segment.
    pub fn code_region(self) -> (usize, usize) {
        let (mut base, mut size) = (0usize, 0usize);
        unsafe { ffi::cordial_linker_get_library_code_region(self.0, &mut base, &mut size) };
        (base, size)
    }

    pub fn symbol(self, name: &str) -> Option<*mut c_void> {
        let c = CString::new(name).ok()?;
        let p = unsafe { ffi::cordial_linker_dlsym(self.0, c.as_ptr()) };
        (!p.is_null()).then_some(p)
    }
}

/// Initialise the linker's solist and register its built-in `libdl.so`.
///
/// Must be called once, before anything else in this module.
pub fn init() {
    unsafe { ffi::cordial_linker_init() }
}

/// Register a virtual library: an soname that exists only as the symbol table
/// given here. This is how Cordial provides `libc.so`, `libandroid.so`,
/// `libEGL.so` and the rest — the loaded object's `DT_NEEDED` entries resolve
/// against these instead of against anything on disk.
pub fn register(name: &str, symbols: &[(String, *mut c_void)]) -> Result<Library, Error> {
    // bionic's soinfo::set_soname() stores the pointer it is given rather than
    // copying the string (linker_soinfo.cpp: `soname_ = soname`). AOSP gets away
    // with it because callers pass string literals. A CString dropped at the end
    // of this function would leave every registered library with a dangling
    // soname, and DT_NEEDED lookups would then silently fail to match.
    //
    // So the name is leaked deliberately. There are a dozen of these for the
    // process lifetime.
    let cname: &'static CString = Box::leak(Box::new(CString::new(name)?));

    let cnames = symbols
        .iter()
        .map(|(s, _)| CString::new(s.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let name_ptrs: Vec<*const c_char> = cnames.iter().map(|c| c.as_ptr()).collect();
    let addrs: Vec<*mut c_void> = symbols.iter().map(|(_, a)| *a).collect();

    let handle = unsafe {
        ffi::cordial_linker_load_library(
            cname.as_ptr(),
            name_ptrs.as_ptr(),
            addrs.as_ptr(),
            symbols.len(),
        )
    };
    if handle.is_null() {
        Err(Error::Linker(last_error()))
    } else {
        Ok(Library(handle))
    }
}

/// Directory the linker searches for real objects.
pub fn set_library_path(path: &str) -> Result<(), Error> {
    let c = CString::new(path)?;
    unsafe { ffi::cordial_linker_update_ld_library_path(c.as_ptr()) };
    Ok(())
}

/// Load a real ELF object, resolving its imports against previously registered
/// libraries.
pub fn dlopen(soname: &str, flags: c_int) -> Result<Library, Error> {
    let c = CString::new(soname)?;
    let handle = unsafe { ffi::cordial_linker_dlopen(c.as_ptr(), flags) };
    if handle.is_null() {
        Err(Error::Linker(last_error()))
    } else {
        Ok(Library(handle))
    }
}

fn last_error() -> String {
    let p = unsafe { ffi::cordial_linker_dlerror() };
    if p.is_null() {
        "unknown linker error".into()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

#[derive(Debug)]
pub enum Error {
    Linker(String),
    NulByte(std::ffi::NulError),
}

impl From<std::ffi::NulError> for Error {
    fn from(e: std::ffi::NulError) -> Self {
        Error::NulByte(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Linker(s) => write!(f, "{s}"),
            Error::NulByte(e) => write!(f, "invalid name: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The JNI virtual machine Roblox's native code calls back into.
///
/// Roblox registers 518 natives statically, but the traffic that matters runs the
/// other way: native code reaching for Java classes it expects Android to
/// provide. libjnivm answers those calls and records what was asked for, which is
/// how the framework-API backlog stops being a guess.
pub mod jni {
    use std::ffi::{c_char, c_int, c_void, CString};

    extern "C" {
        fn cordial_jni_create_vm() -> *mut c_void;
        fn cordial_jni_env() -> *mut c_void;
        fn cordial_jni_dump_classes(path: *const c_char) -> c_int;
        fn cordial_jni_call_onload(f: *mut c_void, err: *mut c_char, err_len: usize) -> c_int;
    }

    /// Call Roblox's `JNI_OnLoad` with the process JavaVM.
    ///
    /// Any C++ exception is caught on the far side: letting one cross the FFI
    /// boundary gives a core dump and no explanation.
    pub fn call_on_load(f: *mut c_void) -> Result<i32, String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `f` is libroblox's JNI_OnLoad export; `err` is a live buffer of
        // the length passed alongside it.
        let rc = unsafe { cordial_jni_call_onload(f, err.as_mut_ptr() as *mut c_char, err.len()) };
        match rc {
            -1 => Err("no JavaVM, or JNI_OnLoad not found".into()),
            -2 | -3 => {
                let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
                Err(String::from_utf8_lossy(&err[..end]).into_owned())
            }
            v => Ok(v),
        }
    }

    /// Create the process's `JavaVM`. Returns `None` if one already exists.
    pub fn create_vm() -> Option<*mut c_void> {
        // SAFETY: the VM is process-global and owned by the shim.
        let vm = unsafe { cordial_jni_create_vm() };
        (!vm.is_null()).then_some(vm)
    }

    /// The calling thread's `JNIEnv*`.
    pub fn env() -> Option<*mut c_void> {
        // SAFETY: returns null when no VM exists, which is checked.
        let env = unsafe { cordial_jni_env() };
        (!env.is_null()).then_some(env)
    }

    /// Write C++ stubs for every Java class and method the native code reached
    /// for. This is the observed Phase 2 backlog.
    pub fn dump_classes(path: &str) -> Result<(), String> {
        let c = CString::new(path).map_err(|e| e.to_string())?;
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        match unsafe { cordial_jni_dump_classes(c.as_ptr()) } {
            0 => Ok(()),
            -1 => Err("no JavaVM has been created".into()),
            -2 => Err("libjnivm was built without JNI_DEBUG".into()),
            n => Err(format!("class dump failed ({n})")),
        }
    }
}

/// Drive AGDK `GameActivity` bring-up.
///
/// On Android the platform calls `initializeNativeCode` from Java with a real
/// Activity. Cordial builds the arguments through libjnivm and calls the
/// exported JNI native directly. The returned handle is what every later
/// callback carries — surface creation, resize, input.
pub mod game_activity {
    use std::ffi::{c_char, c_void, CString};

    extern "C" {
        fn cordial_game_activity_init(
            f: *mut c_void,
            internal_path: *const c_char,
            obb_path: *const c_char,
            external_path: *const c_char,
            err: *mut c_char,
            err_len: usize,
        ) -> i64;
    }

    pub fn initialize(
        native: *mut c_void,
        internal_path: &str,
        obb_path: &str,
        external_path: &str,
    ) -> Result<i64, String> {
        let internal = CString::new(internal_path).map_err(|e| e.to_string())?;
        let obb = CString::new(obb_path).map_err(|e| e.to_string())?;
        let external = CString::new(external_path).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];

        // SAFETY: `native` is libroblox's initializeNativeCode export; the paths
        // outlive the call. The shim takes the JNI environment from the VM
        // itself — Rust cannot name `jnivm::ENV` and must not pretend to.
        let handle = unsafe {
            cordial_game_activity_init(
                native,
                internal.as_ptr(),
                obb.as_ptr(),
                external.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };

        if handle == 0 {
            let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
            let msg = String::from_utf8_lossy(&err[..end]).into_owned();
            Err(if msg.is_empty() {
                "initializeNativeCode returned a null handle".into()
            } else {
                msg
            })
        } else {
            Ok(handle)
        }
    }
}
