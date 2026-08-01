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
    use std::ffi::{c_char, c_int, c_void, CString};

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

    extern "C" {
        fn cordial_game_activity_start(
            handle: i64,
            width: c_int,
            height: c_int,
            format: c_int,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
    }

    /// Drive the Activity lifecycle and hand the engine its surface.
    pub fn start(handle: i64, width: i32, height: i32, format: i32) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `handle` came from `initialize`; `err` is a live buffer.
        let rc = unsafe {
            cordial_game_activity_start(
                handle,
                width,
                height,
                format,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
            Err(String::from_utf8_lossy(&err[..end]).into_owned())
        }
    }

    extern "C" {
        fn cordial_set_init_params(
            f: *mut c_void,
            assets: *const c_char,
            width: c_int,
            height: c_int,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
    }

    /// `MainGameActivity.nativeAppBridgeSetInitParams` — where the service lives,
    /// what the device is, and what the viewport looks like. The engine renders
    /// its own app shell and draws nothing until it has these.
    pub fn set_init_params(
        native: *mut c_void,
        assets: &str,
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        let a = CString::new(assets).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `a` outlives the call.
        let rc = unsafe {
            cordial_set_init_params(
                native,
                a.as_ptr(),
                width,
                height,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
            Err(String::from_utf8_lossy(&err[..end]).into_owned())
        }
    }

    extern "C" {
        fn cordial_asset_manager_init(f: *mut c_void, err: *mut c_char, n: usize) -> c_int;
        fn cordial_storage_init(
            f: *mut c_void,
            a: *const c_char,
            b: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_call_bare(f: *mut c_void, err: *mut c_char, n: usize) -> c_int;
        fn cordial_init_flags(
            f: *mut c_void,
            settings: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_appbridge_init(
            f: *mut c_void,
            assets: *const c_char,
            w: c_int,
            h: c_int,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_appbridge_call_bare(f: *mut c_void, err: *mut c_char, n: usize) -> c_int;
        fn cordial_read_local_flags(f: *mut c_void, err: *mut c_char, n: usize) -> c_int;
        fn cordial_appbridge_call_bare_cls(
            f: *mut c_void,
            class_name: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_init_client_settings(
            f: *mut c_void,
            a: *const c_char,
            b: *const c_char,
            c: *const c_char,
            out_result: *mut c_int,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_post_client_settings_loaded(f: *mut c_void, err: *mut c_char, n: usize)
            -> c_int;
        fn cordial_preload_flag_overrides(
            f: *mut c_void,
            json: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_call_static_strings(
            f: *mut c_void,
            class_name: *const c_char,
            args: *const *const c_char,
            n: usize,
            err: *mut c_char,
            n_err: usize,
        ) -> c_int;
        fn cordial_call_static_bool_string(
            f: *mut c_void,
            class_name: *const c_char,
            flag: c_int,
            text: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_set_device_info(
            f: *mut c_void,
            width: c_int,
            height: c_int,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_activity_lifecycle(
            f: *mut c_void,
            activity: *const c_char,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
        fn cordial_appbridge_start_app(
            f: *mut c_void,
            assets: *const c_char,
            w: c_int,
            h: c_int,
            err: *mut c_char,
            n: usize,
        ) -> c_int;
    }

    fn take_err(err: Vec<u8>) -> String {
        let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
        String::from_utf8_lossy(&err[..end]).into_owned()
    }

    /// `JNIAAssetManagerSetup.initNative` — hands the engine its asset manager.
    pub fn asset_manager_init(native: *mut c_void) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `err` is a live buffer.
        let rc = unsafe {
            cordial_asset_manager_init(native, err.as_mut_ptr() as *mut c_char, err.len())
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `LocalStorageManager.initStorageManagerNativeV3`.
    pub fn storage_init(native: *mut c_void, a: &str, b: &str) -> Result<(), String> {
        let ca = CString::new(a).map_err(|e| e.to_string())?;
        let cb = CString::new(b).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: as above; both paths outlive the call.
        let rc = unsafe {
            cordial_storage_init(
                native,
                ca.as_ptr(),
                cb.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// A static native on a named class taking up to three `String` arguments.
    ///
    /// `NativeSettingsInterface.nativeSetFilesDirectory` and friends are how the
    /// app tells the engine which directories it owns. Nothing here called them,
    /// so the engine resolved `appData`, `cache`, `http` and `sounds` against the
    /// working directory instead of absolute storage.
    pub fn call_static_strings(
        native: *mut c_void,
        class_name: &str,
        args: &[&str],
    ) -> Result<(), String> {
        let cls = CString::new(class_name).map_err(|e| e.to_string())?;
        let owned: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; every buffer outlives the call.
        let rc = unsafe {
            cordial_call_static_strings(
                native,
                cls.as_ptr(),
                ptrs.as_ptr(),
                ptrs.len(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// A static native taking `(boolean, String)` — `setTaskSchedulerBackgroundMode`.
    pub fn call_static_bool_string(
        native: *mut c_void,
        class_name: &str,
        flag: bool,
        text: &str,
    ) -> Result<(), String> {
        let cls = CString::new(class_name).map_err(|e| e.to_string())?;
        let t = CString::new(text).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; every buffer outlives the call.
        let rc = unsafe {
            cordial_call_static_bool_string(
                native,
                cls.as_ptr(),
                if flag { 1 } else { 0 },
                t.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `NativeSettingsInterface.nativeSetDeviceInfo(DeviceParams)`.
    pub fn set_device_info(native: *mut c_void, width: i32, height: i32) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `err` outlives the call.
        let rc = unsafe {
            cordial_set_device_info(
                native,
                width,
                height,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `FlagJniInterface.nativeInitializeNativeFlags` — what `bootstrapTheApp`
    /// exists to reach. Without it the engine reports `onFlagsFailed` and stops.
    pub fn init_flags(native: *mut c_void, settings_json: &str) -> Result<(), String> {
        let json = CString::new(settings_json).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; both buffers outlive the call.
        let rc = unsafe {
            cordial_init_flags(
                native,
                json.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `NativeGLInterface.readLocalFlags()` — the offline counterpart to the
    /// network `ClientSettings` fetch. Not on the `ActivityNativeMain` chain
    /// Cordial drives (its only dex caller is a different startup path), so
    /// nothing else here calls it unless a caller in `load.rs` does.
    pub fn read_local_flags(native: *mut c_void) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `err` is a live buffer.
        let rc =
            unsafe { cordial_read_local_flags(native, err.as_mut_ptr() as *mut c_char, err.len()) };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// A no-argument native on a named class. `nativeAppBridgeAppStart` is on
    /// `NativeAppBridgeInterface`, not `NativeGLInterface`.
    pub fn call_bare_on(native: *mut c_void, class_name: &str) -> Result<(), String> {
        let cls = CString::new(class_name).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; buffers outlive the call.
        let rc = unsafe {
            cordial_appbridge_call_bare_cls(
                native,
                cls.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `NativeGLInterface.nativeInitClientSettings(String, String, String)I` —
    /// what the real app calls after fetching client settings itself. Cordial
    /// *is* the host app in this architecture, so this is the legitimate
    /// interface, not a workaround. Returns the engine's own `int` result
    /// code, which is a better signal than anything printed to the log.
    pub fn init_client_settings(native: *mut c_void, a: &str, b: &str, c: &str) -> Result<i32, String> {
        let ca = CString::new(a).map_err(|e| e.to_string())?;
        let cb = CString::new(b).map_err(|e| e.to_string())?;
        let cc = CString::new(c).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        let mut out: c_int = 0;
        // SAFETY: `native` is the exported JNI native; all buffers outlive the call.
        let rc = unsafe {
            cordial_init_client_settings(
                native,
                ca.as_ptr(),
                cb.as_ptr(),
                cc.as_ptr(),
                &mut out as *mut c_int,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(out) } else { Err(take_err(err)) }
    }

    /// `NativeGLInterface.nativePostClientSettingsLoadedInitialization3(List)V`
    /// — the finishing step of the client-settings handshake, called with an
    /// empty `ArrayList`.
    pub fn post_client_settings_loaded(native: *mut c_void) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: as above.
        let rc = unsafe {
            cordial_post_client_settings_loaded(native, err.as_mut_ptr() as *mut c_char, err.len())
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `MainGameActivity.nativePreloadFlagOverrides(String)V` — takes whatever
    /// JSON text is given and hands it straight through, so candidate shapes
    /// can be compared by their effect on the flags verdict / JNI trace.
    pub fn preload_flag_overrides(native: *mut c_void, json: &str) -> Result<(), String> {
        let cs = CString::new(json).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `cs`/`err` outlive the call.
        let rc = unsafe {
            cordial_preload_flag_overrides(
                native,
                cs.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `NativeGLInterface.nativeAppBridgeV2InitWithParams` — the real app-bridge
    /// entry. The launcher Activity targets `ActivityNativeMain`, whose chain runs
    /// through here rather than through AGDK's `MainGameActivity`.
    pub fn appbridge_init(
        native: *mut c_void,
        assets: &str,
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        let a = CString::new(assets).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `a` outlives the call.
        let rc = unsafe {
            cordial_appbridge_init(
                native,
                a.as_ptr(),
                width,
                height,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// A `NativeGLInterface` native taking no arguments — `nativeAppBridgeStartLuaAppDM`.
    pub fn appbridge_call_bare(native: *mut c_void) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: as above.
        let rc = unsafe {
            cordial_appbridge_call_bare(native, err.as_mut_ptr() as *mut c_char, err.len())
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// `nativeAppBridgeV2StartAppWithParams` — the call that hands the engine
    /// its window. Everything before it is setup.
    pub fn appbridge_start_app(
        native: *mut c_void,
        assets: &str,
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        let a = CString::new(assets).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `a` outlives the call.
        let rc = unsafe {
            cordial_appbridge_start_app(
                native,
                a.as_ptr(),
                width,
                height,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// One of `JNIActivityLifecycleCallbacks`' natives. The engine stores
    /// per-Activity context — including the JNI environment it later reaches
    /// through — as these fire.
    pub fn activity_lifecycle(native: *mut c_void, activity: &str) -> Result<(), String> {
        let a = CString::new(activity).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `native` is the exported JNI native; `a` outlives the call.
        let rc = unsafe {
            cordial_activity_lifecycle(
                native,
                a.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
    }

    /// A native taking nothing but the JNI pair — `nativeRetryInit`.
    pub fn call_bare(native: *mut c_void) -> Result<(), String> {
        let mut err = vec![0u8; 512];
        // SAFETY: as above.
        let rc = unsafe { cordial_call_bare(native, err.as_mut_ptr() as *mut c_char, err.len()) };
        if rc == 0 { Ok(()) } else { Err(take_err(err)) }
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

    extern "C" {
        fn cordial_game_activity_touch(
            handle: i64,
            action: c_int,
            x: f32,
            y: f32,
            button_state: c_int,
            action_button: c_int,
            event_time_ms: i64,
            down_time_ms: i64,
            consumed: *mut c_int,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
        fn cordial_game_activity_key(
            handle: i64,
            down: c_int,
            key_code: c_int,
            scan_code: c_int,
            meta_state: c_int,
            repeat_count: c_int,
            unicode_char: c_int,
            event_time_ms: i64,
            down_time_ms: i64,
            consumed: *mut c_int,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
    }

    /// Deliver a synthesised mouse pointer event through `onTouchEventNative`.
    ///
    /// `action` is an Android `MotionEvent.ACTION_*` constant. Returns
    /// `Ok(Some(consumed))` on success; `Ok(None)` if `onTouchEventNative` has
    /// not been registered yet, which happens for every call that arrives
    /// before `initializeNativeCode` has finished — a normal race during
    /// startup, not a failure. `x`/`y` are window-relative pixels, matching the
    /// `dpiScale = 1.0` Cordial reports in `PlatformParams`.
    #[allow(clippy::too_many_arguments)]
    pub fn touch(
        handle: i64,
        action: i32,
        x: f32,
        y: f32,
        button_state: i32,
        action_button: i32,
        event_time_ms: i64,
        down_time_ms: i64,
    ) -> Result<Option<bool>, String> {
        let mut err = vec![0u8; 512];
        let mut consumed: c_int = 0;
        // SAFETY: `handle` came from `initialize`; `err`/`consumed` are live.
        let rc = unsafe {
            cordial_game_activity_touch(
                handle,
                action,
                x,
                y,
                button_state,
                action_button,
                event_time_ms,
                down_time_ms,
                &mut consumed,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        match rc {
            0 => Ok(Some(consumed != 0)),
            -2 => Ok(None),
            _ => Err(take_err(err)),
        }
    }

    /// Deliver a synthesised key event through `onKeyDownNative`/`onKeyUpNative`.
    ///
    /// See `touch`'s doc comment for the `Ok(None)` convention.
    #[allow(clippy::too_many_arguments)]
    pub fn key(
        handle: i64,
        down: bool,
        key_code: i32,
        scan_code: i32,
        meta_state: i32,
        repeat_count: i32,
        unicode_char: i32,
        event_time_ms: i64,
        down_time_ms: i64,
    ) -> Result<Option<bool>, String> {
        let mut err = vec![0u8; 512];
        let mut consumed: c_int = 0;
        // SAFETY: as above.
        let rc = unsafe {
            cordial_game_activity_key(
                handle,
                down as c_int,
                key_code,
                scan_code,
                meta_state,
                repeat_count,
                unicode_char,
                event_time_ms,
                down_time_ms,
                &mut consumed,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        match rc {
            0 => Ok(Some(consumed != 0)),
            -2 => Ok(None),
            _ => Err(take_err(err)),
        }
    }

    extern "C" {
        fn cordial_game_activity_lifecycle(
            handle: i64,
            native_name: *const c_char,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
        fn cordial_game_activity_window_focus(
            handle: i64,
            focused: c_int,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
        fn cordial_game_activity_surface_redraw_needed(
            handle: i64,
            err: *mut c_char,
            err_len: usize,
        ) -> c_int;
    }

    /// One `GameActivity` native shaped `(J)V` — `onPauseNative`,
    /// `onStopNative`, `onSurfaceDestroyedNative`, and `terminateNativeCode`
    /// at teardown. `Ok(None)` when `native_name` was never registered —
    /// treated as "did not happen" rather than an error, matching
    /// `touch`/`key`'s convention.
    pub fn lifecycle(handle: i64, native_name: &str) -> Result<Option<()>, String> {
        let name = CString::new(native_name).map_err(|e| e.to_string())?;
        let mut err = vec![0u8; 512];
        // SAFETY: `handle` came from `initialize`; `name`/`err` outlive the call.
        let rc = unsafe {
            cordial_game_activity_lifecycle(
                handle,
                name.as_ptr(),
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        match rc {
            0 => Ok(Some(())),
            -2 => Ok(None),
            _ => Err(take_err(err)),
        }
    }

    /// `onWindowFocusChangedNative(hasFocus)`. `start` already drives the
    /// `true` case inline at bring-up; this is for the `false` case Android
    /// sends immediately before `onPauseNative` when a run ends.
    pub fn window_focus(handle: i64, focused: bool) -> Result<Option<()>, String> {
        let mut err = vec![0u8; 512];
        // SAFETY: `handle` came from `initialize`; `err` is a live buffer.
        let rc = unsafe {
            cordial_game_activity_window_focus(
                handle,
                if focused { 1 } else { 0 },
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        match rc {
            0 => Ok(Some(())),
            -2 => Ok(None),
            _ => Err(take_err(err)),
        }
    }

    /// `onSurfaceRedrawNeededNative` — the "repaint now" nudge, driven from
    /// X11 `Expose`.
    pub fn surface_redraw_needed(handle: i64) -> Result<Option<()>, String> {
        let mut err = vec![0u8; 512];
        // SAFETY: as above.
        let rc = unsafe {
            cordial_game_activity_surface_redraw_needed(
                handle,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        match rc {
            0 => Ok(Some(())),
            -2 => Ok(None),
            _ => Err(take_err(err)),
        }
    }
}
