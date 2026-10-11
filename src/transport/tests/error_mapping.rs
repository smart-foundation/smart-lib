//! The error mapping is total: every `TransportError` variant maps to
//! exactly one contract `T.ErrorCode`, and the mapping is asserted here
//! (the same L1 totality law core owns, applied to this seam).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::schema::types_capnp;
use crate::transport::channel::{CloseReason, TransportError};

fn code(error: &TransportError) -> types_capnp::ErrorCode {
    error.error_code()
}

#[test]
fn every_variant_maps_to_a_declared_code() {
    let cases: Vec<(TransportError, types_capnp::ErrorCode)> = vec![
        (TransportError::Empty, types_capnp::ErrorCode::Temporary),
        (
            TransportError::Closed {
                reason: CloseReason::Shutdown,
            },
            types_capnp::ErrorCode::Cancelled,
        ),
        (
            TransportError::Closed {
                reason: CloseReason::Protocol {
                    detail: "x".into(),
                },
            },
            types_capnp::ErrorCode::ProtocolViolation,
        ),
        (
            TransportError::AlreadyClosed,
            types_capnp::ErrorCode::Failed,
        ),
        (
            TransportError::Framing {
                detail: "x".into(),
            },
            types_capnp::ErrorCode::ProtocolViolation,
        ),
        (
            TransportError::LimitExceeded { limit: 8 },
            types_capnp::ErrorCode::LimitExceeded,
        ),
        (
            TransportError::Connect {
                detail: "x".into(),
            },
            types_capnp::ErrorCode::Unavailable,
        ),
        (
            TransportError::UnsupportedCapability {
                kind: "region".into(),
                detail: "x".into(),
            },
            types_capnp::ErrorCode::NotSupported,
        ),
        (
            TransportError::BufferNotResolvable { id: 1, detail: "x".into() },
            types_capnp::ErrorCode::NotFound,
        ),
        (
            TransportError::ArenaNotEstablished,
            types_capnp::ErrorCode::Unavailable,
        ),
        (
            TransportError::Message { detail: "x".into() },
            types_capnp::ErrorCode::Failed,
        ),
        (
            TransportError::NotCompiled {
                transport: crate::transport::channel::TransportKind::Tcp,
            },
            types_capnp::ErrorCode::NotSupported,
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(code(&error), expected, "{error:?} maps to its declared code");
    }
}

#[test]
fn host_errnos_map_honestly() {
    // Linux errno values, mapped by the transport seam.
    let perms = TransportError::Host {
        errno: 13,
        detail: "EACCES".into(),
    };
    assert_eq!(code(&perms), types_capnp::ErrorCode::PermissionDenied);
    let eperm = TransportError::Host {
        errno: 1,
        detail: "EPERM".into(),
    };
    assert_eq!(code(&eperm), types_capnp::ErrorCode::PermissionDenied);
    let enoent = TransportError::Host {
        errno: 2,
        detail: "ENOENT".into(),
    };
    assert_eq!(code(&enoent), types_capnp::ErrorCode::NotFound);
    let reset = TransportError::Host {
        errno: 104,
        detail: "ECONNRESET".into(),
    };
    assert_eq!(code(&reset), types_capnp::ErrorCode::Unavailable);
    let again = TransportError::Host {
        errno: 11,
        detail: "EAGAIN".into(),
    };
    assert_eq!(code(&again), types_capnp::ErrorCode::Temporary);
}

#[test]
fn details_are_never_empty_and_errors_write_t_error() {
    let errors = [
        TransportError::Empty,
        TransportError::Closed {
            reason: CloseReason::Shutdown,
        },
        TransportError::AlreadyClosed,
        TransportError::Framing {
            detail: "bad length".into(),
        },
        TransportError::LimitExceeded { limit: 1 },
        TransportError::Connect {
            detail: "refused".into(),
        },
        TransportError::UnsupportedCapability {
            kind: "live handle".into(),
            detail: "cannot cross".into(),
        },
        TransportError::BufferNotResolvable {
            id: 5,
            detail: "no namespace".into(),
        },
        TransportError::ArenaNotEstablished,
        TransportError::Message {
            detail: "parse failed".into(),
        },
        TransportError::Host {
            errno: 13,
            detail: "denied".into(),
        },
        TransportError::NotCompiled {
            transport: crate::transport::channel::TransportKind::UnixSocket,
        },
    ];
    for error in errors {
        assert!(!error.detail().is_empty(), "{error:?} has a detail");
        // The generated `T.Error` builder is filled with the same mapping.
        let mut message = capnp::message::Builder::new_default();
        error.write_t_error(message.init_root::<types_capnp::error::Builder<'_>>());
        let reader = message
            .get_root_as_reader::<types_capnp::error::Reader<'_>>()
            .expect("root");
        assert_eq!(reader.get_code().expect("code"), error.error_code());
        assert!(!reader.get_message().expect("message").is_empty());
        assert_eq!(
            reader.get_retryable(),
            matches!(error, TransportError::Empty),
            "only Empty claims retryable"
        );
    }
}

#[test]
fn close_reason_wire_codes_are_stable_and_unknown_codes_survive() {
    assert_eq!(CloseReason::Shutdown.wire_code(), 1);
    assert_eq!(CloseReason::PeerClosed.wire_code(), 2);
    assert_eq!(CloseReason::Cancelled.wire_code(), 3);
    // An unknown code is forwarded, never dropped (unknown-tolerance).
    match CloseReason::from_wire(99, String::new()) {
        CloseReason::Protocol { detail } => assert!(detail.contains("99")),
        other => panic!("expected a Protocol detail, got {other:?}"),
    }
}
