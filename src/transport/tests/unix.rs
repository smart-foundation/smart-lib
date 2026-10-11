//! UnixSocket: framing, SCM_RIGHTS fd passing, peer identity.

extern crate std;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::channel::{
    AttachedCap, CapabilityTransfer, CapHandoff, CapOutcome, Channel, ChannelListener, CloseReason,
    MessageOut, OwnedMessage, OsHandle, TransportError, TransportKind,
};
use crate::transport::sys;
use crate::transport::unix::{UnixChannel, UnixListener};

#[cfg(all(target_os = "linux", feature = "std"))]
use crate::transport::bootstrap::{namespace_root_for, RegionPass, Role};
#[cfg(all(target_os = "linux", feature = "std"))]
use crate::transport::shmem::{Arena, Direction, ShmemHandle};
#[cfg(all(target_os = "linux", feature = "std"))]
use crate::transport::channel::{PeerCredentials, RegionNotice};

use super::support;

fn pair() -> (UnixChannel, UnixChannel) {
    let (a, b) = sys::unix_socketpair().expect("socketpair");
    (UnixChannel::from_fd(a), UnixChannel::from_fd(b))
}

#[test]
fn round_trip_and_close_reason() {
    let (a, b) = pair();
    a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        4, 2, 0,
    ))))
    .expect("send");

    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(
        (version.get_major(), version.get_minor(), version.get_patch()),
        (4, 2, 0)
    );

    // The reply direction.
    b.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        9, 9, 9,
    ))))
    .expect("reply");
    let batch = a.recv(&mut scratch).expect("recv reply");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(version.get_major(), 9);

    // Close with a typed reason: the far side observes exactly it.
    Box::new(a).close(CloseReason::Shutdown).expect("close");
    match b.recv(&mut scratch) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
        other => panic!("expected Closed{{Shutdown}}, got {other:?}"),
    }
}

/// THE SCM_RIGHTS witness: a memfd is attached, the kernel re-opens it on
/// the far side, and the far side reads the producer's bytes through a
/// *different* fd number. A fake transfer could not produce that.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn scm_rights_transfers_a_real_memfd() {
    let (a, b) = pair();

    let region = sys::create_shared_region(4096).expect("memfd");
    let payload = b"the kernel re-opened this fd";
    unsafe {
        core::ptr::copy_nonoverlapping(payload.as_ptr(), region.ptr, payload.len());
    }
    let sent_fd = region.fd;

    let mut cap = CapHandoff::OsHandle(OsHandle(sent_fd as u64));
    match a.attach(&mut cap).expect("attach an os handle") {
        CapOutcome::Native { .. } => {}
        other => panic!("an OsHandle must transfer natively on Linux, got {other:?}"),
    }
    a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        1, 1, 1,
    ))))
    .expect("send with the attachment");

    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv with the attachment");
    assert_eq!(batch.caps.len(), 1, "the fd rides its message");
    let far_fd = match &batch.caps[0].1 {
        AttachedCap::OsHandle { fd } => fd.0 as i32,
        other => panic!("expected an OsHandle attachment, got {other:?}"),
    };
    assert_ne!(
        far_fd, sent_fd,
        "the kernel must have re-opened the fd: a real transfer has a new number"
    );

    // The far side reads the producer's bytes through its own fd.
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    let mut file = unsafe { std::fs::File::from_raw_fd(far_fd) };
    file.seek(SeekFrom::Start(0)).expect("seek");
    let mut content = alloc::vec![0u8; payload.len()];
    file.read_exact(&mut content).expect("read through the passed fd");
    // Hand the fd back to the transport, which owns it now.
    let _ = file.into_raw_fd();
    assert_eq!(content, payload, "the passed fd reads the same bytes");

    // Both endpoints agree this process is the peer.
    let creds = b.peer_credentials().expect("SO_PEERCRED works here");
    assert_eq!(creds.uid, this_process_uid());
    assert_eq!(creds.gid, this_process_gid());
    sys::release_shared_region(&region);
}

/// This process's real uid/gid, read from /proc (the honest way to check
/// SO_PEERCRED without adding a libc dependency).
fn this_process_uid() -> u32 {
    proc_status_field("Uid:", 0)
}
fn this_process_gid() -> u32 {
    proc_status_field("Gid:", 0)
}
fn proc_status_field(prefix: &str, index: usize) -> u32 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest
                .split_whitespace()
                .nth(index)
                .and_then(|v| v.parse().ok())
                .expect("field");
        }
    }
    panic!("no {prefix} line in /proc/self/status");
}

/// Passing a region *with its notice* transfers a `BufferId` namespace: the
/// far side resolves ids the producer published in the arena it never had
/// before the attach.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn region_notice_transfers_a_buffer_namespace() {
    let (a, b) = pair();

    let arena = Arena::create(support::test_arena_config()).expect("arena");
    let root = namespace_root_for(arena.epoch());
    arena.set_namespace_root(root);
    arena.establish();

    let payload = support::bulk_payload(0x5A, 24 * 1024);
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(root);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    arena.publish(id, &payload).expect("publish");

    // Before the attach, the far side cannot resolve anything.
    assert!(
        matches!(
            b.resolve_buffer(id),
            Err(TransportError::BufferNotResolvable { .. })
        ),
        "no namespace before the transfer"
    );

    let config = arena.config();
    let notice = RegionNotice {
        namespace_root: root,
        slot_count: config.slot_count,
        slot_bytes: config.slot_bytes,
        ring_slots: config.ring_slots,
        ring_bytes: config.ring_bytes,
        epoch: arena.epoch(),
    };
    let mut cap = CapHandoff::Region {
        fd: arena.region_handle().expect("memfd"),
        notice,
    };
    assert!(matches!(
        a.attach(&mut cap).expect("attach region"),
        CapOutcome::Native { .. }
    ));
    a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        2, 2, 2,
    ))))
    .expect("send");

    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv");
    assert_eq!(batch.caps.len(), 1);
    assert!(
        matches!(&batch.caps[0].1, AttachedCap::Region { .. }),
        "the region notice arrived"
    );

    // After the transfer, the far side resolves the producer's published
    // frame — through a mapping of the same memfd, with zero copies.
    let frame = b.resolve_buffer(id).expect("resolve after the transfer");
    assert_eq!(&*frame, &payload[..]);
}

/// Descriptor attachments are in-band and arrive intact.
#[test]
fn descriptor_attachment_is_in_band() {
    let (a, b) = pair();
    let mut cap = CapHandoff::Descriptor(support::capability_descriptor("audio/device/speaker"));
    match a.attach(&mut cap).expect("attach descriptor") {
        CapOutcome::InBand { .. } => {}
        other => panic!("descriptors are in-band on a socket, got {other:?}"),
    }
    a.send(MessageOut::Owned(OwnedMessage::new(support::version_message(
        1, 0, 0,
    ))))
    .expect("send");
    let mut scratch = Vec::new();
    let batch = b.recv(&mut scratch).expect("recv");
    match &batch.caps[0].1 {
        AttachedCap::Descriptor(bytes) => assert_eq!(
            support::capability_descriptor_target(bytes).as_deref(),
            Some("audio/device/speaker")
        ),
        _ => panic!("expected a descriptor"),
    }
}

/// A live handle cannot cross a kernel boundary: refused with a typed
/// reason, and the value is untouched.
#[test]
fn live_handle_is_refused_on_a_socket() {
    let (a, _b) = pair();
    let mut cap = support::live_capability("no-cross");
    match a.attach(&mut cap) {
        Err(TransportError::UnsupportedCapability { kind, detail }) => {
            assert_eq!(kind, "live handle");
            assert!(detail.contains("kernel boundary"));
        }
        other => panic!("expected UnsupportedCapability, got {other:?}"),
    }
    assert!(matches!(cap, CapHandoff::Live(_)), "handed back untouched");
}

/// The listener: bind, accept, exchange, close.
#[test]
fn listener_accepts_and_exchanges() {
    let path = "/tmp/smart-lib-transport-test.sock";
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).expect("bind");
    let server = std::thread::spawn(move || {
        let channel = listener.accept().expect("accept");
        let mut scratch = Vec::new();
        let batch = channel.recv(&mut scratch).expect("recv");
        let version = batch.root::<types_capnp::version::Owned>().expect("root");
        assert_eq!(version.get_major(), 42);
        Box::new(channel).close(CloseReason::Shutdown).expect("server close");
    });
    let client = UnixChannel::connect(path).expect("connect");
    client
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
            42, 0, 0,
        ))))
        .expect("send");
    server.join().expect("server thread");
    let _ = std::fs::remove_file(path);
}

/// The declared capability-transfer story.
#[test]
fn caps_declare_os_handle_transfer() {
    let (a, _b) = pair();
    let caps = a.caps();
    assert_eq!(caps.kind, TransportKind::UnixSocket);
    #[cfg(all(target_os = "linux", feature = "std"))]
    assert_eq!(
        caps.capability_transfer,
        CapabilityTransfer::OsHandle,
        "Linux has SCM_RIGHTS: the declaration is the real thing"
    );
    assert!(caps.peer_identity, "SO_PEERCRED exists on AF_UNIX");
    assert!(caps.blocking);
    assert!(!caps.note.is_empty());
}

/// The negotiate hook: the caller sends an `AbiHandshakeRequest`, the peer
/// answers with an `AbiHandshakeResponse`, and the transport moves both.
#[test]
fn negotiate_hook_round_trips_a_handshake() {
    let (a, b) = pair();
    let responder = std::thread::spawn(move || {
        let mut scratch = Vec::new();
        let batch = b.recv(&mut scratch).expect("recv the handshake");
        let request = batch
            .root::<crate::schema::abi_capnp::abi_handshake_request::Owned>()
            .expect("root as the handshake request");
        assert!(request.get_accept_downgrade());
        // The mock peer decides: matched.
        b.send(MessageOut::Owned(OwnedMessage::new(
            support::handshake_response_matched(),
        )))
        .expect("answer the handshake");
    });
    let response = a
        .negotiate(MessageOut::Owned(OwnedMessage::new(
            support::handshake_request(true),
        )))
        .expect("negotiate");
    assert!(
        matches!(
            response.get().expect("response").which().expect("which"),
            crate::schema::abi_capnp::abi_handshake_response::Matched(_)
        ),
        "the caller reads the peer's verdict"
    );
    responder.join().expect("responder thread");
}

/// The bootstrap over a real socket pair: the arena's memfd crosses with
/// SCM_RIGHTS, both sides establish, and a publish through the owner's view
/// resolves through the peer's — different fd, same pages.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn bootstrap_over_a_real_unix_link() {
    let (link_a, link_b) = crate::transport::bootstrap::UnixBootstrapLink::pair()
        .expect("bootstrap link pair");
    let config = support::test_arena_config();
    let arena = Arena::create(config).expect("owner creates the arena");

    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(
            &link_b,
            Role::Peer,
            None,
            &Default::default(),
        )
        .expect("peer bootstraps")
    });

    let owner_handle = crate::transport::bootstrap::run_bootstrap(
        &link_a,
        Role::Owner,
        Some(&arena),
        &Default::default(),
    )
    .expect("owner bootstraps");
    let peer_handle = peer_thread.join().expect("peer thread");

    assert_eq!(owner_handle.direction, Direction::A);
    assert_eq!(peer_handle.direction, Direction::B);
    assert!(arena.is_established(), "the shared state word is ESTABLISHED");

    // Publish on the owner; resolve on the peer (a different mapping).
    let root = arena.namespace_root();
    let payload = support::bulk_payload(0x33, 40 * 1024);
    let mut id_message = capnp::message::Builder::new_default();
    id_message
        .init_root::<types_capnp::buffer_id::Builder<'_>>()
        .set_id(root);
    let id = id_message
        .get_root_as_reader::<types_capnp::buffer_id::Reader<'_>>()
        .expect("root");
    arena.publish(id, &payload).expect("publish");
    let frame = peer_handle
        .arena
        .resolve(id)
        .expect("resolve on the peer's mapping");
    assert_eq!(&*frame, &payload[..]);
    assert_eq!(peer_handle.arena.stats().resolve_copy_bytes, 0);

    // The peer's region is a different fd of the same object.
    let owner_fd = arena.region_handle().expect("owner fd");
    let peer_fd = peer_handle.arena.region_handle().expect("peer fd");
    assert_ne!(
        owner_fd, peer_fd,
        "SCM_RIGHTS re-opened the descriptor: the kernel moved the authority"
    );
    let _ = ShmemHandle::new;
    let _ = PeerCredentials {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let _ = RegionPass::Shared;
}

/// A bootstrapped Shmem channel pair: frames ride the rings, bulk rides
/// the arena.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn bootstrapped_channels_exchange_frames() {
    let (link_a, link_b) = crate::transport::bootstrap::UnixBootstrapLink::pair()
        .expect("bootstrap link pair");
    let config = support::test_arena_config();
    let arena = Arena::create(config).expect("arena");
    let peer_thread = std::thread::spawn(move || {
        crate::transport::bootstrap::run_bootstrap(
            &link_b,
            Role::Peer,
            None,
            &Default::default(),
        )
        .expect("peer bootstraps")
    });
    let owner_handle = crate::transport::bootstrap::run_bootstrap(
        &link_a,
        Role::Owner,
        Some(&arena),
        &Default::default(),
    )
    .expect("owner bootstraps");
    let peer_handle = peer_thread.join().expect("peer thread");

    let owner = crate::transport::shmem::ShmemChannel::from_handle(owner_handle);
    let peer = crate::transport::shmem::ShmemChannel::from_handle(peer_handle);

    owner
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
            11, 22, 33,
        ))))
        .expect("send over the ring");
    let mut scratch = Vec::new();
    let batch = peer.recv(&mut scratch).expect("recv over the ring");
    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!(
        (version.get_major(), version.get_minor(), version.get_patch()),
        (11, 22, 33)
    );

    Box::new(owner).close(CloseReason::Shutdown).expect("close");
    match peer.recv(&mut scratch) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
        other => panic!("expected Closed{{Shutdown}}, got {other:?}"),
    }
}
