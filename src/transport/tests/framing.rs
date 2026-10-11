//! The framing is verified byte-for-byte against capnp's own stream
//! codec: my length-prefix arithmetic (which must be exact, because a
//! length is written *before* the message) is only correct if the two
//! encoders agree byte for byte.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use capnp::message::ReaderSegments;

use crate::transport::channel::{frame, CloseReason, MessageOut, OwnedMessage};

use super::support;

/// A multi-segment builder: capnp allocates a second segment when the
/// first fills, which is what exercises the segment-table arithmetic.
fn multi_segment_builder() -> capnp::message::Builder<capnp::message::HeapAllocator> {
    // Build past one segment: a 4 MiB string field forces the allocator to
    // allocate a second segment, which is what exercises the segment-table
    // arithmetic (multiple 4-byte lengths, the padding rules).
    let mut message = capnp::message::Builder::new_default();
    {
        let mut version = message.init_root::<crate::schema::types_capnp::version::Builder<'_>>();
        version.set_major(1);
        version.set_minor(2);
        version.set_patch(3);
        let big = "x".repeat(4 * 1024 * 1024);
        version.set_pre_release(&big);
    }
    message
}

#[test]
fn writer_is_byte_identical_to_capnp() {
    let cases: Vec<(&str, capnp::message::Builder<capnp::message::HeapAllocator>)> = vec![
        ("single-segment", support::version_message(1, 2, 3)),
        ("multi-segment", multi_segment_builder()),
    ];
    for (name, builder) in cases {
        let segments = builder.get_segments_for_output();
        let mut mine = Vec::new();
        frame::write_flat_message(&mut mine, &segments).expect("my writer");

        let mut theirs = Vec::new();
        capnp::serialize::write_message(&mut theirs, &builder).expect("capnp writer");

        assert_eq!(
            mine, theirs,
            "{name}: my flat-message encoding must be byte-identical to capnp's"
        );
        assert_eq!(
            mine.len() as u32,
            frame::flat_message_len(&segments),
            "{name}: the length arithmetic must predict the encoding"
        );

        // And the fixed-buffer (arena ring) path produces the same bytes.
        let mut into = alloc::vec![0u8; mine.len() + 16];
        let written = frame::flat_message_into(&mut into, &segments).expect("into");
        assert_eq!(&into[..written], &mine[..], "{name}: both writers agree");

        // Round trip: what we wrote, capnp can read.
        let reader = capnp::serialize::read_message(
            &mut &mine[..],
            capnp::message::ReaderOptions::new(),
        )
        .expect("capnp reads my encoding");
        let version = reader
            .get_root::<crate::schema::types_capnp::version::Reader<'_>>()
            .expect("root");
        assert_eq!((version.get_major(), version.get_minor(), version.get_patch()), (1, 2, 3));
    }
}

#[test]
fn frame_round_trip_through_a_byte_sink() {
    let payload = b"hello framing";
    let mut sink = Vec::new();
    frame::write_frame(&mut sink, frame::KIND_DATA, payload).expect("write");

    // The length prefix is (kind + payload), big-endian.
    let total = u32::from_be_bytes([sink[0], sink[1], sink[2], sink[3]]);
    assert_eq!(total as usize, 1 + payload.len());

    let mut reader = &sink[..];
    let mut scratch = Vec::new();
    let (kind, got) = frame::read_frame(&mut reader, &mut scratch).expect("read");
    assert_eq!(kind, frame::KIND_DATA);
    assert_eq!(got, payload);
}

#[test]
fn close_reasons_round_trip() {
    let reasons = [
        CloseReason::Shutdown,
        CloseReason::PeerClosed,
        CloseReason::Cancelled,
        CloseReason::Protocol {
            detail: "frame length 7 out of range".to_string(),
        },
        CloseReason::Transport {
            detail: "host failure (errno 104)".to_string(),
        },
        CloseReason::Unavailable {
            detail: "memfd_create failed".to_string(),
        },
    ];
    for reason in reasons {
        let payload = frame::encode_close(&reason);
        let decoded = frame::decode_close(&payload).expect("decode");
        assert_eq!(decoded, reason, "{reason:?} must survive the wire");
    }
}

#[test]
fn attach_payloads_round_trip() {
    use crate::transport::channel::{CapId, RegionNotice};

    // Descriptor.
    let data = support::capability_descriptor("framing/target");
    let payload = frame::encode_attach(frame::CAP_KIND_DESCRIPTOR, CapId(9), &data);
    assert_eq!(payload[0], frame::CAP_KIND_DESCRIPTOR);
    assert_eq!(u64::from_be_bytes(payload[1..9].try_into().unwrap()), 9);
    let data_len = u16::from_be_bytes([payload[9], payload[10]]) as usize;
    assert_eq!(&payload[11..11 + data_len], &data[..]);

    // Region notice: the full arena geometry.
    let notice = RegionNotice {
        namespace_root: 1 << 40,
        slot_count: 16,
        slot_bytes: 64 * 1024,
        ring_slots: 16,
        ring_bytes: 64 * 1024,
        epoch: 3,
    };
    let encoded = frame::encode_region_notice(&notice);
    assert_eq!(frame::decode_region_notice(&encoded).expect("decode"), notice);
    assert!(frame::decode_region_notice(&encoded[..27]).is_err());
}

#[test]
fn frame_length_limits_are_enforced() {
    // A declared length beyond the maximum is refused, never allocated.
    let mut bad = Vec::new();
    bad.extend_from_slice(&(frame::MAX_FRAME_BYTES + 1).to_be_bytes());
    bad.push(frame::KIND_DATA);
    let mut reader = &bad[..];
    let mut scratch = Vec::new();
    match frame::read_frame(&mut reader, &mut scratch) {
        Err(crate::transport::channel::TransportError::Framing { .. }) => {}
        other => panic!("expected Framing, got {other:?}"),
    }
}

#[test]
fn owned_message_carries_its_segments() {
    let builder = support::version_message(9, 8, 7);
    let owned = OwnedMessage::new(builder);
    assert_eq!(owned.builder().get_segments_for_output().len(), 1);
    let message = MessageOut::Owned(owned);
    let segments = message.get_segment(0).expect("segment 0");
    assert!(!segments.is_empty());
    assert!(message.get_segment(9).is_none());
}
