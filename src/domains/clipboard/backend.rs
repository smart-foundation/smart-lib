//! The `ClipboardBackend` trait: one object-safe method per `Clipboard`
//! contract method (`getContent @0` through `setOffer @16`), plus the
//! subscription's `pull`/`cancel`, each returning [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is **no permission string on any method**. Authority is the held
//! capability reference; a caller holds a `Clipboard` capability or it does not
//! and every call is refused before a backend is reached. The host-write verbs
//! (`setContent`, `setOffer`, `clearContent`, the history mutators, the size
//! limits) change the host clipboard and/or this process's own ledger; at the
//! shell they are routed through the host-write gate, and their live bring-back
//! proof is the founder's (charter law #8). The engine method is honest about
//! being a write by its very existence, not by a permission argument.
//!
//! ## Object safety
//!
//! Every method borrows `&self` and returns owned values, so the trait is
//! object-safe and dispatchable as `dyn ClipboardBackend` at the transport
//! seam. The mutable half (history ledger, subscriptions, size limits) uses
//! interior mutability inside the concrete backend; the trait shape stays
//! `&self`.
//!
//! ## What is real vs honestly degraded (law #6)
//!
//! The live selection read/write is REAL where the host has a selection
//! (native X11 or Wayland). Everything that assumes a *host* history keeper
//! (which neither X11's CLIPBOARD manager nor wlr-data-control provides) is
//! served from a bounded engine-side ledger and published as DEGRADED, or
//! answered `notSupported` where the concept does not exist — see
//! [`super::availability`] for the matrix and [`super::backend`]'s per-method
//! reasons, and `crate::platforms::linux::clipboard` for the native decision.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::ids::ClipboardId;
use crate::core::Result;

use super::types::{
    ClipboardChange, ClipboardContent, ClipboardItem, ClipboardSizeLimits, ConversionOutcome,
    ContentOutcome, Page, Pagination, PreviewOutcome, SubscriptionId, ValidationOutcome,
};

/// A protocol-compliant clipboard backend: one method per contract method, with
/// honest `Unavailable`-with-reason for everything the platform cannot serve.
pub trait ClipboardBackend {
    /// Publish this backend's honest clipboard capability tokens (law #6): the
    /// selection transport spoken and whether read/write are available. A
    /// consumer can tell a native-from-a-real-display answer from a decided
    /// `Unavailable` without a permission string.
    fn clipboard_caps(&self) -> (String, bool, bool) {
        ("none".into(), false, false)
    }

    // ── The live selection, spoken natively (overridden per platform) ────────

    /// Read the current clipboard content for a MIME type (`getContent @0`).
    /// `empty` is a first-class answer when the clipboard holds nothing; the
    /// default (no platform backend) is an honest `Unavailable`-with-reason.
    fn get_content(&self, _mime_type: &str) -> Result<ContentOutcome> {
        Err(crate::core::Error::unavailable(
            "no-backend: no native selection transport (a Wayland data-control client or an X11 \
             selection client) is available on this host; the archive body's wl-paste/xclip \
             shellout is the mechanism this domain replaces",
        ))
    }

    /// Set clipboard content (`setContent @1`). Host-mutating (it becomes the
    /// selection owner); the live bring-back proof is founder-run.
    fn set_content(&self, _content: ClipboardContent) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no native selection transport on this host; setContent is unreachable \
             without one and the wl-paste/xclip shellout is not supported",
        ))
    }

    /// Clear the current clipboard content (`clearContent @2`). Host-mutating.
    fn clear_content(&self) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no native selection transport on this host; the selection cannot be \
             cleared without one",
        ))
    }

    /// List the current selection's MIME targets (`getSupportedMimeTypes @6`).
    /// Not host-mutating; overridden by backends that can introspect the owner.
    fn get_supported_mime_types(&self) -> Result<Vec<String>> {
        Err(crate::core::Error::unavailable(
            "no-backend: no native selection transport on this host to introspect the current \
             selection's targets",
        ))
    }

    // ── The change subscription (pull model) ─────────────────────────────────

    /// Mint a change-subscription handle (`subscribeChanges @5`).
    fn subscribe_changes(&self) -> Result<SubscriptionId> {
        Err(crate::core::Error::not_supported(
            "no-backend: change notification needs a seat connection (a Wayland data-control \
             device or an X selection); neither is available on this host",
        ))
    }

    /// Pull the next bounded batch of changes for a subscription
    /// (`ClipboardChangeSubscription.pull @0`), collapsed to `max_events`.
    fn pull_changes(&self, _subscription: SubscriptionId, _max_events: u32) -> Result<Vec<ClipboardChange>> {
        Err(crate::core::Error::not_supported(
            "no-backend: change notification needs a seat connection on this host",
        ))
    }

    /// Retire a subscription (`ClipboardChangeSubscription.cancel @1`).
    fn cancel_changes(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: change notification needs a seat connection on this host",
        ))
    }

    // ── History — a bounded engine-side ledger, not a host history ────────────
    //
    // The default is the honest `notSupported`: neither a clipboard manager nor
    // a data-control device keeps a history. A backend that keeps a ledger of
    // its own copies (the Linux backend does) overrides these; the availability
    // matrix publishes them as DEGRADED so a caller knows the ledger is this
    // process's, not the seat's.

    /// Retrieve this backend's copy history with pagination (`getHistory @3`).
    fn get_history(&self, _page: Pagination) -> Result<Page<ClipboardItem>> {
        Err(crate::core::Error::not_supported(
            "no-backend: neither the clipboard selection manager nor a data-control device keeps \
             a history across clients; this backend runs no copy ledger on this host",
        ))
    }

    /// Clear this backend's copy history (`clearHistory @4`).
    fn clear_history(&self) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no copy history is kept on this host, so there is none to clear",
        ))
    }

    /// Search this backend's copy history (`searchHistory @7`).
    fn search_history(&self, _query: &str, _page: Pagination) -> Result<Page<ClipboardItem>> {
        Err(crate::core::Error::not_supported(
            "no-backend: no copy history is kept on this host to search",
        ))
    }

    /// Pin an item against auto-expiry (`pinItem @8`).
    fn pin_item(&self, _item_id: ClipboardId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: pinning is a copy-ledger operation; no ledger is kept on this host",
        ))
    }

    /// Unpin a pinned item (`unpinItem @9`).
    fn unpin_item(&self, _item_id: ClipboardId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: pinning is a copy-ledger operation; no ledger is kept on this host",
        ))
    }

    /// Remove one item from the copy ledger (`deleteItem @10`).
    fn delete_item(&self, _item_id: ClipboardId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: deleting is a copy-ledger operation; no ledger is kept on this host",
        ))
    }

    // ── Preview, validation, conversion (payload-shaped) ─────────────────────

    /// A low-resolution preview (`getPreview @11`), truncated for text content.
    fn get_preview(&self, _item_id: ClipboardId) -> Result<PreviewOutcome> {
        Err(crate::core::Error::not_supported(
            "no-backend: a preview is derived from a stored item; no copy ledger is kept on this \
             host, and the live selection has no stable item id to preview",
        ))
    }

    /// Validate that content matches its claimed MIME type
    /// (`validateContent @12`) — a security-relevant finding, so a code, not a
    /// bare boolean.
    fn validate_content(&self, _item_id: ClipboardId) -> Result<ValidationOutcome> {
        Err(crate::core::Error::not_supported(
            "no-backend: validation is a copy-ledger operation; no ledger is kept on this host",
        ))
    }

    /// Convert content to a target MIME type (`convertContent @13`).
    fn convert_content(&self, _item_id: ClipboardId, _target_mime_type: &str) -> Result<ConversionOutcome> {
        Err(crate::core::Error::not_supported(
            "no-backend: conversion is a copy-ledger operation; no ledger is kept on this host",
        ))
    }

    // ── Size limits (a policy the backend declares, not a host fact) ─────────

    /// Get the configured size limits (`getSizeLimits @14`).
    fn get_size_limits(&self) -> Result<ClipboardSizeLimits> {
        Err(crate::core::Error::not_supported(
            "no-backend: size limits are this backend's policy; no policy is declared on this host",
        ))
    }

    /// Set the size limits (`setSizeLimits @15`). Host-mutating (policy).
    fn set_size_limits(&self, _limits: ClipboardSizeLimits) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setting size limits changes this backend's policy, which is not declared \
             on this host",
        ))
    }

    /// Offer several representations of one copy in a single call
    /// (`setOffer @16`). Host-mutating.
    fn set_offer(&self, _offers: &[ClipboardContent]) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no native selection transport on this host to offer several \
             representations to; the wl-paste/xclip shellout is not supported",
        ))
    }

    /// A nameable reason a method is unavailable — used by tooling that mirrors
    /// the availability matrix. Not a permission string; a diagnostic label.
    fn last_unavailable_reason(&self, _method: &str) -> Option<String> {
        None
    }
}

/// A **shared** clipboard backend: one instance behind a `Mutex`, reachable from
/// every daemon connection and from the selection watcher.
///
/// # Why this exists, in one paragraph
///
/// `capnp-rpc` gives every connection its own `LocalPool`, so a daemon that
/// wants two clients at once serves each connection on its own thread. The
/// domain's backend is one object — the host's clipboard and its ledger — so it
/// has to cross that boundary, and crossing a thread boundary in Rust means
/// `Send + Sync`. The platform backends are `Send` but not `Sync` (an
/// `X11SelectionHost` owns a connection that must not be driven from two threads
/// at once), and a `Mutex` is exactly the honest answer: the selection protocol
/// is serialized, because it *is* serialized.
///
/// The impl is a thin delegation of every method, so a caller cannot tell a
/// shared backend from a plain one, and no behaviour lives here.
#[cfg(feature = "std")]
impl<T> ClipboardBackend for std::sync::Mutex<T>
where
    T: ClipboardBackend + ?Sized,
{
    fn clipboard_caps(&self) -> (String, bool, bool) {
        self.lock().expect("clipboard backend lock").clipboard_caps()
    }

    fn get_content(&self, mime_type: &str) -> Result<ContentOutcome> {
        self.lock().expect("clipboard backend lock").get_content(mime_type)
    }

    fn set_content(&self, content: ClipboardContent) -> Result<()> {
        self.lock().expect("clipboard backend lock").set_content(content)
    }

    fn set_offer(&self, offers: &[ClipboardContent]) -> Result<()> {
        self.lock().expect("clipboard backend lock").set_offer(offers)
    }

    fn clear_content(&self) -> Result<()> {
        self.lock().expect("clipboard backend lock").clear_content()
    }

    fn get_supported_mime_types(&self) -> Result<Vec<String>> {
        self.lock().expect("clipboard backend lock").get_supported_mime_types()
    }

    fn subscribe_changes(&self) -> Result<SubscriptionId> {
        self.lock().expect("clipboard backend lock").subscribe_changes()
    }

    fn pull_changes(&self, subscription: SubscriptionId, max_events: u32) -> Result<Vec<ClipboardChange>> {
        self.lock()
            .expect("clipboard backend lock")
            .pull_changes(subscription, max_events)
    }

    fn cancel_changes(&self, subscription: SubscriptionId) -> Result<()> {
        self.lock().expect("clipboard backend lock").cancel_changes(subscription)
    }

    fn get_history(&self, page: Pagination) -> Result<Page<ClipboardItem>> {
        self.lock().expect("clipboard backend lock").get_history(page)
    }

    fn clear_history(&self) -> Result<()> {
        self.lock().expect("clipboard backend lock").clear_history()
    }

    fn search_history(&self, query: &str, page: Pagination) -> Result<Page<ClipboardItem>> {
        self.lock().expect("clipboard backend lock").search_history(query, page)
    }

    fn pin_item(&self, item_id: ClipboardId) -> Result<()> {
        self.lock().expect("clipboard backend lock").pin_item(item_id)
    }

    fn unpin_item(&self, item_id: ClipboardId) -> Result<()> {
        self.lock().expect("clipboard backend lock").unpin_item(item_id)
    }

    fn delete_item(&self, item_id: ClipboardId) -> Result<()> {
        self.lock().expect("clipboard backend lock").delete_item(item_id)
    }

    fn get_preview(&self, item_id: ClipboardId) -> Result<PreviewOutcome> {
        self.lock().expect("clipboard backend lock").get_preview(item_id)
    }

    fn validate_content(&self, item_id: ClipboardId) -> Result<ValidationOutcome> {
        self.lock().expect("clipboard backend lock").validate_content(item_id)
    }

    fn convert_content(&self, item_id: ClipboardId, target_mime_type: &str) -> Result<ConversionOutcome> {
        self.lock()
            .expect("clipboard backend lock")
            .convert_content(item_id, target_mime_type)
    }

    fn get_size_limits(&self) -> Result<ClipboardSizeLimits> {
        self.lock().expect("clipboard backend lock").get_size_limits()
    }

    fn set_size_limits(&self, limits: ClipboardSizeLimits) -> Result<()> {
        self.lock().expect("clipboard backend lock").set_size_limits(limits)
    }

    fn last_unavailable_reason(&self, method: &str) -> Option<String> {
        self.lock()
            .expect("clipboard backend lock")
            .last_unavailable_reason(method)
    }
}
