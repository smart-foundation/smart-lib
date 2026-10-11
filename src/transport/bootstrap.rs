//! The Shmem authentication bootstrap: who authenticates whom, over which
//! channel, and what makes the arena safe afterward.
//!
//! ## The problem (02-HOST-CAPABILITIES §4.2, exact words)
//!
//! > **Shmem carries no authority.** A shared arena carries data, not
//! > authority. Bootstrap rule: capability handles are exchanged **once**
//! > over an authenticated channel (UnixSocket SCM_RIGHTS or a bootstrap
//! > token), then handles live in the ring.
//!
//! A raw shared region is *spoofable*: on a host where the arena has a name
//! (`shm_open`), any process that can open that name can write frames and
//! forge `BufferId`s. This protocol closes that hole by moving the region
//! itself over a channel that authenticates the peer.
//!
//! ## Who authenticates whom
//!
//! Both sides, once, using the kernel as the referee:
//!
//! * **The owner** (arena creator) knows whom it invited: it checks the
//!   peer's `SO_PEERCRED` (pid/uid/gid, Linux, verified per `sys.rs`)
//!   against the expected peer before sending the offer.
//! * **The peer** authenticates the *region*, not the sender: it receives
//!   the region's fd over `SCM_RIGHTS`, and the kernel's guarantee is the
//!   whole point — a passed fd refers to the *same* kernel object. For an
//!   **anonymous memfd** (which is what `Arena::create` makes on Linux)
//!   no third process can open the region by name, so holding the fd *is*
//!   exclusive knowledge of the arena. The peer validates the region's
//!   self-describing header (magic, format, geometry, epoch, seals) before
//!   accepting it.
//!
//! ## Over which channel
//!
//! The bootstrap runs **before** ABI negotiation and before any `Channel`
//! exists (it is what makes a Shmem channel possible at all), so it uses
//! its own minimal link: [`BootstrapLink`]. The production implementation
//! is a raw `AF_UNIX` socket with `SCM_RIGHTS` (`UnixBootstrapLink`); a
//! `MockLink` exists for tests (`tests/bootstrap.rs`). The wire format is
//! therefore *not* capnp — it is a fixed little-endian encoding that cannot
//! drift with an ABI, because it exists to establish the ABI's channel.
//!
//! ## What makes the arena safe afterward
//!
//! 1. **The fd object identity.** After the transfer, both sides hold
//!    mappings of the same memfd; nobody else can obtain one.
//! 2. **The seals.** `F_SEAL_SHRINK | F_SEAL_GROW` mean the geometry
//!    cannot change under the peer (verified on this box; the heap backing
//!    declares `seals_word() == 0`, honestly).
//! 3. **The namespace root.** Slot ids resolve only inside
//!    `[namespace_root, namespace_root + slot_count)`; anything else is
//!    refused with a typed error, so a stale or foreign `BufferId` can
//!    never alias a slot.
//! 4. **The state word.** The arena is `INERT` until the owner observes the
//!    peer's `Accept` and sets `ESTABLISHED` in the shared header. Before
//!    that, publish/resolve/send all answer `ArenaNotEstablished` — the
//!    arena cannot be used by a peer that never completed the exchange.
//! 5. **Per-call validity.** Revocation has no channel on Shmem (it is
//!    data, not authority): the *owner* re-checks handle validity per call,
//!    which the domain layer does; the transport only guarantees the
//!    namespace and the establishment.
//!
//! ## The protocol
//!
//! ```text
//! Owner                              Peer
//!   | ---- Offer{geometry, root, epoch, fd} --> |   (SCM_RIGHTS, if the
//!   |                                          |    link can pass fds)
//!   |        (peer checks identity + header)   |
//!   | <---------------- Accept{root, epoch} --- |
//!   |        (owner sets ESTABLISHED)          |
//!   | ---- frames over the arena rings ------- |
//! ```
//!
//! Any mismatch (version, geometry, epoch, peer, seals, corrupt region) is
//! a `Refuse` with a typed reason, and **both sides stay inert**: a refused
//! bootstrap is never a half-open arena.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::transport::channel::{OsHandle, PeerCredentials, TransportError};
use crate::transport::shmem::{Arena, ArenaConfig, Direction, ShmemHandle};

/// Wire version of the bootstrap encoding.
pub const BOOTSTRAP_VERSION: u32 = 1;

const MSG_OFFER: u8 = 0x01;
const MSG_ACCEPT: u8 = 0x02;
const MSG_REFUSE: u8 = 0x03;

/// A region crossing a bootstrap link. The two forms are honest about what
/// they are: a real OS handle (the kernel enforces identity) or a shared
/// `Arc` (the in-process test double — same process, same arena object).
pub enum RegionPass {
    /// The region's OS descriptor: the kernel guarantees this names the
    /// same object on both sides, which is the whole transfer.
    OsHandle(OsHandle),
    /// The same process's arena object (the test double's form).
    Shared(Arc<Arena>),
}

impl Clone for RegionPass {
    fn clone(&self) -> Self {
        match self {
            RegionPass::OsHandle(handle) => RegionPass::OsHandle(*handle),
            RegionPass::Shared(arena) => RegionPass::Shared(arena.clone()),
        }
    }
}

/// A typed refusal reason. The far side (and the operator) can name exactly
/// what failed; `unknown` is deliberately absent — one of these always is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapRefusal {
    /// Encoding version mismatch.
    VersionMismatch,
    /// Arena geometry does not match what the peer expects.
    GeometryMismatch,
    /// The epoch does not match.
    EpochMismatch,
    /// The peer's identity is not the expected one.
    UnexpectedPeer,
    /// The region's self-description is invalid (magic, format, seals).
    CorruptRegion,
}

impl BootstrapRefusal {
    /// The human-readable form carried in the refusal's detail.
    pub fn as_str(self) -> &'static str {
        match self {
            BootstrapRefusal::VersionMismatch => "version mismatch",
            BootstrapRefusal::GeometryMismatch => "geometry mismatch",
            BootstrapRefusal::EpochMismatch => "epoch mismatch",
            BootstrapRefusal::UnexpectedPeer => "unexpected peer",
            BootstrapRefusal::CorruptRegion => "corrupt region",
        }
    }
}

/// One bootstrap message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapMessage {
    /// Owner → Peer: the arena's self-description. The region itself rides
    /// the link's handle channel (SCM_RIGHTS) when the link has one.
    Offer {
        /// Bootstrap encoding version.
        version: u32,
        /// The arena's epoch (unique per arena; derives the namespace root).
        epoch: u32,
        /// The arena geometry the peer must match exactly.
        config: ArenaConfig,
        /// First `T.BufferId` the arena resolves.
        namespace_root: u64,
    },
    /// Peer → Owner: the peer validated everything and binds the namespace.
    Accept {
        /// The accepted arena's epoch.
        epoch: u32,
        /// The accepted namespace root.
        namespace_root: u64,
    },
    /// Either side: typed refusal; both sides stay inert.
    Refuse {
        /// The typed reason class.
        reason: BootstrapRefusal,
        /// The human detail (never a placeholder).
        detail: String,
    },
}

impl BootstrapMessage {
    /// Encode (fixed little-endian, version-tagged). Total: every message
    /// form round-trips, asserted in `tests/bootstrap.rs`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            BootstrapMessage::Offer {
                version,
                epoch,
                config,
                namespace_root,
            } => {
                out.push(MSG_OFFER);
                out.extend_from_slice(&version.to_le_bytes());
                out.extend_from_slice(&epoch.to_le_bytes());
                out.extend_from_slice(&config.slot_bytes.to_le_bytes());
                out.extend_from_slice(&config.slot_count.to_le_bytes());
                out.extend_from_slice(&config.ring_slots.to_le_bytes());
                out.extend_from_slice(&config.ring_bytes.to_le_bytes());
                out.extend_from_slice(&namespace_root.to_le_bytes());
            }
            BootstrapMessage::Accept {
                epoch,
                namespace_root,
            } => {
                out.push(MSG_ACCEPT);
                out.extend_from_slice(&epoch.to_le_bytes());
                out.extend_from_slice(&namespace_root.to_le_bytes());
            }
            BootstrapMessage::Refuse { reason, detail } => {
                out.push(MSG_REFUSE);
                out.push(*reason as u8);
                out.extend_from_slice(&(detail.len() as u16).to_le_bytes());
                out.extend_from_slice(detail.as_bytes());
            }
        }
        out
    }

    /// Decode. Unknown tags are a refusal-grade error, never a panic.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransportError> {
        let (&tag, rest) = bytes.split_first().ok_or_else(|| TransportError::Framing {
            detail: "empty bootstrap message".to_string(),
        })?;
        let u32_at = |at: usize| -> Result<u32, TransportError> {
            rest.get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| TransportError::Framing {
                    detail: "bootstrap message truncated".to_string(),
                })
        };
        let u64_at = |at: usize| -> Result<u64, TransportError> {
            rest.get(at..at + 8)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| TransportError::Framing {
                    detail: "bootstrap message truncated".to_string(),
                })
        };
        match tag {
            MSG_OFFER => Ok(BootstrapMessage::Offer {
                version: u32_at(0)?,
                epoch: u32_at(4)?,
                config: ArenaConfig {
                    slot_bytes: u32_at(8)?,
                    slot_count: u32_at(12)?,
                    ring_slots: u32_at(16)?,
                    ring_bytes: u32_at(20)?,
                },
                namespace_root: u64_at(24)?,
            }),
            MSG_ACCEPT => Ok(BootstrapMessage::Accept {
                epoch: u32_at(0)?,
                namespace_root: u64_at(4)?,
            }),
            MSG_REFUSE => {
                let (&reason, rest) = rest.split_first().ok_or_else(|| TransportError::Framing {
                    detail: "refuse message truncated".to_string(),
                })?;
                let reason = match reason {
                    0 => BootstrapRefusal::VersionMismatch,
                    1 => BootstrapRefusal::GeometryMismatch,
                    2 => BootstrapRefusal::EpochMismatch,
                    3 => BootstrapRefusal::UnexpectedPeer,
                    4 => BootstrapRefusal::CorruptRegion,
                    _ => {
                        return Err(TransportError::Framing {
                            detail: alloc::format!("unknown refusal reason {reason}"),
                        })
                    }
                };
                let len = rest
                    .get(0..2)
                    .map(|b| u16::from_le_bytes(b.try_into().unwrap()) as usize)
                    .ok_or_else(|| TransportError::Framing {
                        detail: "refuse detail length missing".to_string(),
                    })?;
                let detail = core::str::from_utf8(
                    rest.get(2..2 + len).ok_or_else(|| TransportError::Framing {
                        detail: "refuse detail truncated".to_string(),
                    })?,
                )
                .map_err(|_| TransportError::Framing {
                    detail: "refuse detail is not utf-8".to_string(),
                })?
                .to_string();
                Ok(BootstrapMessage::Refuse { reason, detail })
            }
            _ => Err(TransportError::Framing {
                detail: alloc::format!("unknown bootstrap tag {tag}"),
            }),
        }
    }
}

/// The minimal link the bootstrap needs: identity, one message, and one
/// region handle per message.
pub trait BootstrapLink: Send + Sync {
    /// The peer's identity, where the link can produce one. This is what
    /// the offer is authenticated against.
    fn peer_credentials(&self) -> Option<PeerCredentials>;

    /// Send a message; `region` is `Some` when the message carries a region
    /// handle (the link passes it however the host allows — SCM_RIGHTS on
    /// Linux).
    fn send(
        &self,
        message: &BootstrapMessage,
        region: Option<&RegionPass>,
    ) -> Result<(), TransportError>;

    /// Receive a message and the region handle that came with it.
    fn recv(&self) -> Result<(BootstrapMessage, Option<RegionPass>), TransportError>;
}

/// What the owner requires of the peer before offering the arena.
#[derive(Clone, Copy, Debug, Default)]
pub struct PeerExpectation {
    /// Expected peer pid (`None` = any).
    pub pid: Option<i32>,
    /// Expected peer uid (`None` = any).
    pub uid: Option<u32>,
    /// Expected peer gid (`None` = any).
    pub gid: Option<u32>,
}

impl PeerExpectation {
    /// `None` when the expectation is empty — which means "no identity
    /// requirement", a weakened posture that must be a conscious choice.
    pub fn is_empty(&self) -> bool {
        self.pid.is_none() && self.uid.is_none() && self.gid.is_none()
    }

    fn accepts(&self, creds: &PeerCredentials) -> bool {
        self.pid.map_or(true, |p| p == creds.pid)
            && self.uid.map_or(true, |u| u == creds.uid)
            && self.gid.map_or(true, |g| g == creds.gid)
    }
}

/// Wait for a bootstrap verdict, tolerating a link that answers `Empty`
/// (the mock does; real links block in the kernel).
///
/// A verdict is mandatory in both directions, so waiting for one is the
/// protocol's own requirement — but the wait is bounded, because a verdict
/// that never arrives is itself an answer and must not spin forever.
fn recv_verdict(
    link: &dyn BootstrapLink,
) -> Result<(BootstrapMessage, Option<RegionPass>), TransportError> {
    const ATTEMPTS: u32 = 100_000;
    for _ in 0..ATTEMPTS {
        match link.recv() {
            Err(TransportError::Empty) => core::hint::spin_loop(),
            other => return other,
        }
    }
    Err(TransportError::message(
        "the bootstrap verdict never arrived: the exchange timed out",
    ))
}

/// Run one side of the bootstrap.
///
/// * `Role::Owner` — create (or be handed) the arena, offer it, and on the
///   peer's `Accept` establish it.
/// * `Role::Peer` — receive the offer, authenticate the sender and the
///   region, and accept.
///
/// On either side's refusal both arenas stay inert.
pub fn run_bootstrap(
    link: &dyn BootstrapLink,
    role: Role,
    arena: Option<&Arc<Arena>>,
    expectation: &PeerExpectation,
) -> Result<ShmemHandle, TransportError> {
    match role {
        Role::Owner => owner_side(link, arena, expectation),
        Role::Peer => peer_side(link, expectation),
    }
}

/// Which side of the exchange this call runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The arena's creator: offers the region, then establishes on Accept.
    Owner,
    /// The invited peer: validates the offer and accepts.
    Peer,
}

fn owner_side(
    link: &dyn BootstrapLink,
    arena: Option<&Arc<Arena>>,
    expectation: &PeerExpectation,
) -> Result<ShmemHandle, TransportError> {
    let arena = arena.ok_or_else(|| TransportError::ArenaNotEstablished)?;
    // 1. Authenticate the peer before anything valuable moves.
    if !expectation.is_empty() {
        let creds = link.peer_credentials().ok_or_else(|| TransportError::UnsupportedCapability {
            kind: "peer identity".to_string(),
            detail: "the bootstrap link cannot identify the peer, so the \
                     expected identity cannot be checked"
                .to_string(),
        })?;
        if !expectation.accepts(&creds) {
            return Err(TransportError::UnsupportedCapability {
                kind: "peer identity".to_string(),
                detail: alloc::format!(
                    "peer {creds:?} does not satisfy the expectation {expectation:?}"
                ),
            });
        }
    }
    // 2. Assign the namespace and offer the region.
    let namespace_root = namespace_root_for(arena.epoch());
    arena.set_namespace_root(namespace_root);
    let offer = BootstrapMessage::Offer {
        version: BOOTSTRAP_VERSION,
        epoch: arena.epoch(),
        config: arena.config(),
        namespace_root,
    };
    let region = match arena.region_handle() {
        Some(fd) => Some(RegionPass::OsHandle(fd)),
        // The heap backing cannot cross a process boundary; the in-process
        // form (Arc) is the honest one for that case.
        None => Some(RegionPass::Shared(arena.clone())),
    };
    link.send(&offer, region.as_ref())?;
    // 3. Wait for the verdict.
    let (response, _) = recv_verdict(link)?;
    match response {
        BootstrapMessage::Accept {
            epoch,
            namespace_root: accepted_root,
        } => {
            if epoch != arena.epoch() || accepted_root != namespace_root {
                return Err(TransportError::Framing {
                    detail: "accept does not match the offered arena".to_string(),
                });
            }
            // 4. Both sides can use the arena now.
            arena.establish();
            Ok(ShmemHandle::new(
                arena.clone(),
                Direction::A,
                link.peer_credentials(),
            ))
        }
        BootstrapMessage::Refuse { reason, detail } => Err(TransportError::Framing {
            detail: alloc::format!("bootstrap refused by peer: {} ({detail})", reason.as_str()),
        }),
        _ => Err(TransportError::Framing {
            detail: "peer answered an offer with a non-verdict".to_string(),
        }),
    }
}

fn peer_side(
    link: &dyn BootstrapLink,
    expectation: &PeerExpectation,
) -> Result<ShmemHandle, TransportError> {
    let (offer, region) = recv_verdict(link)?;
    let BootstrapMessage::Offer {
        version,
        epoch,
        config,
        namespace_root,
    } = offer
    else {
        return Err(TransportError::Framing {
            detail: "expected an offer".to_string(),
        });
    };
    let refuse = |reason: BootstrapRefusal, detail: String| -> TransportError {
        let msg = BootstrapMessage::Refuse {
            reason,
            detail: detail.clone(),
        };
        let _ = link.send(&msg, None);
        TransportError::Framing {
            detail: alloc::format!("bootstrap refused: {} ({detail})", reason.as_str()),
        }
    };
    if version != BOOTSTRAP_VERSION {
        return Err(refuse(
            BootstrapRefusal::VersionMismatch,
            alloc::format!("offer speaks {version}, this build speaks {BOOTSTRAP_VERSION}"),
        ));
    }
    // The peer authenticates the *sender* it expected, exactly as the owner
    // does: symmetric, once, before the region is trusted.
    if !expectation.is_empty() {
        match link.peer_credentials() {
            Some(creds) if expectation.accepts(&creds) => {}
            Some(creds) => {
                return Err(refuse(
                    BootstrapRefusal::UnexpectedPeer,
                    alloc::format!("peer {creds:?} does not satisfy {expectation:?}"),
                ))
            }
            None => {
                return Err(refuse(
                    BootstrapRefusal::UnexpectedPeer,
                    "link cannot identify the sender".to_string(),
                ))
            }
        }
    }
    let region = region.ok_or_else(|| {
        refuse(
            BootstrapRefusal::CorruptRegion,
            "offer carried no region handle".to_string(),
        )
    })?;
    let arena = Arena::adopt(&region, &config, epoch).map_err(|e| {
        refuse(
            BootstrapRefusal::CorruptRegion,
            alloc::format!("region rejected: {e}"),
        )
    })?;
    // The peer's own verification is complete (identity, region, geometry,
    // epoch), so it publishes its Accept and marks its view established.
    // The owner independently establishes after receiving this Accept; the
    // word is shared, so both sides converge, and no refusal path ever
    // touches it.
    link.send(
        &BootstrapMessage::Accept {
            epoch,
            namespace_root,
        },
        None,
    )?;
    arena.establish();
    Ok(ShmemHandle::new(arena, Direction::B, link.peer_credentials()))
}

/// The namespace root for an arena with `epoch`: `epoch << 32`. Because
/// `Arena::create` mints a fresh epoch per arena (a process-global
/// counter), two arenas can never share a namespace, so a foreign
/// `BufferId` is structurally out of range rather than accidentally valid.
pub fn namespace_root_for(epoch: u32) -> u64 {
    (epoch as u64) << 32
}


// ─── links ──────────────────────────────────────────────────────────────

/// The production bootstrap link: a raw `AF_UNIX` socket. The region handle
/// rides the message's `SCM_RIGHTS` ancillary channel, which is the
/// capability transfer this whole protocol exists to perform.
pub struct UnixBootstrapLink {
    /// The connected `AF_UNIX` descriptor.
    fd: i32,
    /// The peer's identity, from `SO_PEERCRED` when the host has it.
    peer: Option<PeerCredentials>,
}

impl UnixBootstrapLink {
    /// A connected pair over `socketpair` (tests and local peers).
    pub fn pair() -> Result<(Self, Self), TransportError> {
        let (a, b) = crate::transport::sys::unix_socketpair()?;
        Ok((Self::from_fd(a), Self::from_fd(b)))
    }

    /// Connect to a path (client side of the bootstrap).
    pub fn connect(path: &str) -> Result<Self, TransportError> {
        let fd = crate::transport::sys::unix_socket()?;
        if let Err(e) = crate::transport::sys::unix_connect(fd, path) {
            crate::transport::sys::close_fd(fd);
            return Err(e);
        }
        Ok(Self::from_fd(fd))
    }

    /// Wrap an accepted/connected socket.
    pub fn from_fd(fd: i32) -> Self {
        let peer = crate::transport::sys::peer_credential_triple(fd)
            .ok()
            .map(|(pid, uid, gid)| PeerCredentials { pid, uid, gid });
        Self { fd, peer }
    }

    fn send_raw(&self, bytes: &[u8], fd: Option<i32>) -> Result<(), TransportError> {
        let mut framed = Vec::with_capacity(4 + bytes.len());
        framed.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        framed.extend_from_slice(bytes);
        let fd_storage;
        let first_fds: &[i32] = match &fd {
            Some(f) => {
                fd_storage = *f;
                core::slice::from_ref(&fd_storage)
            }
            None => &[],
        };
        let mut written = 0usize;
        let mut first = true;
        while written < framed.len() {
            // The ancillary handle rides the first sendmsg, which carries
            // the frame's first bytes — exactly how the kernel associates
            // ancillary data with a stream.
            let fds = if first { first_fds } else { &[] };
            let n = crate::transport::sys::send_with_fds(self.fd, &framed[written..], fds)?;
            if n == 0 {
                return Err(TransportError::Host {
                    errno: 0,
                    detail: "sendmsg wrote 0 bytes".to_string(),
                });
            }
            written += n;
            first = false;
        }
        Ok(())
    }

    fn recv_raw(&self) -> Result<(Vec<u8>, Option<i32>), TransportError> {
        let mut len_buf = [0u8; 4];
        let (n, fds) = crate::transport::sys::recv_with_fds(self.fd, &mut len_buf)?;
        if n == 0 {
            return Err(TransportError::Closed {
                reason: crate::transport::channel::CloseReason::PeerClosed,
            });
        }
        if n != 4 {
            return Err(TransportError::Framing {
                detail: "short bootstrap length read".to_string(),
            });
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 4096 {
            return Err(TransportError::Framing {
                detail: alloc::format!("bootstrap message length {len} out of range"),
            });
        }
        let mut buf = alloc::vec![0u8; len];
        let mut read = 0usize;
        let mut extra = Vec::new();
        while read < len {
            let (n, more) = crate::transport::sys::recv_with_fds(self.fd, &mut buf[read..])?;
            if n == 0 {
                return Err(TransportError::Closed {
                    reason: crate::transport::channel::CloseReason::PeerClosed,
                });
            }
            read += n;
            extra.extend(more);
        }
        let fd = fds.into_iter().next().or_else(|| extra.into_iter().next());
        Ok((buf, fd))
    }
}

impl BootstrapLink for UnixBootstrapLink {
    fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.peer
    }

    fn send(
        &self,
        message: &BootstrapMessage,
        region: Option<&RegionPass>,
    ) -> Result<(), TransportError> {
        let fd = match region {
            Some(RegionPass::OsHandle(handle)) => Some(handle.0 as i32),
            // The Shared form exists for the in-process test double; over a
            // real socket it cannot be expressed, and pretending otherwise
            // would be a fake transfer.
            Some(RegionPass::Shared(_)) => {
                return Err(TransportError::UnsupportedCapability {
                    kind: "region".to_string(),
                    detail: "an in-memory arena cannot cross a socket".to_string(),
                })
            }
            None => None,
        };
        self.send_raw(&message.encode(), fd)
    }

    fn recv(&self) -> Result<(BootstrapMessage, Option<RegionPass>), TransportError> {
        let (bytes, fd) = self.recv_raw()?;
        let message = BootstrapMessage::decode(&bytes)?;
        let region = fd.map(|fd| RegionPass::OsHandle(OsHandle(fd as u64)));
        Ok((message, region))
    }
}

/// The mock link: an in-memory duplex with synthetic peer credentials.
///
/// It is a test double and is labelled as one: it moves *messages* and the
/// `RegionPass::Shared` form, and it deliberately cannot manufacture an
/// `OsHandle` — kernel-guaranteed transfers are proven against the real
/// `UnixBootstrapLink` in `tests/unix.rs`.
///
/// `inbox` is what this endpoint's `recv` reads; `outbox` is the peer's
/// inbox, which is where this endpoint's `send` lands.
pub struct MockLink {
    /// What this endpoint's `recv` reads.
    inbox: Arc<Mutex<VecDeque<(BootstrapMessage, Option<RegionPass>)>>>,
    /// The peer's inbox, which is where this endpoint's `send` lands.
    outbox: Arc<Mutex<VecDeque<(BootstrapMessage, Option<RegionPass>)>>>,
    /// Synthetic peer identity for the test double.
    peer: Option<PeerCredentials>,
}

use crate::transport::sync::Mutex;
use alloc::collections::VecDeque;

impl MockLink {
    /// A connected pair: `a.send` lands in `b`'s inbox and vice versa.
    pub fn pair(creds_a: PeerCredentials, creds_b: PeerCredentials) -> (Self, Self) {
        let inbox_a: Arc<Mutex<VecDeque<(BootstrapMessage, Option<RegionPass>)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let inbox_b = Arc::new(Mutex::new(VecDeque::new()));
        (
            MockLink {
                inbox: inbox_a.clone(),
                outbox: inbox_b.clone(),
                peer: Some(creds_b),
            },
            MockLink {
                inbox: inbox_b,
                outbox: inbox_a,
                peer: Some(creds_a),
            },
        )
    }

}

impl MockLink {
    /// A pair where neither side can identify the other (the "link cannot
    /// identify the peer" refusal path).
    pub fn blind_pair() -> (Self, Self) {
        let inbox_a: Arc<Mutex<VecDeque<(BootstrapMessage, Option<RegionPass>)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let inbox_b = Arc::new(Mutex::new(VecDeque::new()));
        (
            MockLink {
                inbox: inbox_a.clone(),
                outbox: inbox_b.clone(),
                peer: None,
            },
            MockLink {
                inbox: inbox_b,
                outbox: inbox_a,
                peer: None,
            },
        )
    }
}

impl BootstrapLink for MockLink {
    fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.peer
    }

    fn send(
        &self,
        message: &BootstrapMessage,
        region: Option<&RegionPass>,
    ) -> Result<(), TransportError> {
        self.outbox
            .lock()
            .push_back((message.clone(), region.cloned()));
        Ok(())
    }

    /// The mock never blocks: an empty mailbox is an honest `Empty`, which
    /// is what makes the refusal tests deterministic (a real link blocks in
    /// the kernel instead).
    fn recv(&self) -> Result<(BootstrapMessage, Option<RegionPass>), TransportError> {
        self.inbox
            .lock()
            .pop_front()
            .ok_or_else(|| TransportError::Empty)
    }
}
