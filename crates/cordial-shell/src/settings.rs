//! The minimal settings fallback ADR-002 requires.
//!
//! Not the settings surface — the UI plugin owns that once it takes over at
//! T3. This exists purely for the recoverability argument ADR-002 makes: if
//! the UI plugin is broken (bad update, protocol mismatch after a Cordial
//! upgrade), the user still needs a way to see what is installed and turn
//! the offending one off, plus enough of Cordial's own appearance and
//! graphics preference to be usable, without a terminal.
//!
//! `AdwPreferencesWindow` with several `AdwPreferencesPage`s — Appearance,
//! General, Plugins — each becomes its own tab/sidebar entry for free; that
//! is libadwaita's own page-switcher, not something built here.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

use cordial_plugins::capability::Capability;
use cordial_plugins::manifest::{self, Plugin};

use crate::flags_file::{self, RENDERER_BACKEND_FLAG};
use crate::shell_config::{AppearanceScheme, ShellConfig};

/// A plugin as this settings surface cares about it.
///
/// `cordial_plugins::manifest::Plugin` is what a plugin *requests*; whether
/// it is currently enabled is a registry concern, not something the manifest
/// carries, so it lives here rather than being added to a crate we were told
/// not to touch.
pub struct PluginState {
    pub plugin: Plugin,
    pub enabled: bool,
}

/// Where the settings window gets its plugin list, and where a toggle lands.
///
/// The real implementation reads `manifest::discover(manifest::plugin_root())`
/// for the list and, for `set_enabled(_, false)`, most likely clears that
/// plugin's entry from `grants::path()` — a plugin holding no granted
/// capabilities is already inert, which sidesteps needing a second on/off
/// concept alongside the grants file. `DemoRegistry` below is a placeholder
/// standing in for that until the plugin host exists to ask.
pub trait PluginRegistry {
    fn list(&self) -> Vec<PluginState>;
    fn set_enabled(&self, id: &str, enabled: bool);
}

/// In-memory stand-in for the plugin registry.
///
/// The manifests are real `cordial_plugins::manifest::parse` output, not
/// hand-built structs, so this exercises the actual parser and capability
/// names rather than a shape that merely looks similar.
pub struct DemoRegistry {
    plugins: RefCell<Vec<PluginState>>,
}

impl DemoRegistry {
    pub fn installed() -> Rc<Self> {
        // Not a real plugin directory — nothing here is ever read from disk —
        // but manifest::parse still wants a `dir` to resolve `entry` against.
        let dir = std::path::PathBuf::from("/nonexistent/demo-plugin-dir");
        let plugin = |json: &str| -> Plugin {
            manifest::parse(json, &dir).expect("demo manifest is well-formed; covered by cordial-plugins' own tests")
        };
        let plugins = vec![
            PluginState {
                plugin: plugin(
                    r#"{"id":"presence-rpc","name":"Discord Presence","entry":"main.ts","capabilities":["lifecycle.read","presence.set"]}"#,
                ),
                enabled: true,
            },
            PluginState {
                plugin: plugin(
                    r#"{"id":"fps-tweaks","name":"FPS Tweaks","entry":"main.ts","capabilities":["flags.read","flags.write"]}"#,
                ),
                enabled: true,
            },
            PluginState {
                plugin: plugin(
                    r#"{"id":"asset-overlay-demo","name":"Asset Overlay Demo","entry":"main.ts","capabilities":["assets.override"]}"#,
                ),
                enabled: false,
            },
        ];
        Rc::new(Self { plugins: RefCell::new(plugins) })
    }
}

impl PluginRegistry for DemoRegistry {
    fn list(&self) -> Vec<PluginState> {
        self.plugins
            .borrow()
            .iter()
            .map(|s| PluginState { plugin: s.plugin.clone(), enabled: s.enabled })
            .collect()
    }

    fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(state) = self.plugins.borrow_mut().iter_mut().find(|s| s.plugin.manifest.id == id) {
            state.enabled = enabled;
        }
    }
}

fn capability_summary(plugin: &Plugin) -> String {
    if plugin.requested.is_empty() {
        return "Requests no capabilities".to_string();
    }
    let names: Vec<&str> = plugin.requested.iter().copied().map(Capability::name).collect();
    format!("Requests: {}", names.join(", "))
}

/// "Appearance" — Cordial's own theme preference, independent of anything
/// else on the desktop. `AppearanceScheme::apply` is what makes this take
/// effect immediately; this function only wires the row to it and to disk.
fn build_appearance_page(config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description(
            "This is Cordial's own preference, not the desktop's. System follows \
             org.freedesktop.appearance and updates live; Light and Dark stay fixed \
             regardless of what the desktop is doing.",
        )
        .build();

    // Order has to match AppearanceScheme::index/from_index.
    let model = gtk::StringList::new(&["Light", "Dark", "System"]);
    let row = adw::ComboRow::builder()
        .title("Theme")
        .model(&model)
        .selected(config.borrow().appearance.index())
        .build();

    row.connect_selected_notify(move |row| {
        let scheme = AppearanceScheme::from_index(row.selected());
        scheme.apply();
        config.borrow_mut().appearance = scheme;
        if let Err(e) = crate::shell_config::save(&config_path, &config.borrow()) {
            eprintln!("shell: could not save {}: {e}", config_path.display());
        }
    });

    group.add(&row);
    page.add(&group);
    page
}

/// "General" — ordinary client settings. Only the renderer preference is
/// wired to anything real; see `flags_file.rs` for why resolution and
/// monitor placement are not offered here despite being genuine
/// `cordial-load` options in their own right.
fn build_general_page(flags_path: Rc<PathBuf>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("General").icon_name("preferences-other-symbolic").build();

    let group = adw::PreferencesGroup::builder()
        .title("Graphics")
        .description(
            "Written to the flags.json cordial-runtime already treats as your own \
             overrides — always wins over a plugin's, and needs a relaunch: FString \
             flags are read once at startup.",
        )
        .build();

    let current = flags_file::read_string_flag(&flags_path, RENDERER_BACKEND_FLAG);
    let selected = if current.as_deref() == Some("Vulkan") { 1 } else { 0 };

    let model = gtk::StringList::new(&["Automatic", "Vulkan"]);
    let row = adw::ComboRow::builder()
        .title("Renderer")
        .subtitle("Automatic lets Roblox dlopen Vulkan and fall back to GLES2 itself")
        .model(&model)
        .selected(selected)
        .build();

    row.connect_selected_notify(move |row| {
        let value = if row.selected() == 1 { Some("Vulkan") } else { None };
        if let Err(e) = flags_file::set_string_flag(&flags_path, RENDERER_BACKEND_FLAG, value) {
            eprintln!("shell: could not update {}: {e}", flags_path.display());
        }
    });

    group.add(&row);
    page.add(&group);
    page
}

/// "Plugins" — the escape hatch ADR-002 actually asks for.
fn build_plugins_page(registry: Rc<dyn PluginRegistry>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Plugins")
        .icon_name("application-x-addon-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Installed plugins")
        .description("Turning a plugin off here is the escape hatch ADR-002 asks for: it stays installed, it just stops running.")
        .build();

    for state in registry.list() {
        let title = if state.plugin.manifest.name.is_empty() {
            state.plugin.manifest.id.clone()
        } else {
            state.plugin.manifest.name.clone()
        };

        let row = adw::SwitchRow::builder()
            .title(title)
            .subtitle(capability_summary(&state.plugin))
            .active(state.enabled)
            .build();

        let registry = registry.clone();
        let id = state.plugin.manifest.id.clone();
        row.connect_active_notify(move |row| {
            registry.set_enabled(&id, row.is_active());
        });

        group.add(&row);
    }

    page.add(&group);
    page
}

/// Builds the `AdwPreferencesWindow`: Appearance, General and Plugins, one
/// `AdwPreferencesPage` each.
pub fn build_preferences_window(
    parent: &impl IsA<gtk::Window>,
    registry: Rc<dyn PluginRegistry>,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
    flags_path: Rc<PathBuf>,
) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .title("Settings")
        .search_enabled(false)
        .build();

    window.add(&build_appearance_page(config, config_path));
    window.add(&build_general_page(flags_path));
    window.add(&build_plugins_page(registry));

    window
}
