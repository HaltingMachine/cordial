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

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};
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
    set_wm_normal_hints: unsafe extern "C" fn(Display, Window, *mut XSizeHints),
    set_class_hint: unsafe extern "C" fn(Display, Window, *mut XClassHint) -> c_int,
    set_wm_hints: unsafe extern "C" fn(Display, Window, *mut XWMHints) -> c_int,
    move_window: unsafe extern "C" fn(Display, Window, c_int, c_int) -> c_int,
    intern_atom: unsafe extern "C" fn(Display, *const c_char, c_int) -> c_ulong,
    change_property: unsafe extern "C" fn(
        Display, Window, c_ulong, c_ulong, c_int, c_int, *const c_void, c_int,
    ) -> c_int,
    send_event: unsafe extern "C" fn(Display, Window, c_int, c_long, *mut c_void) -> c_int,
    sync: unsafe extern "C" fn(Display, c_int) -> c_int,
    store_name: unsafe extern "C" fn(Display, Window, *const c_char) -> c_int,
    flush: unsafe extern "C" fn(Display) -> c_int,
    destroy_window: unsafe extern "C" fn(Display, Window) -> c_int,
    // ---- input, added for keyboard/mouse delivery ----
    select_input: unsafe extern "C" fn(Display, Window, c_long),
    connection_number: unsafe extern "C" fn(Display) -> c_int,
    pending: unsafe extern "C" fn(Display) -> c_int,
    next_event: unsafe extern "C" fn(Display, *mut c_void) -> c_int,
    /// `XLookupString` doubles as the keysym lookup and the ASCII/Latin-1 text
    /// lookup, and — unlike `XKeycodeToKeysym` — takes the event's `state` into
    /// account, so Shift and the rest of the modifier state do not have to be
    /// reimplemented by hand.
    lookup_string:
        unsafe extern "C" fn(*mut c_void, *mut c_char, c_int, *mut c_ulong, *mut c_void) -> c_int,
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
            select_input: sym!("XSelectInput"),
            set_wm_normal_hints: sym!("XSetWMNormalHints"),
            set_class_hint: sym!("XSetClassHint"),
            set_wm_hints: sym!("XSetWMHints"),
            move_window: sym!("XMoveWindow"),
            intern_atom: sym!("XInternAtom"),
            change_property: sym!("XChangeProperty"),
            send_event: sym!("XSendEvent"),
            sync: sym!("XSync"),
            connection_number: sym!("XConnectionNumber"),
            pending: sym!("XPending"),
            next_event: sym!("XNextEvent"),
            lookup_string: sym!("XLookupString"),
        })
    }
}

/// A mapped host window and the Android-side state the engine queries about it.
pub struct HostWindow {
    xlib: Xlib,
    display: Display,
    window: Window,
    /// `XConnectionNumber(display)` — the socket Xlib reads the wire protocol
    /// from. Polling this with a zero timeout is what lets input delivery avoid
    /// ever calling into Xlib when there is nothing queued, which is what keeps
    /// it from blocking the render loop (see `pump_input_events`, below).
    conn_fd: c_int,
    /// Dimensions the engine asked for via `ANativeWindow_setBuffersGeometry`,
    /// which override the window's own size in every query. Android reports the
    /// buffer geometry, not the surface geometry, and the engine sizes its
    /// framebuffers from the answer.
    buffers: Mutex<Geometry>,
    input: Mutex<InputState>,
}

/// Buttons and timing carried across calls to `pump_input_events`, the way a
/// real `InputDevice` accumulates gesture state between individual X11 events.
struct InputState {
    /// Android `MotionEvent.BUTTON_*` bits currently held down.
    buttons: i32,
    /// `uptimeMillis()` of the button that started the current gesture — reset
    /// to the current time whenever `buttons` goes from zero to non-zero, and
    /// left alone until it goes back to zero. Android's own `downTime` has this
    /// exact meaning: constant across a MOVE/UP sequence, not per-event.
    down_time_ms: i64,
    clock: std::time::Instant,
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


/// `XSizeHints`. Only the leading fields matter here, but the struct has to be
/// the full size Xlib expects or `XSetWMNormalHints` reads past the end.
#[repr(C)]
struct XSizeHints {
    flags: c_long,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    min_width: c_int,
    min_height: c_int,
    max_width: c_int,
    max_height: c_int,
    width_inc: c_int,
    height_inc: c_int,
    min_aspect_x: c_int,
    min_aspect_y: c_int,
    max_aspect_x: c_int,
    max_aspect_y: c_int,
    base_width: c_int,
    base_height: c_int,
    win_gravity: c_int,
}

#[repr(C)]
struct XClassHint {
    res_name: *mut c_char,
    res_class: *mut c_char,
}

#[repr(C)]
struct XWMHints {
    flags: c_long,
    input: c_int,
    initial_state: c_int,
    icon_pixmap: c_ulong,
    icon_window: Window,
    icon_x: c_int,
    icon_y: c_int,
    icon_mask: c_ulong,
    window_group: c_ulong,
}

/// Where to put the window, in root coordinates.
///
/// A window created at 0,0 lands on the primary monitor, which is not where
/// anyone wants a game window if they kept a second screen for exactly this.
/// `CORDIAL_MONITOR=<n>` centres the window on the nth monitor reported by
/// Xinerama (0 is the first); `CORDIAL_WINDOW_POS=<x>,<y>` overrides with
/// explicit top-left coordinates and wins if both are set.
///
/// Centring rather than pinning to the monitor's corner, because a monitor
/// origin is not a sensible place for a window — on a layout like
/// `0,0 3440x1440` beside `3440,240 1920x1200`, the corner is where the bezel
/// is.
///
/// Xinerama rather than RandR because the query is one call with no resource
/// management, and every multi-head X server that supports RandR also answers
/// Xinerama. Returns (0, 0) when nothing is configured or the query fails, which
/// is exactly the previous behaviour.
struct Placement {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    fullscreen: bool,
    /// Which monitor was asked for, for `_NET_WM_FULLSCREEN_MONITORS`. A window
    /// manager fullscreens onto whichever monitor it thinks the window is on,
    /// and it does not have to agree with where the window was put — so naming
    /// the monitor explicitly is the only reliable way to say which screen.
    monitor: Option<c_long>,
}

fn placement(win_w: c_int, win_h: c_int) -> Placement {
    let fullscreen = std::env::var_os("CORDIAL_FULLSCREEN").is_some();
    let mut p = Placement { x: 0, y: 0, width: win_w, height: win_h, fullscreen, monitor: None };

    if let Ok(pos) = std::env::var("CORDIAL_WINDOW_POS") {
        let mut parts = pos.split(',').map(str::trim);
        if let (Some(Ok(x)), Some(Ok(y))) = (
            parts.next().map(str::parse::<c_int>),
            parts.next().map(str::parse::<c_int>),
        ) {
            p.x = x;
            p.y = y;
            return p;
        }
        eprintln!("[android] CORDIAL_WINDOW_POS={pos:?} is not <x>,<y>; ignoring");
    }

    let Ok(want) = std::env::var("CORDIAL_MONITOR") else {
        return p;
    };
    let Ok(want) = want.trim().parse::<usize>() else {
        eprintln!("[android] CORDIAL_MONITOR must be a number; ignoring");
        return p;
    };

    #[repr(C)]
    struct XineramaScreenInfo {
        screen_number: c_int,
        x_org: i16,
        y_org: i16,
        width: i16,
        height: i16,
    }

    const RTLD_NOW: c_int = 2;
    // SAFETY: dlopen/dlsym with literal names; every result is null-checked.
    unsafe {
        let lib = dlopen(c"libXinerama.so.1".as_ptr(), RTLD_NOW);
        if lib.is_null() {
            eprintln!("[android] CORDIAL_MONITOR needs libXinerama; ignoring");
            return p;
        }
        let query = dlsym(lib, c"XineramaQueryScreens".as_ptr());
        if query.is_null() {
            return p;
        }
        let query: unsafe extern "C" fn(Display, *mut c_int) -> *mut XineramaScreenInfo =
            std::mem::transmute(query);
        // The caller already has a display open; re-opening here would be a
        // second connection for one query, so this runs against the same one.
        let d = CURRENT_DISPLAY.load(std::sync::atomic::Ordering::Relaxed);
        if d == 0 {
            return p;
        }
        let mut n: c_int = 0;
        let screens = query(d as Display, &mut n);
        if screens.is_null() || n <= 0 {
            return p;
        }
        let list = std::slice::from_raw_parts(screens, n as usize);
        let m = match list.get(want) {
            Some(m) => m,
            None => {
                eprintln!(
                    "[android] CORDIAL_MONITOR={want} but only {n} monitor(s); using the first"
                );
                &list[0]
            }
        };
        p.monitor = Some(want.min(n as usize - 1) as c_long);
        if p.fullscreen {
            // Cover the monitor exactly. The window manager fullscreens onto
            // whichever monitor the window occupies, so filling it first is
            // what pins fullscreen to the requested screen rather than the
            // primary one.
            p.x = m.x_org as c_int;
            p.y = m.y_org as c_int;
            p.width = m.width as c_int;
            p.height = m.height as c_int;
        } else {
            // Clamped at the origin so an oversized window still starts
            // on-screen rather than off the top-left of its monitor.
            p.x = m.x_org as c_int + ((m.width as c_int - win_w) / 2).max(0);
            p.y = m.y_org as c_int + ((m.height as c_int - win_h) / 2).max(0);
        }
        p
    }
}

/// The open display, so `window_origin` can query monitors on the same
/// connection rather than opening a second one for a single call.
static CURRENT_DISPLAY: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

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

    // KeyPressMask | KeyReleaseMask | ButtonPressMask | ButtonReleaseMask |
    // PointerMotionMask, from X.h. Not StructureNotifyMask or FocusChangeMask —
    // this window's size is fixed for its lifetime and AGDK's own focus call
    // already runs once, unconditionally, in `game_activity.cpp`.
    const INPUT_EVENT_MASK: c_long = 0x1 | 0x2 | 0x4 | 0x8 | 0x40;

    // SAFETY: `display` is open; the geometry and border/background pixels are
    // plain values.
    CURRENT_DISPLAY.store(display as usize, std::sync::atomic::Ordering::Relaxed);
    let place = placement(width as c_int, height as c_int);
    // Reported always, not behind a trace flag: "the window opened on the wrong
    // screen" is a user-visible complaint, and this line is what separates
    // "Cordial computed the wrong position" from "the window manager ignored
    // the one it was given".
    println!(
        "[android] window placement: {}x{} at {},{}{}",
        place.width, place.height, place.x, place.y,
        if place.fullscreen { " (fullscreen)" } else { "" }
    );
    let (ox, oy) = (place.x, place.y);
    // Fullscreen resizes the surface as well as the window: the engine sizes
    // its framebuffers from what `geometry()` reports, so a window covering a
    // 1920x1200 monitor while the surface still says 1280x720 would render a
    // corner of the screen.
    let (width, height) = (place.width as u32, place.height as u32);

    let (window, conn_fd) = unsafe {
        let root = (xlib.default_root_window)(display);
        let w = (xlib.create_simple_window)(display, root, ox, oy, width, height, 0, 0, 0);
        // XStoreName sets WM_NAME, which is XA_STRING — Latin-1, not UTF-8.
        // An em dash here renders as mojibake, so the title is kept ASCII
        // rather than encoded twice for a window caption.
        let ascii: String = title
            .chars()
            .map(|c| if c.is_ascii() { c } else { '-' })
            .collect();
        let name = CString::new(ascii).unwrap_or_default();
        (xlib.store_name)(display, w, name.as_ptr());

        // Without WM hints a window manager is free to place this wherever it
        // likes and to decide it does not take keyboard focus. Both were
        // happening: the window landed on the primary monitor whatever
        // `CORDIAL_MONITOR` said, and key events went elsewhere while mouse
        // events still arrived, because ButtonPress is delivered by pointer
        // position but KeyPress follows the focus.
        //
        // USPosition rather than PPosition: it means "the user asked for this
        // position", which window managers honour where they routinely override
        // a mere program preference.
        let mut hints = XSizeHints {
            flags: 1 << 0, // USPosition
            x: ox,
            y: oy,
            width: width as c_int,
            height: height as c_int,
            min_width: 0, min_height: 0, max_width: 0, max_height: 0,
            width_inc: 0, height_inc: 0,
            min_aspect_x: 0, min_aspect_y: 0, max_aspect_x: 0, max_aspect_y: 0,
            base_width: 0, base_height: 0, win_gravity: 0,
        };
        (xlib.set_wm_normal_hints)(display, w, &mut hints);

        // InputHint | StateHint, asking to be given the keyboard.
        let mut wm = XWMHints {
            flags: (1 << 0) | (1 << 1),
            input: 1,
            initial_state: 1, // NormalState
            icon_pixmap: 0, icon_window: 0, icon_x: 0, icon_y: 0,
            icon_mask: 0, window_group: 0,
        };
        (xlib.set_wm_hints)(display, w, &mut wm);

        // WM_CLASS, so the window is addressable by rule in a tiling or
        // scripted setup rather than only by title.
        let res_name = CString::new("cordial").unwrap_or_default();
        let res_class = CString::new("Cordial").unwrap_or_default();
        let mut class = XClassHint {
            res_name: res_name.as_ptr() as *mut c_char,
            res_class: res_class.as_ptr() as *mut c_char,
        };
        (xlib.set_class_hint)(display, w, &mut class);

        (xlib.select_input)(display, w, INPUT_EVENT_MASK);
        (xlib.map_window)(display, w);
        // Let the window manager finish its own placement before arguing with
        // it. Moving before it has acted is a race that the window manager
        // wins, which is exactly what happened: Cordial computed 3760,480 and
        // the window still came up at 25,62.
        (xlib.sync)(display, 0);

        let root = (xlib.default_root_window)(display);
        const SUBSTRUCTURE_REDIRECT: c_long = 1 << 20;
        const SUBSTRUCTURE_NOTIFY: c_long = 1 << 19;
        const CLIENT_MESSAGE: c_int = 33;
        let atom = |n: &str| -> c_ulong {
            let c = CString::new(n).unwrap_or_default();
            (xlib.intern_atom)(display, c.as_ptr(), 0)
        };

        // An XClientMessageEvent, laid out by hand. Xlib's XEvent union is
        // large and only the leading fields matter here.
        let mut msg = [0u8; 96];
        let mut send = |message_type: c_ulong, data: [c_long; 5]| {
            msg.fill(0);
            let p = msg.as_mut_ptr();
            *(p as *mut c_int) = CLIENT_MESSAGE;
            *(p.add(8) as *mut c_ulong) = 1; // serial
            *(p.add(16) as *mut c_int) = 1; // send_event
            *(p.add(24) as *mut usize) = display as usize;
            *(p.add(32) as *mut Window) = w;
            *(p.add(40) as *mut c_ulong) = message_type;
            *(p.add(48) as *mut c_int) = 32; // format
            for (i, v) in data.iter().enumerate() {
                *(p.add(56 + i * 8) as *mut c_long) = *v;
            }
            (xlib.send_event)(
                display, root, 0,
                SUBSTRUCTURE_REDIRECT | SUBSTRUCTURE_NOTIFY,
                msg.as_mut_ptr() as *mut c_void,
            );
        };

        if place.fullscreen {
            // Name the monitor outright. `_NET_WM_STATE_FULLSCREEN` alone
            // fullscreens onto whichever monitor the window manager believes
            // the window occupies, which is the thing that was wrong.
            if let Some(m) = place.monitor {
                let a = atom("_NET_WM_FULLSCREEN_MONITORS");
                if a != 0 {
                    send(a, [m, m, m, m, 1]);
                }
            }
            let state = atom("_NET_WM_STATE");
            let fs = atom("_NET_WM_STATE_FULLSCREEN");
            if state != 0 && fs != 0 {
                const ADD: c_long = 1;
                send(state, [ADD, fs as c_long, 0, 1, 0]);
            }
        } else if (ox, oy) != (0, 0) {
            (xlib.move_window)(display, w, ox, oy);
        }
        (xlib.flush)(display);
        (xlib.sync)(display, 0);

        (w, (xlib.connection_number)(display))
    };

    let host = HostWindow {
        xlib,
        display,
        window,
        conn_fd,
        buffers: Mutex::new(Geometry {
            width: width as i32,
            height: height as i32,
            format: WINDOW_FORMAT_RGBA_8888,
        }),
        input: Mutex::new(InputState {
            buttons: 0,
            down_time_ms: 0,
            clock: std::time::Instant::now(),
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

    /// The X connection's descriptor, so the looper can wait on input rather
    /// than poll for it.
    pub fn connection_fd(&self) -> c_int {
        self.conn_fd
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

// ------------------------------------------------------------- input pump
//
// Mouse and keyboard, delivered to the engine through the same AGDK
// `GameActivity` natives real Android input goes through — `onTouchEventNative`
// and `onKeyDownNative`/`onKeyUpNative` — via `cordial-linker-sys`'s
// `game_activity` module and the synthesised `MotionEvent`/`KeyEvent` objects in
// `native/game_activity.cpp`.
//
// The design constraint is that this must never block: it runs inside
// `looper::pump`'s own ~50ms-timeout loop, on the thread that also owns the
// engine's message pump, so any call here that waits is a frame the engine
// never gets to render. `XPending`/`XNextEvent` are what actually read queued
// events, but calling either when nothing is queued risks a blocking read in
// at least some libX11 builds. So every drain starts with a zero-timeout
// `poll(2)` on Xlib's own connection fd (`XConnectionNumber`) — a pure
// kernel-side check that can only return immediately — and only touches Xlib
// at all when that says there is something to read.

/// The common prefix shared by `XKeyEvent`, `XButtonEvent` and `XMotionEvent`.
///
/// Xlib deliberately lays these three structs out identically — that is
/// documented behaviour, not a coincidence being relied on here — except for
/// one field whose *meaning* differs: `keycode` for key events, `button` for
/// button events, `is_hint` for motion. It is read generically as `detail` and
/// interpreted according to `type_`.
#[repr(C)]
struct XInputEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut c_void,
    window: c_ulong,
    root: c_ulong,
    subwindow: c_ulong,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    detail: c_uint,
    same_screen: c_int,
}

// X11 event `type` values, from X.h.
const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const BUTTON_RELEASE: c_int = 5;
const MOTION_NOTIFY: c_int = 6;

// X11 modifier bits (X.h) actually consulted below.
const SHIFT_MASK: c_uint = 1 << 0;
const LOCK_MASK: c_uint = 1 << 1; // Caps Lock
const CONTROL_MASK: c_uint = 1 << 2;
const MOD1_MASK: c_uint = 1 << 3; // Alt, on essentially every layout in practice

// `android.view.KeyEvent.META_*`.
const META_SHIFT_ON: i32 = 1;
const META_ALT_ON: i32 = 2;
const META_CTRL_ON: i32 = 0x1000;
const META_CAPS_LOCK_ON: i32 = 0x100000;

fn android_meta_state(x11_state: c_uint) -> i32 {
    let mut m = 0;
    if x11_state & SHIFT_MASK != 0 {
        m |= META_SHIFT_ON;
    }
    if x11_state & CONTROL_MASK != 0 {
        m |= META_CTRL_ON;
    }
    if x11_state & MOD1_MASK != 0 {
        m |= META_ALT_ON;
    }
    if x11_state & LOCK_MASK != 0 {
        m |= META_CAPS_LOCK_ON;
    }
    m
}

// `android.view.MotionEvent.BUTTON_*` / `ACTION_*` — only the ones this module
// produces.
const BUTTON_PRIMARY: i32 = 1;
const BUTTON_SECONDARY: i32 = 2;
const BUTTON_TERTIARY: i32 = 4;
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
const ACTION_HOVER_MOVE: i32 = 7;
const ACTION_BUTTON_PRESS: i32 = 11;
const ACTION_BUTTON_RELEASE: i32 = 12;

/// X11 numbers buttons 1/2/3 as left/middle/right; Android's bit assignment
/// puts secondary (right) before tertiary (middle). Buttons 4/5 are X11's
/// representation of the scroll wheel as button clicks — Android instead wants
/// `ACTION_SCROLL` with an axis value, which this does not yet synthesise (see
/// the report), so they are dropped rather than delivered as wrong clicks.
fn x11_button_to_android(button: c_uint) -> Option<i32> {
    match button {
        1 => Some(BUTTON_PRIMARY),
        2 => Some(BUTTON_TERTIARY),
        3 => Some(BUTTON_SECONDARY),
        _ => None,
    }
}

/// A pragmatic subset of X11 keysyms mapped to `android.view.KeyEvent.KEYCODE_*`.
/// Covers what a desktop text field and basic UI navigation need — letters,
/// digits, common punctuation, arrows, and the usual control keys. Anything
/// outside this set is dropped rather than guessed at; see the report for what
/// that leaves out.
fn keysym_to_android(keysym: c_ulong) -> Option<i32> {
    let k = keysym as u32;
    Some(match k {
        0x30..=0x39 => 7 + (k - 0x30) as i32,  // 0..9 -> AKEYCODE_0..9
        0x61..=0x7a => 29 + (k - 0x61) as i32, // a..z -> AKEYCODE_A..Z
        0x41..=0x5a => 29 + (k - 0x41) as i32, // A..Z (shifted) -> the same keycodes
        0x0020 => 62,                          // space
        0xff0d | 0xff8d => 66,                 // Return, KP_Enter
        0xff08 => 67,                          // BackSpace
        0xff09 => 61,                          // Tab
        0xff1b => 111,                         // Escape
        0xff51 => 21,                          // Left
        0xff52 => 19,                          // Up
        0xff53 => 22,                          // Right
        0xff54 => 20,                          // Down
        0xffe1 => 59,                          // Shift_L
        0xffe2 => 60,                          // Shift_R
        0xffe3 => 113,                         // Control_L
        0xffe4 => 114,                         // Control_R
        0xffe9 => 57,                          // Alt_L
        0xffea => 58,                          // Alt_R
        0xffe5 => 115,                         // Caps_Lock
        0xffff => 112,                         // Delete (forward delete)
        0xff50 => 122,                         // Home
        0xff57 => 123,                         // End
        0xff55 => 92,                          // Page_Up
        0xff56 => 93,                          // Page_Down
        0xff63 => 124,                         // Insert
        0x002c => 55,                          // comma
        0x002e => 56,                          // period
        0x002f => 76,                          // slash
        0x003b => 74,                          // semicolon
        0x0027 => 75,                          // apostrophe
        0x0060 => 68,                          // grave
        0x002d => 69,                          // minus
        0x003d => 70,                          // equal
        0x005b => 71,                          // bracketleft
        0x005d => 72,                          // bracketright
        0x005c => 73,                          // backslash
        _ => return None,
    })
}

// `*mut c_void` rather than a typed `*mut PollFd`, to match the `poll`
// declaration `bionic::mod` already has for the emulated libc's own use of the
// same host symbol — `rustc` warns (`clashing_extern_declarations`) about two
// `extern "C" fn poll` with different signatures anywhere in the crate, since
// both ultimately bind the one process-wide C symbol.
extern "C" {
    fn poll(fds: *mut c_void, nfds: c_ulong, timeout_ms: c_int) -> c_int;
}
#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 0x001;

fn deliver_touch(
    handle: i64,
    action: i32,
    x: f32,
    y: f32,
    button_state: i32,
    action_button: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    match cordial_linker_sys::game_activity::touch(
        handle,
        action,
        x,
        y,
        button_state,
        action_button,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onTouchEventNative(action={action}) -> {consumed}"))
        }
        // Not registered yet — a normal race against initializeNativeCode
        // early in startup.
        Ok(None) => {}
        Err(e) => super::trace(format_args!("onTouchEventNative(action={action}) failed: {e}")),
    }
}

fn deliver_key(
    handle: i64,
    down: bool,
    key_code: i32,
    scan_code: i32,
    meta_state: i32,
    repeat_count: i32,
    unicode_char: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    match cordial_linker_sys::game_activity::key(
        handle,
        down,
        key_code,
        scan_code,
        meta_state,
        repeat_count,
        unicode_char,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onKey{}Native(code={key_code}) -> {consumed}",
                if down { "Down" } else { "Up" }))
        }
        Ok(None) => {}
        Err(e) => super::trace(format_args!(
            "onKey{}Native(code={key_code}) failed: {e}",
            if down { "Down" } else { "Up" }
        )),
    }
}

impl HostWindow {
    fn now_ms(&self) -> i64 {
        let state = self.input.lock().unwrap_or_else(|e| e.into_inner());
        state.clock.elapsed().as_millis() as i64
    }

    fn dispatch_button(&self, handle: i64, ev: &XInputEvent, press: bool) {
        let Some(android_button) = x11_button_to_android(ev.detail) else {
            return;
        };
        let (x, y) = (ev.x as f32, ev.y as f32);

        let mut state = self.input.lock().unwrap_or_else(|e| e.into_inner());
        let now = state.clock.elapsed().as_millis() as i64;

        if press {
            if state.buttons == 0 {
                state.down_time_ms = now;
            }
            state.buttons |= android_button;
            let (buttons, down_time) = (state.buttons, state.down_time_ms);
            drop(state);
            // Real Android mouse input delivers exactly this pair for a
            // click: ACTION_DOWN establishes the gesture, then
            // ACTION_BUTTON_PRESS names which button did it.
            deliver_touch(handle, ACTION_DOWN, x, y, buttons, 0, now, down_time);
            deliver_touch(handle, ACTION_BUTTON_PRESS, x, y, buttons, android_button, now, down_time);
        } else {
            state.buttons &= !android_button;
            let (buttons, down_time) = (state.buttons, state.down_time_ms);
            drop(state);
            deliver_touch(handle, ACTION_BUTTON_RELEASE, x, y, buttons, android_button, now, down_time);
            deliver_touch(handle, ACTION_UP, x, y, buttons, 0, now, down_time);
        }
    }

    fn dispatch_motion(&self, handle: i64, ev: &XInputEvent) {
        let (x, y) = (ev.x as f32, ev.y as f32);
        let state = self.input.lock().unwrap_or_else(|e| e.into_inner());
        let now = state.clock.elapsed().as_millis() as i64;
        let (buttons, down_time) = (state.buttons, state.down_time_ms);
        drop(state);
        // A held button makes this a drag — part of the gesture the DOWN
        // started, hence ACTION_MOVE with the same down_time. No button held
        // makes it a hover, which is what a mouse (as opposed to touch) sends
        // when it moves without a button down.
        let action = if buttons != 0 { ACTION_MOVE } else { ACTION_HOVER_MOVE };
        deliver_touch(handle, action, x, y, buttons, 0, now, down_time);
    }

    fn dispatch_key(&self, handle: i64, buf: &mut [u8; 256], down: bool) {
        let mut keysym: c_ulong = 0;
        let mut text = [0u8; 8];
        // SAFETY: `buf` holds the XKeyEvent `XNextEvent` just filled, laid out
        // identically to `XInputEvent` above (that layout compatibility is
        // documented Xlib behaviour). A null compose-status argument is
        // documented to mean "skip compose-key processing", not "pass a valid
        // pointer" — Xlib treats it as optional.
        let n = unsafe {
            (self.xlib.lookup_string)(
                buf.as_mut_ptr() as *mut c_void,
                text.as_mut_ptr() as *mut c_char,
                text.len() as c_int,
                &mut keysym,
                std::ptr::null_mut(),
            )
        };
        let ev = unsafe { &*(buf.as_ptr() as *const XInputEvent) };
        let Some(keycode) = keysym_to_android(keysym) else {
            super::trace(format_args!("unmapped X11 keysym {keysym:#x}"));
            return;
        };
        let unicode = if n > 0 { text[0] as i32 } else { 0 };
        let meta = android_meta_state(ev.state);
        let now = self.now_ms();

        // Real per-key downTime tracking (one slot per held key) is not
        // implemented; both fields use the current time on every call. That
        // is a simplification, not a faithful `downTime`, and is called out in
        // the report — it does not block a key reaching the engine, only the
        // precision of one timing field most UI code does not consult.
        deliver_key(handle, down, keycode, ev.detail as i32, meta, 0, unicode, now, now);
    }

    /// Drain and deliver whatever X11 input is already queued, then return.
    /// See the module-level comment above for why this never blocks.
    fn pump_input_events(&self, handle: i64) {
        let mut pfd = PollFd { fd: self.conn_fd, events: POLLIN, revents: 0 };
        // SAFETY: `pfd` is a live array of length 1; a 0ms timeout makes this a
        // pure non-blocking check.
        let ready = unsafe { poll(&mut pfd as *mut PollFd as *mut c_void, 1, 0) };
        if ready <= 0 {
            return;
        }

        // Bounded so a burst of queued motion events cannot turn one drain
        // call into unbounded work inside the render loop's own timing
        // budget.
        const MAX_EVENTS_PER_DRAIN: usize = 64;
        for _ in 0..MAX_EVENTS_PER_DRAIN {
            // SAFETY: `self.display` is open; reached only after `poll` above
            // found the connection readable (or a previous iteration left
            // events already queued client-side).
            if unsafe { (self.xlib.pending)(self.display) } <= 0 {
                break;
            }
            let mut buf = [0u8; 256];
            // SAFETY: 256 bytes covers every concrete event struct in the
            // `XEvent` union on every platform Xlib ships for; `buf` is live
            // for the call.
            unsafe { (self.xlib.next_event)(self.display, buf.as_mut_ptr() as *mut c_void) };
            let event_type = unsafe { *(buf.as_ptr() as *const c_int) };

            match event_type {
                BUTTON_PRESS | BUTTON_RELEASE => {
                    let ev = unsafe { &*(buf.as_ptr() as *const XInputEvent) };
                    self.dispatch_button(handle, ev, event_type == BUTTON_PRESS);
                }
                MOTION_NOTIFY => {
                    let ev = unsafe { &*(buf.as_ptr() as *const XInputEvent) };
                    self.dispatch_motion(handle, ev);
                }
                KEY_PRESS | KEY_RELEASE => {
                    self.dispatch_key(handle, &mut buf, event_type == KEY_PRESS);
                }
                _ => {}
            }
        }
    }
}

/// Drain and deliver whatever host input is queued, for the current window (if
/// one is open — the loader/asset-only paths that never call `open()` make
/// this a no-op).
pub fn pump_input_events(handle: i64) {
    if let Some(w) = current() {
        w.pump_input_events(handle);
    }
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
