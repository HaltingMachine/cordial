//! Translating bionic's legacy `__sF` stdio handles onto glibc's streams.
//!
//! Pre-API-23 bionic exposed `FILE __sF[3]` and defined `stdout` as the macro
//! `&__sF[1]`. Roblox targets SDK 35 and reaches the standard streams through the
//! modern pointer variables — but something statically linked into
//! `libroblox.so` was built against the older headers and still uses `__sF`.
//!
//! Cordial provides `__sF` as zeroed storage (see [`super::LEGACY_SF`]), which is
//! enough to stop glibc walking a function pointer at exit. It is not enough to
//! *use*: handing `&__sF[k]` to glibc's `fflush` makes it read a `FILE` out of
//! zeroes and dereference a null lock, and the crash lands inside libc with
//! nothing naming the caller.
//!
//! So every `FILE*`-taking function Roblox imports is wrapped here. A pointer
//! inside the `__sF` region is translated to the host's real stream; anything
//! else — a `FILE*` from `fopen`, which is already glibc's — passes straight
//! through.
//!
//! The one thing not knowable from outside is bionic's `sizeof(FILE)`, which
//! decides which of the three slots a pointer names. Rather than guess a stride
//! and mis-map two streams out of three, the region is split into equal thirds:
//! a pointer in the first third is `stdin`, the second `stdout`, the last
//! `stderr`. That is correct for any stride the array was built with, because
//! the slots are equal-sized by construction.

use std::ffi::{c_char, c_int, c_void};

extern "C" {
    static stdin: *mut c_void;
    static stdout: *mut c_void;
    static stderr: *mut c_void;

    fn fflush(f: *mut c_void) -> c_int;
    fn fclose(f: *mut c_void) -> c_int;
    fn fwrite(p: *const c_void, size: usize, n: usize, f: *mut c_void) -> usize;
    fn fread(p: *mut c_void, size: usize, n: usize, f: *mut c_void) -> usize;
    fn fputs(s: *const c_char, f: *mut c_void) -> c_int;
    fn fputc(c: c_int, f: *mut c_void) -> c_int;
    fn fgets(s: *mut c_char, n: c_int, f: *mut c_void) -> *mut c_char;
    fn fileno(f: *mut c_void) -> c_int;
    fn ferror(f: *mut c_void) -> c_int;
    fn feof(f: *mut c_void) -> c_int;
    fn clearerr(f: *mut c_void);
    fn setvbuf(f: *mut c_void, buf: *mut c_char, mode: c_int, size: usize) -> c_int;
    fn fseek(f: *mut c_void, off: std::ffi::c_long, whence: c_int) -> c_int;
    fn ftell(f: *mut c_void) -> std::ffi::c_long;
    fn getc(f: *mut c_void) -> c_int;
    fn ungetc(c: c_int, f: *mut c_void) -> c_int;
}

/// Translate a `FILE*` that may be one of bionic's legacy handles.
fn real(f: *mut c_void) -> *mut c_void {
    let base = super::legacy_sf_range();
    let addr = f as usize;
    if addr < base.start || addr >= base.end {
        return f;
    }
    // Equal thirds, so the mapping holds whatever stride the caller assumed.
    let third = (base.end - base.start) / 3;
    let index = (addr - base.start) / third.max(1);
    // SAFETY: reading glibc's own stream pointers.
    unsafe {
        match index {
            0 => stdin,
            1 => stdout,
            _ => stderr,
        }
    }
}

macro_rules! wrap {
    ($name:ident, $host:ident, ($($arg:ident : $ty:ty),*) -> $ret:ty) => {
        extern "C" fn $name(f: *mut c_void $(, $arg: $ty)*) -> $ret {
            // SAFETY: forwarding the caller's arguments to the host's own
            // implementation, with only the stream pointer translated.
            unsafe { $host(real(f) $(, $arg)*) }
        }
    };
}

wrap!(s_fflush, fflush, () -> c_int);
wrap!(s_fclose, fclose, () -> c_int);
wrap!(s_fileno, fileno, () -> c_int);
wrap!(s_ferror, ferror, () -> c_int);
wrap!(s_feof, feof, () -> c_int);
wrap!(s_ftell, ftell, () -> std::ffi::c_long);
wrap!(s_getc, getc, () -> c_int);

extern "C" fn s_clearerr(f: *mut c_void) {
    // SAFETY: as above.
    unsafe { clearerr(real(f)) }
}

extern "C" fn s_fwrite(p: *const c_void, size: usize, n: usize, f: *mut c_void) -> usize {
    // SAFETY: as above.
    unsafe { fwrite(p, size, n, real(f)) }
}

extern "C" fn s_fread(p: *mut c_void, size: usize, n: usize, f: *mut c_void) -> usize {
    // SAFETY: as above.
    unsafe { fread(p, size, n, real(f)) }
}

extern "C" fn s_fputs(s: *const c_char, f: *mut c_void) -> c_int {
    // SAFETY: as above.
    unsafe { fputs(s, real(f)) }
}

extern "C" fn s_fputc(c: c_int, f: *mut c_void) -> c_int {
    // SAFETY: as above.
    unsafe { fputc(c, real(f)) }
}

extern "C" fn s_fgets(s: *mut c_char, n: c_int, f: *mut c_void) -> *mut c_char {
    // SAFETY: as above.
    unsafe { fgets(s, n, real(f)) }
}

extern "C" fn s_ungetc(c: c_int, f: *mut c_void) -> c_int {
    // SAFETY: as above.
    unsafe { ungetc(c, real(f)) }
}

extern "C" fn s_setvbuf(f: *mut c_void, buf: *mut c_char, mode: c_int, size: usize) -> c_int {
    // SAFETY: as above.
    unsafe { setvbuf(real(f), buf, mode, size) }
}

extern "C" fn s_fseek(f: *mut c_void, off: std::ffi::c_long, whence: c_int) -> c_int {
    // SAFETY: as above.
    unsafe { fseek(real(f), off, whence) }
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("fflush", s_fflush),
        f!("fclose", s_fclose),
        f!("fwrite", s_fwrite),
        f!("fread", s_fread),
        f!("fputs", s_fputs),
        f!("fputc", s_fputc),
        f!("fgets", s_fgets),
        f!("fileno", s_fileno),
        f!("ferror", s_ferror),
        f!("feof", s_feof),
        f!("clearerr", s_clearerr),
        f!("setvbuf", s_setvbuf),
        f!("fseek", s_fseek),
        f!("ftell", s_ftell),
        f!("getc", s_getc),
        f!("ungetc", s_ungetc),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_handles_map_to_the_host_streams() {
        let range = super::super::legacy_sf_range();
        let third = (range.end - range.start) / 3;

        // SAFETY: comparing against glibc's own stream pointers.
        unsafe {
            assert_eq!(real(range.start as *mut c_void), stdin);
            assert_eq!(real((range.start + third) as *mut c_void), stdout);
            assert_eq!(real((range.start + 2 * third) as *mut c_void), stderr);
        }
    }

    #[test]
    fn a_real_file_pointer_passes_through() {
        // Anything from fopen is already glibc's and must not be rewritten.
        let f = 0x1234_5678usize as *mut c_void;
        assert_eq!(real(f), f);
    }
}
