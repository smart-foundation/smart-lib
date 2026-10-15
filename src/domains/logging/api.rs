//! The public logging API: a thin, borrowing wrapper over [`LoggingBackend`]
//! exposing the calls a client actually touches. Borrowed and zero-cost — it
//! adds no state over `&B`.

use alloc::vec::Vec;

use crate::core::Result;

use super::backend::LoggingBackend;
use super::types::{Level, LogDomain, LogEntry, LogFilter, LogOutcome, Page, Pagination, SubscriptionId};

/// A borrowing handle to any [`LoggingBackend`].
pub struct Logging<'a, B: LoggingBackend + ?Sized>(&'a B);

impl<'a, B: LoggingBackend + ?Sized> Logging<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Logging(backend)
    }

    /// Write one entry and get the contract's own outcome back (never a silent
    /// success).
    #[must_use = "the outcome is the answer; ignoring it is never useful"]
    pub fn log(&self, level: Level, domain: &str, message: &str) -> Result<LogOutcome> {
        self.0.log(level, domain, message)
    }

    /// A bounded query over retained entries.
    pub fn query(&self, filter: &LogFilter, page: Pagination) -> Result<Page<LogEntry>> {
        self.0.get_entries(filter, page)
    }

    /// Subscribe to the stream and pull the first batch under the caller's bound.
    pub fn stream(&self, filter: &LogFilter, max_entries: u32) -> Result<(SubscriptionId, Vec<LogEntry>)> {
        let subscription = self.0.subscribe(filter)?;
        let first = self.0.pull(subscription, max_entries)?;
        Ok((subscription, first))
    }

    /// Retire a subscription obtained from [`Logging::stream`].
    pub fn close_stream(&self, subscription: SubscriptionId) -> Result<()> {
        self.0.cancel(subscription)
    }

    /// The first page of the domain list, with each domain's level and sampling
    /// rate (so a consumer can see what it is missing).
    #[must_use]
    pub fn domains(&self, page: Pagination) -> Result<Page<LogDomain>> {
        self.0.list_domains(page)
    }

    /// The backend's published capability tokens (law #6).
    #[must_use]
    pub fn caps(&self) -> alloc::vec::Vec<&'static str> {
        self.0.logging_caps()
    }
}
