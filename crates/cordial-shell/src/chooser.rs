//! The chooser — ADR-002's T1 surface, painted before the plugin host is up.
//!
//! It has to be real `AdwPreferencesGroup`/`AdwActionRow` widgets rather than
//! hand-drawn boxes, for the same reason the rest of this crate is libadwaita:
//! this is the first thing anyone launching Cordial sees, and getting the
//! chrome (row hover states, focus rings, spacing against the header bar) to
//! merely *look* like libadwaita by hand would be more work than using it.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

/// One entry in the chooser.
///
/// Once the plugin host is wired in, entries arrive over
/// `cap:core.launcher.register` (ADR-002, "the plugin declares *what*, core
/// decides *how*") and core resolves and launches the target itself — a
/// plugin never holds a spawn primitive. This struct is the shape both sides
/// will agree on; it does not know or care where it came from.
pub struct Entry {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_name: Option<String>,
}

/// Where the chooser gets its entries. The only thing this crate depends on;
/// swap `PlaceholderSource` for whatever reads the plugin host's registered
/// entries later and nothing below this trait has to change.
pub trait EntrySource {
    fn entries(&self) -> Vec<Entry>;
}

/// Stand-in entries. There is no plugin host in this standalone binary, so
/// these are fixed rather than discovered — enough to exercise the populated
/// chooser; `empty()` exists to exercise the other branch, the
/// `AdwStatusPage` a fresh install with nothing registered would actually see.
pub struct PlaceholderSource {
    entries: Vec<(String, String, Option<String>, Option<String>)>,
}

impl PlaceholderSource {
    pub fn demo() -> Self {
        Self {
            entries: vec![
                (
                    "roblox".into(),
                    "Roblox".into(),
                    Some("Continue as the last signed-in account".into()),
                    Some("applications-games-symbolic".into()),
                ),
                (
                    "studio".into(),
                    "Roblox Studio".into(),
                    Some("Not this runtime — opens via an existing Vinegar install (ADR-002)".into()),
                    Some("applications-engineering-symbolic".into()),
                ),
            ],
        }
    }

    #[allow(dead_code)] // exercised from tests, kept for anyone wiring up an empty-state screenshot by hand
    pub fn empty() -> Self {
        Self { entries: Vec::new() }
    }
}

impl EntrySource for PlaceholderSource {
    fn entries(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .map(|(id, title, subtitle, icon_name)| Entry {
                id: id.clone(),
                title: title.clone(),
                subtitle: subtitle.clone(),
                icon_name: icon_name.clone(),
            })
            .collect()
    }
}

/// Builds the chooser: an `AdwStatusPage` when there is nothing to launch,
/// otherwise an `AdwPreferencesGroup` of action rows, one per entry, clamped
/// to a readable width rather than stretched edge to edge.
pub fn build(source: &dyn EntrySource) -> gtk::Widget {
    let entries = source.entries();

    if entries.is_empty() {
        let status = adw::StatusPage::builder()
            .icon_name("applications-games-symbolic")
            .title("Nothing to launch yet")
            .description("Plugins that contribute launcher entries will appear here.")
            .vexpand(true)
            .build();
        return status.upcast();
    }

    let group = adw::PreferencesGroup::builder().title("Launch").build();

    for entry in entries {
        let row = adw::ActionRow::builder().title(entry.title).activatable(true).build();
        if let Some(subtitle) = &entry.subtitle {
            row.set_subtitle(subtitle);
        }
        if let Some(icon_name) = &entry.icon_name {
            let image = gtk::Image::from_icon_name(icon_name);
            image.set_icon_size(gtk::IconSize::Large);
            row.add_prefix(&image);
        }
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

        // T2 is the user picking an entry; what happens next belongs to core
        // resolving and launching the target (or, before that plumbing
        // exists, to the plugin host taking over). Neither is wired into this
        // standalone binary, so this only proves the row itself is live.
        let id = entry.id.clone();
        row.connect_activated(move |_| {
            println!("chooser: {id:?} activated — no launch target wired into the standalone shell yet");
        });

        group.add(&row);
    }

    let clamp = adw::Clamp::builder().maximum_size(480).child(&group).build();
    let scroller = gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).build();
    scroller.upcast()
}
