//! The shell's one error type: the contract's 20-code vocabulary, carried as
//! the engine's `core::Error`, plus the shell-level failures (parse, IO,
//! honest "not migrated yet" answers).
//!
//! Law #6 (honest degradation) is the load-bearing rule here: an unavailable
//! domain or verb carries its reason, and the reason is printed in the shell's
//! aesthetic — never a fabricated success.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartError {
    /// The named thing does not exist.
    NotFound,
    /// The operation failed for a stated reason.
    Failed(String),
    /// The request violated the shape (a bad selector, a bad pipe op).
    InvalidArgument(String),
    /// Honest degradation: the engine could not serve it, with the reason.
    Unavailable(String),
    /// The engine refuses the verb, with the reason (polkit, no backend).
    NotSupported(String),
    /// A host/filesystem error.
    Io(String),
    /// The operator cancelled.
    Cancelled,
}

impl SmartError {
    pub fn failed(reason: impl Into<String>) -> Self {
        SmartError::Failed(reason.into())
    }
    pub fn invalid_argument(reason: impl Into<String>) -> Self {
        SmartError::InvalidArgument(reason.into())
    }
    pub fn unavailable(reason: impl Into<String>) -> Self {
        SmartError::Unavailable(reason.into())
    }
    pub fn not_supported(reason: impl Into<String>) -> Self {
        SmartError::NotSupported(reason.into())
    }
    pub fn io(reason: impl Into<String>) -> Self {
        SmartError::Io(reason.into())
    }
}

impl fmt::Display for SmartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmartError::NotFound => write!(f, "not found"),
            SmartError::Failed(m) => write!(f, "{m}"),
            SmartError::InvalidArgument(m) => write!(f, "{m}"),
            SmartError::Unavailable(m) => write!(f, "{m}"),
            SmartError::NotSupported(m) => write!(f, "{m}"),
            SmartError::Io(m) => write!(f, "{m}"),
            SmartError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for SmartError {}

impl From<smart_lib::core::Error> for SmartError {
    /// The engine's error maps one-for-one: the code, the message and the
    /// context survive, because the reason is the honest part.
    fn from(e: smart_lib::core::Error) -> Self {
        use smart_lib::core::ErrorCode;
        let reason = if e.context.is_empty() {
            e.to_string()
        } else {
            format!("{e}")
        };
        match e.code {
            ErrorCode::NotFound => SmartError::NotFound,
            ErrorCode::Unavailable => SmartError::Unavailable(reason),
            ErrorCode::NotSupported => SmartError::NotSupported(reason),
            ErrorCode::InvalidArgument => SmartError::InvalidArgument(reason),
            ErrorCode::PermissionDenied => SmartError::NotSupported(reason),
            ErrorCode::Failed => SmartError::Failed(reason),
            _ => SmartError::Failed(reason),
        }
    }
}

impl From<std::io::Error> for SmartError {
    fn from(e: std::io::Error) -> Self {
        SmartError::Io(e.to_string())
    }
}

impl From<toml::de::Error> for SmartError {
    fn from(e: toml::de::Error) -> Self {
        SmartError::InvalidArgument(format!("config: {e}"))
    }
}

pub type SmartResult<T> = Result<T, SmartError>;
