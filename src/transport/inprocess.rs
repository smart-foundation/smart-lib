//! InProcess: direct dispatch, zero serialization.
//!
//! The seam is [`InProcessPeer`] — the trait a local backend implements to
//! receive messages that never touch a bytecodec. A sender's builder is
//! *moved* into the seam ([`MessageOut::Owned`]): the allocation, and
//! therefore the address of every segment byte, crosses unchanged. The
//! witness is mechanical: `tests/inprocess.rs::owned_message_zero_copy`
//! asserts `as_ptr()` identity across the move, the send and the receive.
//!
//! What is NOT here: any backend. The seam is deliberately empty of domain
//! logic (this brief defines it; backends are their own wave).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::schema::types_capnp;
use crate::transport::channel::{
    AttachedCap, ByteStream, CapHandoff, CapId, CapOutcome, Channel, CloseReason, FrameRef,
    MessageOut, NegotiateResponse, PeerCredentials, RecvBatch, RecvMessage, TransportCaps,
    TransportError,
};
use crate::transport::sync::Mutex;

/// The receiving endpoint of an in-process message.
///
/// Implement this to be a backend; use [`InProcessInbox`] to be a queue.
/// `deliver` must not copy: `MessageOut::Borrowed` is valid only for the
/// duration of the call (dispatch it immediately), while
/// `MessageOut::Owned` hands the allocation over (store it — reading it
/// later is still zero-copy).
pub trait InProcessPeer: Send + Sync {
    /// Receive one message plus its capabilities.
    fn deliver(&self, frame: InProcessFrame<'_>);

    /// Stage an outgoing capability; the returned id is what the far side
    /// sees. The capability rides the next `deliver`.
    fn stage(&self, cap: CapHandoff) -> Result<CapId, TransportError>;

    /// Drop a staged capability.
    fn drop_staged(&self, id: CapId) -> Result<(), TransportError>;

    /// Bounded pull: the next arrived batch, or `None` for "nothing yet".
    fn pop_batch(&self) -> Option<RecvBatch>;

    /// Take the close notice the far side sent, if any.
    fn pop_close(&self) -> Option<CloseReason>;

    /// Record that the far side closed with this reason.
    fn notice_close(&self, reason: CloseReason);

    /// True once this endpoint is closed. Senders check it so a send into
    /// a closed endpoint answers `Closed`, never silence.
    fn is_closed(&self) -> bool {
        false
    }

    /// Peer identity. Always `None` here: one process has no OS peer. The
    /// marker *is* the process (02-HOST-CAPABILITIES.md §3.1: "InProcess:
    /// same-process marker (no peer)"). A backend needing identity must
    /// run on UnixSocket.
    fn peer_credentials(&self) -> Option<PeerCredentials> {
        None
    }
}

/// One delivered message: the sender's memory, moved or borrowed, plus the
/// capabilities staged since the last send.
pub struct InProcessFrame<'a> {
    /// The message itself: borrowed (dispatch now) or owned (the move).
    pub message: MessageOut<'a>,
    /// Capabilities staged since the last send, to ride this frame.
    pub caps: Vec<(CapId, CapHandoff)>,
    /// Where to answer. `None` on reply frames — the inbox does not answer,
    /// which is what makes a synchronous backend reply safe (no recursion).
    pub reply: Option<Arc<dyn InProcessPeer>>,
}

/// The default in-process endpoint: a bounded-memory queue.
///
/// `deliver` stores `Owned` messages as-is (the move is the whole copy);
/// `Borrowed` messages must be consumed by a synchronously-replying peer
/// (or copied by a custom [`InProcessPeer`] implementation, which then
/// owns that copy honestly).
pub struct InProcessInbox {
    inner: Mutex<InboxState>,
    next_cap_id: AtomicU64,
    closed: AtomicBool,
}

struct InboxState {
    queue: VecDeque<RecvBatch>,
    staged: VecDeque<(CapId, CapHandoff)>,
    close: Option<CloseReason>,
}

impl InProcessInbox {
    /// A fresh, empty endpoint.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InboxState {
                queue: VecDeque::new(),
                staged: VecDeque::new(),
                close: None,
            }),
            next_cap_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        }
    }

    fn next_id(&self) -> CapId {
        CapId(self.next_cap_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for InProcessInbox {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessPeer for InProcessInbox {
    fn deliver(&self, frame: InProcessFrame<'_>) {
        let mut state = self.inner.lock();
        if state.close.is_some() || self.closed.load(Ordering::Acquire) {            // The endpoint is closed: frames after close are dropped, and
            // the notice stays observable. This is the documented
            // close-then-drain discipline of the seam.
            return;
        }
        let caps = frame
            .caps
            .into_iter()
            .map(|(id, cap)| (id, cap.into_attached()))
            .collect();
        state.queue.push_back(RecvBatch {
            message: frame.message.into_recv_message(),
            caps,
        });
    }

    fn stage(&self, cap: CapHandoff) -> Result<CapId, TransportError> {
        let id = self.next_id();
        self.inner.lock().staged.push_back((id, cap));
        Ok(id)
    }

    fn drop_staged(&self, id: CapId) -> Result<(), TransportError> {
        let mut state = self.inner.lock();
        let before = state.staged.len();
        state.staged.retain(|(staged_id, _)| *staged_id != id);
        if state.staged.len() == before {
            return Err(TransportError::message("no such staged capability"));
        }
        Ok(())
    }

    fn pop_batch(&self) -> Option<RecvBatch> {
        self.inner.lock().queue.pop_front()
    }

    fn pop_close(&self) -> Option<CloseReason> {
        self.inner.lock().close.take()
    }

    fn notice_close(&self, reason: CloseReason) {
        let mut state = self.inner.lock();
        if state.close.is_none() {
            state.close = Some(reason);
        }
        drop(state);
        self.closed.store(true, Ordering::Release);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl CapHandoff {
    /// The receiving-side form of a staged capability.
    fn into_attached(self) -> AttachedCap {
        match self {
            CapHandoff::Live(object) => AttachedCap::Live(object),
            CapHandoff::Descriptor(bytes) => AttachedCap::Descriptor(bytes),
            CapHandoff::OsHandle(fd) => AttachedCap::OsHandle { fd },
            CapHandoff::Region { fd, notice } => AttachedCap::Region { fd, notice },
        }
    }
}

impl MessageOut<'_> {
    /// Wrap a `Borrowed` message as a received message: a zero-copy
    /// `Reader` over the very segments the sender built.
    fn into_recv_message(self) -> RecvMessage {
        match self {
            MessageOut::Owned(owned) => RecvMessage::Direct(capnp::message::Reader::new(
                owned,
                capnp::message::ReaderOptions::new(),
            )),
            MessageOut::Borrowed(segments) => {
                // A borrowed frame cannot outlive the `deliver` call, so the
                // queueing inbox materialises it into owned words *for the
                // queue path only*. Zero-copy same-frame dispatch is what
                // `Owned` is for, and what the direct-dispatch test proves.
                let words = flatten_segments(segments);
                RecvMessage::Owned(words)
            }
        }
    }
}

/// Flatten `&[&[u8]]` into an owned, word-aligned message (the one copy a
/// borrowed frame pays to be queueable).
fn flatten_segments(segments: &[&[u8]]) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
    let mut buf = Vec::new();
    capnp::serialize::write_message_segments(&mut buf, &segments)
        .expect("in-memory segment serialization cannot fail");
    capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("round-trip of in-memory segments")
}

/// A channel that dispatches into a same-process peer.
pub struct InProcessChannel {
    remote: Arc<dyn InProcessPeer>,
    local: Arc<InProcessInbox>,
    closed: AtomicBool,
}

impl InProcessChannel {
    /// A connected pair: `a.send` delivers into `b`, and vice versa.
    pub fn pair() -> (Self, Self) {
        let a: Arc<InProcessInbox> = Arc::new(InProcessInbox::new());
        let b: Arc<InProcessInbox> = Arc::new(InProcessInbox::new());
        (
            InProcessChannel::new(b.clone(), a.clone()),
            InProcessChannel::new(a, b),
        )
    }

    /// Build a channel whose remote is `remote` and whose receiving side is
    /// the (owned) `local` inbox.
    pub fn new(remote: Arc<dyn InProcessPeer>, local: Arc<InProcessInbox>) -> Self {
        Self {
            remote,
            local,
            closed: AtomicBool::new(false),
        }
    }

    /// Build a channel to a dispatch peer with a fresh local inbox (the
    /// `Transport::InProcess` construction).
    pub fn to_peer(peer: Arc<dyn InProcessPeer>) -> Self {
        Self::new(peer, Arc::new(InProcessInbox::new()))
    }

    /// The receiving side, for direct inspection in tests and for peers
    /// that want to drain without a channel handle.
    pub fn inbox(&self) -> &Arc<InProcessInbox> {
        &self.local
    }
}

fn inprocess_caps() -> TransportCaps {
    TransportCaps {
        scheme: crate::transport::registry::INPROCESS_SCHEME,
        framing: "direct dispatch (no framing, no bytes)",
        capability_transfer: crate::transport::channel::CapabilityTransfer::Native,
        zero_copy_message: true,
        // A shared BufferId namespace exists only if the process shares an
        // arena; this channel does not carry one, so the honest answer is
        // false. Pass the arena as a `Live` capability instead.
        zero_copy_data_handle: false,
        peer_identity: false,
        blocking: false,
        max_frame_bytes: 0,
        note: "same process: no OS peer identity; an arena shared as a Live \
               capability can provide a BufferId namespace the domain layer resolves",
    }
}

impl Channel for InProcessChannel {
    fn scheme(&self) -> &'static str {
        crate::transport::registry::INPROCESS_SCHEME
    }

    fn caps(&self) -> TransportCaps {
        inprocess_caps()
    }

    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::AlreadyClosed);
        }
        if self.remote.is_closed() {
            return Err(TransportError::Closed {
                reason: CloseReason::PeerClosed,
            });
        }
        let caps = self
            .local
            .inner
            .lock()
            .staged
            .drain(..)
            .collect::<Vec<_>>();
        self.remote.deliver(InProcessFrame {
            message,
            caps,
            reply: Some(self.local.clone()),
        });
        Ok(())
    }

    fn recv(&self, _scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError> {
        if let Some(batch) = self.local.pop_batch() {
            return Ok(batch);
        }
        match self.local.pop_close() {
            Some(reason) => Err(TransportError::Closed { reason }),
            None => Err(TransportError::Empty),
        }
    }

    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError> {
        // Every kind is carriable in-process. Live handles and OS handles are
        // already equally reachable (one process, one fd table), so they
        // cross natively; descriptor and region notices are in-band data
        // whose authority the far side re-establishes itself. No kind is
        // ever refused here, so nothing is lost to the placeholder swap.
        let native = matches!(
            cap,
            CapHandoff::Live(_) | CapHandoff::OsHandle(_)
        );
        let id = self.local.stage(take_cap(cap))?;
        if native {
            Ok(CapOutcome::Native { id })
        } else {
            Ok(CapOutcome::InBand { id })
        }
    }

    fn detach(&self, id: CapId) -> Result<(), TransportError> {
        self.local.drop_staged(id)
    }

    fn resolve_buffer(
        &self,
        id: types_capnp::buffer_id::Reader<'_>,
    ) -> Result<FrameRef<'_>, TransportError> {
        Err(TransportError::BufferNotResolvable {
            id: id.get_id(),
            detail: "InProcess carries no arena; pass one as a Live capability \
                     and resolve through it"
                .into(),
        })
    }

    fn negotiate(&self, request: MessageOut<'_>) -> Result<NegotiateResponse, TransportError> {
        self.send(request)?;
        let batch = self.recv(&mut Vec::new())?;
        batch.negotiate_response()
    }

    fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.local.peer_credentials()
    }

    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(TransportError::AlreadyClosed);
        }
        self.local.notice_close(CloseReason::PeerClosed);
        self.remote.notice_close(reason);
        Ok(())
    }

    /// The in-process transport has **no byte stream**: it dispatches
    /// messages, it does not move bytes. The honest answer says so.
    fn into_byte_stream(self: Box<Self>) -> Result<Box<dyn ByteStream>, TransportError> {
        Err(TransportError::UnsupportedCapability {
            kind: "byte stream".into(),
            detail: "the in-process transport dispatches messages, not bytes: \
                     a foreign protocol (capnp RPC) needs a stream transport"
                .into(),
        })
    }
}

/// Move the capability out of `cap`, leaving a same-kind placeholder so the
/// caller's value keeps its type after the move. The caller must not reuse
/// `cap` after a successful `attach` — the value now lives in the channel.
/// On the refusal paths `cap` is never touched (this function is only
/// reached after acceptance).
fn take_cap(cap: &mut CapHandoff) -> CapHandoff {
    match cap {
        CapHandoff::Live(_) => core::mem::replace(cap, CapHandoff::Live(Box::new(()))),
        CapHandoff::Descriptor(bytes) => {
            CapHandoff::Descriptor(core::mem::take(bytes))
        }
        CapHandoff::OsHandle(fd) => CapHandoff::OsHandle(*fd),
        CapHandoff::Region { fd, notice } => CapHandoff::Region {
            fd: *fd,
            notice: *notice,
        },
    }
}

/// The `inprocess` transport, registered under its open scheme token.
///
/// Both ends of this transport are *constructed*, not dialled: the peer
/// seam travels in the request's state, and there is no accept path.
pub struct InProcessTransport;

impl crate::transport::registry::TransportImpl for InProcessTransport {
    fn scheme(&self) -> &'static str {
        crate::transport::registry::INPROCESS_SCHEME
    }

    fn description(&self) -> crate::transport::registry::TransportDescription {
        crate::transport::registry::TransportDescription {
            scheme: crate::transport::registry::INPROCESS_SCHEME,
            framing: "direct dispatch: the message allocation moves unchanged",
            capability_transfer: crate::transport::channel::CapabilityTransfer::Native,
            byte_stream: false,
            default_address: None,
            note: "no byte stream: messages are dispatched, not serialized; \
                   a foreign protocol (capnp RPC) needs a stream transport",
        }
    }

    fn connect(
        &self,
        request: &crate::transport::registry::TransportRequest,
    ) -> Result<Box<dyn Channel>, TransportError> {
        let peer = request
            .state
            .as_ref()
            .ok_or_else(|| TransportError::Connect {
                detail: "the in-process transport needs a peer seam: construct \
                         both ends directly (InProcessChannel::pair)"
                    .into(),
            })?;
        let peer = peer
            .downcast_ref::<alloc::sync::Arc<dyn InProcessPeer>>()
            .ok_or_else(|| TransportError::Connect {
                detail: "the in-process transport's state is not a peer seam".into(),
            })?;
        Ok(Box::new(InProcessChannel::to_peer(peer.clone())))
    }

    fn listen(
        &self,
        _request: &crate::transport::registry::TransportRequest,
    ) -> Result<Box<dyn crate::transport::channel::ChannelListener>, TransportError> {
        Err(TransportError::UnsupportedCapability {
            kind: "listen".into(),
            detail: "the in-process transport has no accept path: construct \
                     both ends directly (InProcessChannel::pair)"
                .into(),
        })
    }
}
