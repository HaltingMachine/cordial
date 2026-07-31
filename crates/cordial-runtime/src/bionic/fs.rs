//! Paths that exist on every Android device and on no Linux desktop.
//!
//! Roblox probes `/sys/devices/system/cpu/*/cpufreq/stats/time_in_state` during
//! startup, stores the `FILE*` it gets, and flushes it later **without checking
//! for null**. On Android that is harmless — the file is always there. Here the
//! open fails, and the crash lands inside glibc's `fflush` with nothing pointing
//! back at the path that caused it.
//!
//! Fixing this by hardening the flush would be papering over the wrong side. The
//! framework layer's job is to make the environment look like Android, and on
//! Android these files exist. So they exist here too, and read as empty.
//!
//! Empty rather than fabricated: these are frequency-residency counters, and
//! inventing plausible-looking numbers would feed the engine's own performance
//! heuristics data about a CPU that does not exist. Zero samples is the honest
//! answer for a machine that does not export the statistic.

use std::ffi::{c_char, c_void, CStr, CString};

/// Path prefixes Android guarantees and this host does not provide.
///
/// Matched as a prefix plus a required component, so an unrelated `/sys` read
/// is not silently swallowed — only the specific shapes Roblox is known to probe
/// are answered, and anything else still fails honestly.
const ANDROID_ONLY: &[(&str, &str)] = &[
    ("/sys/devices/system/cpu/", "time_in_state"),
    ("/sys/devices/system/cpu/", "cpufreq/stats"),
];

fn is_android_only(path: &str) -> bool {
    ANDROID_ONLY
        .iter()
        .any(|(prefix, needle)| path.starts_with(prefix) && path.contains(needle))
}

extern "C" {
    #[link_name = "fopen"]
    fn host_fopen(path: *const c_char, mode: *const c_char) -> *mut c_void;
}

/// `fopen`, with Android's sysfs filled in.
pub extern "C" fn bionic_fopen(path: *const c_char, mode: *const c_char) -> *mut c_void {
    // SAFETY: fopen's contract is a NUL-terminated path and mode.
    let file = unsafe { host_fopen(path, mode) };
    if !file.is_null() || path.is_null() {
        return file;
    }

    // SAFETY: checked non-null above.
    let name = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    if !is_android_only(&name) {
        if std::env::var_os("CORDIAL_TRACE").is_some() {
            eprintln!("[fopen] failed: {name}");
        }
        return file;
    }

    // An empty, always-readable stand-in. /dev/null reads EOF immediately, which
    // is what "this counter has no samples" looks like to any parser.
    let devnull = CString::new("/dev/null").expect("literal");
    // SAFETY: both arguments are valid NUL-terminated strings.
    unsafe { host_fopen(devnull.as_ptr(), mode) }
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    vec![("fopen", bionic_fopen as *const () as *mut c_void)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_cpu_stats_are_recognised() {
        assert!(is_android_only(
            "/sys/devices/system/cpu/cpu0/cpufreq/stats/time_in_state"
        ));
        assert!(is_android_only(
            "/sys/devices/system/cpu/cpufreq/stats/cpu3/time_in_state"
        ));
    }

    #[test]
    fn unrelated_paths_still_fail_honestly() {
        // Swallowing every failed open would hide real bugs behind an empty file.
        assert!(!is_android_only("/etc/passwd"));
        assert!(!is_android_only("/sys/class/net/eth0/address"));
        assert!(!is_android_only("/home/user/roblox.log"));
    }

    #[test]
    fn a_missing_android_path_opens_and_reads_empty() {
        let path = CString::new("/sys/devices/system/cpu/cpu0/cpufreq/stats/time_in_state")
            .expect("literal");
        let mode = CString::new("rb").expect("literal");
        let f = bionic_fopen(path.as_ptr(), mode.as_ptr());
        assert!(!f.is_null(), "Roblox flushes this without a null check");

        extern "C" {
            fn fgetc(f: *mut c_void) -> i32;
            fn fclose(f: *mut c_void) -> i32;
        }
        // SAFETY: `f` came from fopen just above and is closed once.
        unsafe {
            assert_eq!(fgetc(f), -1, "should read as empty, not as arbitrary data");
            fclose(f);
        }
    }
}
