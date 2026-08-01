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
    let (window, conn_fd) = unsafe {
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
        (xlib.select_input)(display, w, INPUT_EVENT_MASK);
        (xlib.map_window)(display, w);
        (xlib.flush)(display);
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
