//! The `Channel` abstraction — law #4 of the charter, the design centre of
//! this layer.
//!
//! # One API shape over four transports
//!
//! ```text
//!   InProcess   Shmem   UnixSocket   Tcp
//!       \         |         |         /
//!        \        |         |        /
//!         +-------+---------+-------+
//!                   Channel
//! ```
//!
//! A caller selects a [`Transport`], calls [`connect`], and talks to the
//! result through [`Channel`] only. Nothing downstream of this seam may
//! name a transport — that is what makes the four interchangeable (witness:
//! `tests/interchange.rs::four_transports_one_api`, one test body, four
//! factories).
//!
//! # Capability-transfer semantics, per transport (the honest table)
//!
//! This table is the whole point of the layer. It mirrors
//! `knowledge/02-HOST-CAPABILITIES.md` §4.2 and is declared, per transport,
//! by [`TransportCaps`] — published, never guessed (charter law #6):
//!
//! | Transport      | What moves as authority            | Mechanic                                  | Declared gap |
//! |----------------|------------------------------------|-------------------------------------------|--------------|
//! | `InProcess`    | the handle itself (`Box`d object)  | moving the Rust value IS the transfer     | none inside one process |
//! | `Shmem`        | **nothing, by itself**             | one-time authenticated bootstrap (§`bootstrap`) exchanges the arena; afterward handles live in the arena | no authority in the arena; no peer identity — must be paired |
//! | `UnixSocket`   | an OS handle (fd), 253/message     | `SCM_RIGHTS` — the kernel re-opens it      | non-Linux targets: ancillary not compiled here (honest `None`) |
//! | `Tcp`          | **nothing**                        | in-band descriptor bytes only             | no fd transfer exists; peer identity none; `DataHandle.buffer` has no namespace |
//!
//! # std / no_std split
//!
//! `InProcess` and `Shmem` are `alloc`-only and always compiled. `UnixSocket`
//! and `Tcp` need host sockets and are compiled when the `std` feature is on
//! (the skeleton's widening feature; the brief's `std-transports` feature is
//! a requested manifest line — see the report). With `std` off, the two
//! socket transports still exist as [`Transport`] values and answer
//! `connect` with an honest `Unavailable`, they simply are not linked.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use capnp::message::ReaderSegments;
use core::any::Any;

use crate::schema::{abi_capnp, types_capnp};

pub use crate::transport::inprocess::InProcessPeer;
pub use crate::transport::shmem::{Arena, ArenaConfig, ArenaStats, ShmemHandle};
pub use crate::transport::sync::Mutex;

/// Wire version of this transport's framing. Bumping it is a breaking change
/// of the seam; the frame layout is private to `src/transport/**`.
pub const FRAMING_VERSION: u16 = 1;

/// What this transport can honestly do. Published per channel; a fabricated
/// value here is the one unforgivable defect (charter law #6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportCaps {
    /// The open scheme token this transport answers to (never used in a
    /// required position of a domain API — it is diagnostics only).
    ///
    /// The token is an *open* string: the set of schemes is whatever the
    /// registry holds (`crate::transport::registry`), not an enum here.
    pub scheme: &'static str,
    /// The framing actually used, as declared text (never a placeholder).
    pub framing: &'static str,
    /// How a capability reference moves on this transport.
    pub capability_transfer: CapabilityTransfer,
    /// A message moves without a byte copy (true only for `InProcess`).
    pub zero_copy_message: bool,
    /// `T.DataHandle.buffer` is resolvable here (a shared `BufferId`
    /// namespace exists). False ⇒ the producer must not emit that arm.
    pub zero_copy_data_handle: bool,
    /// Peer identity is observable on this transport (SO_PEERCRED-class).
    pub peer_identity: bool,
    /// `recv` blocks until a frame arrives (true) or may answer `Empty`
    /// (false). Both are honest; the caller drives pulls either way.
    pub blocking: bool,
    /// Declared maximum frame size in bytes (0 = no declared bound).
    pub max_frame_bytes: u32,
    /// The honest gap, named. Empty only when there is none.
    pub note: &'static str,
}

/// The capability-transfer semantics a transport declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityTransfer {
    /// The handle itself crosses (InProcess).
    Native,
    /// An OS handle crosses and the kernel re-opens it (UnixSocket+SCM_RIGHTS).
    OsHandle,
    /// Nothing crosses by itself; exactly one authenticated bootstrap
    /// exchange establishes the arena, and handles live in it afterward.
    BootstrapOnly,
    /// Only in-band descriptor bytes cross; authority must be re-verified
    /// per call by the capability service (Tcp).
    InBandToken,
    /// Nothing crosses, and nothing is attempted.
    None,
}

impl CapabilityTransfer {
    /// The stable name of these semantics.
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityTransfer::Native => "native",
            CapabilityTransfer::OsHandle => "os-handle",
            CapabilityTransfer::BootstrapOnly => "bootstrap-only",
            CapabilityTransfer::InBandToken => "in-band-token",
            CapabilityTransfer::None => "none",
        }
    }
}

/// Peer identity where the host can produce one. `None` means this
/// transport has no peer identity — not "the peer is nobody".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCredentials {
    /// Peer process id.
    pub pid: i32,
    /// Peer user id.
    pub uid: u32,
    /// Peer group id.
    pub gid: u32,
}

/// An OS-level handle (a file descriptor on unix targets). Opaque by
/// design: the transport moves it, the host interprets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsHandle(pub u64);

impl OsHandle {
    /// The raw value (an fd on unix targets; opaque elsewhere).
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A capability identifier, unique per channel direction. Allocated by the
/// channel; never derived from a uid or a path (charter law #5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapId(pub u64);

/// What a caller hands to [`Channel::attach`].
pub enum CapHandoff {
    /// A live handle in this process: for `InProcess`, moving the value is
    /// the whole mechanism (law #5).
    Live(
        /// The handle itself; the receiver downcasts it back.
        Box<dyn Any + Send>,
    ),
    /// In-band descriptor bytes — the serialized `capability.capnp`
    /// `Capability` struct. The only form Tcp can carry.
    Descriptor(
        /// The serialized `Capability` struct.
        Vec<u8>,
    ),
    /// An OS handle. `UnixSocket` passes it with SCM_RIGHTS; the kernel
    /// re-opens it on the far side, which is a genuine transfer of authority.
    OsHandle(OsHandle),
    /// A shared-memory arena: the far side may resolve `T.BufferId`s from
    /// it once it holds the region handle. The kernel moves the handle
    /// (`SCM_RIGHTS` on Linux), which is what makes the *namespace*
    /// transfer real — an anonymous memfd cannot be opened by name.
    Region {
        /// The region's OS descriptor (rides SCM_RIGHTS where available).
        fd: OsHandle,
        /// The arena geometry the far side needs to adopt it.
        notice: RegionNotice,
    },
}

/// The in-band half of a [`CapHandoff::Region`]: exactly the arena's
/// bootstrap-offer geometry, so the receiving side can adopt the region
/// without any out-of-band knowledge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionNotice {
    /// First `T.BufferId` this arena resolves.
    pub namespace_root: u64,
    /// Number of data slots.
    pub slot_count: u32,
    /// Payload bytes per slot.
    pub slot_bytes: u32,
    /// Control-ring slots per direction.
    pub ring_slots: u32,
    /// Bytes per control-ring slot.
    pub ring_bytes: u32,
    /// Bootstrap epoch of this arena.
    pub epoch: u32,
}

impl core::fmt::Debug for AttachedCap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The payloads are opaque; the form is the reportable fact.
        let form = match self {
            AttachedCap::Live(_) => "Live",
            AttachedCap::Descriptor(b) => {
                return f.debug_struct("AttachedCap::Descriptor").field("bytes", &b.len()).finish()
            }
            AttachedCap::OsHandle { fd } => {
                return f.debug_struct("AttachedCap::OsHandle").field("fd", fd).finish()
            }
            AttachedCap::Region { fd, notice } => {
                return f
                    .debug_struct("AttachedCap::Region")
                    .field("fd", fd)
                    .field("namespace_root", &notice.namespace_root)
                    .finish()
            }
        };
        f.write_str(form)
    }
}

/// What the receiving side holds after an attachment arrived.
pub enum AttachedCap {
    /// The live handle crossed (InProcess). The receiver owns it now.
    Live(Box<dyn Any + Send>),
    /// Descriptor bytes only; authority must be re-verified per call.
    Descriptor(Vec<u8>),
    /// An OS handle the kernel re-opened for us.
    OsHandle {
        /// The received descriptor.
        fd: OsHandle,
    },
    /// A region handle plus its notice; `resolve_buffer` serves its slots.
    Region {
        /// The received region descriptor.
        fd: OsHandle,
        /// The received arena geometry.
        notice: RegionNotice,
    },
}

/// A borrowed view of one published frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameRef<'a>(&'a [u8]);

impl<'a> FrameRef<'a> {
    /// Wrap a published frame's bytes.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }
    /// The frame's bytes.
    pub fn as_slice(&self) -> &'a [u8] {
        self.0
    }
    /// The frame's length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether the frame is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'a> core::ops::Deref for FrameRef<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

/// The honest outcome of [`Channel::attach`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapOutcome {
    /// The reference crosses as authority (InProcess `Live`; UnixSocket
    /// `OsHandle`/`Region`-with-fd). `id` identifies it on the far side.
    Native {
        /// The id the far side will see.
        id: CapId,
    },
    /// The bytes cross; authority does not. The far side must re-verify
    /// the descriptor against its capability service before trusting it.
    /// This is the whole Tcp story and the Shmem arena-resident story.
    InBand {
        /// The id the far side will see.
        id: CapId,
    },
    /// Refused with a typed reason naming what could not move. The
    /// `cap` argument was handed back untouched.
    Refused(TransportError),
}

/// A message handed to [`Channel::send`].
///
/// `Borrowed` is valid only until `send` returns (stream transports copy it
/// into the frame; the InProcess seam dispatches it by reference).
/// `Owned` moves the builder's allocation into the transport — that is the
/// zero-copy InProcess path: the segments' addresses are preserved.
pub enum MessageOut<'a> {
    /// The sender's segments, valid for the duration of `send`.
    Borrowed(&'a [&'a [u8]]),
    /// The sender's builder, moved: the zero-copy InProcess path.
    Owned(OwnedMessage),
}

/// An owned capnp message builder. Wrapping the concrete heap allocator
/// (not a generic parameter) is what lets a `MessageOut` cross a `dyn`
/// seam; `HeapAllocator` is the standard choice and is itself no-copy to
/// move (it is a `Vec` of segments).
pub struct OwnedMessage(capnp::message::Builder<capnp::message::HeapAllocator>);

impl OwnedMessage {
    /// Wrap a builder (moving its allocation, not its bytes).
    pub fn new(builder: capnp::message::Builder<capnp::message::HeapAllocator>) -> Self {
        Self(builder)
    }
    /// The wrapped builder.
    pub fn builder(&self) -> &capnp::message::Builder<capnp::message::HeapAllocator> {
        &self.0
    }
    /// Unwrap the builder.
    pub fn into_builder(self) -> capnp::message::Builder<capnp::message::HeapAllocator> {
        self.0
    }
}

impl capnp::message::ReaderSegments for OwnedMessage {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        self.0.get_segments_for_output().get(id as usize).copied()
    }
}

impl capnp::message::ReaderSegments for MessageOut<'_> {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        match self {
            MessageOut::Borrowed(segments) => segments.get(id as usize).copied(),
            MessageOut::Owned(owned) => owned.get_segment(id),
        }
    }
}

impl core::fmt::Debug for RecvBatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The message bytes are not the interesting part of a failure
        // report; the shape is.
        f.debug_struct("RecvBatch")
            .field("segments", &self.message.segment_count())
            .field("caps", &self.caps.len())
            .finish()
    }
}

/// One received message plus every capability that arrived with its frame.
pub struct RecvBatch {
    /// The received message.
    pub message: RecvMessage,
    /// Every capability that arrived with this frame.
    pub caps: Vec<(CapId, AttachedCap)>,
}

impl RecvBatch {
    /// Reads the message root as `T`.
    pub fn root<T>(&self) -> Result<T::Reader<'_>, TransportError>
    where
        T: capnp::traits::Owned,
    {
        self.message.root::<T>()
    }

    /// The received message, ready to serve as an ABI handshake response.
    /// One declared copy (the cold path: once per connect), because the
    /// response must outlive the frame it arrived in.
    pub fn negotiate_response(&self) -> Result<NegotiateResponse, TransportError> {
        let mut words = Vec::new();
        capnp::serialize::write_message_segments(&mut words, &MsgSegments(&self.message))
            .map_err(frame::capnp_to_transport)?;
        let reader = capnp::serialize::read_message(
            &mut &words[..],
            capnp::message::ReaderOptions::new(),
        )
        .map_err(frame::capnp_to_transport)?;
        Ok(capnp::message::TypedReader::new(reader))
    }
}

/// A `ReaderSegments` view over a received message, for the cold-path
/// re-encoding above.
struct MsgSegments<'a>(&'a RecvMessage);

impl ReaderSegments for MsgSegments<'_> {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        match &self.0 {
            RecvMessage::Direct(reader) => reader.get_segments().get_segment(id),
            RecvMessage::Owned(reader) => reader.get_segments().get_segment(id),
        }
    }
}

/// A received message. Both variants own their segments, so no lifetime
/// escapes into caller code.
pub enum RecvMessage {
    /// The message memory moved in as-is (InProcess): zero-copy receive.
    Direct(
        /// A reader over the moved builder's own segments.
        capnp::message::Reader<OwnedMessage>,
    ),
    /// Framed over a byte stream: one kernel-side copy, one parse copy.
    /// Declared honestly in `TransportCaps::note`.
    Owned(
        /// A reader over owned, word-aligned segments.
        capnp::message::Reader<capnp::serialize::OwnedSegments>,
    ),
}

impl RecvMessage {
    /// Reads the message root as `T`.
    pub fn root<T>(&self) -> Result<T::Reader<'_>, TransportError>
    where
        T: capnp::traits::Owned,
    {
        match self {
            RecvMessage::Direct(reader) => reader
                .get_root::<T::Reader<'_>>()
                .map_err(|e| TransportError::message(e.to_string())),
            RecvMessage::Owned(reader) => reader
                .get_root::<T::Reader<'_>>()
                .map_err(|e| TransportError::message(e.to_string())),
        }
    }

    /// The number of segments in the received message.
    pub fn segment_count(&self) -> usize {
        match self {
            RecvMessage::Direct(reader) => reader.get_segments().len(),
            RecvMessage::Owned(reader) => reader.get_segments().len(),
        }
    }
}

/// Why a channel closed, or was closed. Typed so the far side can react;
/// `detail` is a declared reason, never a placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// Orderly local shutdown.
    Shutdown,
    /// The far side closed first.
    PeerClosed,
    /// The caller cancelled.
    Cancelled,
    /// A framing or ABI violation was observed.
    Protocol {
        /// What was violated, named.
        detail: String,
    },
    /// A host/transport-level failure with the honest detail.
    Transport {
        /// The host-level detail.
        detail: String,
    },
    /// The transport can no longer function.
    Unavailable {
        /// Why the transport can no longer function.
        detail: String,
    },
}

impl CloseReason {
    /// Stable wire code for this reason (see `frame::KIND_CLOSE`).
    pub fn wire_code(&self) -> u16 {
        match self {
            CloseReason::Shutdown => 1,
            CloseReason::PeerClosed => 2,
            CloseReason::Cancelled => 3,
            CloseReason::Protocol { .. } => 4,
            CloseReason::Transport { .. } => 5,
            CloseReason::Unavailable { .. } => 6,
        }
    }

    /// Rebuild a reason from its wire code and detail (see `wire_code`).
    pub fn from_wire(code: u16, detail: String) -> Self {
        match code {
            1 => CloseReason::Shutdown,
            2 => CloseReason::PeerClosed,
            3 => CloseReason::Cancelled,
            4 => CloseReason::Protocol { detail },
            5 => CloseReason::Transport { detail },
            6 => CloseReason::Unavailable { detail },
            // An unknown close code is still a close; it is forwarded as a
            // protocol detail rather than dropped (unknown-tolerance).
            _ => CloseReason::Protocol {
                detail: if detail.is_empty() {
                    alloc::format!("unknown close code {code}")
                } else {
                    detail
                },
            },
        }
    }

    /// The contract's `T.ErrorCode` this reason corresponds to.
    pub fn error_code(&self) -> types_capnp::ErrorCode {
        match self {
            CloseReason::Shutdown | CloseReason::PeerClosed | CloseReason::Cancelled => {
                types_capnp::ErrorCode::Cancelled
            }
            CloseReason::Protocol { .. } => types_capnp::ErrorCode::ProtocolViolation,
            CloseReason::Transport { .. } => types_capnp::ErrorCode::Failed,
            CloseReason::Unavailable { .. } => types_capnp::ErrorCode::Unavailable,
        }
    }
}

/// Every failure this layer can produce, with the contract's `T.ErrorCode`
/// it maps to (totality asserted in `tests/error_mapping.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Bounded pull found nothing yet. Not a transport fault: the caller
    /// drives pulls, and "nothing" is a real answer.
    /// Bounded pull found nothing yet (see the type docs).
    Empty,
    /// The far side closed with the typed reason it declared.
    Closed {
        /// The reason the far side declared.
        reason: CloseReason,
    },
    /// This side already closed.
    AlreadyClosed,
    /// The wire bytes do not match this framing version/layout.
    Framing {
        /// What was wrong with the wire bytes.
        detail: String,
    },
    /// The frame exceeded the declared maximum.
    LimitExceeded {
        /// The declared maximum, in bytes.
        limit: u32,
    },
    /// Connect/bind failed.
    Connect {
        /// The host-level detail.
        detail: String,
    },
    /// A capability kind the transport cannot carry was presented.
    UnsupportedCapability {
        /// Which capability kind.
        kind: String,
        /// Why it cannot move here.
        detail: String,
    },
    /// A `T.BufferId` no live arena on this channel resolves.
    BufferNotResolvable {
        /// The id that could not be resolved.
        id: u64,
        /// Why not (never a fabricated success).
        detail: String,
    },
    /// The arena exists but is not established (bootstrap not done).
    ArenaNotEstablished,
    /// Capnp serialization failure.
    Message {
        /// The serialization detail.
        detail: String,
    },
    /// A host call failed; `errno` is the raw value (0 when unknown).
    Host {
        /// The raw errno.
        errno: i32,
        /// The host-level detail.
        detail: String,
    },
    /// This transport is not compiled for this configuration.
    NotCompiled {
        /// The scheme token that is missing in this build.
        scheme: &'static str,
    },
    /// No transport is registered for this scheme token.
    ///
    /// The honest answer for a paradigm this engine does not serve: it names
    /// the scheme that was asked for and the schemes that do exist, so the
    /// caller can pick a supported one. Never a crash, never a silent
    /// fallback to another paradigm.
    UnknownScheme {
        /// The scheme token that was asked for.
        scheme: String,
        /// The scheme tokens that are registered.
        known: Vec<String>,
    },
}

impl TransportError {
    /// A capnp-serialization failure with its detail.
    pub fn message(detail: impl Into<String>) -> Self {
        TransportError::Message {
            detail: detail.into(),
        }
    }

    /// The contract's `T.ErrorCode` this error maps to. Total: every
    /// variant maps to exactly one code (L1's rule, applied to this seam).
    pub fn error_code(&self) -> types_capnp::ErrorCode {
        match self {
            TransportError::Empty => types_capnp::ErrorCode::Temporary,
            TransportError::Closed { reason } => reason.error_code(),
            TransportError::AlreadyClosed => types_capnp::ErrorCode::Failed,
            TransportError::Framing { .. } => types_capnp::ErrorCode::ProtocolViolation,
            TransportError::LimitExceeded { .. } => types_capnp::ErrorCode::LimitExceeded,
            TransportError::Connect { .. } => types_capnp::ErrorCode::Unavailable,
            TransportError::UnsupportedCapability { .. } => types_capnp::ErrorCode::NotSupported,
            TransportError::BufferNotResolvable { .. } => types_capnp::ErrorCode::NotFound,
            TransportError::ArenaNotEstablished => types_capnp::ErrorCode::Unavailable,
            TransportError::Message { .. } => types_capnp::ErrorCode::Failed,
            TransportError::Host { errno, .. } => host_errno_to_code(*errno),
            TransportError::NotCompiled { .. } => types_capnp::ErrorCode::NotSupported,
            TransportError::UnknownScheme { .. } => types_capnp::ErrorCode::Unavailable,
        }
    }

    /// A human-readable detail. Never empty.
    pub fn detail(&self) -> String {
        match self {
            TransportError::Empty => "no frame available".to_string(),
            TransportError::Closed { reason } => alloc::format!("peer closed: {reason:?}"),
            TransportError::AlreadyClosed => "channel already closed".to_string(),
            TransportError::Framing { detail } => detail.clone(),
            TransportError::LimitExceeded { limit } => {
                alloc::format!("frame exceeds declared maximum of {limit} bytes")
            }
            TransportError::Connect { detail } => detail.clone(),
            TransportError::UnsupportedCapability { kind, detail } => {
                alloc::format!("cannot carry {kind}: {detail}")
            }
            TransportError::BufferNotResolvable { id, detail } => {
                alloc::format!("buffer id {id} not resolvable: {detail}")
            }
            TransportError::ArenaNotEstablished => {
                "arena is not established: run the authenticated bootstrap first".to_string()
            }
            TransportError::Message { detail } => detail.clone(),
            TransportError::Host { errno, detail } => {
                alloc::format!("host failure (errno {errno}): {detail}")
            }
            TransportError::NotCompiled { scheme } => {
                alloc::format!("{scheme} is not compiled in this configuration")
            }
            TransportError::UnknownScheme { scheme, known } => {
                alloc::format!(
                    "transport scheme '{scheme}' is not served here; the registered schemes are: {}",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            }
        }
    }

    /// Fill a generated `T.Error` builder. The transport owns its error
    /// type; the mapping is published here so core can adopt or replace it
    /// without touching transports.
    pub fn write_t_error(&self, mut builder: types_capnp::error::Builder<'_>) {
        builder.set_code(self.error_code());
        builder.set_message(&self.detail());
        builder.set_retryable(matches!(self, TransportError::Empty));
    }
}

/// Honest errno → `T.ErrorCode` mapping for host failures (Linux values).
fn host_errno_to_code(errno: i32) -> types_capnp::ErrorCode {
    match errno {
        1 => types_capnp::ErrorCode::PermissionDenied,      // EPERM
        2 => types_capnp::ErrorCode::NotFound,              // ENOENT
        13 => types_capnp::ErrorCode::PermissionDenied,     // EACCES
        32 => types_capnp::ErrorCode::ProtocolViolation,    // EPIPE
        104 | 111 | 107 | 110 => types_capnp::ErrorCode::Unavailable, // ECONNRESET/REFUSED/NOTCONN/TIMEDOUT
        11 => types_capnp::ErrorCode::Temporary,            // EAGAIN
        _ => types_capnp::ErrorCode::Failed,
    }
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.detail())
    }
}

impl core::error::Error for TransportError {}

/// The ONE API shape all four transports implement.
///
/// Object-safe by construction (the seam is the one place `dyn` belongs);
/// `Send` because channels move between threads. There is no `Sync` bound:
/// sharing a channel is the caller's choice via `Arc`, and every impl is
/// `Sync` anyway.
pub trait Channel: Send {
    /// The open scheme token this channel answers to (diagnostics only).
    fn scheme(&self) -> &'static str;

    /// The declared capabilities of this transport instance (law #6).
    fn caps(&self) -> TransportCaps;

    /// Send one message. `Owned` messages move without a byte copy on
    /// `InProcess`; on stream transports the frame write is the copy and
    /// is declared in `caps().note`. Capabilities staged with `attach`
    /// since the last send ride this frame.
    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError>;

    /// Bounded pull of one message plus the capabilities that arrived with
    /// it. `scratch` is caller-owned frame storage (reused across calls;
    /// the only allocation is its growth). `Empty` means "nothing yet" —
    /// never a fabricated message.
    fn recv(&self, scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError>;

    /// Stage a capability to ride the next send. On refusal the value is
    /// handed back through `cap` untouched.
    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError>;

    /// Drop a staged capability.
    fn detach(&self, id: CapId) -> Result<(), TransportError>;

    /// Resolve a `T.BufferId` against this channel's live arenas. `Err` is
    /// honest: no transport may pretend a namespace it does not have.
    fn resolve_buffer(&self, id: types_capnp::buffer_id::Reader<'_>) -> Result<FrameRef<'_>, TransportError>;

    /// Where `abi.capnp`'s negotiate lands in the connect sequence:
    /// `connect` → (Shmem: bootstrap) → `negotiate` → send/recv. The
    /// transport moves the handshake messages; the compatibility decision
    /// is the abi domain's, read from the returned response (`get()`).
    fn negotiate(
        &self,
        request: MessageOut<'_>,
    ) -> Result<NegotiateResponse, TransportError>;

    /// Peer identity where the host can produce one. `None` means this
    /// transport has none (Shmem, Tcp) — not that the peer is anonymous
    /// by choice.
    fn peer_credentials(&self) -> Option<PeerCredentials>;

    /// Close with a typed reason. The reason is propagated to the far side
    /// where the transport can carry it.
    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError>;

    /// Take this channel's **byte stream**, for a protocol that owns its own
    /// framing — the contract's own call model (capnp RPC) rides here.
    ///
    /// This *consumes* the channel on purpose: a connection carries either
    /// this engine's framed messages or a foreign protocol's bytes, never
    /// both interleaved. A transport with no byte stream (InProcess
    /// dispatches messages, it does not move bytes) answers honestly with
    /// the reason it has none.
    fn into_byte_stream(self: Box<Self>) -> Result<Box<dyn ByteStream>, TransportError>;
}

/// The read half of a channel's raw byte stream.
///
/// The stream is `Send + Sync`: the RPC runtime pumps it from a thread of
/// its own, so the far end's socket is read on that thread while the
/// executor keeps running.
pub trait ByteReader: Send + Sync {
    /// Read up to `buf.len()` bytes. `Ok(0)` means the far side closed.
    fn read(&self, buf: &mut [u8]) -> Result<usize, TransportError>;
}

/// The write half of a channel's raw byte stream.
pub trait ByteWriter: Send + Sync {
    /// Write every byte of `buf` or fail.
    fn write_all(&self, buf: &[u8]) -> Result<(), TransportError>;
    /// Push buffered bytes toward the far side.
    fn flush(&self) -> Result<(), TransportError>;
}

/// A channel's raw byte stream: what a foreign protocol (capnp RPC) rides.
///
/// The transport's own framing stays *off* this path: the bytes here are
/// the foreign protocol's bytes, end to end.
pub trait ByteStream: ByteReader + ByteWriter {
    /// Close the stream: the far side sees the end of the connection and
    /// this side's reader unblocks.
    fn close(&self);
}

/// How a caller selects a transport.
///
/// The selection is an **open scheme token** plus the address in that
/// scheme's own vocabulary — never a closed enum of paradigms. The four
/// transports this tree ships register themselves into
/// [`crate::transport::registry::TransportRegistry`]; a fifth needs no
/// change here.
pub type Transport = crate::transport::registry::TransportRequest;

/// Connect the client side of a transport.
///
/// The sequence is documented on [`Channel::negotiate`]: connect →
/// (Shmem: the arena must already be established by the bootstrap) →
/// negotiate → send/recv.
pub fn connect(transport: Transport) -> Result<Box<dyn Channel>, TransportError> {
    crate::transport::registry::TransportRegistry::with_builtins().connect(&transport)
}

/// Connect and run the ABI handshake in one step (the connect sequence's
/// second stage, materialised).
pub fn connect_negotiated(
    transport: Transport,
    request: MessageOut<'_>,
) -> Result<(Box<dyn Channel>, NegotiateResponse), TransportError> {
    let channel = connect(transport)?;
    let response = channel.negotiate(request)?;
    Ok((channel, response))
}

/// The peer's answer to an ABI handshake: the parsed `AbiHandshakeResponse`,
/// owned by the transport's read path so no lifetime escapes into caller
/// code. `response.get()` yields the contract's reader; the compatibility
/// *decision* belongs to the abi domain, not to this seam.
pub type NegotiateResponse = capnp::message::TypedReader<
    capnp::serialize::OwnedSegments,
    abi_capnp::abi_handshake_response::Owned,
>;

/// The server side of a transport-selection: accepts far-side connections.
pub trait ChannelListener: Send {
    /// The open scheme token this listener serves.
    fn scheme(&self) -> &'static str;
    /// Accept one connection. Blocking; the accept loop is the caller's.
    fn accept(&self) -> Result<Box<dyn Channel>, TransportError>;
}

/// Bind a listener for the given transport (where one exists).
pub fn listen(transport: &Transport) -> Result<Box<dyn ChannelListener>, TransportError> {
    crate::transport::registry::TransportRegistry::with_builtins().listen(transport)
}

/// The wire framing shared by the stream transports (UnixSocket, Tcp).
///
/// ```text
/// frame := u32be total_len | kind:u8 | payload
/// kind 0x01 DATA       payload = one capnp flat message (self-delimiting)
/// kind 0x02 NEGOTIATE  payload = one capnp flat message (handshake pair)
/// kind 0x03 CLOSE      payload = u16be reason_code | u16be detail_len | detail
/// kind 0x04 ATTACH     payload = u8 cap_kind | u64be cap_id | u16be data_len | data
/// kind 0x05 DETACH     payload = u64be cap_id
/// ```
///
/// `cap_kind`: 1 = descriptor bytes, 2 = region notice (28 bytes: root,
/// slot_count, slot_bytes, ring_slots, ring_bytes, epoch), 3 = bare OS
/// handle (no data). Kinds 2 and 3 carry their `OsHandle` in the ancillary
/// channel of the same frame when the host has SCM_RIGHTS.
pub mod frame {
    use super::*;

    /// A capnp data message.
    pub const KIND_DATA: u8 = 0x01;
    /// An ABI handshake message (request or response).
    pub const KIND_NEGOTIATE: u8 = 0x02;
    /// A close notice with a typed reason.
    pub const KIND_CLOSE: u8 = 0x03;
    /// A capability attachment.
    pub const KIND_ATTACH: u8 = 0x04;
    /// A capability detach.
    pub const KIND_DETACH: u8 = 0x05;

    /// 64 MiB: the declared maximum frame size for every stream transport.
    pub const MAX_FRAME_BYTES: u32 = 64 * 1024 * 1024;

    /// An attachment carrying a descriptor payload.
    pub const CAP_KIND_DESCRIPTOR: u8 = 1;
    /// An attachment carrying a region notice (and its fd in ancillary).
    pub const CAP_KIND_REGION: u8 = 2;
    /// An attachment carrying only an OS handle (in ancillary).
    pub const CAP_KIND_OS_HANDLE: u8 = 3;

    /// Bytes of the segment table for `count` segments: word 0 holds
    /// `(count-1)` and segment 0's length-in-words; further lengths follow
    /// two per word, zero-padded to a word. This is capnp's stream format
    /// (`serialize::write_segment_table_internal`), replicated here because
    /// a length prefix must be computed *before* the message is written.
    fn segment_table_bytes(count: usize) -> usize {
        if count <= 1 {
            8
        } else {
            8 + (count & !1) * 4
        }
    }

    fn segment_len_words<S: capnp::message::ReaderSegments>(segments: &S, i: usize) -> u32 {
        let seg = segments.get_segment(i as u32).unwrap_or(&[]);
        debug_assert!(seg.len() % 8 == 0, "capnp segments are word-sized");
        (seg.len() / 8) as u32
    }

    fn check_segments<S: capnp::message::ReaderSegments>(segments: &S) -> Result<usize, TransportError> {
        let count = segments.len();
        if count == 0 || count >= 512 {
            return Err(TransportError::Framing {
                detail: alloc::format!("implausible segment count {count}"),
            });
        }
        for i in 0..count {
            let seg = segments.get_segment(i as u32).unwrap_or(&[]);
            if seg.len() % 8 != 0 {
                return Err(TransportError::Framing {
                    detail: alloc::format!("segment {i} is not word-aligned"),
                });
            }
        }
        Ok(count)
    }

    /// Byte length of the capnp flat-message encoding of `segments`:
    /// the segment table plus each segment verbatim (capnp's stream format
    /// does not pad segments). Verified byte-identical against
    /// `capnp::serialize::write_message` in `tests/framing.rs`.
    pub fn flat_message_len<S: capnp::message::ReaderSegments>(segments: &S) -> u32 {
        let count = segments.len();
        let mut size = segment_table_bytes(count);
        for i in 0..count {
            size += segments.get_segment(i as u32).unwrap_or(&[]).len();
        }
        size as u32
    }

    /// Write a capnp flat message (segment table + verbatim segments).
    pub fn write_flat_message<S: capnp::message::ReaderSegments, W: capnp::io::Write>(
        write: &mut W,
        segments: &S,
    ) -> Result<(), TransportError> {
        let count = check_segments(segments)?;
        let mut word = [0u8; 8];
        word[0..4].copy_from_slice(&((count as u32) - 1).to_le_bytes());
        word[4..8].copy_from_slice(&segment_len_words(segments, 0).to_le_bytes());
        write.write_all(&word).map_err(capnp_to_transport)?;
        if count > 1 {
            if count < 4 {
                let mut buf = [0u8; 8];
                for idx in 1..count {
                    buf[(idx - 1) * 4..idx * 4]
                        .copy_from_slice(&segment_len_words(segments, idx).to_le_bytes());
                }
                if count == 2 {
                    for b in &mut buf[4..8] {
                        *b = 0;
                    }
                }
                write.write_all(&buf).map_err(capnp_to_transport)?;
            } else {
                let mut buf = alloc::vec![0u8; (count & !1) * 4];
                for idx in 1..count {
                    buf[(idx - 1) * 4..idx * 4]
                        .copy_from_slice(&segment_len_words(segments, idx).to_le_bytes());
                }
                if count % 2 == 0 {
                    let start = buf.len() - 4;
                    for b in &mut buf[start..] {
                        *b = 0;
                    }
                }
                write.write_all(&buf).map_err(capnp_to_transport)?;
            }
        }
        for i in 0..count {
            let seg = segments.get_segment(i as u32).unwrap_or(&[]);
            write.write_all(seg).map_err(capnp_to_transport)?;
        }
        Ok(())
    }

    /// Write a capnp flat message into a fixed byte buffer, returning the
    /// number of bytes written. This is the shared-arena counterpart of
    /// [`write_flat_message`]: it writes straight into a ring slot, with no
    /// intermediate buffer and no allocation. Verified against
    /// `write_flat_message` in `tests/framing.rs` (same bytes, both paths).
    pub fn flat_message_into<S: capnp::message::ReaderSegments>(
        dst: &mut [u8],
        segments: &S,
    ) -> Result<usize, TransportError> {
        let count = check_segments(segments)?;
        let table_bytes = segment_table_bytes(count);
        let mut total = table_bytes;
        for i in 0..count {
            total += segments.get_segment(i as u32).unwrap_or(&[]).len();
        }
        if dst.len() < total {
            return Err(TransportError::LimitExceeded {
                limit: dst.len() as u32,
            });
        }
        let out = &mut dst[..total];
        out[0..4].copy_from_slice(&((count as u32) - 1).to_le_bytes());
        out[4..8].copy_from_slice(&segment_len_words(segments, 0).to_le_bytes());
        for idx in 1..count {
            let at = 4 + idx * 4;
            out[at..at + 4].copy_from_slice(&segment_len_words(segments, idx).to_le_bytes());
        }
        for b in &mut out[4 + count * 4..table_bytes] {
            *b = 0;
        }
        let mut written = table_bytes;
        for i in 0..count {
            let seg = segments.get_segment(i as u32).unwrap_or(&[]);
            out[written..written + seg.len()].copy_from_slice(seg);
            written += seg.len();
        }
        Ok(total)
    }

    /// Write one framed payload: `u32be(len(kind)+len(payload)) | kind | payload`.
    /// The length never counts itself — the reader consumes the four length
    /// bytes first and then exactly `total` more.
    pub fn write_frame<W: capnp::io::Write>(
        write: &mut W,
        kind: u8,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        let total = 1u32
            .checked_add(payload.len() as u32)
            .ok_or_else(|| TransportError::LimitExceeded {
                limit: MAX_FRAME_BYTES,
            })?;
        debug_assert_eq!(
            total as usize + 4,
            4 + 1 + payload.len(),
            "the prefix counts kind + payload only"
        );
        if total > MAX_FRAME_BYTES {
            return Err(TransportError::LimitExceeded {
                limit: MAX_FRAME_BYTES,
            });
        }
        write.write_all(&total.to_be_bytes()).map_err(capnp_to_transport)?;
        write.write_all(&[kind]).map_err(capnp_to_transport)?;
        write.write_all(payload).map_err(capnp_to_transport)?;
        Ok(())
    }

    /// Read exactly `n` bytes, retrying on partial reads.
    pub fn read_exact<R: capnp::io::Read>(
        read: &mut R,
        buf: &mut [u8],
    ) -> Result<(), TransportError> {
        read.read_exact(buf).map_err(capnp_to_transport)
    }

    /// Read one frame into `scratch`; returns `(kind, payload)` borrowed
    /// from it.
    pub fn read_frame<'b, R: capnp::io::Read>(
        read: &mut R,
        scratch: &'b mut Vec<u8>,
    ) -> Result<(u8, &'b [u8]), TransportError> {
        let mut len_buf = [0u8; 4];
        read_exact(read, &mut len_buf)?;
        let total = u32::from_be_bytes(len_buf);
        if total == 0 || total > MAX_FRAME_BYTES {
            return Err(TransportError::Framing {
                detail: alloc::format!("declared frame length {total} out of range"),
            });
        }
        scratch.clear();
        scratch.resize(total as usize, 0);
        read_exact(read, scratch)?;
        Ok((scratch[0], &scratch[1..]))
    }

    /// Translate a capnp codec error into this layer's error vocabulary.
    pub fn capnp_to_transport(e: capnp::Error) -> TransportError {
        TransportError::Message {
            detail: e.to_string(),
        }
    }

    /// Encode a CLOSE payload.
    pub fn encode_close(reason: &CloseReason) -> Vec<u8> {
        let detail = close_detail(reason);
        let mut out = Vec::with_capacity(4 + detail.len());
        out.extend_from_slice(&reason.wire_code().to_be_bytes());
        out.extend_from_slice(&(detail.len() as u16).to_be_bytes());
        out.extend_from_slice(detail.as_bytes());
        out
    }

    /// Decode a CLOSE payload.
    pub fn decode_close(payload: &[u8]) -> Result<CloseReason, TransportError> {
        if payload.len() < 4 {
            return Err(TransportError::Framing {
                detail: "close payload too short".to_string(),
            });
        }
        let code = u16::from_be_bytes([payload[0], payload[1]]);
        let detail_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
        if payload.len() < 4 + detail_len {
            return Err(TransportError::Framing {
                detail: "close detail length exceeds payload".to_string(),
            });
        }
        let detail = core::str::from_utf8(&payload[4..4 + detail_len])
            .map_err(|_| TransportError::Framing {
                detail: "close detail is not utf-8".to_string(),
            })?
            .to_string();
        Ok(CloseReason::from_wire(code, detail))
    }

    fn close_detail(reason: &CloseReason) -> String {
        match reason {
            CloseReason::Protocol { detail }
            | CloseReason::Transport { detail }
            | CloseReason::Unavailable { detail } => detail.clone(),
            _ => String::new(),
        }
    }

    /// Encode an ATTACH payload (the fd, if any, rides ancillary).
    pub fn encode_attach(kind: u8, id: CapId, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + data.len());
        out.push(kind);
        out.extend_from_slice(&id.0.to_be_bytes());
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    /// Encode a region notice as ATTACH data (28 bytes, big-endian).
    pub fn encode_region_notice(notice: &RegionNotice) -> Vec<u8> {
        let mut out = Vec::with_capacity(28);
        out.extend_from_slice(&notice.namespace_root.to_be_bytes());
        out.extend_from_slice(&notice.slot_count.to_be_bytes());
        out.extend_from_slice(&notice.slot_bytes.to_be_bytes());
        out.extend_from_slice(&notice.ring_slots.to_be_bytes());
        out.extend_from_slice(&notice.ring_bytes.to_be_bytes());
        out.extend_from_slice(&notice.epoch.to_be_bytes());
        out
    }

    /// Decode a region notice from ATTACH data.
    pub fn decode_region_notice(data: &[u8]) -> Result<RegionNotice, TransportError> {
        if data.len() != 28 {
            return Err(TransportError::Framing {
                detail: "region notice must be 28 bytes".to_string(),
            });
        }
        Ok(RegionNotice {
            namespace_root: u64::from_be_bytes(data[0..8].try_into().unwrap()),
            slot_count: u32::from_be_bytes(data[8..12].try_into().unwrap()),
            slot_bytes: u32::from_be_bytes(data[12..16].try_into().unwrap()),
            ring_slots: u32::from_be_bytes(data[16..20].try_into().unwrap()),
            ring_bytes: u32::from_be_bytes(data[20..24].try_into().unwrap()),
            epoch: u32::from_be_bytes(data[24..28].try_into().unwrap()),
        })
    }
}

