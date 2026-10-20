//! Tcp: stream framing, and the honest-degradation story the conformance
//! model requires (DEGRADED/UNAVAILABLE with a reason, never a fake
//! capability).

extern crate std;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::channel::{
    CapabilityTransfer, CapHandoff, CapOutcome, Channel, ChannelListener, CloseReason, MessageOut,
    OwnedMessage, TransportError,
};
use crate::transport::tcp::{TcpChannel, TcpListener};

use super::support;

/// A connected pair over loopback (the mock peer for every Tcp test).
fn pair() -> (TcpChannel, Box<dyn Channel>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server = std::thread::spawn(move || listener.accept().expect("accept"));
    let client = TcpChannel::connect(&addr).expect("connect");
    let server: Box<dyn Channel> = server.join().expect("server thread");
    (client, server)
}

#[test]
fn round_trip_and_close_reason() {
    let (a, b) = pair();
    a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        6, 6, 6,
    ))))
    .expect("send");
    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(version.get_major(), 6);

    Box::new(a).close(CloseReason::Shutdown).expect("close");
    match b.recv(&mut scratch) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
        other => panic!("expected Closed{{Shutdown}}, got {other:?}"),
    }
}

/// THE Tcp honesty test: what is lost here is *declared*, not faked.
///
/// Asserts every clause of `02-HOST-CAPABILITIES.md` §4.2's Tcp row:
/// in-band tokens only, no OS handles, no peer identity, no BufferId
/// namespace.
#[test]
fn honest_degradation_is_declared_and_mechanical() {
    let (a, b) = pair();
    let caps = a.caps();

    // 1. The declared capability-transfer semantics: in-band token only.
    assert_eq!(caps.scheme, "tcp");
    assert_eq!(
        caps.capability_transfer,
        CapabilityTransfer::InBandToken,
        "TCP has no fd transfer: the ceiling is an in-band descriptor"
    );

    // 2. No zero-copy of any kind.
    assert!(!caps.zero_copy_message);
    assert!(
        !caps.zero_copy_data_handle,
        "no shared BufferId namespace exists between two hosts"
    );

    // 3. No peer identity: `peer_credentials()` is None, declared.
    assert!(!caps.peer_identity);
    assert!(a.peer_credentials().is_none());
    assert!(b.peer_credentials().is_none());

    // 4. A live handle cannot cross a network boundary.
    let mut live = support::live_capability("cannot-cross");
    match a.attach(&mut live) {
        Err(TransportError::UnsupportedCapability { kind, detail }) => {
            assert_eq!(kind, "live handle");
            assert!(detail.contains("network boundary"));
        }
        other => panic!("expected UnsupportedCapability, got {other:?}"),
    }

    // 5. An OS handle cannot cross either — the honest gap named exactly.
    let mut handle = CapHandoff::OsHandle(crate::transport::channel::OsHandle(7));
    match a.attach(&mut handle) {
        Err(TransportError::UnsupportedCapability { kind, detail }) => {
            assert_eq!(kind, "os handle");
            assert!(
                detail.contains("no fd transfer") || detail.contains("SCM_RIGHTS"),
                "the detail names the missing mechanism: {detail}"
            );
        }
        other => panic!("expected UnsupportedCapability, got {other:?}"),
    }

    // 6. A descriptor is the ceiling, and it says so: InBand.
    let mut descriptor =
        CapHandoff::Descriptor(support::capability_descriptor("network/interface/wlan0"));
    match a.attach(&mut descriptor).expect("descriptor attach") {
        CapOutcome::InBand { .. } => {}
        other => panic!("descriptors are InBand on Tcp, got {other:?}"),
    }

    // 7. No BufferId namespace: resolution is refused, never pretended.
    let mut message = capnp::message::Builder::new_default();
    message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(4242);
    let id = message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    match a.resolve_buffer(id) {
        Err(TransportError::BufferNotResolvable { id: 4242, detail }) => {
            assert!(
                detail.contains("BufferId namespace") || detail.contains("inline"),
                "the detail states the producer-side rule: {detail}"
            );
        }
        other => panic!("expected BufferNotResolvable, got {other:?}"),
    }
    let _ = b;
}

/// The producer-side rule made concrete: the contract's `T.DataHandle`
/// **inline** arm round-trips over Tcp, and a message that *would* need the
/// buffer arm is refused rather than silently mis-delivered. This is the
/// shape the domain layer follows after reading `caps()`.
#[test]
fn inline_data_handles_round_trip_and_buffer_arms_are_refused() {
    let (a, b) = pair();

    // Inline: the honest form below the contract's 1 KB threshold.
    let small = b"small payload, inline is correct here";
    a.send(MessageOut::Owned(OwnedMessage::new(
        support::inline_data_handle_message(small),
    )))
    .expect("send inline");
    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv");
    let handle = batch.root::<types_capnp::data_handle::Owned>().expect("root");
    match handle.which().expect("which") {
        types_capnp::data_handle::Inline(data) => {
            assert_eq!(data.expect("inline bytes"), small)
        }
        _ => panic!("the inline arm must have round-tripped"),
    }

    // A bulk frame that used the buffer arm would be unresolvable on the
    // far side: the transport refuses instead of delivering a dangling
    // reference.
    a.send(MessageOut::Owned(OwnedMessage::new(
        support::buffer_data_handle_message(99),
    )))
    .expect("send the reference");
    let batch = b.recv(&mut scratch).expect("recv the reference");
    let handle = batch.root::<types_capnp::data_handle::Owned>().expect("root");
    let buffer_id = match handle.which().expect("which") {
        types_capnp::data_handle::Buffer(id) => id.expect("buffer id reader"),
        _ => panic!("the buffer arm must have round-tripped as a reference"),
    };
    assert_eq!(buffer_id.get_id(), 99);
    assert!(
        matches!(
            b.resolve_buffer(buffer_id),
            Err(TransportError::BufferNotResolvable { .. })
        ),
        "a buffer arm over TCP is unresolvable, and the transport says so"
    );
}

/// The negotiate hook over TCP.
#[test]
fn negotiate_hook_round_trips_a_handshake() {
    let (a, b) = pair();
    let responder = std::thread::spawn(move || {
        let mut scratch = Vec::new();
        let batch = b.recv(&mut scratch).expect("recv the handshake");
        assert!(batch
            .root::<crate::schema::abi_capnp::abi_handshake_request::Owned>()
            .expect("root")
            .get_accept_downgrade());
        b.send(MessageOut::Owned(OwnedMessage::new(
            support::handshake_response_matched(),
        )))
        .expect("answer the handshake");
    });
    let response = a
        .negotiate(MessageOut::Owned(OwnedMessage::new(
            support::handshake_request(true),
        )))
        .expect("negotiate over TCP");
    assert!(matches!(
        response.get().expect("response").which().expect("which"),
        crate::schema::abi_capnp::abi_handshake_response::Matched(_)
    ));
    responder.join().expect("responder thread");
}
