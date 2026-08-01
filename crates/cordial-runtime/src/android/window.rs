//! A host window, and `ANativeWindow_*` over it.
//!
//! Android hands the engine an `ANativeWindow` and it renders into that. On
//! Linux the equivalent is a window-system surface, so this creates one and
//! implements the ten `ANativeWindow_*` entry points Roblox imports against it.
//!
//! X11 is loaded with `dlopen` rather than linked. Cordial has to run its loader
//! and asset tests on machines with no display at all — CI, containers, a remote
//! shell — and a link-time dependency would make the whole binary refuse to
//! start there. Loading late means "no window" is a runtime condition the caller
//! can handle, which is what it actually is.
//!
//! Wayland is the better long-term target and Roblox's Android build has no
//! opinion either way. X11 first because `eglCreateWindowSurface` takes an
//! `xcb_window_t`/`Window` directly, whereas Wayland needs an `wl_egl_window`
//! and a surface role — more moving parts for the same first frame.

use std::ffi::{c_char, c_int, c_ulong, c_void, CString};
use std::sync::{Mutex, OnceLock};

/// Android pixel formats, from `android/native_window.h`.
pub const WINDOW_FORMAT_RGBA_8888: i32 = 1;

type Display = *mut c_void;
type Window = c_ulong;

struct Xlib {
    open_display: unsafe extern "C" fn(*const c_char) -> Display,
    default_root_window: unsafe extern "C" fn(Display) -> Window,
    create_simple_window: unsafe extern "C" fn(
        Display, Window, c_int, c_int, u32, u32, u32, c_ulong, c_ulong,
    ) -> Window,
    map_window: unsafe extern "C" fn(Display, Window) -> c_int,
    store_name: unsafe extern "C" fn(Display, Window, *const c_char) -> c_int,
    flush: unsafe extern "C" fn(Display) -> c_int,
    destroy_window: unsafe extern "C" fn(Display, Window) -> c_int,
}

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;

impl Xlib {
    fn load() -> Result<Self, String> {
        // SAFETY: a literal soname; the handle is never closed.
        let lib = unsafe { dlopen(c"libX11.so.6".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("libX11.so.6 is not available".into());
        }
        macro_rules! sym {
            ($name:literal) => {{
                let name = CString::new($name).unwrap();
                // SAFETY: the handle is open and the names are Xlib's documented
                // exports, so the signatures are the ones declared above.
                let p = unsafe { dlsym(lib, name.as_ptr()) };
                if p.is_null() {
                    return Err(format!("libX11 has no {}", $name));
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Ok(Xlib {
            open_display: sym!("XOpenDisplay"),
            default_root_window: sym!("XDefaultRootWindow"),
            create_simple_window: sym!("XCreateSimpleWindow"),
            map_window: sym!("XMapWindow"),
            store_name: sym!("XStoreName"),
            flush: sym!("XFlush"),
            destroy_window: sym!("XDestroyWindow"),
        })
    }
}

/// A mapped host window and the Android-side state the engine queries about it.
pub struct HostWindow {
    xlib: Xlib,
    display: Display,
    window: Window,
    /// Dimensions the engine asked for via `ANativeWindow_setBuffersGeometry`,
    /// which override the window's own size in every query. Android reports the
    /// buffer geometry, not the surface geometry, and the engine sizes its
    /// framebuffers from the answer.
    buffers: Mutex<Geometry>,
}

#[derive(Clone, Copy)]
struct Geometry {
    width: i32,
    height: i32,
    format: i32,
}

// The window lives for the process and X11 calls are serialised by the caller.
unsafe impl Send for HostWindow {}
unsafe impl Sync for HostWindow {}

static WINDOW: OnceLock<HostWindow> = OnceLock::new();

/// Open a window. Fails cleanly when there is no display, which is a normal
/// condition rather than an error — the loader and asset paths do not need one.
pub fn open(width: u32, height: u32, title: &str) -> Result<&'static HostWindow, String> {
    if let Some(w) = WINDOW.get() {
        return Ok(w);
    }
    let xlib = Xlib::load()?;

    // SAFETY: a null display name means $DISPLAY, per Xlib's contract.
    let display = unsafe { (xlib.open_display)(std::ptr::null()) };
    if display.is_null() {
        return Err("no X display (is DISPLAY set?)".into());
    }

    // SAFETY: `display` is open; the geometry and border/background pixels are
    // plain values.
    let window = unsafe {
        let root = (xlib.default_root_window)(display);
        let w = (xlib.create_simple_window)(display, root, 0, 0, width, height, 0, 0, 0);
        // XStoreName sets WM_NAME, which is XA_STRING — Latin-1, not UTF-8.
        // An em dash here renders as mojibake, so the title is kept ASCII
        // rather than encoded twice for a window caption.
        let ascii: String = title
            .chars()
            .map(|c| if c.is_ascii() { c } else { '-' })
            .collect();
        let name = CString::new(ascii).unwrap_or_default();
        (xlib.store_name)(display, w, name.as_ptr());
        (xlib.map_window)(display, w);
        (xlib.flush)(display);
        w
    };

    let host = HostWindow {
        xlib,
        display,
        window,
        buffers: Mutex::new(Geometry {
            width: width as i32,
            height: height as i32,
            format: WINDOW_FORMAT_RGBA_8888,
        }),
    };
    Ok(WINDOW.get_or_init(|| host))
}

impl HostWindow {
    /// The X11 `Window`, which is what `eglCreateWindowSurface` takes as its
    /// native window on this platform.
    pub fn egl_native_window(&self) -> c_ulong {
        self.window
    }

    pub fn egl_native_display(&self) -> Display {
        self.display
    }

    pub fn geometry(&self) -> (i32, i32, i32) {
        let g = *self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        (g.width, g.height, g.format)
    }

    pub fn close(&self) {
        // SAFETY: both handles came from this struct's own creation calls.
        unsafe {
            (self.xlib.destroy_window)(self.display, self.window);
            (self.xlib.flush)(self.display);
        }
    }
}

pub fn current() -> Option<&'static HostWindow> {
    WINDOW.get()
}

// ------------------------------------------------------- ANativeWindow_*

/// The `ANativeWindow*` handed to the engine.
///
/// There is exactly one window, so the pointer is the `HostWindow` itself rather
/// than a separately allocated handle. `acquire`/`release` are then genuinely
/// no-ops instead of pretending to refcount something with a single owner.
fn handle() -> *mut c_void {
    WINDOW.get().map_or(std::ptr::null_mut(), |w| w as *const HostWindow as *mut c_void)
}

fn as_window(p: *mut c_void) -> Option<&'static HostWindow> {
    (!p.is_null()).then(|| WINDOW.get()).flatten()
}

extern "C" fn native_window_from_surface(_env: *mut c_void, _surface: *mut c_void) -> *mut c_void {
    // Cordial's Java `Surface` has no state of its own: there is one window and
    // the Surface object exists only so `onSurfaceCreatedNative`'s signature can
    // be satisfied. Returning the single window is therefore correct rather than
    // a simplification.
    let w = handle();
    // The returned pointer is traced, not just the call. A null here means the
    // engine was handed nothing to render into and every later step will fail
    // for a reason that looks unrelated — and "the Surface has no native peer"
    // is exactly the kind of plausible diagnosis that has been wrong before on
    // this engine. Printing the value settles it instead of inviting the guess.
    super::trace(format_args!("ANativeWindow_fromSurface -> {w:?}"));
    w
}

extern "C" fn native_window_acquire(window: *mut c_void) {
    let _ = window;
}

extern "C" fn native_window_release(window: *mut c_void) {
    let _ = window;
}

extern "C" fn native_window_get_width(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().0)
}

extern "C" fn native_window_get_height(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().1)
}

extern "C" fn native_window_get_format(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().2)
}

/// The engine states the buffer size and format it wants. Android resizes the
/// underlying buffers; here the values are recorded and reported back, because
/// the EGL surface is sized by the X window and the engine only needs the two to
/// agree.
extern "C" fn native_window_set_buffers_geometry(
    window: *mut c_void,
    width: i32,
    height: i32,
    format: i32,
) -> i32 {
    let Some(w) = as_window(window) else {
        return -22; // -EINVAL
    };
    let mut g = w.buffers.lock().unwrap_or_else(|e| e.into_inner());
    // Zero means "whatever the window is", per the API.
    if width > 0 {
        g.width = width;
    }
    if height > 0 {
        g.height = height;
    }
    if format > 0 {
        g.format = format;
    }
    0
}

/// Direct software access to the window's pixels.
///
/// Roblox renders through GLES, so this is not on its path. Returning an error
/// rather than a fake buffer is deliberate: a caller that gets a buffer will
/// write to it and expect the result on screen, and silently discarding that
/// would be far harder to diagnose than a refused lock.
extern "C" fn native_window_lock(
    _window: *mut c_void,
    _buffer: *mut c_void,
    _dirty: *mut c_void,
) -> i32 {
    -38 // -ENOSYS
}

extern "C" fn native_window_unlock_and_post(_window: *mut c_void) -> i32 {
    -38 // -ENOSYS
}

/// `eglCreateWindowSurface`, with the native window translated.
///
/// Android's EGL takes an `ANativeWindow*`. The host's EGL, on X11, takes a
/// `Window` — an XID. Roblox naturally passes the `ANativeWindow*` Cordial
/// handed it through `ANativeWindow_fromSurface`, and Mesa read that pointer as
/// an XID and answered:
///
/// ```text
/// [FLog::SurfaceController] Mode 4 failed: Error creating context: eglCreateWindowSurface 3003
/// [FLog::SurfaceController] RenderView is NULL
/// ```
///
/// 3003 is `EGL_BAD_ALLOC`. Substituting the real window is the whole fix, and
/// it belongs here rather than in `glcount` because the translation is not
/// diagnostic — without it there is no surface at all, whether or not anyone
/// asked for call counts.
///
/// There is exactly one window in this runtime, so any pointer arriving here is
/// that window; the argument is replaced unconditionally rather than compared
/// against a handle that could only ever have one value.
extern "C" fn egl_create_window_surface(
    dpy: *mut c_void,
    config: *mut c_void,
    _native_window: *mut c_void,
    attribs: *mut c_void,
) -> *mut c_void {
    crate::android::glcount::CREATE_WINDOW_SURFACE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    let name = CString::new("eglCreateWindowSurface").unwrap_or_default();
    // SAFETY: RTLD_DEFAULT; libEGL is in the global scope by the time the engine
    // reaches this call.
    let f = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
    if f.is_null() {
        return std::ptr::null_mut();
    }
    type Fn_ = extern "C" fn(*mut c_void, *mut c_void, c_ulong, *mut c_void) -> *mut c_void;
    // SAFETY: resolved from the host for exactly this name.
    let f: Fn_ = unsafe { std::mem::transmute(f) };
    let win = current().map(|w| w.egl_native_window()).unwrap_or(0);
    f(dpy, config, win, attribs)
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("ANativeWindow_fromSurface", native_window_from_surface),
        f!("ANativeWindow_acquire", native_window_acquire),
        f!("ANativeWindow_release", native_window_release),
        f!("ANativeWindow_getWidth", native_window_get_width),
        f!("ANativeWindow_getHeight", native_window_get_height),
        f!("ANativeWindow_getFormat", native_window_get_format),
        f!("ANativeWindow_setBuffersGeometry", native_window_set_buffers_geometry),
        f!("ANativeWindow_lock", native_window_lock),
        f!("ANativeWindow_unlockAndPost", native_window_unlock_and_post),
        f!("eglCreateWindowSurface", egl_create_window_surface),
    ]
}
