//! The shell window: an `AdwApplicationWindow` wrapping an `AdwToolbarView`.
//!
//! ADR-002 calls the core shell "a bridge measured in milliseconds, not a
//! product" — window, chooser, and an escape hatch to settings. Nothing here
//! should grow features; anything richer belongs to the UI plugin that takes
//! over at T3.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::chooser;
use crate::flags_file;
use crate::settings;
use crate::shell_config::ShellConfig;

pub fn build(app: &adw::Application, config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) {
    let header = adw::HeaderBar::new();

    let settings_button = gtk::Button::from_icon_name("preferences-system-symbolic");
    settings_button.set_tooltip_text(Some("Settings"));
    header.pack_end(&settings_button);

    // T1: the chooser core paints before the plugin host is up. Real entries
    // arrive over cap:core.launcher.register once that host exists;
    // PlaceholderSource is what stands in for it until then.
    let source = chooser::PlaceholderSource::demo();
    let chooser_widget = chooser::build(&source);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&chooser_widget));

    // --- seam for the engine's surface -----------------------------------
    // At T3 the UI plugin takes over, and later still the engine's own
    // rendered surface is what actually fills this window — a GL/Vulkan
    // widget hosting the Wayland surface the display-backend agent is
    // building concurrently with this crate. That widget does not exist yet
    // and is not built here. When it does, the right shape is a second child
    // under `toolbar_view` (e.g. a second `gtk::Stack` page swapped in on
    // handoff) rather than tearing this window down and building another —
    // ADR-002's open question about a UI-plugin crash notes the shell has to
    // stay retained and hidden rather than destroyed, precisely so the
    // window, header bar and theme survive the handoff instead of flickering
    // through a second window creation.
    // -----------------------------------------------------------------------

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Cordial")
        .default_width(720)
        .default_height(480)
        .content(&toolbar_view)
        .build();

    // Demo data now; see settings.rs for what backs this once the plugin host
    // can actually answer "what's installed". Rc so every click on the
    // settings button reopens against the same (in-memory) state rather than
    // resetting it.
    let registry: Rc<dyn settings::PluginRegistry> = settings::DemoRegistry::installed();
    let flags_path = Rc::new(flags_file::user_flags_path());
    let window_for_settings = window.clone();
    settings_button.connect_clicked(move |_| {
        settings::build_preferences_window(
            &window_for_settings,
            registry.clone(),
            config.clone(),
            config_path.clone(),
            flags_path.clone(),
        )
        .present();
    });

    window.present();
}
