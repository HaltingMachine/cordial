//! The wire protocol between Cordial and a plugin.
//!
//! Newline-delimited JSON over the plugin's stdin and stdout. Chosen because it
//! is debuggable by eye and by `cat`, works with any language, and needs no
//! shared memory — which matters, because ADR-003 rules out plugins having
//! memory access to Cordial and a shared-memory transport would be the first
//! step back toward it.
//!
//! Every request names a capability. The broker checks it before the call is
//! dispatched, so a plugin cannot reach an effect by naming a method it was not
//! granted.

use crate::capability::Capability;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    /// Correlates the response. Plugins may have several calls in flight.
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Response {
    Ok { id: u64, result: serde_json::Value },
    /// The call was refused. `denied` is distinct from `error` on purpose: a
    /// plugin author needs to tell "I was not allowed" from "it went wrong",
    /// and collapsing them produces bug reports about the wrong thing.
    Denied { id: u64, capability: String },
    Error { id: u64, message: String },
}

impl Response {
    pub fn id(&self) -> u64 {
        match self {
            Response::Ok { id, .. } | Response::Denied { id, .. } | Response::Error { id, .. } => {
                *id
            }
        }
    }
}

/// Which capability a method requires, or `None` if the method is unknown.
///
/// A closed mapping rather than a convention like "flags.* needs flags": a
/// typo in a method name must fail as unknown, not fall through to a capability
/// check that happens to pass.
pub fn required_capability(method: &str) -> Option<Capability> {
    Some(match method {
        "flags.list" => Capability::FlagsRead,
        "flags.get" => Capability::FlagsRead,
        "flags.set" => Capability::FlagsWrite,
        "flags.setDynamic" => Capability::FlagsWriteDynamic,
        "log.write" => Capability::Log,
        "lifecycle.subscribe" => Capability::LifecycleRead,
        "presence.set" => Capability::PresenceSet,
        "presence.clear" => Capability::PresenceSet,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let r = Request { id: 7, method: "flags.get".into(), params: serde_json::json!({"k": "v"}) };
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
    }

    #[test]
    fn params_may_be_omitted() {
        let r: Request = serde_json::from_str(r#"{"id":1,"method":"flags.list"}"#).unwrap();
        assert_eq!(r.params, serde_json::Value::Null);
    }

    #[test]
    fn denied_is_not_an_error() {
        let d = Response::Denied { id: 3, capability: "flags.write".into() };
        let line = serde_json::to_string(&d).unwrap();
        assert!(line.contains(r#""status":"denied""#));
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), d);
    }

    #[test]
    fn presence_is_one_capability_covering_set_and_clear() {
        // Clearing presence is not a lesser power than setting it — both say
        // something about what the user is doing — so they share a capability
        // rather than inviting a plugin to ask for two.
        assert_eq!(required_capability("presence.set"), Some(Capability::PresenceSet));
        assert_eq!(required_capability("presence.clear"), Some(Capability::PresenceSet));
    }

    #[test]
    fn an_unknown_method_maps_to_no_capability() {
        assert!(required_capability("flags.delete_everything").is_none());
        assert!(required_capability("flags").is_none());
    }

    #[test]
    fn setting_a_live_flag_needs_its_own_capability() {
        assert_eq!(required_capability("flags.set"), Some(Capability::FlagsWrite));
        assert_eq!(
            required_capability("flags.setDynamic"),
            Some(Capability::FlagsWriteDynamic)
        );
    }
}
