//! What the user has actually approved.
//!
//! A manifest *requests* capabilities; this is what was *granted*. Keeping them
//! in separate files, owned by different parties, is the whole point — if
//! installing a plugin were enough to grant what it asked for, the manifest
//! would be a formality and the capability system would be decorative.
//!
//! ```json
//! {
//!   "flag-inspector": ["flags.read", "log"],
//!   "themer": ["log"]
//! }
//! ```
//!
//! Default deny. A plugin absent from this file gets nothing, and a capability
//! it requested but that is not listed here is refused at the point of use. That
//! is also why there is no "grant everything" entry: it would be the one line
//! anybody pastes from a forum.

use crate::capability::Capability;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    std::env::var_os("CORDIAL_PLUGIN_GRANTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .unwrap_or_else(std::env::temp_dir)
                .join("cordial/plugin-grants.json")
        })
}

/// Parse a grants document. Unknown capability names are refused rather than
/// ignored: silently dropping one would grant less than the user believes they
/// granted, and they would have no way to tell.
pub fn parse(text: &str) -> Result<BTreeMap<String, BTreeSet<Capability>>, String> {
    let raw: BTreeMap<String, Vec<String>> =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for (plugin, names) in raw {
        let mut caps = BTreeSet::new();
        for n in names {
            match Capability::parse(&n) {
                Some(c) => {
                    caps.insert(c);
                }
                None => return Err(format!("unknown capability {n:?} granted to {plugin:?}")),
            }
        }
        out.insert(plugin, caps);
    }
    Ok(out)
}

/// Load grants, or nothing at all.
///
/// A missing file means no plugin has been approved, which is the correct
/// default rather than an error. A malformed file grants nothing and says so —
/// falling back to "grant what was requested" on a parse error would turn a typo
/// into a privilege escalation.
pub fn load(path: &Path) -> BTreeMap<String, BTreeSet<Capability>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match parse(&text) {
        Ok(g) => g,
        Err(e) => {
            println!("  plugin grants: {} is not usable ({e}); granting nothing", path.display());
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_parse() {
        let g = parse(r#"{"a":["log","flags.read"],"b":[]}"#).unwrap();
        assert!(g["a"].contains(&Capability::Log));
        assert!(g["a"].contains(&Capability::FlagsRead));
        assert!(g["b"].is_empty());
    }

    #[test]
    fn an_unknown_capability_is_refused() {
        // Dropping it silently would grant less than the user thinks.
        assert!(parse(r#"{"a":["log","process.spawn"]}"#).is_err());
    }

    #[test]
    fn a_missing_file_grants_nothing() {
        assert!(load(Path::new("/nonexistent/plugin-grants.json")).is_empty());
    }

    #[test]
    fn a_malformed_file_grants_nothing_rather_than_everything() {
        let dir = std::env::temp_dir().join("cordial-grants-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(load(&p).is_empty());
    }
}
