//! The three faces of the product: the library window, the quick palette, and
//! the inspector both share.
//!
//! Each view is a gpui `Render` root with the smallest possible amount of
//! state: an `Entity<ClipboardApp>` handle, a focus handle, and the
//! subscription that keeps it in sync. Everything they draw comes from the one
//! store, so the two windows can never disagree about what the service said.

pub mod inspector;
pub mod library;
pub mod palette;
