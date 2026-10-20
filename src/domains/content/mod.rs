//! The **content** domain: the engine's own content store — over one
//! object-safe [`ContentBackend`] trait.
//!
//! Native mechanism: the engine's own in-process store. The contract defines
//! content as the engine's own corpus (there is no host content service on
//! this host to read, and the contract names no libvcard-like library), so
//! the store *is* the mechanism: real items with real ids, versions,
//! timestamps, payloads and tags, and the contract's own optimistic
//! concurrency check on every write.
//!
//! What is not served, and why: the parts of the contract's surface the
//! store does not model — the relationship graph, the reaction and
//! provenance ledgers, the keyring, the geo index, the branches, the
//! composites, the lineage, the audit trail, the rate limiter, the previews,
//! the translations, the quality score, the recommendations and the change
//! feed. Each refuses with the reason it cannot be served; none of them
//! pretends.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Content;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    ReasonClass, CONTENT_AVAILABILITY, CONTENT_SCHEMA_ID,
};
pub use backend::ContentBackend;
pub use types::{ContentFilter, ContentItem, Page, Pagination, SubscriptionId};
