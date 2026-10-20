//! The contract's error vocabulary, carried over capnp RPC's error channel.
//!
//! # Why the channel carries a token
//!
//! capnp RPC's error channel is a string plus a coarse [`capnp::ErrorKind`].
//! The contract's error model is richer: `T.Error` carries a **code** (one of
//! twenty), a message, a context and a retryable flag. The engine's own
//! [`Error`] mirrors that shape exactly.
//!
//! So the code travels as the **first word** of the error description, in
//! the contract's own spelling (`notSupported`, `notFound`, ...), and the
//! message follows. The mapping is total in both directions:
//!
//! * [`to_capnp`] writes the engine's error into the channel, code first;
//! * [`from_capnp`] reads it back, code first, and a description that does
//!   not start with a known token falls back to the capnp kind — never to a
//!   fabricated code.
//!
//! Nothing is lost across the RPC boundary, and a client that only speaks
//! capnp still gets a usable kind.

use alloc::format;
use alloc::string::{String, ToString};

use crate::core::{Error, ErrorCode};

/// The contract's own spelling of a code. This is the token that travels in
/// the error channel's first word.
pub fn code_token(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NotFound => "notFound",
        ErrorCode::PermissionDenied => "permissionDenied",
        ErrorCode::Failed => "failed",
        ErrorCode::Temporary => "temporary",
        ErrorCode::Unavailable => "unavailable",
        ErrorCode::LimitExceeded => "limitExceeded",
        ErrorCode::OutOfMemory => "outOfMemory",
        ErrorCode::InvalidArgument => "invalidArgument",
        ErrorCode::Timeout => "timeout",
        ErrorCode::Cancelled => "cancelled",
        ErrorCode::AlreadyExists => "alreadyExists",
        ErrorCode::NotSupported => "notSupported",
        ErrorCode::Conflict => "conflict",
        ErrorCode::Unknown => "unknown",
        ErrorCode::HardwareFault => "hardwareFault",
        ErrorCode::DriverError => "driverError",
        ErrorCode::ProtocolViolation => "protocolViolation",
        ErrorCode::BusError => "busError",
        ErrorCode::ThermalThrottling => "thermalThrottling",
        ErrorCode::PowerStateFailure => "powerStateFailure",
    }
}

/// Read a contract code token back. `None` for a token the contract does
/// not declare — never a guess.
pub fn code_from_token(token: &str) -> Option<ErrorCode> {
    Some(match token {
        "notFound" => ErrorCode::NotFound,
        "permissionDenied" => ErrorCode::PermissionDenied,
        "failed" => ErrorCode::Failed,
        "temporary" => ErrorCode::Temporary,
        "unavailable" => ErrorCode::Unavailable,
        "limitExceeded" => ErrorCode::LimitExceeded,
        "outOfMemory" => ErrorCode::OutOfMemory,
        "invalidArgument" => ErrorCode::InvalidArgument,
        "timeout" => ErrorCode::Timeout,
        "cancelled" => ErrorCode::Cancelled,
        "alreadyExists" => ErrorCode::AlreadyExists,
        "notSupported" => ErrorCode::NotSupported,
        "conflict" => ErrorCode::Conflict,
        "unknown" => ErrorCode::Unknown,
        "hardwareFault" => ErrorCode::HardwareFault,
        "driverError" => ErrorCode::DriverError,
        "protocolViolation" => ErrorCode::ProtocolViolation,
        "busError" => ErrorCode::BusError,
        "thermalThrottling" => ErrorCode::ThermalThrottling,
        "powerStateFailure" => ErrorCode::PowerStateFailure,
        _ => return None,
    })
}

/// The capnp kind an engine code travels as. The kind is the coarse "how
/// should the caller react" signal; the code (in the description) is the
/// precise one.
fn capnp_kind(code: ErrorCode) -> capnp::ErrorKind {
    match code {
        ErrorCode::NotSupported | ErrorCode::Unavailable | ErrorCode::InvalidArgument => {
            capnp::ErrorKind::Unimplemented
        }
        ErrorCode::Temporary | ErrorCode::Timeout | ErrorCode::OutOfMemory => {
            capnp::ErrorKind::Overloaded
        }
        ErrorCode::Cancelled => capnp::ErrorKind::Disconnected,
        _ => capnp::ErrorKind::Failed,
    }
}

/// The engine's code, read back from a capnp kind whose description carried
/// no token.
fn kind_code(kind: capnp::ErrorKind) -> ErrorCode {
    match kind {
        capnp::ErrorKind::Overloaded => ErrorCode::Temporary,
        capnp::ErrorKind::Disconnected => ErrorCode::Cancelled,
        capnp::ErrorKind::Unimplemented => ErrorCode::NotSupported,
        _ => ErrorCode::Failed,
    }
}

/// The engine's error as the description the channel carries: the contract's
/// code token first, then the message.
pub fn describe(error: &Error) -> String {
    if error.message.is_empty() {
        code_token(error.code).to_string()
    } else {
        format!("{}: {}", code_token(error.code), error.message)
    }
}

/// The engine's error, as a capnp error.
pub fn to_capnp(error: &Error) -> capnp::Error {
    capnp::Error {
        kind: capnp_kind(error.code),
        extra: describe(error),
    }
}

/// The prefix the RPC layer puts in front of a *remote* exception
/// ("remote exception: ..."). It is the transport's decoration, not the
/// contract's vocabulary, so the reader strips it before parsing the code
/// token.
const REMOTE_EXCEPTION_PREFIX: &str = "remote exception: ";

/// The engine's error, read back from the channel.
pub fn from_capnp(error: &capnp::Error) -> Error {
    let text = error.extra.as_str();
    let text = text
        .strip_prefix(REMOTE_EXCEPTION_PREFIX)
        .unwrap_or(text);
    let (token, rest) = match text.split_once(": ") {
        Some((head, tail)) => (head, tail),
        None => (text, ""),
    };
    match code_from_token(token) {
        Some(code) => Error {
            code,
            message: rest.to_string(),
            context: String::new(),
            retryable: code.default_retryable(),
        },
        None => Error {
            code: kind_code(error.kind),
            message: text.to_string(),
            context: String::new(),
            retryable: error.kind == capnp::ErrorKind::Overloaded,
        },
    }
}

/// The engine's error, as the contract's own `T.Error` struct (the shape
/// `T.VoidResult`'s error arm carries).
pub fn write_t_error(error: &Error, builder: &mut crate::schema::types_capnp::error::Builder<'_>) {
    builder.set_code(error.code.into());
    builder.set_message(&error.message);
    builder.set_context(&error.context);
    builder.set_retryable(error.retryable);
}

/// A capnp error for an answer the engine declines to give, carrying the
/// contract's own code token so the caller reads the contract's vocabulary.
pub fn decline(code: ErrorCode, message: &str) -> capnp::Error {
    let error = Error::with_message(code, message);
    to_capnp(&error)
}
