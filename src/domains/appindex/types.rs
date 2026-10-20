use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::appindex_capnp;

/// The install state — the contract's `AppState`. A desktop entry on this
/// host *is* an installed application, so the index only ever reports
/// `Installed`; the other arms are the contract's, carried for the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    /// The backend does not say.
    Any,
    /// Not installed.
    NotInstalled,
    /// Being installed.
    Installing,
    /// Installed.
    Installed,
    /// Being updated.
    Updating,
    /// Being uninstalled.
    Uninstalling,
    /// Errored.
    Errored,
    /// A backend-defined state.
    Custom,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Any
    }
}

impl AppState {
    /// The generated contract enum.
    pub const fn to_generated(self) -> appindex_capnp::AppState {
        match self {
            AppState::Any => appindex_capnp::AppState::Any,
            AppState::NotInstalled => appindex_capnp::AppState::NotInstalled,
            AppState::Installing => appindex_capnp::AppState::Installing,
            AppState::Installed => appindex_capnp::AppState::Installed,
            AppState::Updating => appindex_capnp::AppState::Updating,
            AppState::Uninstalling => appindex_capnp::AppState::Uninstalling,
            AppState::Errored => appindex_capnp::AppState::Errored,
            AppState::Custom => appindex_capnp::AppState::Custom,
        }
    }

    /// Read the generated contract enum back.
    pub fn from_generated(state: appindex_capnp::AppState) -> AppState {
        match state {
            appindex_capnp::AppState::NotInstalled => AppState::NotInstalled,
            appindex_capnp::AppState::Installing => AppState::Installing,
            appindex_capnp::AppState::Installed => AppState::Installed,
            appindex_capnp::AppState::Updating => AppState::Updating,
            appindex_capnp::AppState::Uninstalling => AppState::Uninstalling,
            appindex_capnp::AppState::Errored => AppState::Errored,
            appindex_capnp::AppState::Custom => AppState::Custom,
            _ => AppState::Any,
        }
    }
}

/// One application in the index — a freedesktop desktop entry as this host
/// publishes it.
///
/// The entry is the *whole* record: the host publishes no package id, no
/// version, no developer and no licence for a desktop entry, so those fields
/// are not invented here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppEntry {
    /// The application's identifier — the desktop file's name, which is what
    /// the host itself keys the entry by (`firefox.desktop`).
    pub id: String,
    /// The human name (`Name`).
    pub name: String,
    /// The generic name (`GenericName`).
    pub generic_name: String,
    /// The description (`Comment`).
    pub comment: String,
    /// The command line (`Exec`).
    pub exec: String,
    /// The icon name or path (`Icon`).
    pub icon: String,
    /// Whether the entry wants a terminal (`Terminal`).
    pub terminal: bool,
    /// Whether the entry is hidden from menus (`NoDisplay`).
    pub no_display: bool,
    /// Whether the entry is deleted (`Hidden`).
    pub hidden: bool,
    /// The categories (`Categories`).
    pub categories: Vec<String>,
    /// The keywords (`Keywords`).
    pub keywords: Vec<String>,
    /// The MIME types the entry handles (`MimeType`).
    pub mime_types: Vec<String>,
    /// The directory the entry came from — the open source token.
    pub source: String,
    /// The file the entry came from.
    pub path: String,
    /// The install state.
    pub state: AppState,
    /// Whether the engine has this entry pinned.
    pub pinned: bool,
}

impl AppEntry {
    /// The contract's `AppInfo`.
    ///
    /// The fields a desktop entry does not carry — `packageId`, `version`,
    /// `developer`, `licence`, the sizes and the dates — are left at the
    /// generated defaults rather than filled with a guess: the host publishes
    /// none of them for a desktop entry.
    pub fn write_to(&self, builder: &mut appindex_capnp::app_info::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_description(&self.comment);
        builder.set_source(&self.source);
        builder.set_state(self.state.to_generated());
        builder.set_pinned(self.pinned);
        if !self.icon.is_empty() {
            // The contract's `AppIcon` carries icon *data*; a desktop entry
            // names its icon instead, so the name rides in the icon's
            // metadata rather than being passed off as a payload.
            let mut icon = builder.reborrow().init_icon();
            let mut metadata = icon.reborrow().init_metadata();
            let mut entries = metadata.reborrow().init_entries(1);
            let mut entry = entries.reborrow().get(0);
            entry.set_key("name");
            entry.set_value(&self.icon);
        }
        let mut categories = builder.reborrow().init_categories(self.categories.len() as u32);
        for (index, category) in self.categories.iter().enumerate() {
            categories.set(index as u32, category.as_str());
        }
    }

    /// Read the contract's `AppInfo` back.
    pub fn from_reader(reader: &appindex_capnp::app_info::Reader<'_>) -> Self {
        let mut categories = Vec::new();
        if let Ok(list) = reader.get_categories() {
            for category in list.iter() {
                categories.push(text_of(category));
            }
        }
        let icon = reader
            .get_icon()
            .ok()
            .and_then(|icon| {
                icon.get_metadata()
                    .ok()
                    .and_then(|metadata| metadata.get_entries().ok())
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            text_of(entry.get_key()) == "name"
                        })
                    })
            })
            .map(|entry| text_of(entry.get_value()))
            .unwrap_or_default();
        AppEntry {
            id: String::new(),
            name: crate::domains::appindex::types::text_of(reader.get_name()),
            generic_name: String::new(),
            comment: crate::domains::appindex::types::text_of(reader.get_description()),
            exec: String::new(),
            icon,
            terminal: false,
            no_display: false,
            hidden: false,
            categories,
            keywords: Vec::new(),
            mime_types: Vec::new(),
            source: crate::domains::appindex::types::text_of(reader.get_source()),
            path: String::new(),
            state: AppState::from_generated(
                reader.get_state().unwrap_or(appindex_capnp::AppState::Any),
            ),
            pinned: reader.get_pinned(),
        }
    }
}

/// The index's statistics — the contract's `AppIndexStats`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppStats {
    /// How many entries the index holds.
    pub total_apps: u32,
    /// How many of them are installed (a desktop entry is an installed app).
    pub installed_apps: u32,
    /// The per-source counts.
    pub source_counts: Vec<(String, u32)>,
    /// The per-category counts.
    pub category_counts: Vec<(String, u32)>,
}

/// The search filter — the contract's `AppSearchFilter`, narrowed to the
/// fields a desktop-entry index can answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppSearchFilter {
    /// Substring match over the name, the generic name, the comment and the
    /// keywords.
    pub name_contains: String,
    /// The categories the entry must carry.
    pub categories: Vec<String>,
    /// The sources the entry must come from.
    pub sources: Vec<String>,
}

/// The pagination window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// The first row to return.
    pub offset: u32,
    /// How many rows to return (0 means one — never "everything").
    pub limit: u32,
}

impl Pagination {
    /// The effective limit: a zero limit means one row, never an unbounded
    /// read.
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A page of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// How many rows the store holds in total.
    pub total: u64,
}

/// A subscription handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Read a capnp text field into an owned string.
pub(crate) fn text_of(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}
