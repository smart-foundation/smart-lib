//! The **clipboard** domain: the one system clipboard's content, a bounded
//! engine-side history, the multi-representation *offer* set, previews,
//! validation, conversion, size limits, and change notification — over one
//! object-safe [`ClipboardBackend`] trait that mirrors the frozen
//! `clipboard.capnp` surface (`getContent @0` through `setOffer @16`).
//!
//! What this domain is: the contract's clipboard surface as an honest backend.
//! Reading and writing the *current* selection is spoken natively — on a
//! Wayland seat the `zwlr_data_control` protocol through a real Wayland client
//! connection, on X11 the ICCCM selection protocol through a real X client
//! (`x11rb`) — never the archive body's condemned `wl-paste`/`xclip`
//! subprocess shellout (fork per op, locale text parsing, lost error codes).
//! What the *host* cannot serve is answered honestly with a reason: neither
//! X11's CLIPBOARD selection manager nor the wlr-data-control device is a
//! multi-slot history keeper, so `getHistory`/`clearHistory`/`searchHistory`/
//! `pin`/`unpin`/`delete` are served by a bounded engine-side ledger of every
//! copy this backend saw (its own writes and, when the daemon runs the watcher,
//! another client's) — published as `DEGRADED-with-reason` in the availability
//! matrix, never as a fabricated host history. That ledger can be made durable
//! by the composition root through [`archive::HistoryArchive`]; the domain then
//! keeps the memory and the archive in step without knowing what either is.
//!
//! ## The transport is native (charter law #9)
//!
//! The archive body ran `wl-copy`/`wl-paste`/`xclip` as a subprocess per call:
//! every read was a fork, every parse was locale-dependent, every error code
//! was lost. The current backend speaks the selection protocol itself over a
//! maintained crate behind the mockable [`crate::platforms::linux`] seam
//! (`SelectionHost`), so a read costs no fork and an error keeps its exact
//! cause. See [`crate::platforms::linux::clipboard`] for the native decision's
//! evidence and the shellout-free proof.

pub mod api;
pub mod archive;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Clipboard;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    CLIPBOARD_AVAILABILITY, CLIPBOARD_SCHEMA_ID, ReasonClass,
};
pub use archive::{ArchivedItem, HistoryArchive, NoArchive};
pub use backend::ClipboardBackend;
pub use types::{
    ClipboardChange, ClipboardContent, ClipboardEventType, ClipboardItem, ClipboardPreview,
    ClipboardSizeLimits, ContentOutcome, ConversionOutcome, Page, Pagination, PreviewOutcome,
    SelectionCapabilities, SubscriptionId, ValidationOutcome,
};
