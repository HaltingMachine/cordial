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
pub mod looper;
pub mod window;

use std::ffi::c_void;

/// Everything the Android layer implements so far.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    let mut v = asset::overrides();
    v.extend(config::overrides());
    v.extend(looper::overrides());
    v.extend(window::overrides());
    v
}
