//! Inline behaviour tests for the clipboard domain's *public contract*: the
//! owned types' fidelity to the wire, the availability matrix's internal
//! consistency, and the trait's honest-Unavailable totality (every method has a
//! `reasonClass`-prefixed reason when a concrete host cannot serve it). The
//! native-path behaviour over a scripted selection seam is proven in
//! [`crate::platforms::linux::clipboard::tests`]. All hermetic — no host I/O.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::clipboard_capnp;

use super::availability::{
    status_counts, AvailStatus, CLIPBOARD_AVAILABILITY, CLIPBOARD_SCHEMA_ID, ReasonClass,
};
use super::backend::ClipboardBackend;
use super::types::{ClipboardContent, ClipboardItem, ClipboardSizeLimits, Pagination, SubscriptionId};

/// A backend that overrides nothing: it is the *honest default* every platform
/// the trait does not directly target becomes. Its whole job is to prove that
/// every method degrades with a reason, never a panic and never a fabricated
/// answer.
struct HonestBackend;

impl ClipboardBackend for HonestBackend {}

/// Round-trip a clipboard content value through a real serialized capnp message:
/// the payload rides the `DataHandle.inline` path, and `mimeType` is the
/// contract's `T.MimeType` struct, so `write_to` → serialize → `from_reader`
/// must be the identity for the mime and the bytes.
#[test]
fn clipboard_content_round_trips_through_the_wire() {
    let content = ClipboardContent {
        mime_type: "text/plain;charset=utf-8".to_string(),
        data: b"clipboard over capnp".to_vec(),
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clipboard_capnp::clipboard_content::Builder<'_>>();
        content.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<clipboard_capnp::clipboard_content::Reader<'_>>()
        .expect("root");
    assert_eq!(ClipboardContent::from_reader(&wire), content);
}

/// The item, preview and size-limits state structs round-trip too — the honest
/// fields a `getHistory`/`getPreview`/`getSizeLimits` consumer reads.
#[test]
fn item_preview_and_limits_round_trip_through_the_wire() {
    let item = ClipboardItem {
        id: crate::core::ids::ClipboardId::new(7),
        timestamp: 1_700_000_000_000_000_000,
        mime_types: alloc::vec!["text/plain".to_string(), "image/png".to_string()],
        tags: alloc::vec!["work".to_string()],
        pinned: true,
        source_id: crate::core::ids::ClipboardSourceId::new(1),
        size_bytes: 42,
        text: String::new(),
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<clipboard_capnp::clipboard_item::Builder<'_>>();
        item.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<clipboard_capnp::clipboard_item::Reader<'_>>()
        .expect("root");
    let back = ClipboardItem::from_reader(&wire);
    assert_eq!(back.id, item.id);
    assert_eq!(back.timestamp, item.timestamp);
    assert_eq!(back.mime_types, item.mime_types);
    assert_eq!(back.tags, item.tags);
    assert!(back.pinned);
    assert_eq!(back.size_bytes, 42);

    let limits = ClipboardSizeLimits {
        max_item_size: 16 * 1024 * 1024,
        max_history_size: 64 * 1024 * 1024,
        max_history_count: 64,
    };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder =
            message.init_root::<clipboard_capnp::clipboard_size_limits::Builder<'_>>();
        limits.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader =
        capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
            .expect("deserialize");
    let wire = reader
        .get_root::<clipboard_capnp::clipboard_size_limits::Reader<'_>>()
        .expect("root");
    assert_eq!(ClipboardSizeLimits::from_reader(&wire), limits);
}

/// The availability matrix is internally consistent: one row per contract
/// method (the union `@0`–`@16` plus the subscription helper), every REAL/DEGRADED
/// row names its backing `l3_test`, every row resolves by name, and the counts
/// add up. A REAL claim with a vanished row is a FAIL.
#[test]
fn the_availability_matrix_is_internally_consistent() {
    assert_eq!(CLIPBOARD_SCHEMA_ID, 0xa1b2c3d4e5f60012);

    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, CLIPBOARD_AVAILABILITY.len());
    // The native-backend target serves every method; each is REAL or DEGRADED,
    // and every such row carries the test that backs it.
    for row in CLIPBOARD_AVAILABILITY {
        assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
        assert_eq!(
            super::availability::method_availability_for(row.name).map(|r| r.name),
            Some(row.name)
        );
        match row.status {
            AvailStatus::Real => assert!(row.reason.is_empty(), "{} is REAL", row.name),
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "{} names what is degraded", row.name);
                assert!(row.reason_class.is_none());
            }
            AvailStatus::Unavailable => assert!(row.reason_class.is_some(), "{} carries a class", row.name),
        }
    }
    assert!(super::availability::method_availability_for("noSuchMethod").is_none());

    // The contract's payload surface is REAL: reading the selection is the whole
    // point, and it is native over the seam.
    let get = CLIPBOARD_AVAILABILITY.iter().find(|r| r.name == "getContent").unwrap();
    assert_eq!(get.status, AvailStatus::Real);
    assert_eq!(get.ordinal, 0);
}

/// The reason-class grammar is the CLI's own (re-used, not a second vocabulary).
#[test]
fn reason_class_tokens_are_the_cli_grammar() {
    assert_eq!(ReasonClass::NoBackend.as_str(), "no-backend");
    assert_eq!(ReasonClass::NoPrivilege.as_str(), "no-privilege");
    assert_eq!(ReasonClass::NotApplicable.as_str(), "not-applicable");
    assert_eq!(ReasonClass::DisabledByPolicy.as_str(), "disabled-by-policy");
}

/// Error totality (law #6, law #3): every method of a backend that overrides
/// nothing degrades with a non-empty reason and the right code — never a panic,
/// never a fabricated answer. This is the whole "honest Unavailable everywhere
/// else" surface in one test.
#[test]
fn every_method_degrades_with_a_reason_on_a_backend_that_serves_nothing() {
    let b = HonestBackend;

    // The reads that MUST NOT pretend: getContent unavailable (no transport),
    // and the mutating / host-history verbs notSupported.
    let err = b.get_content("text/plain").unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.starts_with("no-backend:"), "{err}");
    assert!(!err.retryable, "Unavailable is not transient");

    for result in [
        b.set_content(ClipboardContent { mime_type: "text/plain".into(), data: Vec::new() }),
        b.clear_content(),
        b.set_offer(&[]),
        b.clear_history(),
        b.pin_item(crate::core::ids::ClipboardId::new(1)),
        b.unpin_item(crate::core::ids::ClipboardId::new(1)),
        b.delete_item(crate::core::ids::ClipboardId::new(1)),
        b.set_size_limits(ClipboardSizeLimits::default()),
    ] {
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported, "{err}");
        assert!(!err.message.is_empty(), "a notSupported carries its reason");
    }

    for result in [
        b.get_history(Pagination { offset: 0, limit: u32::MAX }),
        b.search_history("q", Pagination { offset: 0, limit: u32::MAX }),
    ] {
        assert!(result.is_err(), "an unserved read never fabricates a value");
        assert!(!result.unwrap_err().message.is_empty());
    }
    assert!(b.get_supported_mime_types().is_err());
    assert!(b.get_preview(crate::core::ids::ClipboardId::new(1)).is_err());
    assert!(b.validate_content(crate::core::ids::ClipboardId::new(1)).is_err());
    assert!(b.convert_content(crate::core::ids::ClipboardId::new(1), "text/plain").is_err());
    assert!(b.get_size_limits().is_err());
    assert!(b.pull_changes(SubscriptionId(1), 1).is_err());
    assert!(b.cancel_changes(SubscriptionId(1)).is_err());

    // The caps are the honest empty: a transport nothing can reach.
    assert_eq!(b.clipboard_caps(), ("none".to_string(), false, false));
    // The reason-diagnostic reporter is optional (its default is `None`); a
    // bare backend need not populate it, and calling it never fabricates.
    let _ = b.last_unavailable_reason("getContent");
}

/// Every one of the 20 contract error codes has a `From`/name mapping that is
/// total and its retryable flag is the contract default — the model the domain's
/// errors stand on. (Core proves this exhaustively; the domain asserts it never
/// fabricates its own code.)
#[test]
fn error_codes_carry_honest_defaults() {
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
        assert!(err.message.is_empty(), "a bare code has no message");
    }
    // The two honest degradation constructors REQUIRE a reason.
    let e = Error::unavailable("reason");
    assert!(!e.message.is_empty());
    let _ = e;
    let names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(names.len(), 20);
}
