//! InProcess: zero serialization, capability moves, close propagation.

extern crate std;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use capnp::message::ReaderSegments;

use crate::schema::types_capnp;
use crate::transport::channel::{
    AttachedCap, CapHandoff, CapOutcome, Channel, CloseReason, MessageOut, OwnedMessage,
    TransportError,
};
use crate::transport::inprocess::{InProcessChannel, InProcessInbox, InProcessPeer};

use super::support;

/// THE InProcess witness: an `Owned` message's builder is *moved* into the
/// transport, so the segment memory the peer reads is byte-for-byte the
/// memory the sender built. Three pointer-identity assertions (before the
/// move, after the move, after the receive) plus a data assertion make the
/// zero-copy claim mechanical rather than rhetorical.
#[test]
fn owned_message_zero_copy_identity() {
    let (client, server) = InProcessChannel::pair();

    let builder = support::version_message(7, 8, 9);
    let segment_ptr_before = builder.get_segments_for_output()[0].as_ptr();
    let segment_len = builder.get_segments_for_output()[0].len();

    // Move the builder into the message: the allocation, and therefore the
    // segment address, survives the move.
    let owned = OwnedMessage::new(builder);
    let segment_ptr_moved = owned.builder().get_segments_for_output()[0].as_ptr();
    assert_eq!(
        segment_ptr_before, segment_ptr_moved,
        "moving the builder must preserve the segment address"
    );

    client
        .send(MessageOut::Owned(owned))
        .expect("send owned message");

    let mut scratch = Vec::new();
    let batch = server.recv(&mut scratch).expect("recv");
    match &batch.message {
        crate::transport::channel::RecvMessage::Direct(reader) => {
            let segments = reader.get_segments();
            let segment = segments.get_segment(0).expect("segment 0");
            assert_eq!(
                segment.as_ptr(),
                segment_ptr_before,
                "the receiver must read the sender's own memory: zero copies"
            );
            assert_eq!(segment.len(), segment_len);
        }
        _ => panic!("InProcess receive must be the zero-copy Direct form"),
    }

    let version = batch.root::<types_capnp::version::Owned>().expect("root");
    assert_eq!((version.get_major(), version.get_minor(), version.get_patch()), (7, 8, 9));
    assert_eq!(version.get_pre_release().expect("pre_release"), "transport-test");
}

/// A `Borrowed` send is valid for the duration of the call; a synchronous
/// peer (a custom `InProcessPeer`) can dispatch it with no copy at all.
#[test]
fn borrowed_frame_dispatches_by_reference() {
    struct RefPeer {
        seen: std::sync::Mutex<Vec<(usize, usize)>>,
    }

    impl InProcessPeer for RefPeer {
        fn deliver(&self, frame: crate::transport::inprocess::InProcessFrame<'_>) {
            match &frame.message {
                MessageOut::Borrowed(segments) => {
                    let seg = segments[0];
                    self.seen
                        .lock()
                        .unwrap()
                        .push((seg.as_ptr() as usize, seg.len()));
                }
                MessageOut::Owned(_) => panic!("expected the borrowed form"),
            }
        }
        fn stage(&self, _cap: CapHandoff) -> Result<crate::transport::channel::CapId, TransportError> {
            Ok(crate::transport::channel::CapId(0))
        }
        fn drop_staged(&self, _id: crate::transport::channel::CapId) -> Result<(), TransportError> {
            Ok(())
        }
        fn pop_batch(&self) -> Option<crate::transport::channel::RecvBatch> {
            None
        }
        fn pop_close(&self) -> Option<CloseReason> {
            None
        }
        fn notice_close(&self, _reason: CloseReason) {}
        fn is_closed(&self) -> bool {
            false
        }
    }

    let peer = Arc::new(RefPeer {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let channel = InProcessChannel::to_peer(peer.clone());

    let builder = support::version_message(1, 0, 0);
    let segments = builder.get_segments_for_output();
    let borrowed: &[&[u8]] = &segments;
    let ptr_before = borrowed[0].as_ptr();
    channel
        .send(MessageOut::Borrowed(borrowed))
        .expect("send borrowed");

    let seen = peer.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the peer must have been dispatched once");
    assert_eq!(
        seen[0].0, ptr_before as usize,
        "a borrowed frame reaches the peer as the very same memory"
    );
}

/// Round trip plus the per-kind capability outcomes, and close-reason
/// propagation.
#[test]
fn round_trip_capabilities_and_close_reason() {
    let (client, server) = InProcessChannel::pair();

    // A Live capability moves natively and arrives as the same object.
    let mut live = support::live_capability("in-process");
    match client.attach(&mut live).expect("attach live") {
        CapOutcome::Native { .. } => {}
        other => panic!("Live must be Native in-process, got {other:?}"),
    }
    // A descriptor is in-band: the authority is re-established by the far
    // side, the bytes are what cross.
    let mut descriptor = CapHandoff::Descriptor(support::capability_descriptor("filesystem/path/~/Documents"));
    match client.attach(&mut descriptor).expect("attach descriptor") {
        CapOutcome::InBand { .. } => {}
        other => panic!("Descriptor must be InBand, got {other:?}"),
    }

    client
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(3, 1, 4))))
        .expect("send");

    let mut scratch = Vec::new();
    let batch = server.recv(&mut scratch).expect("recv");
    assert_eq!(batch.caps.len(), 2, "both staged capabilities ride the frame");
    let mut live_seen = false;
    let mut descriptor_seen = false;
    for (_id, cap) in &batch.caps {
        match cap {
            AttachedCap::Live(object) => {
                let token = object
                    .downcast_ref::<support::LiveToken>()
                    .expect("downcast to LiveToken");
                assert_eq!(token.name, "in-process");
                live_seen = true;
            }
            AttachedCap::Descriptor(bytes) => {
                assert_eq!(
                    support::capability_descriptor_target(bytes).as_deref(),
                    Some("filesystem/path/~/Documents")
                );
                descriptor_seen = true;
            }
            _ => panic!("unexpected capability form"),
        }
    }
    assert!(live_seen && descriptor_seen, "both capability kinds arrived");

    // Close with a typed reason: the far side observes exactly it.
    // (`close` consumes the channel through its trait object.)
    Box::new(client)
        .close(CloseReason::Shutdown)
        .expect("close with reason");
    match server.recv(&mut scratch) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::Shutdown),
        other => panic!("expected Closed{{Shutdown}}, got {other:?}"),
    }
}

/// `recv` is a bounded pull: an empty inbox answers `Empty`, never a
/// fabricated message and never a panic.
#[test]
fn empty_is_an_honest_answer() {
    let (client, _server) = InProcessChannel::pair();
    let mut scratch = Vec::new();
    match client.recv(&mut scratch) {
        Err(TransportError::Empty) => {}
        other => panic!("expected Empty, got {other:?}"),
    }
}

/// Sending into a closed endpoint answers `Closed`, never silence.
#[test]
fn send_after_peer_close_is_reported() {
    let (client, server) = InProcessChannel::pair();
    Box::new(server)
        .close(CloseReason::Shutdown)
        .expect("server closes");
    match client.send(MessageOut::Owned(OwnedMessage::new(support::version_message(0, 0, 1)))) {
        Err(TransportError::Closed { reason }) => assert_eq!(reason, CloseReason::PeerClosed),
        other => panic!("expected Closed{{PeerClosed}}, got {other:?}"),
    }
}

/// Declared caps are the honest ones for this transport.
#[test]
fn caps_are_declared_honestly() {
    let (client, _server) = InProcessChannel::pair();
    let caps = client.caps();
    assert_eq!(caps.scheme, "inprocess");
    assert!(caps.zero_copy_message);
    assert!(!caps.zero_copy_data_handle, "no arena rides this channel");
    assert!(!caps.peer_identity, "one process has no OS peer");
    assert!(!caps.blocking);
}

/// The spin mutex the inbox uses is stressed from many threads: no loss, no
/// deadlock, and every message arrives exactly once.
#[test]
fn inbox_thread_stress() {
    let (client, server) = InProcessChannel::pair();
    let client = Arc::new(client);
    let server = Arc::new(server);
    let threads = 8;
    let per_thread = 200usize;

    let mut handles = Vec::new();
    for t in 0..threads {
        let client = client.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..per_thread {
                let seq = t * per_thread + i;
                client
                    .send(MessageOut::Owned(OwnedMessage::new(support::version_message(
                        (seq & 0xffff) as u32,
                        (seq >> 16) as u32,
                        t as u32,
                    ))))
                    .expect("send under stress");
            }
        }));
    }
    for h in handles {
        h.join().expect("no thread panics");
    }

    let mut scratch = Vec::new();
    let mut total = 0usize;
    loop {
        match server.recv(&mut scratch) {
            Ok(batch) => {
                let version = batch.root::<types_capnp::version::Owned>().expect("root");
                let seq = version.get_major() as usize | ((version.get_minor() as usize) << 16);
                assert_eq!(seq / per_thread as usize, version.get_patch() as usize);
                total += 1;
            }
            Err(TransportError::Empty) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(total, threads * per_thread, "every message arrives exactly once");
}

/// A custom peer object can be a backend: the seam accepts any
/// `Send + Sync` implementor.
#[test]
fn custom_peer_receives_moved_messages() {
    struct Backend {
        inbox: std::sync::Mutex<Vec<Box<dyn std::any::Any + Send>>>,
    }
    impl InProcessPeer for Backend {
        fn deliver(&self, frame: crate::transport::inprocess::InProcessFrame<'_>) {
            match frame.message {
                MessageOut::Owned(owned) => {
                    // The backend owns the allocation: still zero-copy.
                    self.inbox.lock().unwrap().push(Box::new(owned));
                }
                MessageOut::Borrowed(_) => panic!("expected the owned form"),
            }
        }
        fn stage(&self, _cap: CapHandoff) -> Result<crate::transport::channel::CapId, TransportError> {
            Ok(crate::transport::channel::CapId(0))
        }
        fn drop_staged(&self, _id: crate::transport::channel::CapId) -> Result<(), TransportError> {
            Ok(())
        }
        fn pop_batch(&self) -> Option<crate::transport::channel::RecvBatch> {
            None
        }
        fn pop_close(&self) -> Option<CloseReason> {
            None
        }
        fn notice_close(&self, _reason: CloseReason) {}
    }

    let backend = Arc::new(Backend {
        inbox: std::sync::Mutex::new(Vec::new()),
    });
    let channel = InProcessChannel::to_peer(backend.clone());
    channel
        .send(MessageOut::Owned(OwnedMessage::new(support::version_message(5, 5, 5))))
        .expect("send to backend");
    let inbox = backend.inbox.lock().unwrap();
    assert_eq!(inbox.len(), 1, "the backend holds the moved message");
}

/// The default inbox constructible and directly inspectable.
#[test]
fn inbox_default_and_empty() {
    let inbox = InProcessInbox::default();
    assert!(inbox.pop_batch().is_none());
    assert!(inbox.pop_close().is_none());
    let (_client, server) = InProcessChannel::pair();
    assert_eq!(
        server.caps().scheme,
        "inprocess",
        "a pair is two in-process channels"
    );
}
