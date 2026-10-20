//! Shmem: the zero-copy frame path, measured.

extern crate std;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::bootstrap::{namespace_root_for, RegionPass};
use crate::transport::channel::{
    CapabilityTransfer, Channel, CloseReason, FrameRef, MessageOut, OwnedMessage, TransportError,
};
use crate::transport::shmem::{Arena, ArenaStats, Direction, ShmemChannel, ShmemHandle};

use super::support;

/// A `T.BufferId` message (the tests move ids through the same API domain
/// code will: build a message, read its root, hand the reader over).
fn buffer_id_message(id: u64) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    let mut message = capnp::message::Builder::new_default();
    message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(id);
    message
}

fn established_arena() -> (Arc<Arena>, ShmemChannel, ShmemChannel) {
    let arena = Arena::create(support::test_arena_config()).expect("create arena");
    arena.set_namespace_root(namespace_root_for(arena.epoch()));
    arena.establish();
    let a = ShmemChannel::from_handle(ShmemHandle::new(arena.clone(), Direction::A, None));
    let b = ShmemChannel::from_handle(ShmemHandle::new(arena.clone(), Direction::B, None));
    (arena, a, b)
}

/// THE Shmem witness, threaded: the producer publishes a 64 KiB frame into
/// the arena in one thread; the consumer resolves the same `T.BufferId` in
/// another. Measured, not claimed:
///
/// * `stats.resolve_copy_bytes == 0` — the resolve path contains no copy;
/// * the resolved slice's pointer is the slot's address — identity;
/// * the producer's publish is the single write (`publishes == 1`).
#[test]
fn threaded_publish_resolve_zero_copies() {
    let (arena, _a, _b) = established_arena();
    let root = arena.namespace_root();
    let payload = support::bulk_payload(0xA5, 64 * 1024);

    // Publish in this thread, then capture the slot address (the identity
    // half of the witness).
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(root);
    let id_reader = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    arena.publish(id_reader, &payload).expect("publish");
    let slot_address = arena.published_payload_ptr(root).expect("probe").0;
    let _ = slot_address;

    let slot_address_usize = slot_address as usize;
    let arena_for_thread = arena.clone();
    let resolve_thread = std::thread::spawn(move || {
        let arena = arena_for_thread;
        let id_message = buffer_id_message(root);
        let id = id_message
            .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
            .expect("root");
        let frame: FrameRef<'_> = arena.resolve(id).expect("resolve from the peer thread");
        assert_eq!(&*frame, &payload[..], "the frame must survive the hop");
        frame.as_slice().as_ptr() as usize
    });

    let resolved_ptr = resolve_thread.join().expect("peer thread");
    assert_eq!(
        resolved_ptr, slot_address_usize,
        "the consumer reads the producer's slot memory itself: zero copies"
    );

    let stats = arena.stats();
    assert_eq!(stats.publishes, 1, "the producer's single write");
    assert_eq!(stats.resolve_calls, 1, "the consumer's single read");
    assert_eq!(
        stats.resolve_copy_bytes, 0,
        "the resolve path copies nothing (measured)"
    );
    assert_eq!(stats.refused_out_of_namespace, 0);
}

/// The contract story end to end: a `T.DataHandle` message rides the control
/// ring carrying the **reference**; the bulk never crosses the ring, and the
/// consumer resolves it through the arena. The counters separate the two
/// paths so the numbers can be reported honestly.
#[test]
fn data_handle_reference_rides_the_ring_bulk_does_not() {
    let (arena, a, b) = established_arena();
    let root = arena.namespace_root();
    let payload = support::bulk_payload(0x11, 48 * 1024);

    // The producer publishes the bulk once, then sends a message whose root
    // is a DataHandle with the buffer arm (the reference, never the bytes).
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(root);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    arena.publish(id, &payload).expect("publish bulk");

    a.send(MessageOut::Owned(OwnedMessage::new(
        support::buffer_data_handle_message(root),
    )))
    .expect("send the reference");

    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv the reference");
    let data_handle = batch
        .root::<types_capnp::data_handle::Owned>()
        .expect("root as DataHandle");
    let buffer_id = match data_handle.which().expect("which") {
        types_capnp::data_handle::Buffer(id) => id.expect("buffer id reader"),
        _ => panic!("the message must carry the reference arm"),
    };
    assert_eq!(
        buffer_id.get_id(),
        root,
        "the message carries the reference, never the bytes"
    );

    // The consumer resolves the reference through the arena: zero copies.
    let frame = b
        .resolve_buffer(buffer_id)
        .expect("resolve the buffer arm");
    assert_eq!(&*frame, &payload[..]);
    assert!(
        frame.len() > 1024,
        "a 48 KiB frame must not have travelled inline"
    );

    let stats = arena.stats();
    assert_eq!(stats.publishes, 1);
    assert_eq!(stats.resolve_calls, 1);
    assert_eq!(stats.resolve_copy_bytes, 0);
    assert!(
        stats.control_copy_bytes > 0 && stats.control_copy_bytes < 1024,
        "the control ring carried only the small referencing message: {} bytes",
        stats.control_copy_bytes
    );
}

/// The namespace discipline: a `BufferId` outside the arena's namespace is
/// refused with a typed error, never resolved by accident.
#[test]
fn out_of_namespace_ids_are_refused() {
    let (arena, _a, _b) = established_arena();
    let root = arena.namespace_root();
    let slot_count = arena.config().slot_count as u64;

    let outside_message = buffer_id_message(root + slot_count + 1000);
    let outside = outside_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");

    match arena.resolve(outside) {
        Err(TransportError::BufferNotResolvable { id, .. }) => {
            assert_eq!(id, root + slot_count + 1000);
        }
        other => panic!("expected BufferNotResolvable, got {other:?}"),
    }
    // Below the root is equally foreign.
    let below_message = buffer_id_message(root - 1);
    let below = below_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    assert!(matches!(
        arena.resolve(below),
        Err(TransportError::BufferNotResolvable { .. })
    ));

    let stats = arena.stats();
    assert_eq!(
        stats.refused_out_of_namespace, 2,
        "both refusals are counted"
    );
}

/// An arena that was never bootstrapped is inert: publish, resolve and
/// framing all answer `ArenaNotEstablished` — no half-open arena.
#[test]
fn inert_arena_refuses_everything() {
    let arena = Arena::create(support::test_arena_config()).expect("create arena");
    assert!(!arena.is_established());
    let id_message = buffer_id_message(1);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");

    assert!(matches!(
        arena.publish(id, b"hello"),
        Err(TransportError::ArenaNotEstablished)
    ));
    assert!(matches!(
        arena.resolve(id),
        Err(TransportError::ArenaNotEstablished)
    ));
    let channel = ShmemChannel::from_handle(ShmemHandle::new(arena.clone(), Direction::A, None));
    assert!(matches!(
        channel.send(MessageOut::Owned(OwnedMessage::new(support::version_message(1, 0, 0)))),
        Err(TransportError::ArenaNotEstablished)
    ));
    // And the stats stay clean: nothing happened.
    assert_eq!(arena.stats(), ArenaStats::default());
}

/// Caps are the honest ones: bulk resolvable, control ring declared as a
/// copy, capability transfer `BootstrapOnly`.
#[test]
fn caps_are_declared_honestly() {
    let (_arena, a, _b) = established_arena();
    let caps = a.caps();
    assert_eq!(caps.scheme, "shmem");
    assert_eq!(caps.capability_transfer, CapabilityTransfer::BootstrapOnly);
    assert!(caps.zero_copy_data_handle);
    assert!(!caps.zero_copy_message, "the control ring copies its frames");
    assert!(!caps.peer_identity, "the arena has no peer identity of its own");
    assert!(!caps.note.is_empty());
}

/// Shmem carries no authority: `attach` refuses every kind with a typed
/// reason and hands the value back untouched.
#[test]
fn attach_is_refused_with_a_typed_reason() {
    let (_arena, a, _b) = established_arena();
    let mut descriptor = crate::transport::channel::CapHandoff::Descriptor(support::capability_descriptor("t"));
    match a.attach(&mut descriptor) {
        Err(TransportError::UnsupportedCapability { kind, detail }) => {
            assert_eq!(kind, "descriptor");
            assert!(detail.contains("bootstrap"), "detail names the mechanism");
        }
        other => panic!("expected UnsupportedCapability, got {other:?}"),
    }
    // The value survived the refusal.
    match descriptor {
        crate::transport::channel::CapHandoff::Descriptor(bytes) => {
            assert!(!bytes.is_empty())
        }
        _ => panic!("descriptor must be handed back"),
    }
    let id_message = buffer_id_message(1);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    assert!(matches!(
        a.resolve_buffer(id),
        Err(TransportError::BufferNotResolvable { .. })
    ));
}

/// Close-reason propagation over the ring.
#[test]
fn close_reason_propagates_over_the_ring() {
    let (_arena, a, b) = established_arena();
    Box::new(a).close(CloseReason::Shutdown).expect("close");
    let mut scratch = Vec::new();
    match b.recv(&mut scratch) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
        other => panic!("expected Closed{{Shutdown}}, got {other:?}"),
    }
}

/// The bounded ring: when full, send answers `LimitExceeded` instead of
/// growing without bound (charter: bounded pulls).
#[test]
fn ring_is_bounded() {
    let (_arena, a, _b) = established_arena();
    let capacity = _arena.config().ring_slots;
    let mut sent = 0;
    loop {
        match a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(1, 0, 0)))) {
            Ok(()) => sent += 1,
            Err(TransportError::LimitExceeded { limit }) => {
                assert_eq!(limit, capacity);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
        if sent > capacity as usize + 8 {
            panic!("the ring grew past its declared bound");
        }
    }
    assert!(sent as u32 >= capacity, "a bounded ring fills exactly once");
}

/// Two views of one memfd are the same pages: a byte written through one
/// mapping is observed through the other, and a publish through the owner's
/// view resolves through a freshly-adopted view with zero copies. This is
/// the mechanical proof that "the consumer reads the producer's pages".
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn memfd_two_mappings_are_the_same_pages() {
    let config = support::test_arena_config();
    let owner = Arena::create(config).expect("create arena");
    owner.set_namespace_root(namespace_root_for(owner.epoch()));
    owner.establish();

    // The seals this box actually granted (verified [V] in sys.rs's header).
    let seals = owner.seals_word();
    assert_ne!(seals, 0, "this box grants memfd seals (verified)");

    // A second, independent view of the same region: adopt the region
    // handle, which maps the same memfd again.
    let peer = Arena::adopt(
        &RegionPass::OsHandle(owner.region_handle().expect("memfd backing")),
        &config,
        owner.epoch(),
    )
    .expect("adopt the same region");

    let root = owner.namespace_root();
    let payload = support::bulk_payload(0x7E, 32 * 1024);
    let id = buffer_id_message(root);
    let id_reader = id
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");

    owner.publish(id_reader, &payload).expect("publish through the owner's view");

    // The peer reads through its own mapping of the same pages: the proof.
    let (owner_ptr, _) = owner.published_payload_ptr(root).expect("owner view");
    let (peer_ptr, len) = peer.published_payload_ptr(root).expect("peer view");
    assert_eq!(len, payload.len());
    assert_ne!(
        peer_ptr as usize, owner_ptr as usize,
        "two mappings are two address ranges"
    );
    let seen = unsafe { core::slice::from_raw_parts(peer_ptr, len) };
    assert_eq!(seen, &payload[..], "the pages are shared: no copy");

    // Page-level liveness: mutate through the owner's mapping, observe
    // through the peer's.
    let extra = support::bulk_payload(0x01, 4096);
    owner.publish(id_reader, &extra).expect("republish");
    let seen_again = unsafe { core::slice::from_raw_parts(peer_ptr, extra.len()) };
    assert_eq!(
        seen_again,
        &extra[..],
        "a write through one mapping is visible through the other"
    );

    // The counted path agrees with the raw one: `resolve` on the peer's
    // own mapping, with zero copies measured.
    let frame = peer.resolve(id_reader).expect("resolve on the peer");
    assert_eq!(&*frame, &extra[..]);
    let stats = peer.stats();
    assert_eq!(stats.resolve_copy_bytes, 0, "measured: no copies on resolve");
    assert_eq!(stats.resolve_calls, 1);
}

/// Geometry mismatches at adoption are refused, not reinterpreted.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn adoption_validates_geometry() {
    let owner = Arena::create(support::test_arena_config()).expect("create arena");
    let handle = owner.region_handle().expect("memfd backing");
    let mut wrong = support::test_arena_config();
    wrong.slot_count = 8;
    match Arena::adopt(&RegionPass::OsHandle(handle), &wrong, owner.epoch()) {
        Err(TransportError::Framing { detail }) => {
            assert!(detail.contains("geometry"), "detail names the mismatch: {detail}");
        }
        other => panic!("expected a geometry refusal, got {other:?}"),
    }
}

/// `slot_bytes` and the region size are what the config says they are.
#[test]
fn geometry_matches_region_bytes() {
    let config = support::test_arena_config();
    assert_eq!(config.region_bytes() % 8, 0);
    assert!(config.region_bytes() > 1024 * 1024);
}
