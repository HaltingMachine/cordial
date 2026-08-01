//! Cordial's own shell preferences.
//!
//! Distinct from `flags_file.rs`, which speaks to the engine, and from
//! `cordial_plugins::grants`, which speaks to what a plugin may do. This is
//! the shell's own state, and for now that is exactly one thing: which
//! appearance the user asked Cordial itself to use.
//!
//! `$XDG_CONFIG_HOME/cordial/shell.json`, falling back to `$HOME/.config` —
//! the same layout `cordial_plugins::grants::path` and
//! `cordial_plugins::manifest::plugin_root` use — and the same
//! default-on-anything-wrong behaviour as `grants::load`: a missing or
//! malformed file means "use the defaults", not "refuse to start". Nobody
//! but this shell ever writes this file, so a malformed one is far likelier
//! to be an interrupted write than anything adversarial, and refusing to
//! start over that would be a worse failure than quietly falling back.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What appearance Cordial itself should use.
///
/// Not a desktop-wide setting — `AdwStyleManager::set_color_scheme` applies
/// this to this application only. `System` means *follow*
/// `org.freedesktop.appearance color-scheme`, live, the way ADR-011 already
/// relies on for the canvas background; it must never mean *write* that
/// setting. Cordial has no business changing the desktop's theme to satisfy
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceScheme {
    Light,
    Dark,
    System,
}

impl AppearanceScheme {
    /// Order matches the `AdwComboRow` model in `settings.rs` — `index` and
    /// `from_index` are the seam between the two, kept as plain position
    /// rather than a second name-keyed lookup that could drift from the
    /// model's actual contents.
    pub fn index(self) -> u32 {
        match self {
            AppearanceScheme::Light => 0,
            AppearanceScheme::Dark => 1,
            AppearanceScheme::System => 2,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => AppearanceScheme::Light,
            1 => AppearanceScheme::Dark,
            _ => AppearanceScheme::System,
        }
    }

    /// Applies to this process only. `ColorScheme::Default` is what makes
    /// `System` live — libadwaita keeps tracking the portal itself once the
    /// override is lifted, nothing here has to poll or resubscribe.
    pub fn apply(self) {
        let scheme = match self {
            AppearanceScheme::Light => libadwaita::ColorScheme::ForceLight,
            AppearanceScheme::Dark => libadwaita::ColorScheme::ForceDark,
            AppearanceScheme::System => libadwaita::ColorScheme::Default,
        };
        libadwaita::StyleManager::default().set_color_scheme(scheme);
    }
}

impl Default for AppearanceScheme {
    fn default() -> Self {
        AppearanceScheme::System
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub appearance: AppearanceScheme,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self { appearance: AppearanceScheme::default() }
    }
}

/// `CORDIAL_SHELL_CONFIG` overrides the path outright, the same override
/// pattern `cordial_plugins::grants::path` and `manifest::plugin_root` use —
/// useful for tests and for running more than one Cordial config side by
/// side without them fighting over the same file.
pub fn path() -> PathBuf {
    std::env::var_os("CORDIAL_SHELL_CONFIG").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cordial/shell.json")
    })
}

/// Load the config, or the defaults. A missing file is the ordinary case —
/// most people never open settings — and a malformed one is reported and
/// treated the same as missing, per the module docs above.
pub fn load(path: &Path) -> ShellConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ShellConfig::default();
    };
    match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            println!("  shell: {} is not usable ({e}); using defaults", path.display());
            ShellConfig::default()
        }
    }
}

pub fn save(path: &Path, config: &ShellConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config).expect("ShellConfig always serialises");
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("cordial-shell-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn a_missing_file_defaults_to_system() {
        let p = scratch("missing.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(load(&p).appearance, AppearanceScheme::System);
    }

    #[test]
    fn a_malformed_file_falls_back_to_defaults_rather_than_refusing_to_start() {
        let p = scratch("malformed.json");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(load(&p).appearance, AppearanceScheme::System);
    }

    #[test]
    fn a_saved_choice_round_trips() {
        let p = scratch("roundtrip.json");
        save(&p, &ShellConfig { appearance: AppearanceScheme::Dark }).unwrap();
        assert_eq!(load(&p).appearance, AppearanceScheme::Dark);
    }

    #[test]
    fn index_and_from_index_agree_with_each_other() {
        for scheme in [AppearanceScheme::Light, AppearanceScheme::Dark, AppearanceScheme::System] {
            assert_eq!(AppearanceScheme::from_index(scheme.index()), scheme);
        }
    }
}
