//! The domain command modules. `network` is the domain that landed first on the
//! new engine; `clipboard` is the second (the native selection read/write path);
//! every other domain is the migration queue's data, answered honestly — never
//! faked.

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
