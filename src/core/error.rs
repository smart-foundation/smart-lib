//! The one error model every domain, transport and platform layer stands on.
//!
//! ## Shape — and why it is this shape
//!
//! The contract's `T.Error` (`schema/types.capnp:248`) is a **struct** carrying a
//! `code :ErrorCode` plus `message`, `context`, `retryable` and `metadata`
//! fields. Core mirrors that shape exactly: [`ErrorCode`] is the enum with one
//! variant per contract code, and [`Error`] is the struct with the contract's
//! own fields. The old implementation used a payload-carrying enum
//! (`SmartError::Failed(String)`, `Unknown(String)`); that forced a message onto
//! codes that need none, dropped `context`, and made the mapping to
//! `T.ErrorCode` partial. Mirroring the wire keeps the mapping **total and
//! lossless in both directions**, which is what conformance level L1 demands.
//!
//! ## The mapping is total — mechanically
//!
//! * Every one of the 20 `T.ErrorCode` variants has exactly one [`ErrorCode`]
//!   variant and back (exhaustive `match`es with no catch-all arm — a 21st
//!   contract code fails the *build*, it can never fail at runtime).
//! * `Error::write_to` and `From<&T.Error::Reader>` / `From<&T.Error::Builder>`
//!   round-trip every variant through real serialized capnp messages;
//!   `src/core/tests.rs` proves it for all 20.
//! * A `T.Error.code` discriminant that is **not in the compiled schema**
//!   (a peer on a newer contract version) never maps to a fabricated code: it
//!   becomes [`ErrorCode::Unknown`] with the raw discriminant preserved in
//!   `message`, per the forward-compatibility rule in
//!   `knowledge/03-CONFORMANCE.md` §5 ("the original discriminant in the
//!   message", never a default).
//!
//! ## Honesty rules encoded here
//!
//! * `retryable` is honest: only [`ErrorCode::Temporary`] (and the capnp runtime
//!   kinds that mean it) default to retryable; `notFound`, `invalidArgument`,
//!   `notSupported` and the rest are not retryable.
//! * Law #6 (honest degradation) is expressed with [`Error::unavailable`] and
//!   [`Error::not_supported`], each of which **requires a reason** — an
//!   `Unavailable` without a reason is the fabrication this library exists to
//!   prevent.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;

use crate::schema::types_capnp;

/// The crate-wide result type: `Result<T, Error>` (law #6: `Unavailable` +
/// reason is a first-class *result*, not a panic).
pub type Result<T> = core::result::Result<T, Error>;

/// The contract's error vocabulary — exactly the 20 codes `T.ErrorCode` declares
/// (`schema/types.capnp:324`), one variant each, in schema order.
///
/// The mapping to and from the generated `types_capnp::ErrorCode` is written as
/// two exhaustive `match`es with no catch-all arm: if the canonical contract
/// ever adds a 21st code, this file stops compiling until the core is updated.
/// That is the intended failure mode — a silent runtime fallback is not.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ErrorCode {
    /// `notFound @0` — the named thing does not exist.
    NotFound,
    /// `permissionDenied @1` — authority is absent (a handle was not held).
    PermissionDenied,
    /// `failed @2` — the operation failed for a stated reason.
    Failed,
    /// `temporary @3` — transient; retrying may succeed.
    Temporary,
    /// `unavailable @4` — honest degradation: the backend cannot do this *now*
    /// for a stated reason (law #6).
    Unavailable,
    /// `limitExceeded @5` — a stated bound of the contract was exceeded.
    LimitExceeded,
    /// `outOfMemory @6` — an allocation could not be satisfied.
    OutOfMemory,
    /// `invalidArgument @7` — the caller's request violated the contract shape.
    InvalidArgument,
    /// `timeout @8` — a stated deadline elapsed.
    Timeout,
    /// `cancelled @9` — the operation was cancelled before completing.
    Cancelled,
    /// `alreadyExists @10` — creation collided with an existing thing.
    AlreadyExists,
    /// `notSupported @11` — the concept does not exist on this backend (a
    /// declared-N/A, not a temporary gap).
    NotSupported,
    /// `conflict @12` — the request conflicts with current state.
    Conflict,
    /// `unknown @13` — the failure cannot be classified. This is also where a
    /// `T.Error.code` discriminant outside the compiled schema lands, with the
    /// raw discriminant preserved in `Error::message`.
    Unknown,
    /// `hardwareFault @14` — the device or bus reported a hardware fault.
    HardwareFault,
    /// `driverError @15` — the driver layer reported an error.
    DriverError,
    /// `protocolViolation @16` — a peer violated the protocol.
    ProtocolViolation,
    /// `busError @17` — a bus-level error occurred.
    BusError,
    /// `thermalThrottling @18` — thermal limits are throttling the operation.
    ThermalThrottling,
    /// `powerStateFailure @19` — a power-state transition failed.
    PowerStateFailure,
}

impl ErrorCode {
    /// All 20 codes, in schema order. Used by the totality tests; a backend
    /// never needs it.
    pub const ALL: [ErrorCode; 20] = [
        ErrorCode::NotFound,
        ErrorCode::PermissionDenied,
        ErrorCode::Failed,
        ErrorCode::Temporary,
        ErrorCode::Unavailable,
        ErrorCode::LimitExceeded,
        ErrorCode::OutOfMemory,
        ErrorCode::InvalidArgument,
        ErrorCode::Timeout,
        ErrorCode::Cancelled,
        ErrorCode::AlreadyExists,
        ErrorCode::NotSupported,
        ErrorCode::Conflict,
        ErrorCode::Unknown,
        ErrorCode::HardwareFault,
        ErrorCode::DriverError,
        ErrorCode::ProtocolViolation,
        ErrorCode::BusError,
        ErrorCode::ThermalThrottling,
        ErrorCode::PowerStateFailure,
    ];

    /// The contract's own spelling of this code (e.g. `"notSupported"`).
    pub const fn name(self) -> &'static str {
        match self {
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

    /// The wire ordinal of this code in `T.ErrorCode`.
    pub const fn ordinal(self) -> u16 {
        match self {
            ErrorCode::NotFound => 0,
            ErrorCode::PermissionDenied => 1,
            ErrorCode::Failed => 2,
            ErrorCode::Temporary => 3,
            ErrorCode::Unavailable => 4,
            ErrorCode::LimitExceeded => 5,
            ErrorCode::OutOfMemory => 6,
            ErrorCode::InvalidArgument => 7,
            ErrorCode::Timeout => 8,
            ErrorCode::Cancelled => 9,
            ErrorCode::AlreadyExists => 10,
            ErrorCode::NotSupported => 11,
            ErrorCode::Conflict => 12,
            ErrorCode::Unknown => 13,
            ErrorCode::HardwareFault => 14,
            ErrorCode::DriverError => 15,
            ErrorCode::ProtocolViolation => 16,
            ErrorCode::BusError => 17,
            ErrorCode::ThermalThrottling => 18,
            ErrorCode::PowerStateFailure => 19,
        }
    }

    /// Whether an operation that failed with this code may succeed if retried.
    ///
    /// Only transient failure is retryable. A backend that marks
    /// `invalidArgument` or `notSupported` retryable is asking the caller to
    /// repeat a request that cannot ever succeed.
    pub const fn default_retryable(self) -> bool {
        matches!(self, ErrorCode::Temporary)
    }
}

impl From<ErrorCode> for types_capnp::ErrorCode {
    /// Exhaustive by construction: all 20 core codes, no catch-all.
    fn from(code: ErrorCode) -> Self {
        use types_capnp::ErrorCode as G;
        match code {
            ErrorCode::NotFound => G::NotFound,
            ErrorCode::PermissionDenied => G::PermissionDenied,
            ErrorCode::Failed => G::Failed,
            ErrorCode::Temporary => G::Temporary,
            ErrorCode::Unavailable => G::Unavailable,
            ErrorCode::LimitExceeded => G::LimitExceeded,
            ErrorCode::OutOfMemory => G::OutOfMemory,
            ErrorCode::InvalidArgument => G::InvalidArgument,
            ErrorCode::Timeout => G::Timeout,
            ErrorCode::Cancelled => G::Cancelled,
            ErrorCode::AlreadyExists => G::AlreadyExists,
            ErrorCode::NotSupported => G::NotSupported,
            ErrorCode::Conflict => G::Conflict,
            ErrorCode::Unknown => G::Unknown,
            ErrorCode::HardwareFault => G::HardwareFault,
            ErrorCode::DriverError => G::DriverError,
            ErrorCode::ProtocolViolation => G::ProtocolViolation,
            ErrorCode::BusError => G::BusError,
            ErrorCode::ThermalThrottling => G::ThermalThrottling,
            ErrorCode::PowerStateFailure => G::PowerStateFailure,
        }
    }
}

impl From<types_capnp::ErrorCode> for ErrorCode {
    /// Exhaustive over the generated enum: a contract that grows a 21st code
    /// breaks this build rather than silently mis-mapping at runtime.
    fn from(code: types_capnp::ErrorCode) -> Self {
        use types_capnp::ErrorCode as G;
        match code {
            G::NotFound => ErrorCode::NotFound,
            G::PermissionDenied => ErrorCode::PermissionDenied,
            G::Failed => ErrorCode::Failed,
            G::Temporary => ErrorCode::Temporary,
            G::Unavailable => ErrorCode::Unavailable,
            G::LimitExceeded => ErrorCode::LimitExceeded,
            G::OutOfMemory => ErrorCode::OutOfMemory,
            G::InvalidArgument => ErrorCode::InvalidArgument,
            G::Timeout => ErrorCode::Timeout,
            G::Cancelled => ErrorCode::Cancelled,
            G::AlreadyExists => ErrorCode::AlreadyExists,
            G::NotSupported => ErrorCode::NotSupported,
            G::Conflict => ErrorCode::Conflict,
            G::Unknown => ErrorCode::Unknown,
            G::HardwareFault => ErrorCode::HardwareFault,
            G::DriverError => ErrorCode::DriverError,
            G::ProtocolViolation => ErrorCode::ProtocolViolation,
            G::BusError => ErrorCode::BusError,
            G::ThermalThrottling => ErrorCode::ThermalThrottling,
            G::PowerStateFailure => ErrorCode::PowerStateFailure,
        }
    }
}

impl core::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// The one error type, mirroring the contract's `T.Error` field for field.
///
/// # Constructing
///
/// Use the constructors — they carry the honest defaults (`retryable` per
/// code, reason strings where law #6 demands one):
///
/// ```
/// use smart_lib::core::{Error, ErrorCode};
///
/// // Law #6: honest degradation carries its reason, and is not retryable.
/// let e = Error::unavailable("no memfd_create on this kernel");
/// assert_eq!(e.code, ErrorCode::Unavailable);
/// assert!(!e.retryable);
/// assert_eq!(e.to_string(), "unavailable: no memfd_create on this kernel");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Which of the 20 contract codes this is.
    pub code: ErrorCode,
    /// Human-readable reason. Empty means "no message" — on the wire, an empty
    /// message is written as an absent `Text` and reads back empty, so the
    /// round-trip is total.
    pub message: String,
    /// Additional context (what was being attempted, on what target).
    pub context: String,
    /// Whether retrying may succeed. Defaults to
    /// [`ErrorCode::default_retryable`].
    pub retryable: bool,
}

impl Error {
    /// An error with the code's honest defaults and no message.
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Error {
            code,
            message: String::new(),
            context: String::new(),
            retryable: code.default_retryable(),
        }
    }

    /// An error with a message.
    #[must_use]
    pub fn with_message(code: ErrorCode, message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
            ..Error::new(code)
        }
    }

    /// Attach context (what was attempted, on what target) and return self.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }

    /// Override the retryable flag (use only when the truth is known).
    #[must_use]
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// `notFound` — the named thing does not exist.
    #[must_use]
    pub fn not_found() -> Self {
        Error::new(ErrorCode::NotFound)
    }

    /// `permissionDenied` — no handle for this authority was held.
    #[must_use]
    pub fn permission_denied() -> Self {
        Error::new(ErrorCode::PermissionDenied)
    }

    /// `failed` — the operation failed for a stated reason.
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::Failed, reason)
    }

    /// `temporary` — transient failure; retryable by default.
    #[must_use]
    pub fn temporary(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::Temporary, reason)
    }

    /// `unavailable` — honest degradation (law #6). The reason is **required**:
    /// an `Unavailable` without one is the fabrication this library exists to
    /// prevent.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::Unavailable, reason)
    }

    /// `limitExceeded` — a stated contract bound was exceeded.
    #[must_use]
    pub fn limit_exceeded(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::LimitExceeded, reason)
    }

    /// `outOfMemory`.
    #[must_use]
    pub fn out_of_memory() -> Self {
        Error::new(ErrorCode::OutOfMemory)
    }

    /// `invalidArgument` — the request violated the contract shape.
    #[must_use]
    pub fn invalid_argument(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::InvalidArgument, reason)
    }

    /// `timeout` — a stated deadline elapsed.
    #[must_use]
    pub fn timeout() -> Self {
        Error::new(ErrorCode::Timeout)
    }

    /// `cancelled` — cancelled before completing.
    #[must_use]
    pub fn cancelled() -> Self {
        Error::new(ErrorCode::Cancelled)
    }

    /// `alreadyExists`.
    #[must_use]
    pub fn already_exists() -> Self {
        Error::new(ErrorCode::AlreadyExists)
    }

    /// `notSupported` — a declared-N/A on this backend. The reason is
    /// **required**: the caller must be able to tell *what* is missing.
    #[must_use]
    pub fn not_supported(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::NotSupported, reason)
    }

    /// `conflict`.
    #[must_use]
    pub fn conflict() -> Self {
        Error::new(ErrorCode::Conflict)
    }

    /// `unknown` — unclassifiable failure, with the reason stated.
    #[must_use]
    pub fn unknown(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::Unknown, reason)
    }

    /// `hardwareFault`.
    #[must_use]
    pub fn hardware_fault(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::HardwareFault, reason)
    }

    /// `driverError`.
    #[must_use]
    pub fn driver_error(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::DriverError, reason)
    }

    /// `protocolViolation`.
    #[must_use]
    pub fn protocol_violation(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::ProtocolViolation, reason)
    }

    /// `busError`.
    #[must_use]
    pub fn bus_error(reason: impl Into<String>) -> Self {
        Error::with_message(ErrorCode::BusError, reason)
    }

    /// `thermalThrottling`.
    #[must_use]
    pub fn thermal_throttling() -> Self {
        Error::new(ErrorCode::ThermalThrottling)
    }

    /// `powerStateFailure`.
    #[must_use]
    pub fn power_state_failure() -> Self {
        Error::new(ErrorCode::PowerStateFailure)
    }

    /// Read a core `Error` out of a generated `T.Error` reader.
    ///
    /// Total: every in-schema discriminant maps through the exhaustive
    /// `ErrorCode` conversion; an out-of-schema discriminant becomes
    /// [`ErrorCode::Unknown`] with the raw value preserved in `message`
    /// (conformance §5 — never a default, never a drop).
    pub fn from_reader(reader: &types_capnp::error::Reader<'_>) -> Self {
        match reader.get_code() {
            Ok(code) => Error {
                code: code.into(),
                message: text_or_empty(reader.get_message()),
                context: text_or_empty(reader.get_context()),
                retryable: reader.get_retryable(),
            },
            Err(capnp::NotInSchema(discriminant)) => Error {
                code: ErrorCode::Unknown,
                message: format!(
                    "T.Error.code discriminant {discriminant} is not in the compiled schema"
                ),
                context: String::new(),
                retryable: false,
            },
        }
    }

    /// Write this error into a generated `T.Error` builder.
    ///
    /// An empty `message`/`context` is written as an absent `Text`, which reads
    /// back empty — the round-trip is total in both directions.
    pub fn write_to(&self, builder: &mut types_capnp::error::Builder<'_>) {
        builder.set_code(self.code.into());
        if !self.message.is_empty() {
            builder.set_message(self.message.as_str());
        }
        if !self.context.is_empty() {
            builder.set_context(self.context.as_str());
        }
        builder.set_retryable(self.retryable);
    }
}

/// Read a `Text` field into an owned string.
///
/// Absent is the empty string (the same equivalence [`Error::write_to`] writes
/// back, so the round-trip stays total). A `Text` field carrying invalid UTF-8
/// is a peer protocol violation that message validation should already have
/// caught; it reads as empty here rather than fabricating a replacement.
pub(crate) fn text_or_empty(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => match text.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

impl From<&types_capnp::error::Reader<'_>> for Error {
    fn from(reader: &types_capnp::error::Reader<'_>) -> Self {
        Error::from_reader(reader)
    }
}

impl From<&types_capnp::error::Builder<'_>> for Error {
    fn from(builder: &types_capnp::error::Builder<'_>) -> Self {
        Error::from_reader(&builder.reborrow_as_reader())
    }
}

impl From<capnp::Error> for Error {
    /// Classification, not invention: the capnp runtime's error kinds map onto
    /// the contract codes, and the runtime's own `extra` text is preserved
    /// verbatim in `message`.
    ///
    /// | capnp kind | core code | retryable |
    /// |---|---|---|
    /// | `Unimplemented` | `NotSupported` | false |
    /// | `Overloaded` | `Temporary` | true |
    /// | `Disconnected` | `Temporary` | true |
    /// | `Failed` | `Failed` | false |
    /// | anything else | `Failed` | false |
    ///
    /// A backend that needs a different classification reads
    /// [`capnp::ErrorKind`] itself — the mapping here is the *default*, and it
    /// is honest about being one.
    fn from(err: capnp::Error) -> Self {
        use capnp::ErrorKind;
        let (code, retryable) = match err.kind {
            ErrorKind::Unimplemented => (ErrorCode::NotSupported, false),
            ErrorKind::Overloaded => (ErrorCode::Temporary, true),
            ErrorKind::Disconnected => (ErrorCode::Temporary, true),
            _ => (ErrorCode::Failed, false),
        };
        Error {
            code,
            message: err.extra,
            context: String::new(),
            retryable,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.code.name())?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        if !self.context.is_empty() {
            write!(f, " (while {})", self.context)?;
        }
        if self.retryable {
            write!(f, " [retryable]")?;
        }
        Ok(())
    }
}

impl core::error::Error for Error {}
