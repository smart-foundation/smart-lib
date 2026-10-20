//! The service harness: the contract, served over the contract's **own**
//! call model.
//!
//! # What changed, and why
//!
//! This layer used to own a wire vocabulary of its own: `frame.rs` specified
//! an envelope in prose (a fixed header, a kind byte, a payload), and
//! `network.rs` dispatched on the contract's **ordinals** through a
//! hand-written match. Both are gone.
//!
//! What is here now:
//!
//! * the contract's own **capnp RPC** (`capnp-rpc`) — the RPC protocol
//!   implementation, the two-party connection and the capability export
//!   table;
//! * the contract's own **generated `Server` traits** — one method per
//!   contract method, typed params, typed results, dispatched by the
//!   generated code;
//! * the contract's own **interface ids** — the router routes a call to the
//!   interface the caller named, and the interface id it checks is the one
//!   the generated code assigned.
//!
//! A client that only knows the contract can drive this service. A contract
//! only mock can drive it too. There is no second vocabulary to learn.
//!
//! ## What this layer is
//!
//! ```text
//!   domain backend ──▶ generated Server trait ──▶ capnp RPC ──▶ transport
//!     (the engine's      (the contract's own       (the contract's   (the byte
//!      trait)             method set)                own call model)  stream)
//! ```
//!
//! ## The capability policy (charter law #5)
//!
//! The daemon is not a privilege broker. It reads what the host already
//! knows and maps it into a capability class, once per connection:
//!
//! | peer identity | class | what it may call |
//! |---|---|---|
//! | uid 0, or `CapEff` holds `CAP_NET_ADMIN` (bit 12, read from `/proc/<pid>/status`) | [`CapabilityClass::Control`] | every servable method, mutating verbs included |
//! | any other uid | [`CapabilityClass::ReadOnly`] | the reads; mutating verbs are refused **before the backend is reached** |
//! | no peer identity (Shmem, Tcp, InProcess) | [`CapabilityClass::ReadOnly`] | the reads — honest: no identity means no authority claim |
//!
//! No polkit prompt, no elevation, no suid: a caller that lacks host
//! privileges gets host-truthful denials, which is what the host would
//! answer anyway. The class is the authority gate — a mutating verb arriving
//! on a read-only connection never reaches the backend, so a backend can
//! never be reached through a capability the peer does not hold (law #5).
//!
//! The class is **published**, not hidden: `capability.capnp`'s
//! `CapabilityService` answers "what do you hold?" with the contract's own
//! `Capability` struct.
//!
//! ## Where the served set comes from
//!
//! The service's interfaces are *data*: each served domain contributes its
//! generated interface id, its discovery descriptor and a per-connection
//! server ([`service::ServedDomain`]). The foundation interfaces —
//! discovery, the ABI handshake and the capability publication — are served
//! on the same connection. Adding a domain is one module and one line; no
//! shared file in this layer changes.
#![cfg(feature = "rpc")]
//!
//! The crate root is `no_std` (law #1); this module is the std-gated seam,
//! so it links `std` for itself.
#[cfg(feature = "std")]
extern crate std;

pub mod abi;
pub mod availability;
pub mod capability;
pub mod cli;
pub mod discovery;
pub mod errors;
pub mod router;
pub mod service;
pub mod stream;

#[cfg(feature = "domain-clock")]
pub mod clock;
#[cfg(feature = "domain-files")]
pub mod files;
#[cfg(feature = "domain-logging")]
pub mod logging;
#[cfg(feature = "domain-network")]
pub mod network;
#[cfg(feature = "domain-process")]
pub mod process;
#[cfg(feature = "domain-settings")]
pub mod settings;
#[cfg(feature = "domain-time")]
pub mod time;

pub use router::{InterfaceRouter, Route};
pub use service::{ClientSession, ServeError, ServedDomain, Service};

use alloc::format;

use crate::transport::channel::PeerCredentials;

/// The capability class one connection is served at. This is the authority
/// handle of the wire path: the class is decided once, from the peer's host
/// identity, and every later decision reads it (law #5 — authority is a
/// handle, not a permission string).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapabilityClass {
    /// uid 0 / `CAP_NET_ADMIN`: the mutating verbs are reachable.
    Control,
    /// Any other peer, and every peer with no host identity: reads only.
    ReadOnly,
}

impl CapabilityClass {
    /// The stable name for diagnostics (never used in a required position).
    pub const fn as_str(self) -> &'static str {
        match self {
            CapabilityClass::Control => "Control",
            CapabilityClass::ReadOnly => "ReadOnly",
        }
    }

    /// True when this class may reach a mutating verb.
    pub const fn is_control(self) -> bool {
        matches!(self, CapabilityClass::Control)
    }
}

impl core::fmt::Display for CapabilityClass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Linux capability bit 12 in a process's effective set: `CAP_NET_ADMIN`,
/// the one the network control verbs need. Read from `/proc/<pid>/status`'s
/// `CapEff` line — a read, allowed by law #8.
#[cfg(all(target_os = "linux", feature = "std"))]
const CAP_NET_ADMIN: u32 = 12;

/// Map a peer's host identity into the capability class it is served at
/// (the daemon's whole policy).
///
/// * `None` — the transport carries no peer identity (Shmem, Tcp,
///   InProcess) — is **ReadOnly**: no identity means no authority claim, and
///   a fabricated Control class from an unidentified peer would be the one
///   unforgivable defect (law #6).
/// * uid 0 is Control (the superuser holds every capability).
/// * otherwise the peer's effective capability set is read from
///   `/proc/<pid>/status` and `CAP_NET_ADMIN` (bit 12) is Control.
/// * every read failure is ReadOnly: an unreadable `/proc` entry means the
///   host could not confirm the capability, so the honest answer is the
///   weaker class, never the stronger.
pub fn classify(peer: Option<PeerCredentials>) -> CapabilityClass {
    let Some(cred) = peer else {
        return CapabilityClass::ReadOnly;
    };
    if cred.uid == 0 {
        return CapabilityClass::Control;
    }
    #[cfg(all(target_os = "linux", feature = "std"))]
    if peer_holds_cap_net_admin(cred.pid) {
        return CapabilityClass::Control;
    }
    CapabilityClass::ReadOnly
}

/// Read the peer's effective capability set from `/proc/<pid>/status` and
/// test `CAP_NET_ADMIN`. Every failure path answers `false`: the kernel's
/// own `/proc` read is the only evidence, and absent evidence is not
/// authority.
#[cfg(all(target_os = "linux", feature = "std"))]
fn peer_holds_cap_net_admin(pid: i32) -> bool {
    let path = format!("/proc/{pid}/status");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    cap_mask_from_status(&text).is_some_and(|mask| mask & (1u64 << CAP_NET_ADMIN) != 0)
}

/// The `CapEff` mask out of a `/proc/<pid>/status` text, or `None` when the
/// line is absent or unparseable. Pure, so the capability rule itself is
/// testable without a host to read.
#[cfg(all(target_os = "linux", feature = "std"))]
pub fn cap_mask_from_status(status: &str) -> Option<u64> {
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("CapEff:") else {
            continue;
        };
        return parse_hex(rest.trim());
    }
    None
}

/// Parse a bare hexadecimal mask (the `/proc/<pid>/status` spelling). No
/// `0x` prefix, digits only; anything else is `None` — never a guess.
#[cfg(all(target_os = "linux", feature = "std"))]
fn parse_hex(text: &str) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for byte in text.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(u64::from(digit))?;
    }
    Some(value)
}

/// Read a contract `Text` field losslessly into an owned string (a peer's
/// invalid UTF-8 is a protocol violation; it reads as replacement
/// characters here rather than fabricating a value).
pub fn text_of(reader: capnp::text::Reader<'_>) -> alloc::string::String {
    alloc::string::String::from_utf8_lossy(reader.as_bytes()).into_owned()
}

/// The close reason a served connection answers with when the daemon shuts
/// it down (an orderly local shutdown — never a fabricated peer fault).
pub fn orderly_close() -> crate::transport::channel::CloseReason {
    crate::transport::channel::CloseReason::Shutdown
}

/// The **call model** this service speaks: the contract's own ("capnp").
///
/// `discovery.capnp`'s `ServiceEndpoint.transport` names a transport token,
/// and its own comment reads "capnp", "rest", "grpc" — that vocabulary is the
/// CALL MODEL, while the engine's transport registry is keyed on the CARRIER
/// ("unix", "tcp"). The contract has one field and two vocabularies, so this
/// engine publishes BOTH, in the contract-legal open metadata:
///
/// * the descriptor's `metadata` carries `callModel` — what the service
///   speaks;
/// * the endpoint's `metadata` carries `callModel` **and** `carrier` — what
///   the service speaks, and the carrier it is reachable over.
///
/// A client that reads an endpoint whose call model it does not speak refuses
/// it with a reason (never a silent wrong protocol); a client that reads the
/// carrier knows where to dial. The founder's ruling on the schema (a
/// declared `callModel` field, or a redefinition of `transport`) is reported,
/// not assumed — the schema tree is not edited here.
pub const CALL_MODEL: &str = "capnp";

/// The metadata key the **call model** is published under.
pub const METADATA_CALL_MODEL: &str = "callModel";

/// The metadata key the **carrier** (the transport) is published under.
pub const METADATA_CARRIER: &str = "carrier";

/// The metadata key the **domain's own capability tokens** are published
/// under, in the discovery descriptor's open metadata.
///
/// The contract's `requiredCapabilityIds` is a list of opaque numeric
/// `T.CapabilityHandle`s; a domain's capabilities are an open vocabulary of
/// words, so the words travel here.
pub const METADATA_CAPABILITIES: &str = "capabilities";
