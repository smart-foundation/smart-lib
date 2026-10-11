//! The Shmem authentication bootstrap, driven through the mock link (and,
//! in `unix.rs`, through a real socket with SCM_RIGHTS).

extern crate std;

use alloc::string::ToString;

use crate::transport::bootstrap::{
    BootstrapLink, MockLink, PeerExpectation, RegionPass, Role, UnixBootstrapLink,
    BOOTSTRAP_VERSION,
};
use crate::transport::channel::{PeerCredentials, TransportError};
use crate::transport::shmem::{Arena, Direction};

use super::support;

fn creds(pid: i32, uid: u32, gid: u32) -> PeerCredentials {
    PeerCredentials { pid, uid, gid }
}

#[test]
fn encode_decode_round_trips_every_form() {
    use crate::transport::bootstrap::BootstrapMessage;
    use crate::transport::shmem::ArenaConfig;

    let offers = BootstrapMessage::Offer {
        version: BOOTSTRAP_VERSION,
        epoch: 7,
        config: ArenaConfig {
            slot_bytes: 64 * 1024,
            slot_count: 16,
            ring_slots: 16,
            ring_bytes: 64 * 1024,
        },
        namespace_root: 7u64 << 32,
    };
    assert_eq!(
        BootstrapMessage::decode(&offers.encode()).expect("decode offer"),
        offers
    );

    let accept = BootstrapMessage::Accept {
        epoch: 7,
        namespace_root: 7u64 << 32,
    };
    assert_eq!(
        BootstrapMessage::decode(&accept.encode()).expect("decode accept"),
        accept
    );

    let refuse = BootstrapMessage::Refuse {
        reason: crate::transport::bootstrap::BootstrapRefusal::UnexpectedPeer,
        detail: "peer uid 1001 is not the invited 1000".to_string(),
    };
    assert_eq!(
        BootstrapMessage::decode(&refuse.encode()).expect("decode refuse"),
        refuse
    );

    // Unknown tags are a typed error, never a panic.
    assert!(BootstrapMessage::decode(&[0x7F]).is_err());
    assert!(BootstrapMessage::decode(&[]).is_err());
}

/// The full mock exchange: who authenticates whom, and what the arena can
/// do afterwards.
#[test]
fn mock_exchange_establishes_both_sides() {
    let (link_owner, link_peer) = MockLink::pair(creds(10, 1000, 1000), creds(20, 1001, 1001));
    let arena = Arena::create(support::test_arena_config()).expect("arena");
    assert!(!arena.is_established(), "inert before the exchange");

    let owner_expectation = PeerExpectation {
        pid: Some(20),
        uid: Some(1001),
        gid: None,
    };
    let peer_expectation = PeerExpectation {
        pid: Some(10),
        uid: Some(1000),
        gid: None,
    };
    let owner_arena = arena.clone();

    // The peer answers in this thread, the owner in the next: the protocol
    // is a round trip, so one of them must be concurrent.
    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(&link_peer, Role::Peer, None, &peer_expectation)
    });

    let owner_handle = crate::transport::bootstrap::run_bootstrap(
        &link_owner,
        Role::Owner,
        Some(&owner_arena),
        &owner_expectation,
    )
    .expect("owner bootstraps");
    let peer_handle = peer_thread.join().expect("peer thread").expect("peer bootstraps");

    assert_eq!(owner_handle.direction, Direction::A);
    assert_eq!(peer_handle.direction, Direction::B);
    assert!(arena.is_established(), "established after the exchange");

    // Both sides see the same namespace; the same object serves both.
    assert_eq!(
        owner_handle.arena.namespace_root(),
        peer_handle.arena.namespace_root()
    );
    let root = arena.namespace_root();
    assert_eq!(root, crate::transport::bootstrap::namespace_root_for(arena.epoch()));

    // What the arena is safe for afterwards: publish on one side, resolve
    // on the other — the in-process `Shared` region form, zero copies.
    let payload = support::bulk_payload(0xC3, 8 * 1024);
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<crate::schema::types_capnp::buffer_id::Builder<'_>>()
        .set_id(root);
    let id = id_message
        .get_root_as_reader::<crate::schema::types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    owner_handle.arena.publish(id, &payload).expect("publish");
    let frame = peer_handle.arena.resolve(id).expect("resolve");
    assert_eq!(&*frame, &payload[..]);
    assert_eq!(peer_handle.arena.stats().resolve_copy_bytes, 0);
}

/// A peer that is not the invited one is refused, and the arena stays
/// inert on both sides.
#[test]
fn unexpected_peer_is_refused_and_both_sides_stay_inert() {
    let (link_owner, link_peer) = MockLink::pair(creds(10, 1000, 1000), creds(20, 1337, 1337));
    let arena = Arena::create(support::test_arena_config()).expect("arena");

    let owner_expectation = PeerExpectation {
        uid: Some(1001),
        ..Default::default()
    };
    let peer_expectation = PeerExpectation::default();

    let peer_thread = std::thread::spawn(move || {
        // The peer checks the owner: they do not match (1000 vs 1001
        // expected? no — the peer expects nothing; the OWNER refuses
        // before the offer, so the peer's recv just fails).
        crate::transport::bootstrap::run_bootstrap(&link_peer, Role::Peer, None, &peer_expectation)
    });

    let owner_result = crate::transport::bootstrap::run_bootstrap(
        &link_owner,
        Role::Owner,
        Some(&arena),
        &owner_expectation,
    );
    assert!(
        matches!(owner_result, Err(TransportError::UnsupportedCapability { .. })),
        "the owner refuses before anything valuable moves: {owner_result:?}"
    );
    assert!(!arena.is_established(), "the arena stays inert");
    // The peer never received an offer, so it fails too — a refused
    // bootstrap is never a half-open arena.
    let peer_result = peer_thread.join().expect("peer thread");
    assert!(peer_result.is_err(), "the peer errors: {peer_result:?}");
}

/// The peer refuses an offer it cannot verify (a crafted version), and the
/// refusal message carries a typed reason.
#[test]
fn peer_refuses_a_bad_version_with_a_typed_reason() {
    let (link_owner, link_peer) = MockLink::pair(creds(1, 1, 1), creds(2, 2, 2));

    // The peer runs and should refuse.
    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(&link_peer, Role::Peer, None, &Default::default())
    });

    // A crafted offer with an impossible version, sent through the *peer's*
    // link so the owner side would see it as its answer.
    let bad_offer = crate::transport::bootstrap::BootstrapMessage::Offer {
        version: BOOTSTRAP_VERSION + 99,
        epoch: 1,
        config: support::test_arena_config(),
        namespace_root: 1,
    };
    // The peer expects an offer from the owner: push one through the
    // owner's link (which lands in the peer's mailbox).
    link_owner
        .send(
            &bad_offer,
            Some(&RegionPass::Shared(
                Arena::create(support::test_arena_config()).expect("arena"),
            )),
        )
        .expect("queue the crafted offer");

    let peer_result = peer_thread.join().expect("peer thread");
    let err = peer_result.expect_err("the peer must refuse");
    match err {
        TransportError::Framing { detail } => {
            assert!(
                detail.contains("version mismatch"),
                "the refusal names the reason: {detail}"
            );
        }
        other => panic!("expected a version-mismatch refusal, got {other:?}"),
    }

    // And the refusal message itself was sent back with its typed reason.
    let (refusal, _) = link_owner.recv().expect("the refusal reached the owner");
    match refusal {
        crate::transport::bootstrap::BootstrapMessage::Refuse { reason, .. } => {
            assert_eq!(
                reason,
                crate::transport::bootstrap::BootstrapRefusal::VersionMismatch
            );
        }
        other => panic!("expected a Refuse, got {other:?}"),
    }
}

/// A link that cannot identify the peer cannot satisfy an identity
/// requirement: refused, not waved through.
#[test]
fn blind_link_cannot_satisfy_an_identity_expectation() {
    let (link_owner, link_peer) = MockLink::blind_pair();
    let arena = Arena::create(support::test_arena_config()).expect("arena");
    let expectation = PeerExpectation {
        uid: Some(1000),
        ..Default::default()
    };

    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(&link_peer, Role::Peer, None, &Default::default())
    });
    let owner_result = crate::transport::bootstrap::run_bootstrap(
        &link_owner,
        Role::Owner,
        Some(&arena),
        &expectation,
    );
    assert!(matches!(
        owner_result,
        Err(TransportError::UnsupportedCapability { .. })
    ));
    let _ = peer_thread.join();
}

/// The real link refuses an in-memory arena: an `Arc` cannot cross a
/// socket, and pretending otherwise would be a fake transfer.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn unix_link_refuses_an_in_memory_arena() {
    let (link, _other) = UnixBootstrapLink::pair().expect("link pair");
    let arena = Arena::create(support::test_arena_config()).expect("arena");
    let offer = crate::transport::bootstrap::BootstrapMessage::Offer {
        version: BOOTSTRAP_VERSION,
        epoch: arena.epoch(),
        config: arena.config(),
        namespace_root: 1,
    };
    match link.send(&offer, Some(&RegionPass::Shared(arena))) {
        Err(TransportError::UnsupportedCapability { kind, .. }) => {
            assert_eq!(kind, "region");
        }
        other => panic!("expected UnsupportedCapability, got {other:?}"),
    }
}

/// The namespace root derivation: two arenas can never share one.
#[test]
fn namespace_roots_are_distinct_per_arena() {
    let a = Arena::create(support::test_arena_config()).expect("a");
    let b = Arena::create(support::test_arena_config()).expect("b");
    assert_ne!(a.epoch(), b.epoch());
    assert_ne!(
        crate::transport::bootstrap::namespace_root_for(a.epoch()),
        crate::transport::bootstrap::namespace_root_for(b.epoch())
    );
}

/// A full two-thread exchange where the arena is adopted through the
/// `Shared` form and the identity checks pass on both sides.
#[test]
fn symmetric_identity_checks_on_both_sides() {
    let (link_owner, link_peer) = MockLink::pair(creds(11, 500, 500), creds(22, 600, 600));
    let arena = Arena::create(support::test_arena_config()).expect("arena");

    let owner_expectation = PeerExpectation {
        pid: Some(22),
        uid: Some(600),
        gid: Some(600),
    };
    let peer_expectation = PeerExpectation {
        pid: Some(11),
        uid: Some(500),
        gid: Some(500),
    };
    let owner_arena = arena.clone();
    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(&link_peer, Role::Peer, None, &peer_expectation)
    });
    let handle = crate::transport::bootstrap::run_bootstrap(
        &link_owner,
        Role::Owner,
        Some(&owner_arena),
        &owner_expectation,
    )
    .expect("both sides authenticated");
    assert_eq!(handle.peer, Some(creds(22, 600, 600)));
    let peer_handle = peer_thread.join().expect("peer thread").expect("peer ok");
    assert_eq!(peer_handle.peer, Some(creds(11, 500, 500)));
}
