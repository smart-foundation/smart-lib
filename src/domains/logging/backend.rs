//! The `LoggingBackend` trait: one object-safe method per `Logging` contract
//! method (`log @0` through `listDomains @8`), each returning
//! [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is **no permission string on any method**. Holding the `Logging`
//! reference is the authority to write and to read; a minted subscription id is
//! the authority to pull.
//!
//! ## Object safety and the honest default surface
//!
//! Every method borrows `&self` and returns owned values, so the trait is
//! object-safe and dispatchable as `dyn LoggingBackend` at the transport seam.
//! Each method has a default body answering `notSupported` **with a
//! `reasonClass`-prefixed reason** — the law #6 baseline a platform backend
//! overrides exactly where it serves something.

use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{
    Level, LogDomain, LogEntry, LogFilter, LogOutcome, Page, Pagination, SubscriptionId,
};

/// A protocol-compliant logging backend: one method per contract method, with an
/// honest `notSupported`-with-reason default for everything a host cannot serve.
pub trait LoggingBackend {
    /// The hot path (`log @0`), and the first ordinal because it is the hottest.
    /// A backend reports whether the entry was accepted, sampled away, or
    /// dropped — never a silent success.
    fn log(&self, _level: Level, _domain: &str, _message: &str) -> Result<LogOutcome> {
        Err(Error::not_supported(
            "no-backend: log ingestion is not served on this backend",
        ))
    }

    /// A bounded query over retained entries (`getEntries @1`).
    fn get_entries(&self, _filter: &LogFilter, _page: Pagination) -> Result<Page<LogEntry>> {
        Err(Error::not_supported(
            "no-backend: the log query path is not served on this backend",
        ))
    }

    /// A live stream (`subscribe @2`). Holding the returned id IS the authority
    /// to pull.
    fn subscribe(&self, _filter: &LogFilter) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: log subscriptions are not served on this backend",
        ))
    }

    /// Pull a bounded batch from a subscription (`LogSubscription.pull @0`).
    fn pull(&self, _subscription: SubscriptionId, _max_entries: u32) -> Result<Vec<LogEntry>> {
        Err(Error::not_supported(
            "no-backend: log subscriptions are not served on this backend",
        ))
    }

    /// Retire a subscription (`LogSubscription.cancel @1`).
    fn cancel(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: log subscriptions are not served on this backend",
        ))
    }

    /// Per-domain level configuration (`setLevel @3`).
    fn set_level(&self, _domain: &str, _level: Level) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: per-domain level configuration is not served on this backend",
        ))
    }

    /// Read a domain's configured level (`getLevel @4`).
    fn get_level(&self, _domain: &str) -> Result<Level> {
        Err(Error::not_supported(
            "no-backend: per-domain level configuration is not served on this backend",
        ))
    }

    /// Clear one domain's retained entries (`clear @5`).
    fn clear(&self, _domain: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: clearing a log domain is not served on this backend",
        ))
    }

    /// Export retained entries in an OPEN content type (`export @6`), returning
    /// the bytes in the contract's `T.DataHandle` shape.
    fn export(&self, _filter: &LogFilter, _mime_type: &str) -> Result<Vec<u8>> {
        Err(Error::not_supported(
            "no-backend: log export is not served on this backend",
        ))
    }

    /// Declared sampling (`setSamplingRate @7`): a producer that will drop 90% of
    /// debug entries says so, instead of a consumer discovering it statistically.
    fn set_sampling_rate(&self, _domain: &str, _rate: f32) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: declared sampling is not served on this backend",
        ))
    }

    /// List known domains and their configured levels (`listDomains @8`).
    fn list_domains(&self, _page: Pagination) -> Result<Page<LogDomain>> {
        Err(Error::not_supported(
            "no-backend: the domain list is not served on this backend",
        ))
    }

    /// Publish this backend's honest logging capability tokens (law #6).
    fn logging_caps(&self) -> Vec<&'static str> {
        alloc::vec!["logging.write", "logging.read", "logging.subscribe", "logging.export"]
    }
}
