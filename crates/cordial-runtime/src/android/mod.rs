//! Cordial's implementation of the Android NDK APIs in `libandroid.so`.
//!
//! Thirty-two functions across four groups, all currently stubbed except assets:
//!
//! | Group | Functions | State |
//! |---|---|---|
//! | `AAsset*` | 6 | implemented — see [`asset`] |
//! | `ANativeWindow_*` | 10 | stubbed; needs a host window and EGL surface |
//! | `ALooper_*` | 7 | stubbed |
//! | `AConfiguration_*` | 9 | stubbed |
//!
//! The order is not arbitrary: assets gate everything, because the engine cannot
//! load a shader or a font without them. See docs/design/path-to-a-frame.md.

pub mod asset;
pub mod gl;

use std::ffi::c_void;

/// Everything the Android layer implements so far.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    asset::overrides()
}
