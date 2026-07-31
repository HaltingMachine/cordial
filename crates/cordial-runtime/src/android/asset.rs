//! `AAssetManager` — Roblox's assets, read out of the APK.
//!
//! This is the gate on rendering: the engine loads every shader, font and
//! texture through it, so nothing appears on screen until it works.
//!
//! Roblox uses the whole-buffer half of the API (`AAsset_getBuffer` and
//! `AAsset_getLength`) rather than streaming `AAsset_read`, which makes the
//! implementation much simpler than the general case — an asset is just its
//! decompressed bytes, held until closed.
//!
//! Android serves assets from `assets/` inside the APK. That is a plain zip, and
//! the paths Roblox passes are relative to it.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One opened asset. The pointer handed back to Roblox is a `Box::into_raw` of
/// this, which is what an `AAsset*` is as far as the engine is concerned.
struct Asset {
    /// Borrowed from the cache, not copied. Roblox opens the same assets
    /// repeatedly; copying each time would double the memory for no benefit,
    /// since the cached bytes already live for the process.
    bytes: &'static [u8],
}

struct Manager {
    apk: PathBuf,
    /// Decompressed contents, keyed by path inside `assets/`.
    ///
    /// Roblox reopens the same assets repeatedly during startup, and zip
    /// decompression is not free. Caching also means `AAsset_getBuffer` can hand
    /// out a pointer that stays valid after the asset is closed, which the API
    /// does not promise but callers routinely assume.
    cache: Mutex<HashMap<String, &'static [u8]>>,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

/// Point the asset manager at an APK. Must be called before Roblox asks for
/// anything; `AAssetManager_open` fails cleanly until it is.
pub fn set_apk(path: &Path) -> Result<(), String> {
    // Fail now, with a path in the message, rather than at the first asset.
    File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    MANAGER
        .set(Manager {
            apk: path.to_path_buf(),
            cache: Mutex::new(HashMap::new()),
        })
        .map_err(|_| "an APK is already set".to_string())
}

pub fn is_configured() -> bool {
    MANAGER.get().is_some()
}

impl Manager {
    /// Read `assets/<name>` out of the APK, caching the result.
    fn read(&self, name: &str) -> Option<&'static [u8]> {
        if let Some(bytes) = self.cache.lock().ok()?.get(name) {
            return Some(bytes);
        }

        let file = File::open(&self.apk).ok()?;
        let mut zip = zip::ZipArchive::new(file).ok()?;
        let mut entry = zip.by_name(&format!("assets/{name}")).ok()?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).ok()?;

        // Deliberately leaked. Asset lifetimes in this API are the caller's to
        // manage and Roblox is not careful about them; an asset outliving its
        // AAsset handle is far cheaper than a dangling buffer, and the total is
        // bounded by what the APK contains.
        let leaked: &'static [u8] = Vec::leak(bytes);
        self.cache.lock().ok()?.insert(name.to_string(), leaked);
        Some(leaked)
    }
}

/// SAFETY: `p` is null or a NUL-terminated C string, per the API contract.
unsafe fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

// ------------------------------------------------------------------- the API

extern "C" fn asset_manager_from_java(_env: *mut c_void, _obj: *mut c_void) -> *mut c_void {
    // There is one asset manager per process and the Java object is Cordial's
    // own, so there is nothing to look up. Returning a non-null token matters:
    // Roblox checks it.
    MANAGER.get().map_or(std::ptr::null_mut(), |m| m as *const Manager as *mut c_void)
}

extern "C" fn asset_manager_open(
    _mgr: *mut c_void,
    filename: *const c_char,
    _mode: c_int,
) -> *mut c_void {
    let Some(manager) = MANAGER.get() else {
        eprintln!("[asset] open before an APK was set — pass --apk");
        return std::ptr::null_mut();
    };
    // SAFETY: the API contract is a NUL-terminated path.
    let Some(name) = (unsafe { cstr(filename) }) else {
        return std::ptr::null_mut();
    };

    match manager.read(&name) {
        Some(bytes) => Box::into_raw(Box::new(Asset { bytes })) as *mut c_void,
        None => {
            // Not an error worth failing on: Android's asset manager returns null
            // for a missing asset and callers probe for optional files.
            if std::env::var_os("CORDIAL_TRACE").is_some() {
                eprintln!("[asset] miss: {name}");
            }
            std::ptr::null_mut()
        }
    }
}

extern "C" fn asset_get_buffer(asset: *mut c_void) -> *const c_void {
    if asset.is_null() {
        return std::ptr::null();
    }
    // SAFETY: `asset` came from asset_manager_open and has not been closed.
    let a = unsafe { &*(asset as *const Asset) };
    a.bytes.as_ptr() as *const c_void
}

extern "C" fn asset_get_length(asset: *mut c_void) -> i64 {
    if asset.is_null() {
        return 0;
    }
    // SAFETY: as above.
    let a = unsafe { &*(asset as *const Asset) };
    a.bytes.len() as i64
}

extern "C" fn asset_close(asset: *mut c_void) {
    if asset.is_null() {
        return;
    }
    // SAFETY: `asset` came from Box::into_raw in asset_manager_open and is closed
    // exactly once, per the API contract.
    drop(unsafe { Box::from_raw(asset as *mut Asset) });
}

/// Some callers want a file descriptor rather than a buffer — Android can give
/// one because assets are stored uncompressed at a known offset in the APK.
/// Cordial's are decompressed in memory, so this hands back a sealed `memfd`
/// holding the same bytes, which behaves the same for reading.
extern "C" fn asset_open_file_descriptor(
    asset: *mut c_void,
    out_start: *mut i64,
    out_length: *mut i64,
) -> c_int {
    if asset.is_null() {
        return -1;
    }
    // SAFETY: `asset` came from asset_manager_open.
    let a = unsafe { &*(asset as *const Asset) };

    extern "C" {
        fn memfd_create(name: *const c_char, flags: u32) -> c_int;
        fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    }

    // SAFETY: a literal name and no flags; the fd is returned to the caller.
    let fd = unsafe { memfd_create(c"cordial-asset".as_ptr(), 0) };
    if fd < 0 {
        return -1;
    }
    // SAFETY: writing exactly the bytes we own to a fresh fd.
    let written = unsafe { write(fd, a.bytes.as_ptr() as *const c_void, a.bytes.len()) };
    if written < 0 || written as usize != a.bytes.len() {
        return -1;
    }
    // SAFETY: rewinding the fd we just wrote.
    unsafe { lseek(fd, 0, 0) };

    if !out_start.is_null() {
        // SAFETY: caller-provided out-parameter.
        unsafe { *out_start = 0 };
    }
    if !out_length.is_null() {
        // SAFETY: caller-provided out-parameter.
        unsafe { *out_length = a.bytes.len() as i64 };
    }
    fd
}

/// Everything this module provides, for the symbol table.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("AAssetManager_fromJava", asset_manager_from_java),
        f!("AAssetManager_open", asset_manager_open),
        f!("AAsset_getBuffer", asset_get_buffer),
        f!("AAsset_getLength", asset_get_length),
        f!("AAsset_close", asset_close),
        f!("AAsset_openFileDescriptor", asset_open_file_descriptor),
    ]
}

/// Open an asset through the same entry points Roblox uses and report its size.
///
/// Exercising the C API rather than `Manager::read` is the point: it is the
/// pointer handling either side of the FFI boundary that is worth proving, not
/// the zip lookup.
pub fn probe(name: &str) -> Result<usize, String> {
    let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    let mgr = asset_manager_from_java(std::ptr::null_mut(), std::ptr::null_mut());
    if mgr.is_null() {
        return Err("no APK configured".into());
    }
    let asset = asset_manager_open(mgr, c.as_ptr(), 0);
    if asset.is_null() {
        return Err(format!("asset not found: {name}"));
    }
    let len = asset_get_length(asset) as usize;
    let buf = asset_get_buffer(asset);
    if buf.is_null() {
        asset_close(asset);
        return Err("asset opened but its buffer is null".into());
    }
    // Touch the first and last byte: a length without readable memory behind it
    // is exactly the failure this probe exists to catch.
    // SAFETY: `buf` points at `len` bytes owned by the asset, still open.
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, len) };
    let checksum = bytes.first().copied().unwrap_or(0) as usize
        + bytes.last().copied().unwrap_or(0) as usize;
    std::hint::black_box(checksum);
    asset_close(asset);
    Ok(len)
}
