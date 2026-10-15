//! The **logging** domain: application-level structured log ingestion and
//! query — over one object-safe [`LoggingBackend`] trait with the frozen
//! `logging.capnp` surface (`log @0` through `listDomains @8`, plus the
//! subscription helpers).
//!
//! What this domain is: the canonical log contract's honest backend. The
//! archive body's `journalctl -o json` / `journalctl -g` subprocess shellouts
//! (fork per query, locale text parsing, lost error codes — pitfall P5) are the
//! condemned past; on this host there is no journal at all.
//!
//! ## What THIS host actually offers, measured
//!
//! | fact | measurement |
//! |---|---|
//! | init system | **runit** — `/run/runit`, `/etc/sv`, 16 services linked into `/var/service` |
//! | journald | **absent** — no `/run/systemd/journal`, no `systemd-journald` process |
//! | syslog socket | **absent** — no `/dev/log` |
//! | per-service svlogd output | **root-only** — `/etc/sv/<n>/log/supervise` → `/run/runit/supervise.<n>-log`, mode `drwx------ root root`, measured `EACCES` for uid 1000 |
//! | world-readable host logs | `Xorg.0.log`, `Xorg.1.log` and their `.old` siblings under `/var/log` (mode 0644) |
//! | root-only host logs | `dmesg.log`, `lastlog`, `btmp` (mode 0600), plus `gdm/` and `lightdm/` directories |
//!
//! So the honest backend is: a complete in-process structured store (the
//! contract's full surface — levels, sampling, filter, pagination, subscription,
//! export) writing to the one sink this process owns (its own stderr, which
//! under a runit service IS the svlogd input), plus a native read of the host's
//! own files that names the ones it cannot read instead of skipping them.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Logging;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, LOGGING_AVAILABILITY,
    LOGGING_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::LoggingBackend;
pub use types::{
    Level, LogDomain, LogEntry, LogFilter, LogOutcome, Page, Pagination, SubscriptionId,
};
