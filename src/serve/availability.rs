//! The registry's availability vocabulary: how a domain's **own** matrix row
//! becomes the contract's `AvailabilityReason` and the registration's own
//! metadata.
//!
//! # One authority on the surface
//!
//! `cli.capnp` says the registry is the authority on what commands exist and
//! what each one needs, and that a client that guesses is a client that
//! lies. So the reason a command cannot run must be the ENGINE's reason —
//! read from the domain's availability matrix, never a string invented here
//! and never a placeholder standing in for an answer.
//!
//! The matrix's four reason classes map onto the contract's own
//! `AvailabilityReason`:
//!
//! | the matrix's class | the registry's answer | what it means to a client |
//! |---|---|---|
//! | `no-backend` | `serviceUnavailable` | the backing service this command needs is not there |
//! | `no-privilege` | `capabilityDenied` | the caller does not hold the capability |
//! | `not-applicable` | `unsupportedPlatform` | the concept does not exist on this host |
//! | `disabled-by-policy` | `disabledByPolicy` | held back deliberately |
//! | anything else | `unknown` | explicitly distinct, so a gap is visible |
//!
//! The `reasonClass` token and the row's own words travel together in the
//! registration's open metadata under the `reason` key — the same pair the
//! runtime answer carries on the error channel, so the surface and the call
//! agree.

use alloc::string::String;
use alloc::string::ToString;

use crate::schema::cli_capnp::AvailabilityReason;

/// The registry's answer for one availability-matrix row.
#[derive(Clone, Debug)]
pub struct MatrixRow {
    /// The contract method the row describes.
    pub name: &'static str,
    /// The row's status: `"REAL"`, `"DEGRADED"` or `"UNAVAILABLE"`.
    pub status: &'static str,
    /// The row's reason class token, exactly when the row is `UNAVAILABLE`.
    pub class: Option<&'static str>,
    /// The row's own words: why it is unavailable, or which capability is
    /// degraded. Empty for a `REAL` row.
    pub reason: &'static str,
}

impl MatrixRow {
    /// The row's reason class and its own words, when the row is
    /// `UNAVAILABLE`: the pair the registry publishes and the runtime
    /// answer carries.
    pub fn unavailable_class(&self) -> Option<(&'static str, &'static str)> {
        if self.status != "UNAVAILABLE" {
            return None;
        }
        Some((self.class.unwrap_or("unknown"), self.reason))
    }

    /// The registry's unavailable answer for this row, when it has one: the
    /// contract's `AvailabilityReason`, and the row's own words with the
    /// reason class token in front of them.
    pub fn unavailable(&self) -> Option<(AvailabilityReason, String)> {
        let (class, reason) = self.unavailable_class()?;
        Some((reason_for(class), detail(class, reason)))
    }

    /// The row's degraded note, when the row is `DEGRADED`: the named
    /// degraded capability, published beside the command that still answers.
    pub fn degraded(&self) -> Option<&'static str> {
        if self.status != "DEGRADED" || self.reason.is_empty() {
            return None;
        }
        Some(self.reason)
    }
}

/// The contract's `AvailabilityReason` a reason class maps to.
pub fn reason_for(class: &str) -> AvailabilityReason {
    match class {
        "no-backend" => AvailabilityReason::ServiceUnavailable,
        "no-privilege" => AvailabilityReason::CapabilityDenied,
        "not-applicable" => AvailabilityReason::UnsupportedPlatform,
        "disabled-by-policy" => AvailabilityReason::DisabledByPolicy,
        _ => AvailabilityReason::Unknown,
    }
}

/// The detail line the registration publishes: the class token, then the
/// row's own words — the same shape the runtime answer carries.
pub fn detail(class: &str, reason: &str) -> String {
    if reason.is_empty() {
        class.to_string()
    } else {
        alloc::format!("{class}: {reason}")
    }
}

/// Read one contract method's row out of a domain's own matrix.
///
/// Every domain declares its **own** `AvailStatus` and `ReasonClass` types
/// (law #2: a domain owns its own files), so the reader takes the name of
/// the domain's `availability` module — the one the domain's serve module
/// imports — and speaks the registry's vocabulary.
///
/// The macro is expanded inside the domain's own serve module, so the
/// module's path is written once.
macro_rules! matrix_reader {
    ($availability:ident) => {
        /// The registry's row for one contract method of this domain, read
        /// from the domain's own availability matrix.
        pub fn matrix_row(name: &str) -> Option<$crate::serve::availability::MatrixRow> {
            let row = $availability::method_availability_for(name)?;
            Some($crate::serve::availability::MatrixRow {
                name: row.name,
                status: match row.status {
                    $availability::AvailStatus::Real => "REAL",
                    $availability::AvailStatus::Degraded => "DEGRADED",
                    $availability::AvailStatus::Unavailable => "UNAVAILABLE",
                },
                class: row.reason_class.map(|class| class.as_str()),
                reason: row.reason,
            })
        }
    };
}

pub(crate) use matrix_reader;
