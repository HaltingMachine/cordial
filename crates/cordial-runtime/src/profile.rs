//! Profiles: one account's storage, and the lock that keeps one window on it.
//!
//! See [ADR-012](../../../docs/adr/ADR-012-profiles-and-instances.md) for the
//! vocabulary, which matters here. An *instance* is a running Cordial process —
//! a window, which is what Roblox means by the word. A *profile* is the
//! directory this module resolves: one account's Roblox storage, plugin set and
//! flag overrides. An instance runs a profile.
//!
//! Nothing structurally prevents two Cordial processes opening the same profile.
//! Unlike Fishstrap on Windows there is no singleton mutex to defeat, because
//! each Cordial process is genuinely independent — which is what makes
//! multi-instance nearly free here. That same freedom is the hazard: two
//! instances on one profile are two processes writing one `appData` and one
//! cookie store, and Roblox's storage is not built for it. The failure does not
//! look like "you did something unsupported"; it looks like Cordial corrupting a
//! login.

use std::ffi::c_int;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

/// `LOCK_EX | LOCK_NB` from `sys/file.h`. Non-blocking on purpose: a second
/// launch should say so immediately rather than hang waiting for the first to
/// exit, which reads as the client failing to start.
const LOCK_EX_NB: c_int = 2 | 4;

/// Where profiles live. `$XDG_DATA_HOME/cordial/profiles`, falling back the way
/// the rest of the tree does.
pub fn root() -> PathBuf {
    std::env::var_os("CORDIAL_PROFILE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                .unwrap_or_else(std::env::temp_dir)
                .join("cordial/profiles")
        })
}

/// A profile name that cannot escape the profile root.
///
/// Names reach this from a command line and, later, from the launcher's own UI,
/// so `../` and absolute paths are refused rather than sanitised — quietly
/// rewriting a name would mean the profile a user selected is not the one they
/// get. Same reasoning as the zip-slip defence in `android::asset`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn dir(name: &str) -> Result<PathBuf, String> {
    if !is_valid_name(name) {
        return Err(format!(
            "{name:?} is not a usable profile name; use letters, digits, - and _"
        ));
    }
    Ok(root().join(name))
}

/// An instance's claim on a profile, released when this is dropped.
///
/// The file is deliberately held open: `flock` is tied to the open description,
/// so closing the handle releases the lock. Storing it here means the lock lives
/// exactly as long as the value, and the process exiting — cleanly or not —
/// releases it, which a lock file containing a PID would not.
#[derive(Debug)]
pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Lock {
    pub fn profile_dir(&self) -> &Path {
        &self.path
    }
}

/// Take this instance's claim on `name`, creating the profile if it is new.
///
/// Fails immediately if another instance holds it. **Advisory, and honestly so:**
/// `flock` does not stop a process that never asks. It stops Cordial doing this
/// by accident, which is the actual failure mode — someone launching twice, not
/// someone defeating a lock.
pub fn acquire(name: &str) -> Result<Lock, String> {
    let path = dir(name)?;
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let lock_path = path.join(".lock");
    let file = File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;

    // SAFETY: `file` is open for the duration of the call and outlives it.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX_NB) } != 0 {
        return Err(format!(
            "profile {name:?} is already open in another Cordial window; \
             use a different profile, or close that one first"
        ));
    }
    Ok(Lock { _file: file, path })
}

/// Move a pre-ADR-012 layout into place, once.
///
/// Storage used to live at `cordial/instances/default/run`, which named a window
/// and contained a login. Renaming without moving would present as being logged
/// out for no reason, which is the class of failure this project keeps a list
/// of, so the directory is moved rather than abandoned.
///
/// Runs only when the old path exists and the new one does not, so it cannot
/// clobber a profile someone has already used.
pub fn migrate_legacy_layout() -> Option<PathBuf> {
    let legacy = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/instances/default");
    let target = root().join("default");
    if !legacy.is_dir() || target.exists() {
        return None;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    match std::fs::rename(&legacy, &target) {
        Ok(()) => {
            println!(
                "  profiles: moved {} to {} (ADR-012)",
                legacy.display(),
                target.display()
            );
            Some(target)
        }
        // A cross-device rename is the one plausible failure. Leaving the old
        // directory in place and saying so beats a half-copied login.
        Err(e) => {
            println!(
                "  profiles: could not move {} to {} ({e}); the old location is untouched",
                legacy.display(),
                target.display()
            );
            None
        }
    }
}

/// Profiles that exist, for the launcher's switcher.
pub fn list() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_valid_name(n))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CORDIAL_PROFILE_ROOT` is process-wide and cargo runs tests in parallel
    /// threads of one process, so two tests pointing it at different scratch
    /// directories will interleave and read each other's. They passed anyway on
    /// the first run, which is exactly how a one-in-three flake gets committed —
    /// this project already has one of those in its history. Serialised instead.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let p = std::env::temp_dir().join(format!("cordial-profile-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        std::env::set_var("CORDIAL_PROFILE_ROOT", &p);
        (p, guard)
    }

    #[test]
    fn a_name_cannot_escape_the_profile_root() {
        // A profile name reaches this from a command line and from the
        // launcher's UI. Sanitising rather than refusing would mean the profile
        // someone selected is not the one they get.
        assert!(!is_valid_name("../../etc"));
        assert!(!is_valid_name("/absolute"));
        assert!(!is_valid_name("has/slash"));
        assert!(!is_valid_name(""));
        assert!(is_valid_name("default"));
        assert!(is_valid_name("alt_account-2"));
    }

    #[test]
    fn a_second_instance_cannot_take_a_held_profile() {
        // The whole point of ADR-012's lock: two instances on one profile are
        // two processes writing one cookie store, and the corruption that
        // follows presents as a Cordial bug rather than as unsupported use.
        let (_root, _g) = scratch("held");
        let first = acquire("default").expect("first instance takes the profile");
        let second = acquire("default");
        assert!(second.is_err(), "a second instance must be refused");
        assert!(second.unwrap_err().contains("already open"));
        drop(first);
    }

    #[test]
    fn releasing_a_profile_lets_the_next_instance_have_it() {
        // flock is tied to the open file description, so dropping the Lock must
        // actually release it — including when the process died rather than
        // exited, which a PID file would not handle.
        let (_root, _g) = scratch("released");
        let first = acquire("default").unwrap();
        drop(first);
        assert!(acquire("default").is_ok(), "the profile must be reusable");
    }

    #[test]
    fn different_profiles_do_not_contend() {
        // Multi-instance is the point: two windows on two profiles is the
        // supported shape and must not block.
        let (_root, _g) = scratch("distinct");
        let a = acquire("main").unwrap();
        let b = acquire("alt").unwrap();
        assert_ne!(a.profile_dir(), b.profile_dir());
    }
}
