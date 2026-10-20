//! The domain command modules. `network` is the domain that landed first on the
//! new engine; `clipboard` is the second (the native selection read/write path);
//! every other domain is the migration queue's data, answered honestly — never
//! faked.
//!
//! The Batch 6 six (`database`, `queue`, `appindex`, `manifest`, `vcard`,
//! `content`) speak to the engine's OWNED types: the engine's own
//! namespaced key/value store and FIFO (`database`, `queue`), the host's own
//! freedesktop desktop-entry corpus (`appindex`), and the engine's own
//! manifest store, RFC 6350 parser/formatter and content store (`manifest`,
//! `vcard`, `content`).

pub mod network;

#[cfg(feature = "domain-clipboard")]
pub mod clipboard;

#[cfg(feature = "domain-service")]
pub mod service;

#[cfg(feature = "domain-files")]
pub mod files;

#[cfg(feature = "domain-process")]
pub mod process;

#[cfg(feature = "domain-settings")]
pub mod settings;

#[cfg(feature = "domain-logging")]
pub mod logging;

#[cfg(feature = "domain-clock")]
pub mod clock;

#[cfg(feature = "domain-time")]
pub mod time;

#[cfg(feature = "domain-audio")]
pub mod audio;

#[cfg(feature = "domain-media")]
pub mod media;

#[cfg(feature = "domain-power")]
pub mod power;

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
