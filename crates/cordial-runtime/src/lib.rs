//! Cordial's runtime layer.
//!
//! Today this is the symbol table and the load path: it registers the Android
//! shared libraries Roblox links against as virtual libraries backed by Cordial's
//! own implementations, then loads `libroblox.so` against them with the AOSP
//! bionic linker.
//!
//! Nothing here runs Roblox yet. See docs/findings.md.

pub mod stubs;
pub mod symtab;
