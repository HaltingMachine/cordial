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
