//! Cordial's plugin host.
//!
//! Plugins are separate processes speaking newline-delimited JSON over stdio,
//! with every call gated by a named capability. See
//! [ADR-003](../../../docs/adr/ADR-003-plugin-isolation.md) for why isolation is
//! by process rather than by a restricted in-process API, and
//! [ADR-005](../../../docs/adr/ADR-005-flag-service.md) for why flag writes are
//! split across two capabilities.

pub mod broker;
pub mod capability;
pub mod host;
pub mod protocol;
