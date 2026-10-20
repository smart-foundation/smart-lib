//! The **manifest** domain: the engine's own manifest store — over one
//! object-safe [`ManifestBackend`] trait.
//!
//! Native mechanism: the engine's own in-process store of manifests. The
//! contract's manifests describe the engine's own bundles, and this host
//! publishes no bundle corpus, so the store *is* the mechanism: real
//! manifests with real names, versions, entry points and dependencies, and a
//! real validation — a required dependency that no manifest in the store
//! offers is an error the caller is told about, never a green light.
//!
//! What is not served, and why: the verbs that need machinery this engine
//! does not have — composition, diffing, signing, verification, migration,
//! template creation and the change feed. Each refuses with the reason it
//! cannot be served; none of them pretends.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Manifest;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    ReasonClass, MANIFEST_AVAILABILITY, MANIFEST_SCHEMA_ID,
};
pub use backend::ManifestBackend;
pub use types::{
    ManifestDependency, ManifestEntry, Page, Pagination, SubscriptionId, ValidationReport,
};
