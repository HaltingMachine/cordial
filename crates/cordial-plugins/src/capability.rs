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
    /// Post a desktop notification through the freedesktop portal.
    ///
    /// Brokered for the same reason as `PresenceSet`: the plugin sends a summary
    /// and a body, Cordial owns the D-Bus connection. A plugin that held the bus
    /// could talk to every other service on it.
    NotifySend,
    /// Open a URL in the user's browser, through the portal.
    ///
    /// The narrowest useful form of "leave the application". Cordial validates
    /// the scheme before handing it to the portal — `http` and `https` only, so
    /// this cannot become `file://` traversal or a handler-hijack for some
    /// arbitrary registered scheme.
    UrlOpen,
    /// Register a directory of files that resolve before Roblox's own assets
    /// of the same name — see
    /// [ADR-010](../../../docs/adr/ADR-010-plugin-asset-overlays.md).
    ///
    /// Narrow on purpose: this is one filesystem root the plugin owns, checked
    /// ahead of the APK for a name match, not a general filesystem capability.
    /// It cannot write into the APK or into anything Cordial extracts from it
    /// — both stay untouched — and it cannot read anything outside the root it
    /// registers. Uninstalling the plugin (or it giving up the root) makes the
    /// original asset resolve again with nothing to clean up, because nothing
    /// was ever overwritten to begin with.
    AssetsOverride,
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
            Capability::NotifySend => "notify.send",
            Capability::UrlOpen => "url.open",
            Capability::AssetsOverride => "assets.override",
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
            "notify.send" => Capability::NotifySend,
            "url.open" => Capability::UrlOpen,
            "assets.override" => Capability::AssetsOverride,
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
            Capability::NotifySend,
            Capability::UrlOpen,
            Capability::AssetsOverride,
        ]
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
