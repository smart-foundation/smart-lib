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
    /// The named thing does not exist — with the engine's own reason (the
    /// host's measured words are never dropped on the floor).
    NotFound(String),
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
    /// The operator cancelled — with the question they declined, so a log
    /// says what was refused and by whom, never a bare "cancelled".
    Cancelled(String),
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
    pub fn cancelled(reason: impl Into<String>) -> Self {
        SmartError::Cancelled(reason.into())
    }
}

impl fmt::Display for SmartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmartError::NotFound(reason) => write!(f, "{reason}"),
            SmartError::Failed(m) => write!(f, "{m}"),
            SmartError::InvalidArgument(m) => write!(f, "{m}"),
            SmartError::Unavailable(m) => write!(f, "{m}"),
            SmartError::NotSupported(m) => write!(f, "{m}"),
            SmartError::Io(m) => write!(f, "{m}"),
            SmartError::Cancelled(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for SmartError {}

/// The error's stable kind, the token a script's JSON answer carries. It is
/// the shell's own vocabulary, and it is the ONLY thing a machine consumes:
/// the message beside it is for the operator who reads it.
pub fn kind(e: &SmartError) -> &'static str {
    match e {
        SmartError::NotFound(_) => "not_found",
        SmartError::Failed(_) => "failed",
        SmartError::InvalidArgument(_) => "invalid_argument",
        SmartError::Unavailable(_) => "unavailable",
        SmartError::NotSupported(_) => "not_supported",
        SmartError::Io(_) => "io",
        SmartError::Cancelled(_) => "cancelled",
    }
}

/// A script-mode answer as JSON: the standard error kind plus the reason.
/// `--json` / piped / non-TTY refusals answer this way — deterministic, never
/// a prose line a parser would have to guess at, and never a silent exit 0.
pub fn json_error(e: &SmartError) -> String {
    let mut out = String::from("{\"error\":{\"kind\":\"");
    out.push_str(kind(e));
    out.push_str("\",\"message\":");
    let message = e.to_string();
    let mut escaped = String::new();
    for c in message.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if (c as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    out.push('"');
    out.push_str(&escaped);
    out.push_str("\"}}");
    out
}

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
            ErrorCode::NotFound => SmartError::NotFound(reason),
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
