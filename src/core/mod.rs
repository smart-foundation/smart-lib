//! Core: the foundation every domain, transport and platform layer stands on.
//!
//! Four files, four jobs:
//!
//! * [`error`] — the one error model, total both directions with the
//!   contract's `T.Error`.
//! * [`ids`] — the contract's opaque identifiers as newtypes (157 of them,
//!   enumerated from `types.capnp`).
//! * [`capability`] — `Cap<T>`, the authority handle (law #5), and the
//!   acquisition-time host mapping hook.
//! * [`values`] — the shared value types domains actually reference.
//!
//! Everything here is `no_std` + `alloc`, `#[must_use]` where it matters, and
//! allocation-free on the hot paths (the ID types and capability handles are
//! single machine words; only `Error`'s reason strings and [`values::Region`]'s
//! rectangle list allocate, both cold-path by construction).
//!
//! ## The error model in five lines — read this first, domain authors
//!
//! 1. Every fallible call returns `core::Result<T>` (i.e. `Result<T, Error>`);
//!    `Error` is one struct with a `code` from the contract's exact 20-code
//!    vocabulary plus the contract's own `message` / `context` / `retryable`.
//! 2. There is no catch-all variant and no `unmapped`: every `T.ErrorCode` maps
//!    1:1 to an [`error::ErrorCode`], and a discriminant from a *newer* peer
//!    becomes `unknown` with the raw value preserved in `message` — never a
//!    guessed code.
//! 3. Honest degradation (law #6) is `Error::unavailable(reason)` or
//!    `Error::not_supported(reason)` — a first-class result with a required
//!    reason, never a panic and never a fabricated zero.
//! 4. `retryable` is honest: only `temporary` defaults to retryable; anything
//!    else is the caller being told the truth.
//! 5. The mapping is exhaustively tested in both directions for every variant
//!    (`tests::error_*`), so L1 conformance holds by construction, not by
//!    review.

pub mod capability;
pub mod error;
pub mod ids;
pub mod values;

#[cfg(test)]
mod tests;

pub use error::{Error, ErrorCode, Result};
