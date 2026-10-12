//! The service harness: turn a domain backend into a capnp service served
//! over any [`Channel`] (`knowledge/05-AI-FIRST-SURFACE.md` §3).
//!
//! The harness is compiled with `domain-network`: it serves the network
//! contract, and the transport-and-envelope half is written so a later
//! domain copies the same shape with its own glue module (charter law #2 —
//! you pay only for the features you enable).
#![cfg(feature = "domain-network")]
//!
//! ## What this layer is
//!
//! ```text
//!   NetworkBackend ──▶ serve() ──▶ Channel ──▶ the four transports
//!        (the contract        (this file:   (src/transport/** — the
//!         as Rust)              the envelope,   channel moves capnp
//!                               the class gate,  messages; the payload
//!                               the loop)        is untouched contract bytes)
//! ```
//!
//! The harness is three small pieces, and the split is deliberate:
//!
//! * [`frame`] — the wire format. Fixed-offset little-endian, zero
//!   dependencies, and **domain-blind**: the only domain-specific number in a
//!   frame is the schema id, which is the contract's own compiled id.
//! * [`network`] — the dispatch glue. One monomorphized match over the
//!   contract's own ordinals, each arm decoding the contract's params struct,
//!   calling the backend, and encoding the contract's result struct. Network
//!   is the first domain served and the template every later domain copies.
//! * this file — the connection loop, the ABI handshake, and the **uid-peer
//!   capability policy** (05 §2): the class a connection is served at.
//!
//! ## The capability policy (charter law #5, 05 §2)
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
//! The `CAP_NET_ADMIN` read is a `/proc` read (law #8: reading is free) and
//! needs `std`; a `no_std` build answers with the uid rule alone and says so
//! — an honest half-policy, never a fabricated grant.

use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use capnp::message::ReaderSegments;

use crate::core::Error;
use crate::schema::abi_capnp;
use crate::transport::channel::{Channel, MessageOut, OwnedMessage, RecvMessage, TransportError};
use crate::transport::channel::PeerCredentials;

// The crate root is `no_std` (law #1), so the `std` halves of this module —
// the `/proc` capability read and the non-blocking yield — make the link
// here, exactly as `src/transport/sys.rs` does. With `std` off they are not
// compiled and the module answers with the `alloc`-only rules it declares.
#[cfg(feature = "std")]
extern crate std;

pub mod frame;
pub mod network;

// The over-the-wire tests live in the integration target
// (`tests/serve_wire.rs`), where they drive this harness through its public
// API exactly as a consumer — and Wave 3's CLI — will.

/// What a domain must provide to be served (05 §3). Network implements this
/// once, in [`network`]; the trait names no domain so a later domain (files,
/// audio...) copies the same shape.
pub trait ServiceDispatch {
    /// The schema identity this service serves (for the registry).
    fn schema_id() -> u64;
    /// Every method's availability row — the wire answers for unavailable.
    fn availability() -> &'static [crate::domains::network::availability::MethodAvailability];
}

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
/// (05 §2, the daemon's whole policy).
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

/// Why a served connection ended in failure. An orderly close (the peer
/// sending its close frame, or the transport answering `Closed`) is **not**
/// an error: [`serve`] returns `Ok`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeError {
    /// The ABI handshake did not complete (the first message was not a
    /// handshake this service can answer).
    Handshake(TransportError),
    /// The transport failed mid-session.
    Transport(TransportError),
}

impl core::fmt::Display for ServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ServeError::Handshake(e) => write!(f, "abi handshake failed: {e}"),
            ServeError::Transport(e) => write!(f, "transport failure: {e}"),
        }
    }
}

impl core::error::Error for ServeError {}

/// The ABI this service speaks (the handshake's `matched` arm). The namespace
/// is the contract family; the version is this harness's wire generation;
/// the one schema version is the served domain's own compiled id.
const ABI_NAMESPACE: &str = "smart-protocol";
const ABI_MAJOR: u32 = 0;
const ABI_MINOR: u32 = 1;

/// Serve `backend` over `channel` at `class`: run the ABI handshake, then
/// the recv/dispatch/send loop until the peer closes.
///
/// The channel is **borrowed**: the transport seam hands out
/// `Box<dyn Channel>`, which cannot be moved into a by-value generic
/// parameter, and the loop never needed ownership — `recv` and `send` both
/// borrow. The caller owns the channel's lifetime and closes it when the
/// session ends (the daemon does exactly that, per connection).
///
/// Static dispatch all the way down — one monomorphized match per method,
/// no `dyn` on the hot path, and the only per-call allocation is the
/// response frame itself (05 §3).
pub fn serve<B, C>(backend: &B, channel: &C, class: CapabilityClass) -> Result<(), ServeError>
where
    B: crate::domains::network::backend::NetworkBackend,
    C: Channel + ?Sized,
{
    let mut scratch = Vec::new();
    let mut subscriptions = network::Subscriptions::new();

    // ── 1. the ABI handshake ────────────────────────────────────────────
    // The client's handshake request arrives as the channel's first
    // message; the answer rides back as a normal message (the transports
    // accept either frame kind on their negotiate read).
    let batch = recv_message(channel, &mut scratch).map_err(ServeError::Handshake)?;
    let (answer, compatible) = handshake_response(&batch)
        .map_err(|e| ServeError::Handshake(TransportError::message(e.to_string())))?;
    channel
        .send(MessageOut::Owned(OwnedMessage::new(answer)))
        .map_err(ServeError::Handshake)?;
    if !compatible {
        // A caller that cannot speak this contract's family cannot be
        // served: the incompatible answer was sent, and the session ends
        // here. The caller closes the channel — the harness borrows it, so
        // the close (and its typed reason) is the caller's to send.
        return Ok(());
    }

    // ── 2. the dispatch loop ────────────────────────────────────────────
    loop {
        match channel.recv(&mut scratch) {
            Ok(batch) => {
                let response = network::respond::<B>(
                    backend,
                    &mut subscriptions,
                    class,
                    envelope_bytes(&batch.message),
                );
                channel
                    .send(MessageOut::Borrowed(&[&response.encode()]))
                    .map_err(ServeError::Transport)?;
            }
            // "nothing yet" is a real answer on a non-blocking channel; the
            // caller drives the next pull.
            Err(TransportError::Empty) => wait_for_work(),
            // The peer closed: the session is over, and that is not a fault.
            Err(TransportError::Closed { .. }) => return Ok(()),
            Err(e) => return Err(ServeError::Transport(e)),
        }
    }
}

/// Build the handshake answer to the client's request, and say whether the
/// session may continue.
///
/// A caller whose ABI namespace is not this contract's gets the
/// `incompatible` arm with a `T.Error` naming the mismatch — the honest
/// answer, never a silent accept. A matching caller gets the `matched` arm
/// carrying this service's ABI: the contract namespace, the harness's wire
/// generation, and the served domain's schema id (so the client can see
/// exactly which contract it reached).
fn handshake_response(
    batch: &crate::transport::channel::RecvBatch,
) -> Result<
    (
        capnp::message::Builder<capnp::message::HeapAllocator>,
        bool,
    ),
    Error,
> {
    // The request is read through the received-message API: the transport
    // hands over its own segments, and the handshake is the contract's own
    // `AbiHandshakeRequest` struct — never a serve-frame byte re-parse.
    let request = batch
        .message
        .root::<abi_capnp::abi_handshake_request::Owned>()
        .map_err(|e| {
            Error::with_message(
                crate::core::ErrorCode::from(e.error_code()),
                e.detail(),
            )
        })?;
    let caller = request.get_caller_abi()?;
    let namespace = caller.get_namespace()?;

    let mut response = capnp::message::Builder::new_default();
    if namespace != ABI_NAMESPACE {
        let mut builder = response.init_root::<abi_capnp::abi_handshake_response::Builder<'_>>();
        let mut error = builder.reborrow().init_incompatible();
        error.set_code(crate::schema::types_capnp::ErrorCode::ProtocolViolation);
        error.set_message(format!(
            "this service speaks the {ABI_NAMESPACE} contract family; the caller declared \
             {namespace:?}"
        ));
        return Ok((response, false));
    }
    {
        let mut builder = response.init_root::<abi_capnp::abi_handshake_response::Builder<'_>>();
        let mut matched = builder.reborrow().init_matched();
        matched.set_namespace(ABI_NAMESPACE);
        let mut version = matched.reborrow().init_version();
        version.set_major(ABI_MAJOR);
        version.set_minor(ABI_MINOR);
        version.set_patch(0);
        let mut schemas = matched.reborrow().init_schema_versions(1);
        let mut schema = schemas.reborrow().get(0);
        schema.set_id(network::NetworkService::schema_id());
        schema.set_major(ABI_MAJOR);
        schema.set_minor(ABI_MINOR);
        schema.set_name("smart-protocol/network");
    }
    Ok((response, true))
}

/// The first segment of a received message: the serve frame's bytes (the
/// frame is padded to a word boundary, and the frame's own header lengths
/// say how much of it is real).
fn envelope_bytes(message: &RecvMessage) -> &[u8] {
    match message {
        RecvMessage::Direct(reader) => reader.get_segments().get_segment(0).unwrap_or(&[]),
        RecvMessage::Owned(reader) => reader.get_segments().get_segment(0).unwrap_or(&[]),
    }
}

/// Receive one message, waiting on the transports whose `recv` is
/// non-blocking (InProcess answers `Empty` when nothing is queued yet).
fn recv_message<C: Channel + ?Sized>(
    channel: &C,
    scratch: &mut Vec<u8>,
) -> Result<crate::transport::channel::RecvBatch, TransportError> {
    loop {
        match channel.recv(scratch) {
            Err(TransportError::Empty) => wait_for_work(),
            other => return other,
        }
    }
}

/// Yield the thread while a non-blocking channel has nothing yet. With
/// `std` this is an honest yield; without it, the CPU hint is the whole
/// story (a no_std service has no scheduler to ask).
fn wait_for_work() {
    #[cfg(feature = "std")]
    std::thread::yield_now();
    #[cfg(not(feature = "std"))]
    core::hint::spin_loop();
}

/// The close reason a served connection answers with when the daemon shuts
/// it down (an orderly local shutdown — never a fabricated peer fault).
pub fn orderly_close() -> crate::transport::channel::CloseReason {
    crate::transport::channel::CloseReason::Shutdown
}
