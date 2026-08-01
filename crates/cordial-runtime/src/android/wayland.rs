//! The native Wayland backend. **Not implemented yet.**
//!
//! [ADR-011](../../../../docs/adr/ADR-011-wayland-and-libadwaita.md) makes
//! Wayland the target and X11 the diagnostic fallback. The selection
//! scaffolding in [`super`], the shared input module in [`super::input`], and
//! the backend-aware surface creation in [`super::vulkan`] are all in place and
//! were the point of that refactor. What is missing is this file: the registry
//! bind, `xdg_shell`, `wl_egl_window`, `wl_seat` input through xkbcommon, and
//! the `zwp_text_input_v3` bridge that hands text entry to whatever input
//! method the user actually runs.
//!
//! Every entry point below reports unavailability rather than pretending. That
//! is the same rule `native/opensles.cpp` follows and that `AGENTS.md` states
//! outright: a stub that returns success moves the failure somewhere with no
//! relationship to the cause. `open` returning `Err` here means
//! `backend()` falls back to X11 and says so, which is a legible outcome.
//!
//! Because of that, `backend()` requires `CORDIAL_WAYLAND=1` to select this at
//! all. Choosing it merely because `WAYLAND_DISPLAY` is set — which it is on
//! every modern desktop — would make the client refuse to start for everyone
//! the moment this file was merged.

use std::ffi::c_void;

/// The Wayland equivalent of [`super::window::HostWindow`]. Constructed only
/// once this backend exists; there is deliberately no way to obtain one today.
pub struct WaylandWindow {
    _private: (),
}

impl WaylandWindow {
    /// The `wl_display` file descriptor, polled the way the X11 backend polls
    /// its connection so input never blocks the render loop.
    pub fn connection_fd(&self) -> std::ffi::c_int {
        unreachable!("no WaylandWindow can exist until the backend is implemented")
    }

    pub fn geometry(&self) -> (i32, i32, i32) {
        unreachable!("no WaylandWindow can exist until the backend is implemented")
    }
}

pub fn open(_width: u32, _height: u32, _title: &str) -> Result<&'static WaylandWindow, String> {
    Err("the Wayland backend is not implemented yet (ADR-011); \
         unset CORDIAL_WAYLAND to use the X11 fallback"
        .to_string())
}

pub fn current() -> Option<&'static WaylandWindow> {
    None
}

pub fn pump_input_events(_handle: i64) {}

/// Symbol overrides this backend contributes — `eglCreateWindowSurface` and the
/// `ANativeWindow_*` family, once there is a surface to back them with.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    Vec::new()
}
