//! Tcp: capnp stream framing, and the honest-capability story.
//!
//! ## What is lost here, exactly (declared, never faked)
//!
//! `02-HOST-CAPABILITIES.md` §4.2, enforced by `caps()` and the tests in
//! `tests/tcp.rs`:
//!
//! 1. **No capability transfer exists over TCP.** There is no fd passing,
//!    no shared object, no peer identity (`SO_PEERCRED` is a unix-socket
//!    mechanism). The ceiling is: a `Capability` *descriptor*
//!    (`capability.capnp:56`) travels in-band, and the receiving side must
//!    verify it per call against a registered issuer; where the peer is
//!    unknown, a backend may only mint **attenuated, time-boxed** handles
//!    or refuse (02 §3.1 step 1).
//! 2. **No `BufferId` namespace.** `T.DataHandle.buffer`
//!    (`types.capnp:502`) cannot be honoured: there is no shared arena, so
//!    a reference would be unresolvable on the far side. The producer-side
//!    rule is `inline` below the contract's 1 KB threshold or a declared
//!    copy above it — decided at call time by the domain layer reading
//!    `caps().zero_copy_data_handle == false`. This transport does not
//!    re-walk messages to catch violations; doing so would cost a full
//!    traversal per message and buy nothing the caps declaration does not
//!    already state. `resolve_buffer` is the mechanical half: it always
//!    answers `BufferNotResolvable` here.
//! 3. **No zero-copy at all.** Frame write and read are each one copy, on
//!    top of the kernel's.
//!
//! ## Relation to `tunnel.capnp`
//!
//! The overlay transport in the contract carries `T.DataHandle` frames with
//! its own `maxFrameSize`/`more` contract and an open-token security
//! policy. This transport is the *carrier* such a tunnel rides on, and
//! deliberately does not re-implement tunnel semantics: framing here is
//! the length-prefixed stream (`tunnel.capnp`'s frames are payloads), and
//! a frame larger than `frame::MAX_FRAME_BYTES` is rejected with
//! `limitExceeded` — never silently truncated, never silently accepted.

use alloc::string::ToString;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

// The crate is `no_std` (charter law #1); this module is the std-gated
// seam, so it links `std` for itself rather than asking the crate root to
// (the root is the skeleton agent's file).
#[cfg(feature = "std")]
extern crate std;

use crate::transport::channel::{
    frame, ByteReader, ByteStream, ByteWriter, CapHandoff, CapId, CapOutcome, Channel, CloseReason,
    FrameRef, MessageOut, NegotiateResponse, PeerCredentials, RecvBatch, RecvMessage, TransportCaps,
    TransportError,
};
use crate::transport::sync::Mutex;

/// A `Channel` over a `TcpStream`.
pub struct TcpChannel {
    stream: std::net::TcpStream,
    write_scratch: Mutex<Vec<u8>>,
    closed: AtomicBool,
}

fn tcp_caps() -> TransportCaps {
    TransportCaps {
        scheme: crate::transport::registry::TCP_SCHEME,
        framing: "length-prefixed capnp flat messages over TCP",
        capability_transfer: crate::transport::channel::CapabilityTransfer::InBandToken,
        zero_copy_message: false,
        zero_copy_data_handle: false,
        peer_identity: false,
        blocking: true,
        max_frame_bytes: frame::MAX_FRAME_BYTES,
        note: "no fd transfer and no peer identity exist over TCP; \
               capability descriptors travel in-band only and must be \
               verified per call by the capability service; DataHandle.buffer \
               has no BufferId namespace here, so the producer must choose \
               inline (below 1KB) or a declared copy; this transport never \
               walks messages to fake the check",
    }
}

impl TcpChannel {
    /// Connect to `host:port`.
    pub fn connect(addr: &str) -> Result<Self, TransportError> {
        let stream = std::net::TcpStream::connect(addr).map_err(|e| TransportError::Connect {
            detail: alloc::format!("TcpStream::connect({addr}): {e}"),
        })?;
        Ok(Self::from_stream(stream))
    }

    /// Wrap a connected stream.
    pub fn from_stream(stream: std::net::TcpStream) -> Self {
        Self {
            stream,
            write_scratch: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }

    fn send_frame(&self, kind: u8, payload: &[u8]) -> Result<(), TransportError> {
        use std::io::Write;
        let mut scratch = self.write_scratch.lock();
        scratch.clear();
        let total = 1 + payload.len() as u32;
        scratch.extend_from_slice(&total.to_be_bytes());
        scratch.push(kind);
        scratch.extend_from_slice(payload);
        (&self.stream)
            .write_all(&scratch)
            .map_err(|e| TransportError::Host {
                errno: e.raw_os_error().unwrap_or(0),
                detail: alloc::format!("tcp write: {e}"),
            })
    }

    /// Read one frame into `scratch`; returns its kind.
    fn read_frame_into(&self, scratch: &mut Vec<u8>) -> Result<u8, TransportError> {
        use std::io::Read;
        let mut len_buf = [0u8; 4];
        (&self.stream)
            .read_exact(&mut len_buf)
            .map_err(|e| TcpIoError::into_transport(e))?;
        let total = u32::from_be_bytes(len_buf);
        if total == 0 || total > frame::MAX_FRAME_BYTES {
            return Err(TransportError::Framing {
                detail: alloc::format!("declared frame length {total} out of range"),
            });
        }
        scratch.resize(total as usize, 0);
        (&self.stream)
            .read_exact(scratch)
            .map_err(|e| TcpIoError::into_transport(e))?;
        Ok(scratch[0])
    }
}

impl Channel for TcpChannel {
    fn scheme(&self) -> &'static str {
        crate::transport::registry::TCP_SCHEME
    }

    fn caps(&self) -> TransportCaps {
        tcp_caps()
    }

    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::AlreadyClosed);
        }
        let mut scratch = self.write_scratch.lock();
        scratch.clear();
        scratch.extend_from_slice(&[0u8; 4]);
        scratch.push(frame::KIND_DATA);
        frame::write_flat_message(&mut *scratch, &MessageSegments(&message))?;
        let total = (scratch.len() - 4) as u32;
        scratch[0..4].copy_from_slice(&total.to_be_bytes());
        use std::io::Write;
        (&self.stream)
            .write_all(&scratch)
            .map_err(|e| TransportError::Host {
                errno: e.raw_os_error().unwrap_or(0),
                detail: alloc::format!("tcp write: {e}"),
            })
    }

    fn recv(&self, scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError> {
        let kind = self.read_frame_into(scratch)?;
        match kind {
            frame::KIND_DATA | frame::KIND_NEGOTIATE => {
                let reader = capnp::serialize::read_message(
                    &mut &scratch[1..],
                    capnp::message::ReaderOptions::new(),
                )
                .map_err(|e| TransportError::message(e.to_string()))?;
                Ok(RecvBatch {
                    message: RecvMessage::Owned(reader),
                    caps: Vec::new(),
                })
            }
            frame::KIND_CLOSE => {
                let reason = frame::decode_close(&scratch[1..])?;
                Err(TransportError::Closed { reason })
            }
            _ => Err(TransportError::Framing {
                detail: alloc::format!("unexpected frame kind {kind} on Tcp"),
            }),
        }
    }

    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError> {
        // In-band descriptors are the only thing TCP can carry; the value
        // is handed back untouched on every refusal.
        match cap {
            CapHandoff::Descriptor(_) => {
                let id = CapId(1);
                Ok(CapOutcome::InBand { id })
            }
            CapHandoff::Live(_) => Err(TransportError::UnsupportedCapability {
                kind: "live handle".to_string(),
                detail: "no object crosses a network boundary".to_string(),
            }),
            CapHandoff::OsHandle(_) => Err(TransportError::UnsupportedCapability {
                kind: "os handle".to_string(),
                detail: "TCP has no fd transfer: nothing equivalent to \
                         SCM_RIGHTS exists"
                    .to_string(),
            }),
            CapHandoff::Region { .. } => Err(TransportError::UnsupportedCapability {
                kind: "region".to_string(),
                detail: "a BufferId namespace requires shared memory, which \
                         two TCP hosts do not have"
                    .to_string(),
            }),
        }
    }

    fn detach(&self, _id: CapId) -> Result<(), TransportError> {
        // Nothing was staged, so there is nothing to detach; answering
        // success silently would pretend the descriptor left.
        Err(TransportError::message(
            "Tcp stages no capabilities: descriptors travel inside messages",
        ))
    }

    fn resolve_buffer(
        &self,
        id: crate::schema::types_capnp::buffer_id::Reader<'_>,
    ) -> Result<FrameRef<'_>, TransportError> {
        Err(TransportError::BufferNotResolvable {
            id: id.get_id(),
            detail: "TCP shares no BufferId namespace; DataHandle.buffer \
                     cannot be honoured and the producer must have chosen \
                     inline or a declared copy"
                .to_string(),
        })
    }

    fn negotiate(&self, request: MessageOut<'_>) -> Result<NegotiateResponse, TransportError> {
        let mut scratch = self.write_scratch.lock();
        scratch.clear();
        scratch.extend_from_slice(&[0u8; 4]);
        scratch.push(frame::KIND_NEGOTIATE);
        frame::write_flat_message(&mut *scratch, &MessageSegments(&request))?;
        let total = (scratch.len() - 4) as u32;
        scratch[0..4].copy_from_slice(&total.to_be_bytes());
        {
            use std::io::Write;
            (&self.stream)
                .write_all(&scratch)
                .map_err(|e| TransportError::Host {
                    errno: e.raw_os_error().unwrap_or(0),
                    detail: alloc::format!("tcp write: {e}"),
                })?;
        }
        let kind = self.read_frame_into(&mut scratch)?;
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
        // Declared: no peer identity exists over TCP.
        None
    }

    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(TransportError::AlreadyClosed);
        }
        let payload = frame::encode_close(&reason);
        let _ = self.send_frame(frame::KIND_CLOSE, &payload);
        Ok(())
    }

    /// The stream's raw bytes, for a protocol that owns its own framing.
    ///
    /// The socket moves out of the channel: this layer's length-prefixed
    /// framing is not used on this path, and the two never interleave.
    fn into_byte_stream(self: Box<Self>) -> Result<Box<dyn ByteStream>, TransportError> {
        let channel = *self;
        Ok(Box::new(TcpByteStream {
            stream: channel.stream,
        }))
    }
}

/// The `tcp` scheme's byte stream: the socket's own bytes.
pub struct TcpByteStream {
    stream: std::net::TcpStream,
}

impl ByteReader for TcpByteStream {
    fn read(&self, buf: &mut [u8]) -> Result<usize, TransportError> {
        use std::io::Read;
        (&self.stream).read(buf).map_err(TcpIoError::into_transport)
    }
}

impl ByteWriter for TcpByteStream {
    fn write_all(&self, buf: &[u8]) -> Result<(), TransportError> {
        use std::io::Write;
        (&self.stream)
            .write_all(buf)
            .map_err(TcpIoError::into_transport)
    }

    fn flush(&self) -> Result<(), TransportError> {
        use std::io::Write;
        (&self.stream).flush().map_err(TcpIoError::into_transport)
    }
}

impl ByteStream for TcpByteStream {
    fn close(&self) {
        use std::net::Shutdown;
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// The `tcp` transport, registered under its open scheme token.
pub struct TcpTransport;

impl crate::transport::registry::TransportImpl for TcpTransport {
    fn scheme(&self) -> &'static str {
        crate::transport::registry::TCP_SCHEME
    }

    fn description(&self) -> crate::transport::registry::TransportDescription {
        crate::transport::registry::TransportDescription {
            scheme: crate::transport::registry::TCP_SCHEME,
            framing: "length-prefixed capnp flat messages over TCP",
            capability_transfer: crate::transport::channel::CapabilityTransfer::InBandToken,
            byte_stream: true,
            default_address: None,
            note: "no fd transfer and no peer identity exist over TCP; \
                   capability descriptors travel in-band only; the raw byte \
                   stream is available for a foreign protocol (capnp RPC)",
        }
    }

    fn connect(
        &self,
        request: &crate::transport::registry::TransportRequest,
    ) -> Result<Box<dyn Channel>, TransportError> {
        Ok(Box::new(TcpChannel::connect(&request.address)?))
    }

    fn listen(
        &self,
        request: &crate::transport::registry::TransportRequest,
    ) -> Result<Box<dyn crate::transport::channel::ChannelListener>, TransportError> {
        Ok(Box::new(TcpListener::bind(&request.address)?))
    }
}

struct MessageSegments<'a>(&'a MessageOut<'a>);

impl capnp::message::ReaderSegments for MessageSegments<'_> {
    fn get_segment(&self, id: u32) -> Option<&[u8]> {
        self.0.get_segment(id)
    }
}

/// A server-side listener.
pub struct TcpListener {
    listener: std::net::TcpListener,
}

impl TcpListener {
    /// Bind and listen on `host:port`.
    pub fn bind(addr: &str) -> Result<Self, TransportError> {
        let listener = std::net::TcpListener::bind(addr).map_err(|e| TransportError::Connect {
            detail: alloc::format!("TcpListener::bind({addr}): {e}"),
        })?;
        Ok(Self { listener })
    }
}

impl TcpListener {
    /// The bound address, as a dialable `host:port` string.
    pub fn local_addr(&self) -> Result<String, TransportError> {
        self.listener
            .local_addr()
            .map(|a| a.to_string())
            .map_err(|e| TransportError::Connect {
                detail: alloc::format!("local_addr: {e}"),
            })
    }
}

impl crate::transport::channel::ChannelListener for TcpListener {
    fn scheme(&self) -> &'static str {
        crate::transport::registry::TCP_SCHEME
    }

    fn accept(&self) -> Result<Box<dyn Channel>, TransportError> {
        let (stream, _peer) = self
            .listener
            .accept()
            .map_err(|e| TransportError::Host {
                errno: e.raw_os_error().unwrap_or(0),
                detail: alloc::format!("tcp accept: {e}"),
            })?;
        Ok(Box::new(TcpChannel::from_stream(stream)))
    }
}

/// Local helper: an IO error is either "peer closed" (clean EOF at a frame
/// boundary) or a host failure. Both are honest; neither is a panic.
trait TcpIoError {
    fn into_transport(self) -> TransportError;
}

impl TcpIoError for std::io::Error {
    fn into_transport(self) -> TransportError {
        match self.kind() {
            std::io::ErrorKind::UnexpectedEof => TransportError::Closed {
                reason: CloseReason::PeerClosed,
            },
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted => {
                TransportError::Closed {
                    reason: CloseReason::Transport {
                        detail: self.to_string(),
                    },
                }
            }
            _ => TransportError::Host {
                errno: self.raw_os_error().unwrap_or(0),
                detail: self.to_string(),
            },
        }
    }
}
