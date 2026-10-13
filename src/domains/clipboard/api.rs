//! The public clipboard API: a thin, borrowing wrapper over [`ClipboardBackend`]
//! exposing the handful of calls a client actually touches. Borrowed and
//! zero-cost — it adds no state over `&B`.

use alloc::vec::Vec;

use crate::core::Result;
use crate::core::ids::ClipboardId;

use super::backend::ClipboardBackend;
use super::types::{
    ClipboardContent, ClipboardItem, ClipboardSizeLimits, ContentOutcome, Page, Pagination,
    PreviewOutcome, SubscriptionId,
};

/// A borrowing handle to any [`ClipboardBackend`]. The caller has already
/// acquired authority to hold it (law #5: the handle is the authority).
pub struct Clipboard<'a, B: ClipboardBackend + ?Sized>(&'a B);

impl<'a, B: ClipboardBackend + ?Sized> Clipboard<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Clipboard(backend)
    }

    /// Read the current clipboard content for a MIME type.
    #[must_use = "the outcome is the answer; ignoring it is never useful"]
    pub fn content(&self, mime_type: &str) -> Result<ContentOutcome> {
        self.0.get_content(mime_type)
    }

    /// Set the clipboard content (host-mutating; gate-routed).
    pub fn set(&self, content: ClipboardContent) -> Result<()> {
        self.0.set_content(content)
    }

    /// Offer several representations of one copy (host-mutating; gate-routed).
    pub fn offer(&self, offers: &[ClipboardContent]) -> Result<()> {
        self.0.set_offer(offers)
    }

    /// Clear the current clipboard content (host-mutating; gate-routed).
    pub fn clear(&self) -> Result<()> {
        self.0.clear_content()
    }

    /// The bounded copy history.
    #[must_use]
    pub fn history(&self, page: Pagination) -> Result<Page<ClipboardItem>> {
        self.0.get_history(page)
    }

    /// A preview of one history item.
    #[must_use]
    pub fn preview(&self, item: ClipboardId) -> Result<PreviewOutcome> {
        self.0.get_preview(item)
    }

    /// This backend's size-limit policy.
    #[must_use]
    pub fn size_limits(&self) -> Result<ClipboardSizeLimits> {
        self.0.get_size_limits()
    }

    /// Subscribe to change events and pull the first bounded batch.
    #[must_use]
    pub fn changes(&self, max_events: u32) -> Result<(SubscriptionId, Vec<super::types::ClipboardChange>)> {
        let sub = self.0.subscribe_changes()?;
        let first = self.0.pull_changes(sub, max_events)?;
        Ok((sub, first))
    }

    /// Retire a subscription obtained from [`Clipboard::changes`].
    pub fn close_changes(&self, sub: SubscriptionId) -> Result<()> {
        self.0.cancel_changes(sub)
    }
}
