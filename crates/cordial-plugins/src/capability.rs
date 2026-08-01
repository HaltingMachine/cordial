//! What a plugin is allowed to do, and nothing else.
//!
//! Capabilities are named, granted per plugin, and checked at the point of use.
//! The list is closed: there is no capability that means "anything", and
//! [ADR-003](../../../docs/adr/ADR-003-plugin-isolation.md) is explicit that a
//! capability handing over the machine is not a capability but the absence of
//! one. That is why there is no `process.spawn`, no filesystem path, and no
//! memory access here.
//!
//! Adding a variant is a design decision, not a convenience. If a plugin needs
//! something, the question is what *narrow, named* effect it needs — not what
//! access would let it arrange the effect itself.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read the resolved FastFlag set, including which layer set each value.
    FlagsRead,
    /// Contribute flags. Startup flags land in the plugin's own `flags.json` and
    /// take effect at the next launch; see ADR-005 for why that is not the same
    /// surface as changing one live.
    FlagsWrite,
    /// Change a `DFFlag`/`DFInt`/`DFString` while the client runs. Deliberately
    /// distinct from `FlagsWrite`: the static families cannot be changed live at
    /// all, and an API that accepted them here would silently do nothing.
    FlagsWriteDynamic,
    /// Emit log lines into Cordial's own output.
    Log,
    /// Observe client lifecycle events — launch, ready, shutdown.
    LifecycleRead,
    /// Publish Discord Rich Presence.
    ///
    /// The effect, not the channel. Cordial owns the connection to Discord's
    /// IPC socket and the plugin sends a presence payload; it never learns the
    /// socket's location, cannot read Discord's state, and cannot send arbitrary
    /// frames. See ADR-007 — a plugin can never hold a host resource, because a
    /// Flatpak permission is app-wide and permanent while a capability is
    /// per-plugin and revocable, and the two cannot be made to mean the same
    /// thing.
    ///
    /// Off unless granted, and privacy-relevant: what someone is playing and
    /// when is not always something they want broadcast.
    PresenceSet,
}

impl Capability {
    /// The wire name, which is what appears in a manifest.
    pub fn name(self) -> &'static str {
        match self {
            Capability::FlagsRead => "flags.read",
            Capability::FlagsWrite => "flags.write",
            Capability::FlagsWriteDynamic => "flags.write.dynamic",
            Capability::Log => "log",
            Capability::LifecycleRead => "lifecycle.read",
            Capability::PresenceSet => "presence.set",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "flags.read" => Capability::FlagsRead,
            "flags.write" => Capability::FlagsWrite,
            "flags.write.dynamic" => Capability::FlagsWriteDynamic,
            "log" => Capability::Log,
            "lifecycle.read" => Capability::LifecycleRead,
            "presence.set" => Capability::PresenceSet,
            _ => return None,
        })
    }

    /// Every capability, so a UI can present the full set rather than a
    /// hand-maintained copy that drifts.
    pub fn all() -> &'static [Capability] {
        &[
            Capability::FlagsRead,
            Capability::FlagsWrite,
            Capability::FlagsWriteDynamic,
            Capability::Log,
            Capability::LifecycleRead,
            Capability::PresenceSet,
        ]
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
