//! UnixSocket: capnp messages over `AF_UNIX` with the contract's framing,
//! and **full** capability transfer where the host supports it.
//!
//! ## Capability transfer (the honest part)
//!
//! On Linux (`std` builds) this transport passes OS handles with
//! `SCM_RIGHTS`: the kernel re-opens the fd on the far side, which is a
//! genuine movement of authority — a passed memfd *is* a transfer of a
//! `BufferId` namespace (`02-HOST-CAPABILITIES.md` §4.2). The test
//! `tests/unix.rs::scm_rights_transfers_a_real_memfd` proves the far side
//! receives a *different* fd number that reads the same bytes.
//!
//! On non-Linux unix targets the stream framing still works, and the
//! capability transfer honestly reports [`CapabilityTransfer::None`] with a
//! note. macOS's SCM_RIGHTS needs a `cmsghdr` alignment variant that cannot
//! be compiled or verified on this machine, so it is deliberately not
//! shipped (see `sys.rs`'s header); `attach` refuses with a typed reason
//! instead of faking the transfer.
//!
//! ## Copies on the receive path (declared, measured in the report)
//!
//! kernel → read scratch: 1; scratch → capnp `OwnedSegments` words: 1.
//! Inherent to crossing a kernel boundary; the zero-copy transports are
//! InProcess (message) and Shmem (`DataHandle.buffer`).

use alloc::string::ToString;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::schema::types_capnp;
use crate::transport::channel::{
    frame, AttachedCap, CapHandoff, CapId, CapOutcome, Channel, CloseReason, FrameRef,
    MessageOut, NegotiateResponse, OsHandle, PeerCredentials, RecvBatch, RecvMessage, RegionNotice,
    TransportCaps, TransportError, TransportKind,
};
use crate::transport::shmem::{Arena, ArenaConfig};
use crate::transport::sync::Mutex;
use crate::transport::sys;

enum StagedCap {
    Descriptor(Vec<u8>),
    /// A bare OS handle: the kernel re-opens it on the far side. No notice
    /// travels, so the far side receives exactly the handle (its meaning is
    /// the receiver's business, e.g. a memfd it was told about in-band).
    OsHandle { fd: i32 },
    Region { fd: i32, notice: RegionNotice },
}

enum ReceivedCap {
    Descriptor(Vec<u8>),
    OsHandle { fd: i32 },
    /// A region attachment: the long-lived mapping lives in `regions`
    /// (keyed by the same id) so `resolve_buffer` keeps working after the
    /// capability has been handed to the caller in a batch.
    Region { region: CapId },
}

#[derive(Default)]
struct UnixState {
    staged: Vec<(CapId, StagedCap)>,
    received: BTreeMap<CapId, ReceivedCap>,
    /// Adopted region mappings, owned by the channel: a `BufferId`
    /// namespace survives for the channel's lifetime, not one batch.
    regions: Vec<(CapId, Arc<Arena>)>,
    next_id: u64,
}

/// A `Channel` over a connected `AF_UNIX` stream socket.
pub struct UnixChannel {
    fd: i32,
    state: Mutex<UnixState>,
    write_scratch: Mutex<Vec<u8>>,
    closed: AtomicBool,
    peer: Option<PeerCredentials>,
}

/// Ancillary support compiled in for this target?
#[cfg(all(target_os = "linux", feature = "std"))]
const ANCILLARY: bool = true;
#[cfg(not(all(target_os = "linux", feature = "std")))]
const ANCILLARY: bool = false;

fn unix_caps() -> TransportCaps {
    let (transfer, note) = if ANCILLARY {
        (
            crate::transport::channel::CapabilityTransfer::OsHandle,
            "SCM_RIGHTS passes fds per frame on Linux (verified on this \
             box); a passed fd is a genuine capability transfer because the \
             kernel re-opened it; a passed memfd transfers a BufferId \
             namespace; receive is two declared copies (kernel, then capnp \
             parse)",
        )
    } else {
        (
            crate::transport::channel::CapabilityTransfer::None,
            "this target has no compiled ancillary-data path: capability \
             transfer is unavailable here, declared rather than faked \
             (macOS needs a cmsghdr-alignment variant; see sys.rs)",
        )
    };
    TransportCaps {
        kind: TransportKind::UnixSocket,
        framing: "length-prefixed capnp flat messages over SOCK_STREAM",
        capability_transfer: transfer,
        zero_copy_message: false,
        // True only once a region has been attached; the declared answer
        // for a fresh channel is false (no namespace yet).
        zero_copy_data_handle: false,
        peer_identity: true,
        blocking: true,
        max_frame_bytes: frame::MAX_FRAME_BYTES,
        note,
    }
}

impl UnixChannel {
    /// Wrap a connected socket.
    pub fn from_fd(fd: i32) -> Self {
        let peer = sys::peer_credential_triple(fd)
            .ok()
            .map(|(pid, uid, gid)| PeerCredentials { pid, uid, gid });
        Self {
            fd,
            state: Mutex::new(UnixState::default()),
            write_scratch: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            peer,
        }
    }

    /// Connect to a path (a leading NUL byte denotes the Linux abstract
    /// namespace).
    /// Connect to a path.
    pub fn connect(path: &str) -> Result<Self, TransportError> {
        let fd = sys::unix_socket()?;
        if let Err(e) = sys::unix_connect(fd, path) {
            sys::close_fd(fd);
            return Err(e);
        }
        Ok(Self::from_fd(fd))
    }

    /// Write one frame, optionally passing `fds` in its ancillary channel.
    /// The frame is assembled in the channel's reusable scratch (no
    /// per-send allocation after the first).
    fn send_frame(&self, kind: u8, payload: &[u8], fds: &[i32]) -> Result<(), TransportError> {
        let mut scratch = self.write_scratch.lock();
        scratch.clear();
        let total = 1 + payload.len() as u32;
        scratch.reserve(4 + total as usize);
        scratch.extend_from_slice(&total.to_be_bytes());
        scratch.push(kind);
        scratch.extend_from_slice(payload);
        let mut written = 0usize;
        while written < scratch.len() {
            let n = sys::send_with_fds(self.fd, &scratch[written..], fds)?;
            if n == 0 {
                return Err(TransportError::Closed {
                    reason: CloseReason::Transport {
                        detail: "sendmsg wrote 0 bytes".to_string(),
                    },
                });
            }
            written += n;
        }
        Ok(())
    }

    /// Send a capnp message frame (the flat message is the payload).
    ///
    /// The length prefix counts *kind + payload*, never itself: the reader
    /// consumes the 4 length bytes first and then exactly that many.
    fn send_message_frame(&self, kind: u8, message: &MessageOut<'_>) -> Result<(), TransportError> {
        let mut scratch = self.write_scratch.lock();
        scratch.clear();
        scratch.extend_from_slice(&[0u8; 4]);
        scratch.push(kind);
        frame::write_flat_message(&mut *scratch, &MessageSegments(message))?;
        let total = (scratch.len() - 4) as u32;
        scratch[0..4].copy_from_slice(&total.to_be_bytes());
        let mut written = 0usize;
        while written < scratch.len() {
            let n = sys::send_with_fds(self.fd, &scratch[written..], &[])?;
            if n == 0 {
                return Err(TransportError::Closed {
                    reason: CloseReason::Transport {
                        detail: "sendmsg wrote 0 bytes".to_string(),
                    },
                });
            }
            written += n;
        }
        Ok(())
    }

    /// Read one frame into `scratch`; returns `(kind, fds)`. Ancillary fds
    /// are collected from every `recvmsg` of the frame, because a partial
    /// read can split where the kernel attaches them.
    fn read_frame_into(&self, scratch: &mut Vec<u8>) -> Result<(u8, Vec<i32>), TransportError> {
        let mut len_buf = [0u8; 4];
        let (n, mut fds) = sys::recv_with_fds(self.fd, &mut len_buf)?;
        if n == 0 {
            return Err(TransportError::Closed {
                reason: CloseReason::PeerClosed,
            });
        }
        if n != 4 {
            return Err(TransportError::Framing {
                detail: alloc::format!("short frame length read: {n} bytes"),
            });
        }
        let total = u32::from_be_bytes(len_buf);
        if total == 0 || total > frame::MAX_FRAME_BYTES {
            return Err(TransportError::Framing {
                detail: alloc::format!("declared frame length {total} out of range"),
            });
        }
        scratch.resize(total as usize, 0);
        let mut read = 0usize;
        while read < total as usize {
            let (n, more) = sys::recv_with_fds(self.fd, &mut scratch[read..])?;
            if n == 0 {
                return Err(TransportError::Closed {
                    reason: CloseReason::PeerClosed,
                });
            }
            read += n;
            fds.extend(more);
        }
        Ok((scratch[0], fds))
    }

    fn register_received_cap(
        state: &mut UnixState,
        fds: &[i32],
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if payload.len() < 11 {
            return Err(TransportError::Framing {
                detail: "attach payload too short".to_string(),
            });
        }
        let cap_kind = payload[0];
        let id = CapId(u64::from_be_bytes(payload[1..9].try_into().unwrap()));
        let data_len = u16::from_be_bytes([payload[9], payload[10]]) as usize;
        if payload.len() < 11 + data_len {
            return Err(TransportError::Framing {
                detail: "attach data length exceeds payload".to_string(),
            });
        }
        let data = &payload[11..11 + data_len];
        let received = match cap_kind {
            frame::CAP_KIND_DESCRIPTOR => ReceivedCap::Descriptor(data.to_vec()),
            frame::CAP_KIND_OS_HANDLE => {
                let fd = *fds.first().ok_or_else(|| TransportError::Framing {
                    detail: "os-handle attach arrived without its fd: the transfer \
                             did not happen, so nothing is recorded"
                        .to_string(),
                })?;
                ReceivedCap::OsHandle { fd }
            }
            frame::CAP_KIND_REGION => {
                let notice = frame::decode_region_notice(data)?;
                let fd = *fds.first().ok_or_else(|| TransportError::Framing {
                    detail: "region notice arrived without its fd: the transfer \
                             did not happen, so nothing is recorded"
                        .to_string(),
                })?;
                let config = ArenaConfig {
                    slot_bytes: notice.slot_bytes,
                    slot_count: notice.slot_count,
                    ring_slots: notice.ring_slots,
                    ring_bytes: notice.ring_bytes,
                };
                let arena = Arena::adopt(
                    &crate::transport::bootstrap::RegionPass::OsHandle(OsHandle(fd as u64)),
                    &config,
                    notice.epoch,
                )?;
                // The mapping is long-lived (the channel owns it through
                // `regions`); the capability handed to the caller only
                // references it, so a BufferId namespace survives the batch.
                state.regions.push((id, arena));
                ReceivedCap::Region { region: id }
            }
            _ => {
                return Err(TransportError::Framing {
                    detail: alloc::format!("unknown attach kind {cap_kind}"),
                })
            }
        };
        state.received.insert(id, received);
        Ok(())
    }
}

struct MessageSegments<'a>(&'a MessageOut<'a>);

impl capnp::message::ReaderSegments for MessageSegments<'_> {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        self.0.get_segment(id)
    }
}

impl Channel for UnixChannel {
    fn kind(&self) -> TransportKind {
        TransportKind::UnixSocket
    }

    fn caps(&self) -> TransportCaps {
        unix_caps()
    }

    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::AlreadyClosed);
        }
        let caps = core::mem::take(&mut self.state.lock().staged);
        // Attachments precede the data frame they belong to, so the far
        // side's bounded pull returns the message with its capabilities.
        for (id, cap) in caps {
            match cap {
                StagedCap::Descriptor(bytes) => {
                    let payload = frame::encode_attach(frame::CAP_KIND_DESCRIPTOR, id, &bytes);
                    self.send_frame(frame::KIND_ATTACH, &payload, &[])?;
                }
                StagedCap::OsHandle { fd } => {
                    let payload = frame::encode_attach(frame::CAP_KIND_OS_HANDLE, id, &[]);
                    self.send_frame(frame::KIND_ATTACH, &payload, &[fd])?;
                }
                StagedCap::Region { fd, notice } => {
                    let data = frame::encode_region_notice(&notice);
                    let payload = frame::encode_attach(frame::CAP_KIND_REGION, id, &data);
                    self.send_frame(frame::KIND_ATTACH, &payload, &[fd])?;
                }
            }
        }
        self.send_message_frame(frame::KIND_DATA, &message)?;
        Ok(())
    }

    fn recv(&self, scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError> {
        loop {
            let (kind, fds) = self.read_frame_into(scratch)?;
            let payload = &scratch[1..];
            match kind {
                frame::KIND_DATA | frame::KIND_NEGOTIATE => {
                    let reader = capnp::serialize::read_message(
                        &mut &payload[..],
                        capnp::message::ReaderOptions::new(),
                    )
                    .map_err(|e| TransportError::message(e.to_string()))?;
                    // Hand the caller every capability that arrived with
                    // this frame; region *mappings* stay owned by the
                    // channel (only the notice is handed over).
                    let mut caps = Vec::new();
                    let mut state = self.state.lock();
                    for (id, received) in core::mem::take(&mut state.received) {
                        let attached = match received {
                            ReceivedCap::Region { region } => {
                                let (_, arena) = state
                                    .regions
                                    .iter()
                                    .find(|(cap_id, _)| *cap_id == region)
                                    .map(|(id, arena)| (*id, arena.clone()))
                                    .ok_or_else(|| TransportError::Framing {
                                        detail: "region mapping vanished".to_string(),
                                    })?;
                                AttachedCap::Region {
                                    fd: OsHandle(0),
                                    notice: RegionNotice {
                                        namespace_root: arena.namespace_root(),
                                        slot_count: arena.config().slot_count,
                                        slot_bytes: arena.config().slot_bytes,
                                        ring_slots: arena.config().ring_slots,
                                        ring_bytes: arena.config().ring_bytes,
                                        epoch: arena.epoch(),
                                    },
                                }
                            }
                            other => other.into_attached(),
                        };
                        caps.push((id, attached));
                    }
                    return Ok(RecvBatch {
                        message: RecvMessage::Owned(reader),
                        caps,
                    });
                }
                frame::KIND_CLOSE => {
                    let reason = frame::decode_close(payload)?;
                    return Err(TransportError::Closed { reason });
                }
                frame::KIND_ATTACH => {
                    let mut state = self.state.lock();
                    Self::register_received_cap(&mut state, &fds, payload)?;
                    // An ATTACH frame is not a message: keep pulling. The
                    // capability surfaces with the next data frame.
                    continue;
                }
                frame::KIND_DETACH => {
                    let id = detach_id(payload)?;
                    self.state.lock().received.remove(&id);
                    continue;
                }
                _ => {
                    return Err(TransportError::Framing {
                        detail: alloc::format!("unknown frame kind {kind}"),
                    })
                }
            }
        }
    }

    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError> {
        let id = {
            let mut state = self.state.lock();
            state.next_id += 1;
            CapId(state.next_id)
        };
        let staged = match cap {
            CapHandoff::Descriptor(bytes) => StagedCap::Descriptor(bytes.clone()),
            CapHandoff::OsHandle(fd) => {
                if !ANCILLARY {
                    return Err(TransportError::UnsupportedCapability {
                        kind: "os handle".to_string(),
                        detail: "no ancillary-data path on this target".to_string(),
                    });
                }
                StagedCap::OsHandle { fd: fd.0 as i32 }
            }
            CapHandoff::Region { fd, notice } => {
                if !ANCILLARY {
                    return Err(TransportError::UnsupportedCapability {
                        kind: "region".to_string(),
                        detail: "no ancillary-data path on this target".to_string(),
                    });
                }
                StagedCap::Region {
                    fd: fd.0 as i32,
                    notice: *notice,
                }
            }
            CapHandoff::Live(_) => {
                return Err(TransportError::UnsupportedCapability {
                    kind: "live handle".to_string(),
                    detail: "a Rust object cannot cross a kernel boundary; \
                     pass a descriptor (in-band) or an OS handle"
                        .to_string(),
                })
            }
        };
        self.state.lock().staged.push((id, staged));
        if ANCILLARY && matches!(cap, CapHandoff::OsHandle(_) | CapHandoff::Region { .. }) {
            Ok(CapOutcome::Native { id })
        } else {
            Ok(CapOutcome::InBand { id })
        }
    }

    fn detach(&self, id: CapId) -> Result<(), TransportError> {
        let removed = {
            let mut state = self.state.lock();
            let before = state.staged.len();
            state.staged.retain(|(staged_id, _)| *staged_id != id);
            state.staged.len() != before
        };
        if removed {
            self.send_frame(frame::KIND_DETACH, &id.0.to_be_bytes(), &[])?;
            Ok(())
        } else {
            Err(TransportError::message("no such staged capability"))
        }
    }

    fn resolve_buffer(
        &self,
        id: types_capnp::buffer_id::Reader<'_>,
    ) -> Result<FrameRef<'_>, TransportError> {
        let raw = id.get_id();
        let state = self.state.lock();
        for (_, arena) in &state.regions {
            let root = arena.namespace_root();
            if raw >= root && raw < root + arena.config().slot_count as u64 {
                let (ptr, len) = arena.published_payload_ptr(raw)?;
                // SAFETY: `ptr`/`len` describe a published slot inside a
                // mapping this channel owns (through the arena, alive for
                // as long as the borrow of `self` the return type carries).
                // The same publication protocol as `Arena::resolve` applies.
                return Ok(FrameRef::new(unsafe { core::slice::from_raw_parts(ptr, len) }));
            }
        }
        drop(state);
        Err(TransportError::BufferNotResolvable {
            id: raw,
            detail: "no attached region declares this BufferId namespace".to_string(),
        })
    }

    fn negotiate(&self, request: MessageOut<'_>) -> Result<NegotiateResponse, TransportError> {
        self.send_message_frame(frame::KIND_NEGOTIATE, &request)?;
        let mut scratch = Vec::new();
        let (kind, _fds) = self.read_frame_into(&mut scratch)?;
        // The answer may arrive tagged either way: both kinds carry one
        // capnp message, and only the request needs the negotiate tag.
        if kind != frame::KIND_NEGOTIATE && kind != frame::KIND_DATA {
            return Err(TransportError::Framing {
                detail: alloc::format!("expected a handshake frame, got kind {kind}"),
            });
        }
        let reader = capnp::serialize::read_message(
            &mut &scratch[1..],
            capnp::message::ReaderOptions::new(),
        )
        .map_err(|e| TransportError::message(e.to_string()))?;
        Ok(capnp::message::TypedReader::new(reader))
    }

    fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.peer
    }

    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(TransportError::AlreadyClosed);
        }
        let payload = frame::encode_close(&reason);
        let _ = self.send_frame(frame::KIND_CLOSE, &payload, &[]);
        sys::close_fd(self.fd);
        Ok(())
    }
}

impl ReceivedCap {
    /// The caller-facing form. The `Region` arm never reaches here: the
    /// recv path resolves it against the channel's long-lived mappings.
    fn into_attached(self) -> AttachedCap {
        match self {
            ReceivedCap::Descriptor(bytes) => AttachedCap::Descriptor(bytes),
            ReceivedCap::OsHandle { fd } => AttachedCap::OsHandle {
                fd: OsHandle(fd as u64),
            },
            ReceivedCap::Region { .. } => AttachedCap::Descriptor(Vec::new()),
        }
    }
}

fn detach_id(payload: &[u8]) -> Result<CapId, TransportError> {
    if payload.len() != 8 {
        return Err(TransportError::Framing {
            detail: "detach payload must be 8 bytes".to_string(),
        });
    }
    Ok(CapId(u64::from_be_bytes(payload[0..8].try_into().unwrap())))
}

/// A server-side listener.
pub struct UnixListener {
    fd: i32,
    path: String,
}

impl UnixListener {
    /// Bind and listen on a path.
    pub fn bind(path: &str) -> Result<Self, TransportError> {
        let fd = sys::unix_socket()?;
        if let Err(e) = sys::unix_bind_listen(fd, path) {
            sys::close_fd(fd);
            return Err(e);
        }
        Ok(Self {
            fd,
            path: path.to_string(),
        })
    }
}

impl crate::transport::channel::ChannelListener for UnixListener {
    fn kind(&self) -> TransportKind {
        TransportKind::UnixSocket
    }

    fn accept(&self) -> Result<Box<dyn Channel>, TransportError> {
        let fd = sys::unix_accept(self.fd)?;
        Ok(Box::new(UnixChannel::from_fd(fd)))
    }
}

impl Drop for UnixListener {
    fn drop(&mut self) {
        sys::close_fd(self.fd);
        // A path socket must not outlive its listener.
        if !self.path.starts_with('\0') {
            if let Ok(cpath) = alloc::ffi::CString::new(self.path.as_str()) {
                // SAFETY: NUL-terminated borrow of a live allocation.
                unsafe { sys::unlink_path(cpath.as_ptr()) };
            }
        }
    }
}
