//! Transport layer — charter law #4: **transports are first-class and
//! interchangeable.**
//!
//! One `Channel` abstraction over four transports:
//!
//! | module | transport | what it is |
//! |---|---|---|
//! | [`inprocess`] | InProcess | direct dispatch into a same-process peer; zero serialization |
//! | [`shmem`] | Shmem | zero-copy frames: payload in a shared arena, referenced by `T.BufferId` |
//! | [`unix`] | UnixSocket | capnp over `AF_UNIX`; `SCM_RIGHTS`-class fd passing where the host has it |
//! | [`tcp`] | Tcp | capnp stream framing; honest capability degradation |
//! | [`bootstrap`] | (Shmem's prerequisite) | the one-time authenticated arena exchange |
//!
//! ## The design centre (read this first)
//!
//! The capability-transfer semantics table in [`channel`] is the design
//! centre of this layer: every transport *declares* what it can move as
//! authority, and `attach`/`resolve_buffer` are typed operations whose
//! failure modes are part of the API. Nothing here fakes a transfer; a
//! transport that cannot carry a capability says so with a typed reason
//! (charter law #6).
//!
//! ## std / no_std split (law #1)
//!
//! * **`alloc`-only, always compiled:** [`inprocess`], [`shmem`], [`channel`],
//!   [`bootstrap`] (protocol + mock), [`sync`].
//! * **`std`-gated, host-socket transports:** [`unix`] (also `unix`),
//!   [`tcp`], and the real Unix-side of [`bootstrap`]'s links.
//!
//! With `std` off, `Transport::UnixSocket`/`Transport::Tcp` still exist as
//! values and `connect` answers `TransportError::NotCompiled` — honest
//! unavailability rather than a missing symbol. The brief's
//! `std-transports` feature is a requested manifest line (see the report);
//! until the wire agent adds it, the existing `std` feature is the gate.

pub mod bootstrap;
pub mod channel;
pub mod inprocess;
pub mod shmem;

#[cfg(all(unix, feature = "std"))]
pub mod unix;

#[cfg(feature = "std")]
pub mod tcp;

/// Host primitives (declarations + the smallest amount of honest logic).
/// Linux+std for real implementations; typed `NotCompiled` elsewhere.
pub mod sys;

/// A tiny no_std mutex (see the module for why it exists).
pub mod sync;

#[cfg(test)]
mod tests;
