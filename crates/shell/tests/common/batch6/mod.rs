//! The Batch 6 fixtures: one scripted backend per domain, over the engine's
//! OWNED types, for the six domains whose engine body landed with the
//! engine's Batch 4 — the namespaced key/value store (`database`), the FIFO
//! store (`queue`), the host's own desktop-entry corpus (`appindex`), the
//! manifest store (`manifest`), the RFC 6350 parser/formatter and contact
//! store (`vcard`), and the content store (`content`).

#![allow(dead_code)]

#[cfg(feature = "domain-database")]
pub mod database;
#[cfg(feature = "domain-queue")]
pub mod queue;
#[cfg(feature = "domain-appindex")]
pub mod appindex;
#[cfg(feature = "domain-manifest")]
pub mod manifest;
#[cfg(feature = "domain-vcard")]
pub mod vcard;
#[cfg(feature = "domain-content")]
pub mod content;
