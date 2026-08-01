//! The event registry — who may publish what, and who is listening.
//!
//! ADR-006 is explicit that this has to be "a real runtime object with
//! ownership rules, not a hashmap of strings": it records which plugin
//! declared each type, refuses a publish from anyone else, and must survive a
//! plugin restarting without letting a different plugin claim its namespace
//! in the gap.
//!
//! Namespacing is not a convention a plugin is trusted to follow. A
//! declaration is always recorded as `{plugin}/{name}`, where `{plugin}` is
//! the caller's own id as Cordial already knows it — never a string read out
//! of the request — so there is no field a plugin can set to type its way
//! into another plugin's namespace. The "gap" ADR-006 worries about (a
//! plugin restarting, another plugin grabbing its namespace in between) is
//! closed the same way: only the plugin whose id prefixes a type can ever
//! satisfy the ownership check for it, restart or not, so there is nothing
//! for a different plugin to grab.
//!
//! `cordial` is reserved as the owner of core event types and cannot be
//! claimed by declaring: a plugin that talked its own manifest into being
//! named `cordial` would otherwise be able to mint fake `cordial/...` events
//! and no subscriber could tell them from the real ones. This is what makes
//! "plugins may not publish on core event types" (ADR-006) hold without the
//! registry needing to know the list of core event types at all — a plugin
//! simply can never own anything under that prefix.
//!
//! ADR-006 leaves open whether a subscriber filters at subscribe time or on
//! receipt. This registry filters at subscribe time: `subscribe` records the
//! exact type against the caller, and `subscribers` is a direct lookup rather
//! than a per-publish scan of every plugin's own filter list. That is the
//! option ADR-006 itself calls out as better for both privacy and cost, and
//! it is only available because the registry already has to answer "who
//! declared this" to authorise `publish` — reusing that answer for `subscribe`
//! costs nothing extra. The trade is that a plugin cannot subscribe to a type
//! nobody has declared yet, so a subscriber that starts before its dependency
//! has to wait for it — which is the dependency resolution ADR-006 already
//! describes for first-party plugins ("resolved once, shared, not restarted
//! per dependent"), not a new problem this module invents.

use std::collections::{BTreeMap, BTreeSet};

/// A fully namespaced event type, e.g. `flag-manager/profile-changed`.
/// Always produced by [`EventRegistry::declare`]; nothing in this module
/// accepts one ready-made from a plugin.
pub type EventType = String;

/// The plugin id reserved for Cordial's own events. Not a real plugin — see
/// the module comment for why declaring under it must always fail.
pub const CORE_OWNER: &str = "cordial";

#[derive(Debug, Default)]
pub struct EventRegistry {
    /// Which plugin declared each type. The key's prefix is always that same
    /// plugin's id, enforced by `declare` constructing the key itself, so
    /// this table doubles as the ownership check `may_publish` needs.
    owners: BTreeMap<EventType, String>,
    /// Which types each plugin subscribed to, recorded at subscribe time.
    subscriptions: BTreeMap<String, BTreeSet<EventType>>,
}

impl EventRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name` under `plugin`'s own namespace and return the full
    /// type. Calling this again with the same plugin and name is not an
    /// error — a plugin re-declaring its own types after a restart must land
    /// exactly where it was, not be treated as a conflict with itself.
    pub fn declare(&mut self, plugin: &str, name: &str) -> Result<EventType, String> {
        if plugin == CORE_OWNER {
            return Err(format!(
                "{CORE_OWNER:?} is reserved for Cordial's own events; a plugin may not declare under it"
            ));
        }
        let full = format!("{plugin}/{name}");
        match self.owners.get(&full) {
            Some(owner) if owner != plugin => {
                Err(format!("{full:?} is already declared by {owner:?}"))
            }
            _ => {
                self.owners.insert(full.clone(), plugin.to_string());
                Ok(full)
            }
        }
    }

    /// Whether `plugin` may publish on `event_type` — true only if `plugin`
    /// is the one that declared it. This is the entire enforcement of "a
    /// plugin may only publish on types it declared itself": everything else
    /// in the module exists to make this check trustworthy.
    pub fn may_publish(&self, plugin: &str, event_type: &str) -> bool {
        self.owners.get(event_type).is_some_and(|owner| owner == plugin)
    }

    /// Who declared `event_type`, if anyone.
    pub fn owner(&self, event_type: &str) -> Option<&str> {
        self.owners.get(event_type).map(String::as_str)
    }

    /// Record that `plugin` wants events of `event_type`. Refused if nobody
    /// has declared that type yet: a subscription to nothing would wait
    /// forever with no way to distinguish "the publisher has not started"
    /// from "this type does not exist", and a typo should look like a typo.
    pub fn subscribe(&mut self, plugin: &str, event_type: &str) -> Result<(), String> {
        if !self.owners.contains_key(event_type) {
            return Err(format!("{event_type:?} has not been declared by any plugin"));
        }
        self.subscriptions.entry(plugin.to_string()).or_default().insert(event_type.to_string());
        Ok(())
    }

    /// Every plugin that should receive `event_type`, in the order they
    /// subscribed to anything (stable, since `BTreeMap` iterates by key).
    pub fn subscribers(&self, event_type: &str) -> Vec<&str> {
        self.subscriptions
            .iter()
            .filter(|(_, types)| types.contains(event_type))
            .map(|(plugin, _)| plugin.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaring_namespaces_by_the_plugins_own_id() {
        // The plugin never gets to choose the namespace, only the bare name
        // inside it — ADR-006 is explicit that this is not optional.
        let mut r = EventRegistry::new();
        assert_eq!(r.declare("flag-manager", "profile-changed").unwrap(), "flag-manager/profile-changed");
    }

    #[test]
    fn a_plugin_may_only_publish_on_a_type_it_declared_itself() {
        let mut r = EventRegistry::new();
        let t = r.declare("flag-manager", "profile-changed").unwrap();
        assert!(r.may_publish("flag-manager", &t));
        // The type it never declared, requested by a different plugin that
        // holds events.publish for itself but has no claim on this type.
        assert!(!r.may_publish("evil-plugin", &t));
        // Even a type-shaped string nobody declared at all.
        assert!(!r.may_publish("flag-manager", "flag-manager/never-declared"));
    }

    #[test]
    fn a_namespace_cannot_be_claimed_by_another_plugin() {
        // Two plugins declaring the same bare name land in different
        // namespaces, because the namespace is derived from the id and not
        // chosen — so "claiming" someone else's namespace by declaring is
        // structurally unavailable, and the only door left is publish, which
        // `may_publish` above already closes.
        let mut r = EventRegistry::new();
        let a = r.declare("flag-manager", "changed").unwrap();
        let b = r.declare("launcher", "changed").unwrap();
        assert_ne!(a, b);
        assert_eq!(r.owner(&a), Some("flag-manager"));
        assert_eq!(r.owner(&b), Some("launcher"));
    }

    #[test]
    fn redeclaring_after_a_restart_is_not_a_conflict_with_yourself() {
        // A plugin process restarting must re-declare its own types on the
        // way back up without the registry treating that as a collision —
        // otherwise every plugin restart would need a fresh registry.
        let mut r = EventRegistry::new();
        let first = r.declare("flag-manager", "profile-changed").unwrap();
        let second = r.declare("flag-manager", "profile-changed").unwrap();
        assert_eq!(first, second);
        assert!(r.may_publish("flag-manager", &first));
    }

    #[test]
    fn cordial_is_reserved_and_cannot_be_declared_under() {
        // Otherwise a plugin naming itself "cordial" could mint fake core
        // events and no subscriber could tell them from the real ones —
        // ADR-006's "plugins may not publish on core event types" has to
        // hold even against a plugin that picks its own id adversarially.
        let mut r = EventRegistry::new();
        assert!(r.declare(CORE_OWNER, "launch").is_err());
    }

    #[test]
    fn subscribing_to_an_undeclared_type_is_refused() {
        let mut r = EventRegistry::new();
        assert!(r.subscribe("launcher", "flag-manager/profile-changed").is_err());
    }

    #[test]
    fn subscribing_is_broader_than_publishing() {
        // A plugin holding only events.subscribe (never events.declare or
        // events.publish) must still be able to receive what someone else
        // declared and published — that asymmetry is the point of splitting
        // the three capabilities in the first place.
        let mut r = EventRegistry::new();
        let t = r.declare("flag-manager", "profile-changed").unwrap();
        assert!(r.subscribe("launcher", &t).is_ok());
        assert!(!r.may_publish("launcher", &t));
        assert_eq!(r.subscribers(&t), vec!["launcher"]);
    }

    #[test]
    fn subscribers_are_filtered_by_exact_type_not_by_declaring_plugin() {
        // Subscribing is per type, not "everything plugin X ever declares" —
        // a subscriber to one of a plugin's event types must not also
        // receive that plugin's other, undeclared-to-it types.
        let mut r = EventRegistry::new();
        let x = r.declare("flag-manager", "x").unwrap();
        let y = r.declare("flag-manager", "y").unwrap();
        r.subscribe("p1", &x).unwrap();
        r.subscribe("p2", &y).unwrap();
        assert_eq!(r.subscribers(&x), vec!["p1"]);
        assert_eq!(r.subscribers(&y), vec!["p2"]);
    }
}
