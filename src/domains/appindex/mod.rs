//! The **appindex** domain: the host's own application index — over one
//! object-safe [`AppindexBackend`] trait.
//!
//! Native mechanism: the freedesktop **desktop-entry corpus** the host
//! publishes. The XDG data directories (`$XDG_DATA_HOME/applications` and
//! each `$XDG_DATA_DIRS/applications`) hold the `.desktop` files that the
//! host's own launcher reads, and this backend reads exactly those files —
//! no shellout, no `gtk-launch`, no `xdg-open` (law #9). The answers are
//! therefore the host's own applications, counted and named, and the index
//! says plainly which fields a desktop entry does not carry (no package id,
//! no version, no developer, no licence) instead of inventing them.
//!
//! What is not served, and why: `launch` and `launchUrl` (host-mutating —
//! law #8), the recents and the change feed (the host's own surfaces, which
//! this backend does not read), and the verbs the desktop-entry corpus
//! simply does not describe (`listCapabilities`, `getDependents`).

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Appindex;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    ReasonClass, APPINDEX_AVAILABILITY, APPINDEX_SCHEMA_ID,
};
pub use backend::AppindexBackend;
pub use types::{AppEntry, AppSearchFilter, AppStats, Page, Pagination, SubscriptionId};
