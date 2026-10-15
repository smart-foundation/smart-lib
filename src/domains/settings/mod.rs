//! The **settings** domain: the host's own configuration store, read through
//! the host's own settings engine — over one object-safe [`SettingsBackend`]
//! trait with the frozen `settings.capnp` surface (`get @0` through
//! `getSettingsDiff @17`, plus the subscription interface: 20 contract rows).
//!
//! ## The mechanism on this backend (law #9)
//!
//! glib's `GSettings` engine, spoken through the maintained `gio` crate — the
//! very machinery the host's own `gsettings` command is a thin CLI over:
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `get` / `getAll` / `bulkGet` | the host's compiled schema store plus the host's own dconf database, read through glib's own settings engine |
//! | `listDomains` | the host's own schema source, enumerated |
//! | `watch` / `watchDomain` | the host's own change signal on the engine's own settings object |
//! | `getVersion` / `setIfUnchanged` / `getHistory` / `getSettingsDiff` | this backend's own monotonic version counter over the host's engine, so optimistic concurrency is real |
//! | `set` / `delete` / `clear` / `bulkSet` | the host's own dconf write — the same write `gsettings set` performs — GATED (law #8) |
//!
//! What is DEGRADED, and why: the per-app override verbs, because the contract
//! itself says the host gates them at acquisition and this host's engine
//! carries no per-app namespace (measured); and `listByCategory`, because the
//! contract's category tokens are conventions and glib's schema publishes no
//! per-key category, so the listing records the requested category rather than
//! guessing which keys belong to it.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Settings;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    SETTINGS_AVAILABILITY, SETTINGS_SCHEMA_ID, ReasonClass,
};
pub use backend::{render_value, SettingsBackend, SettingsSubscription};
pub use types::{
    SchemaInfo, SchemaKey, SettingChange, SettingMap, SettingMapEntry, SettingResult, SettingValue,
};
