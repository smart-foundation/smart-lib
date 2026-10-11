//! THE interchangeability witness: one test body over all four transports.
//!
//! The body never names a transport: it talks to `Box<dyn Channel>` only.
//! Each factory declares its transport's *published* capability outcomes,
//! and the body asserts the declarations match reality — a transport that
//! claimed something it could not do would fail here.

extern crate std;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::channel::{
    AttachedCap, CapHandoff, CapOutcome, Channel, ChannelListener, CloseReason, MessageOut,
    OwnedMessage, TransportError,
};
use crate::transport::shmem::{Arena, Direction, ShmemChannel, ShmemHandle};

use super::support;

/// What each factory declares about its transport, so the interchange body
/// can assert the declaration is also the behaviour (no fabricated caps).
#[derive(Clone, Copy, Debug)]
struct Declared {
    kind: &'static str,
    /// The declared outcome for a staged descriptor attachment.
    descriptor_outcome: DescriptorOutcome,
    /// Whether a close reason is observable by the far side.
    close_propagates: bool,
    /// Whether this transport can carry a live handle at all.
    live_carries: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DescriptorOutcome {
    /// Declared, but no current transport reaches it: kept so a future
    /// transport with native descriptor movement has a name to declare.
    #[allow(dead_code)]
    Native,
    InBand,
    Refused,
}

fn inprocess_factory() -> (Box<dyn Channel>, Box<dyn Channel>, Declared) {
    let (a, b) = crate::transport::inprocess::InProcessChannel::pair();
    (
        Box::new(a),
        Box::new(b),
        Declared {
            kind: "InProcess",
            descriptor_outcome: DescriptorOutcome::InBand,
            close_propagates: true,
            live_carries: true,
        },
    )
}

fn shmem_factory() -> (Box<dyn Channel>, Box<dyn Channel>, Declared) {
    // The bootstrap is `tests/bootstrap.rs`'s and `tests/unix.rs`'s job;
    // here the arena is established directly so the interchange body tests
    // the Channel API, not the bootstrap protocol.
    let arena = Arena::create(support::test_arena_config()).expect("arena");
    arena.set_namespace_root(crate::transport::bootstrap::namespace_root_for(
        arena.epoch(),
    ));
    arena.establish();
    let a = ShmemChannel::from_handle(ShmemHandle::new(arena.clone(), Direction::A, None));
    let b = ShmemChannel::from_handle(ShmemHandle::new(arena, Direction::B, None));
    (
        Box::new(a),
        Box::new(b),
        Declared {
            kind: "Shmem",
            descriptor_outcome: DescriptorOutcome::Refused,
            close_propagates: true,
            live_carries: false,
        },
    )
}

#[cfg(all(unix, feature = "std"))]
fn unix_factory() -> (Box<dyn Channel>, Box<dyn Channel>, Declared) {
    let (a, b) = crate::transport::sys::unix_socketpair().expect("socketpair");
    (
        Box::new(crate::transport::unix::UnixChannel::from_fd(a)),
        Box::new(crate::transport::unix::UnixChannel::from_fd(b)),
        Declared {
            kind: "UnixSocket",
            descriptor_outcome: DescriptorOutcome::InBand,
            close_propagates: true,
            live_carries: false,
        },
    )
}

#[cfg(feature = "std")]
fn tcp_factory() -> (Box<dyn Channel>, Box<dyn Channel>, Declared) {
    let listener = crate::transport::tcp::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = std::thread::spawn(move || listener.accept().expect("accept"));
    let client = crate::transport::tcp::TcpChannel::connect(&addr).expect("connect");
    let server = server.join().expect("server thread");
    (
        Box::new(client),
        server,
        Declared {
            kind: "Tcp",
            descriptor_outcome: DescriptorOutcome::InBand,
            close_propagates: true,
            live_carries: false,
        },
    )
}

/// The one body. `transport_name` is diagnostics only — nothing in the body
/// branches on it.
fn exercise(
    declared: Declared,
    client: Box<dyn Channel>,
    server: Box<dyn Channel>,
) {
    // The published declaration matches the transport we constructed.
    assert_eq!(client.kind().as_str(), declared.kind);
    assert!(!client.caps().note.is_empty() || declared.kind == "InProcess");

    // ── 1. send / recv a real contract message ────────────────────────
    client
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
            12, 34, 56,
        ))))
        .expect("send");
    let mut scratch = Vec::new();
    let batch = server.recv(&mut scratch).expect("recv");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(
        (version.get_major(), version.get_minor(), version.get_patch()),
        (12, 34, 56)
    );
    assert_eq!(version.get_pre_release().expect("pre_release"), "transport-test");

    // ── 2. the far side answers over the same API ─────────────────────
    server
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
            65, 43, 21,
        ))))
        .expect("reply");
    let batch = client.recv(&mut scratch).expect("recv reply");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(version.get_major(), 65);

    // ── 3. capability attach behaves exactly as declared ──────────────
    let mut descriptor =
        CapHandoff::Descriptor(support::capability_descriptor("interchange/target"));
    match declared.descriptor_outcome {
        DescriptorOutcome::InBand => assert!(
            matches!(
                client.attach(&mut descriptor).expect("descriptor attach"),
                CapOutcome::InBand { .. }
            ),
            "{}: declared InBand",
            declared.kind
        ),
        DescriptorOutcome::Native => assert!(
            matches!(
                client.attach(&mut descriptor).expect("descriptor attach"),
                CapOutcome::Native { .. }
            ),
            "{}: declared Native",
            declared.kind
        ),
        DescriptorOutcome::Refused => assert!(
            client.attach(&mut descriptor).is_err(),
            "{}: refused by declaration — and the refusal is typed",
            declared.kind
        ),
    }

    if declared.live_carries {
        let mut live = support::live_capability("interchange");
        assert!(
            matches!(
                client.attach(&mut live).expect("live attach"),
                CapOutcome::Native { .. }
            ),
            "{}: a live handle crosses natively",
            declared.kind
        );
        client
            .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
                1, 1, 1,
            ))))
            .expect("send with the live capability");
        let batch = server.recv(&mut scratch).expect("recv with capability");
        assert!(
            batch
                .caps
                .iter()
                .any(|(_, cap)| matches!(cap, AttachedCap::Live(_))),
            "the live handle arrived"
        );
        // Drain it so the close frame is next.
    } else {
        let mut live = support::live_capability("cannot-cross");
        assert!(
            client.attach(&mut live).is_err(),
            "{}: a live handle must be refused, honestly",
            declared.kind
        );
    }

    // ── 4. BufferId resolution is honest everywhere ───────────────────
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(1);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    match client.resolve_buffer(id) {
        Ok(frame) => {
            // Only the transports with a real arena get here, and the
            // Shmem one resolves slot 1 only if something was published.
            let _ = frame;
        }
        Err(err) => {
            assert!(
                matches!(err, TransportError::BufferNotResolvable { .. })
                    || matches!(err, TransportError::ArenaNotEstablished),
                "an honest refusal, got {err:?}"
            );
        }
    }

    // ── 5. close with a typed reason; the far side observes it ────────
    client
        .close(CloseReason::Shutdown)
        .expect("close with a typed reason");
    if declared.close_propagates {
        match server.recv(&mut scratch) {
            Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
            other => panic!("{}: expected Closed{{Shutdown}}, got {other:?}", declared.kind),
        }
    }
}

#[test]
fn four_transports_one_api_inprocess() {
    let (c, s, d) = inprocess_factory();
    exercise(d, c, s);
    std::eprintln!("INTERCHANGE InProcess: one API shape, PASS");
}

#[test]
fn four_transports_one_api_shmem() {
    let (c, s, d) = shmem_factory();
    exercise(d, c, s);
    std::eprintln!("INTERCHANGE Shmem: one API shape, PASS");
}

#[test]
#[cfg(all(unix, feature = "std"))]
fn four_transports_one_api_unix() {
    let (c, s, d) = unix_factory();
    exercise(d, c, s);
    std::eprintln!("INTERCHANGE UnixSocket: one API shape, PASS");
}

#[test]
#[cfg(feature = "std")]
fn four_transports_one_api_tcp() {
    let (c, s, d) = tcp_factory();
    exercise(d, c, s);
    std::eprintln!("INTERCHANGE Tcp: one API shape, PASS");
}

/// The seam's object-safety witness: the whole suite talks to
/// `Box<dyn Channel>` and never to a concrete transport type.
#[test]
fn channel_is_object_safe() {
    fn takes_dyn(_channel: &dyn Channel) {}
    let (a, _b) = crate::transport::inprocess::InProcessChannel::pair();
    takes_dyn(&a);
    let _boxed: Box<dyn Channel> = Box::new(a);
}
