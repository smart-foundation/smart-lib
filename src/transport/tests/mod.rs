//! Test suite for the transport layer.
//!
//! Every assertion here is mechanical evidence for the brief's checklist:
//! per-transport round trips, capability-transfer semantics, copy counts,
//! close-reason propagation, the four-transport interchangeability witness,
//! and the Tcp honest-degradation test.
//!
//! The crate is `no_std` (charter law #1); this module is the test seam, so
//! it links `std` for itself rather than asking the crate root to (the root
//! is the skeleton agent's file).

extern crate std;

pub mod bootstrap;
pub mod error_mapping;
pub mod framing;
pub mod inprocess;
pub mod interchange;
pub mod shmem;

#[cfg(all(unix, feature = "std"))]
pub mod unix;

#[cfg(feature = "std")]
pub mod tcp;

/// Shared helpers: real contract messages, capability payloads, and a peer
/// the InProcess seam dispatches into.
pub mod support;
