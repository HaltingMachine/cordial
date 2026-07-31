//! Roblox's client settings — the FastFlag set the engine runs on.
//!
//! On Android the *application* fetches this document and hands it to the engine
//! through `NativeGLInterface.nativeInitClientSettings`. The engine never
//! fetches it itself: breakpoints on `getaddrinfo`, `connect` and `SSL_connect`
//! are never hit during startup, so there is no request of the engine's own to
//! fix or proxy. Cordial is the host application here, so doing the fetch is the
//! job, not a workaround.
//!
//! ## The call contract, established by experiment
//!
//! `nativeInitClientSettings(String, String, String)` returns an `int`, and that
//! return value is the only trustworthy signal — the engine's own `FLog` output
//! is not routed anywhere visible in this build. Feeding it known-good and
//! known-bad documents settles what it means:
//!
//! | first argument | result |
//! |---|---|
//! | the real document, `{"applicationSettings": {...}}` | `0` |
//! | `{"applicationSettings":{"FFlagNotARealFlag":"True"}}` | `0` |
//! | `this is not json at all` | `1` |
//! | `{}` — valid JSON, no `applicationSettings` key | `1` |
//!
//! So **`0` is success**, the document goes in the *first* argument, and the
//! `applicationSettings` wrapper must be kept rather than unwrapped. Passing the
//! document in either of the other two positions returns `1` regardless of
//! whether it is valid, which is what "this argument is not the settings" looks
//! like. One of those two is an overrides document — the engine has a
//! `ParseFailure on overrides` log string — but which is still unestablished, so
//! both are left empty rather than guessed at.
//!
//! Those first readings were confounded: `--client-settings` fed *two* call
//! sites, so `nativeInitializeNativeFlags` was receiving the document too and
//! the result could not be attributed to this call alone. On the automatic path
//! the flag-names call gets nothing, and the discriminator survives cleanly —
//! empty string gives `1`, the real document gives `0`. The conclusion held, but
//! it had not actually been shown until the two were separated.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Roblox's settings CDN. The application name is `AndroidApp`; it is not a
/// guess — `AndroidClient`, `AndroidPlayer`, `AndroidClientSettings` and
/// `AndroidAppSettings` all return HTTP 400 "The application name is invalid",
/// and `AndroidApp` returns the real 1.2 MB document.
const URL: &str = "https://clientsettingscdn.roblox.com/v2/settings/application/AndroidApp";

/// How long a cached copy is used before refetching.
///
/// Roblox changes flags continuously, so this is not "cache forever". It is long
/// enough that ordinary repeat launches do not each hit the network, and short
/// enough that a machine left running picks up changes within a session or two.
const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

fn cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/clientsettings.json")
}

fn fresh(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let age = SystemTime::now().duration_since(meta.modified().ok()?).ok()?;
    (age < MAX_AGE).then(|| std::fs::read_to_string(path).ok())?
}

/// Looks like a settings document rather than an error page.
///
/// Worth checking before caching: the CDN answers a bad application name with a
/// perfectly well-formed JSON error body, and caching that would produce six
/// hours of failures that look like a flag problem rather than a fetch problem.
fn plausible(body: &str) -> bool {
    body.contains("\"applicationSettings\"")
}

/// The client settings document, from cache when it is fresh and from Roblox
/// otherwise.
///
/// Returns `None` rather than failing the launch: the engine is given whatever
/// it can be given, and a client that starts without flags is more useful than
/// one that refuses to start because a CDN was unreachable.
pub fn load(explicit: Option<&str>) -> Option<String> {
    if let Some(path) = explicit {
        return std::fs::read_to_string(path).ok();
    }
    let cache = cache_path();
    if let Some(body) = fresh(&cache) {
        return Some(body);
    }
    match fetch() {
        Some(body) => {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cache, &body);
            Some(body)
        }
        // A stale copy beats nothing when the network is down.
        None => std::fs::read_to_string(&cache).ok(),
    }
}

fn fetch() -> Option<String> {
    let body = ureq::get(URL)
        .call()
        .ok()?
        .body_mut()
        // The document is ~1.2 MB and ureq's default read limit is smaller, so
        // an unset limit here is the difference between the real settings and a
        // silent truncation that would parse as valid JSON.
        .read_to_string()
        .ok()?;
    plausible(&body).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_body_is_not_mistaken_for_settings() {
        // What the CDN actually returns for a bad application name. It is valid
        // JSON, so only the shape check distinguishes it.
        let err = r#"{"errors":[{"code":1,"message":"The application name is invalid."}]}"#;
        assert!(!plausible(err));
        assert!(plausible(r#"{"applicationSettings":{"FFlagX":"True"}}"#));
    }

    #[test]
    fn an_explicit_path_bypasses_the_network() {
        let dir = std::env::temp_dir().join("cordial-cs-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, r#"{"applicationSettings":{}}"#).unwrap();
        assert_eq!(
            load(Some(p.to_str().unwrap())).as_deref(),
            Some(r#"{"applicationSettings":{}}"#)
        );
    }
}
