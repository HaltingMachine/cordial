//! Symbols Cordial implements itself, because neither a host library nor a stub
//! is right.
//!
//! Three kinds live here:
//!
//! * **bionic-only entry points** with a host equivalent under a different name
//!   (`__errno` is glibc's `__errno_location`).
//! * **FORTIFY wrappers** (`__strlen_chk` and friends). bionic compiles these in
//!   at `-D_FORTIFY_SOURCE`; each is its unchecked counterpart plus a bound the
//!   caller already knows. Forwarding loses the check, not the behaviour.
//! * **diagnostics that must not silently succeed.** `__assert2` is the one that
//!   matters: as a stub returning 0, a failed assertion inside Roblox continues
//!   with corrupt state and fails somewhere unrelated later. That is exactly the
//!   phantom-state debugging the estimation guidance warns about.
//!
//! Everything here is deliberately small. The real bionic surface is a shim's
//! job; see docs/base-evaluation.md §4.

use std::ffi::{c_char, c_int, c_void, CStr};

pub mod pthread;
pub mod trace;

/// Functions Cordial provides. Consulted before any host library.
pub fn function_overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    let mut v = vec![
        f!("__errno", bionic_errno),
        f!("__assert", bionic_assert),
        f!("__assert2", bionic_assert2),
        f!("__gnu_strerror_r", gnu_strerror_r),
        // FORTIFY wrappers.
        f!("__strlen_chk", strlen_chk),
        f!("__strchr_chk", strchr_chk),
        f!("__strncpy_chk2", strncpy_chk2),
        f!("__write_chk", write_chk),
        f!("__umask_chk", umask_chk),
        f!("__poll_chk", poll_chk),
        f!("__readlink_chk", readlink_chk),
        f!("__fread_chk", fread_chk),
        // Android system properties. Desktop values, so Roblox stops believing
        // it is on a phone. Framework-layer policy, implemented here because the
        // native side reads it through libc.
        f!("__system_property_get", system_property_get),
        // Selector numbering differs wholesale between the two libcs.
        f!("sysconf", bionic_sysconf),
    ];
    // Synchronisation primitives whose bionic layout differs from glibc's.
    v.extend(pthread::overrides());
    // A silent abort costs more debugging time than these wrappers cost anything.
    v.extend(trace::always_on());
    if std::env::var_os("CORDIAL_TRACE").is_some() {
        trace::enable();
        v.extend(trace::verbose());
    }
    v
}

/// Data symbols. The *address* is what matters, not a call.
pub fn data_overrides() -> Vec<(&'static str, *mut c_void)> {
    vec![
        (
            "__stack_chk_guard",
            std::ptr::addr_of!(STACK_CHK_GUARD) as *mut c_void,
        ),
        ("__sF", std::ptr::addr_of!(LEGACY_SF) as *mut c_void),
    ]
}

/// bionic's pre-API-23 `FILE __sF[3]`, where `stdout` was the macro `&__sF[1]`.
///
/// Roblox targets SDK 35 and reaches the standard streams through the modern
/// `stdin`/`stdout`/`stderr` pointer variables, which resolve to the host's
/// directly — both libcs store a `FILE*` in a variable of that name, so the
/// indirection matches and no translation is needed. Something statically linked
/// into libroblox.so was nonetheless compiled against the older headers and
/// still imports `__sF`.
///
/// It must at least be readable memory. As a function-pointer stub it is not:
/// glibc walks its stream list at process exit, finds the stub address where a
/// FILE should be, and reports "invalid stdio handle" after everything else has
/// already succeeded.
///
/// Zeroed storage stops the crash. It does **not** make the legacy streams work
/// — anything that actually writes through `&__sF[k]` needs each FILE*-taking
/// function wrapped to remap the pointer onto the host's real stream, which is
/// only worth building once something is observed using it.
static LEGACY_SF: [[u8; LEGACY_FILE_SIZE]; 3] = [[0; LEGACY_FILE_SIZE]; 3];

/// `sizeof(struct __sFILE)` in pre-M bionic on LP64. Only the total matters: the
/// array has to span the addresses a legacy caller would compute.
const LEGACY_FILE_SIZE: usize = 152;

/// The stack canary. Its value is arbitrary as long as it is stable for the
/// process: the function prologue and epilogue compare against the same word.
/// Low byte zero is the usual convention — it terminates string copies.
static STACK_CHK_GUARD: usize = 0x0011_2233_4455_6600usize.to_le();

extern "C" {
    fn __errno_location() -> *mut c_int;
    fn strlen(s: *const c_char) -> usize;
    fn strchr(s: *const c_char, c: c_int) -> *mut c_char;
    fn strncpy(dst: *mut c_char, src: *const c_char, n: usize) -> *mut c_char;
    fn strerror_r(errnum: c_int, buf: *mut c_char, buflen: usize) -> c_int;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn umask(mode: u32) -> u32;
    fn poll(fds: *mut c_void, nfds: u64, timeout: c_int) -> c_int;
    fn readlink(path: *const c_char, buf: *mut c_char, bufsize: usize) -> isize;
    fn fread(ptr: *mut c_void, size: usize, n: usize, stream: *mut c_void) -> usize;
}

extern "C" fn bionic_errno() -> *mut c_int {
    // SAFETY: glibc's per-thread errno slot; the same contract as bionic's.
    unsafe { __errno_location() }
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        "(null)".into()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

extern "C" fn bionic_assert(file: *const c_char, line: c_int, msg: *const c_char) -> ! {
    // SAFETY: bionic passes NUL-terminated strings or null.
    unsafe {
        eprintln!("\n*** Roblox assertion failed ***");
        eprintln!("    {}:{}: {}", cstr(file), line, cstr(msg));
    }
    crate::stubs::report();
    std::process::abort();
}

extern "C" fn bionic_assert2(
    file: *const c_char,
    line: c_int,
    function: *const c_char,
    failed: *const c_char,
) -> ! {
    // SAFETY: bionic passes NUL-terminated strings or null.
    unsafe {
        eprintln!("\n*** Roblox assertion failed ***");
        eprintln!("    {}:{} in {}", cstr(file), line, cstr(function));
        eprintln!("    assertion: {}", cstr(failed));
    }
    crate::stubs::report();
    std::process::abort();
}

/// bionic's GNU-flavoured `strerror_r`, which returns `char*` rather than int.
extern "C" fn gnu_strerror_r(errnum: c_int, buf: *mut c_char, buflen: usize) -> *mut c_char {
    // SAFETY: caller-supplied buffer of at least `buflen` bytes.
    unsafe {
        strerror_r(errnum, buf, buflen);
    }
    buf
}

extern "C" fn strlen_chk(s: *const c_char, _bound: usize) -> usize {
    unsafe { strlen(s) }
}

extern "C" fn strchr_chk(s: *const c_char, c: c_int, _bound: usize) -> *mut c_char {
    unsafe { strchr(s, c) }
}

extern "C" fn strncpy_chk2(
    dst: *mut c_char,
    src: *const c_char,
    n: usize,
    _dst_len: usize,
    _src_len: usize,
) -> *mut c_char {
    unsafe { strncpy(dst, src, n) }
}

extern "C" fn write_chk(fd: c_int, buf: *const c_void, count: usize, _bound: usize) -> isize {
    unsafe { write(fd, buf, count) }
}

extern "C" fn umask_chk(mode: u32) -> u32 {
    unsafe { umask(mode) }
}

extern "C" fn poll_chk(fds: *mut c_void, nfds: u64, timeout: c_int, _bound: usize) -> c_int {
    unsafe { poll(fds, nfds, timeout) }
}

extern "C" fn readlink_chk(
    path: *const c_char,
    buf: *mut c_char,
    size: usize,
    _bound: usize,
) -> isize {
    unsafe { readlink(path, buf, size) }
}

extern "C" fn fread_chk(
    ptr: *mut c_void,
    size: usize,
    n: usize,
    stream: *mut c_void,
    _bound: usize,
) -> usize {
    unsafe { fread(ptr, size, n, stream) }
}

/// What Cordial reports for `ro.*` system properties.
///
/// These are the values §4.2's "Roblox thinks you're mobile" fix turns on. They
/// are guesses until the client is observed reacting to them; the point for now
/// is that the call succeeds and returns something desktop-shaped rather than an
/// empty string.
const PROPERTIES: &[(&str, &str)] = &[
    ("ro.build.version.sdk", "35"),
    ("ro.build.version.release", "15"),
    ("ro.product.model", "Cordial"),
    ("ro.product.manufacturer", "Cordial"),
    ("ro.product.brand", "cordial"),
    ("ro.product.device", "linux"),
    ("ro.product.name", "cordial"),
    ("ro.hardware", "cordial"),
    ("ro.board.platform", "cordial"),
    ("ro.debuggable", "0"),
    ("ro.secure", "1"),
];

/// bionic's `__system_property_get(name, value)`. `value` is a caller buffer of
/// `PROP_VALUE_MAX` (92) bytes; the return is the length written.
extern "C" fn system_property_get(name: *const c_char, value: *mut c_char) -> c_int {
    const PROP_VALUE_MAX: usize = 92;
    if value.is_null() {
        return 0;
    }
    // SAFETY: bionic's contract is a NUL-terminated name and a 92-byte buffer.
    let key = unsafe { cstr(name) };
    let found = PROPERTIES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or("");

    let bytes = found.as_bytes();
    let n = bytes.len().min(PROP_VALUE_MAX - 1);
    // SAFETY: `value` has at least PROP_VALUE_MAX bytes per the ABI, and n < that.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), value as *mut u8, n);
        *value.add(n) = 0;
    }
    n as c_int
}

// ---------------------------------------------------------------------- sysconf

pub mod sysconf_table;

extern "C" {
    fn sysconf(name: c_int) -> i64;
}

/// bionic's `sysconf`, translated.
///
/// The selectors are not shared: bionic's `_SC_PAGESIZE` is 39, glibc's is 30,
/// and glibc reads 39 as `_SC_BC_STRING_MAX`. Roblox asks for the page size
/// during static initialisation and, untranslated, is told 1000 — which is not a
/// power of two, so the allocator that asked aborts. Nothing about that failure
/// points back at `sysconf`, which is why the translation is worth its table.
extern "C" fn bionic_sysconf(name: c_int) -> i64 {
    if let Some(&(_, glibc, _)) = sysconf_table::SYSCONF_MAP
        .iter()
        .find(|(bionic, _, _)| *bionic == name)
    {
        // SAFETY: `glibc` came from the generated table and is a valid selector.
        return unsafe { sysconf(glibc) };
    }

    if let Some((_, sym)) = sysconf_table::UNSUPPORTED
        .iter()
        .find(|(bionic, _)| *bionic == name)
    {
        eprintln!("[bionic] sysconf({sym}) has no glibc equivalent; returning -1");
        return -1;
    }

    eprintln!("[bionic] sysconf({name}) is not a selector bionic defines; returning -1");
    -1
}
