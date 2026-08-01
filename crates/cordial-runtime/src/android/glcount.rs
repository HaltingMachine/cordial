//! Counts the graphics calls Roblox makes.
//!
//! "The engine took the surface" and "the engine is drawing" are different
//! claims. Nothing in the log distinguishes them, and a window can look plausible
//! while the compositor is painting all of it. Counting the calls that only a
//! rendering engine makes — creating a window surface, clearing, drawing,
//! swapping — settles it either way.
//!
//! These wrap the host's real functions and forward, so they cost a counter
//! increment per call and change nothing else.

use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! counters {
    ($($name:ident => $sym:literal),* $(,)?) => {
        $(pub static $name: AtomicU64 = AtomicU64::new(0);)*

        /// Every counter, for reporting.
        pub fn report() -> Vec<(&'static str, u64)> {
            vec![$(($sym, $name.load(Ordering::Relaxed))),*]
        }
    };
}

counters! {
    CREATE_WINDOW_SURFACE => "eglCreateWindowSurface",
    MAKE_CURRENT          => "eglMakeCurrent",
    SWAP_BUFFERS          => "eglSwapBuffers",
    CLEAR                 => "glClear",
    DRAW_ELEMENTS         => "glDrawElements",
    DRAW_ARRAYS           => "glDrawArrays",
    COMPILE_SHADER        => "glCompileShader",
    TEX_IMAGE_2D          => "glTexImage2D",
}

/// The host implementation each wrapper forwards to, resolved once.
fn host(sym: &str) -> *mut c_void {
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    let Ok(c) = CString::new(sym) else {
        return std::ptr::null_mut();
    };
    // SAFETY: RTLD_DEFAULT with a valid symbol name; libEGL and libGLESv2 are
    // already loaded into the global scope by the symbol table.
    unsafe { dlsym(std::ptr::null_mut(), c.as_ptr()) }
}

/// Build a counting wrapper for `sym`, or `None` if the host lacks it.
///
/// The wrappers take and return `*mut c_void` uniformly and forward with the
/// caller's registers untouched, which works because every counted entry point
/// passes its arguments in integer registers. `glClearColor` takes floats and is
/// deliberately not counted for that reason.
macro_rules! forward {
    ($counter:ident, $sym:literal, ($($a:ident),*)) => {{
        extern "C" fn wrapper($($a: *mut c_void),*) -> *mut c_void {
            $counter.fetch_add(1, Ordering::Relaxed);
            type Fn_ = extern "C" fn($(replace!($a)),*) -> *mut c_void;
            // SAFETY: resolved from the host for exactly this name, and called
            // with the arguments the caller passed through unchanged.
            let f: Fn_ = unsafe { std::mem::transmute(host($sym)) };
            f($($a),*)
        }
        ($sym, wrapper as *const () as *mut c_void)
    }};
}
macro_rules! replace {
    ($a:ident) => { *mut c_void };
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    vec![
        forward!(MAKE_CURRENT, "eglMakeCurrent", (a, b, c, d)),
        forward!(SWAP_BUFFERS, "eglSwapBuffers", (a, b)),
        forward!(CLEAR, "glClear", (a)),
        forward!(DRAW_ELEMENTS, "glDrawElements", (a, b, c, d)),
        forward!(DRAW_ARRAYS, "glDrawArrays", (a, b, c)),
        forward!(COMPILE_SHADER, "glCompileShader", (a)),
        forward!(TEX_IMAGE_2D, "glTexImage2D", (a, b, c, d, e, f, g, h, i)),
    ]
}
