//! The **database** domain: key-value storage with namespacing, batching, and
//! change feeds — over one object-safe [`DatabaseBackend`] trait mirroring the
//! frozen `database.capnp` surface.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Database;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, DATABASE_AVAILABILITY,
    DATABASE_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::DatabaseBackend;
pub use types::{Pagination, Page, SubscriptionId, TransactionId};
