//! Cordial's implementation of the Android NDK APIs in `libandroid.so`.
//!
//! Thirty-two functions across four groups, all currently stubbed except assets:
//!
//! | Group | Functions | State |
//! |---|---|---|
//! | `AAsset*` | 6 | implemented — see [`asset`] |
//! | `ANativeWindow_*` | 10 | implemented over an X11 window — see [`window`] |
//! | `ALooper_*` | 7 | implemented over epoll — see [`looper`] |
//! | `AConfiguration_*` | 9 | implemented — see [`config`] |
//!
//! The order is not arbitrary: assets gate everything, because the engine cannot
//! load a shader or a font without them. See docs/design/path-to-a-frame.md.

pub mod asset;
pub mod config;
pub mod gl;
pub mod glcount;
pub mod looper;
pub mod window;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

static TRACE: AtomicBool = AtomicBool::new(false);

/// Log every Android API call. AGDK's `initializeNativeCode` returns a bare 0 on
/// failure with nothing logged, so the only way to find where it stopped is to
/// watch which of these it reached.
pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

pub(crate) fn trace(args: std::fmt::Arguments<'_>) {
    if TRACE.load(Ordering::Relaxed) {
        eprintln!("[android] {args}");
    }
}

/// Everything the Android layer implements so far.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    let mut v = asset::overrides();
    v.extend(config::overrides());
    v.extend(looper::overrides());
    v.extend(window::overrides());
    if std::env::var_os("CORDIAL_COUNT_GL").is_some() {
        v.extend(glcount::overrides());
    }
    v
}
