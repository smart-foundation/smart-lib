//! The error vocabulary, and the one rule it exists to keep: **a refusal the
//! service sent is not the same thing as a client that fell over.**
//!
//! The engine answers a call it will not serve with a `capnp::Error` whose
//! text begins with the contract's reason-class token (`no-backend: …`,
//! `permissionDenied: …`, `notSupported: …`, `notFound: …`,
//! `invalidArgument: …`). That token is *load-bearing*: it is the difference
//! between "the host has no clipboard transport" and "your connection is
//! read-only" and "you asked a nonsense question". A product that flattened
//! them all into "something went wrong" would be throwing away the most
//! useful thing the protocol says.
//!
//! So [`Refusal`] keeps the class, the human sentence, and the verb that was
//! refused, and the UI renders all three — plus a "copy the exact reason"
//! action, so a user can paste the truth into a bug report instead of a
//! screenshot of a spinner.

use std::fmt;

/// The class of an answer the service declined to give.
///
/// `Open` is the arm for a reason class this build has not learned yet: the
/// vocabulary is the contract's, it grows, and a client that panics on an
/// unknown token is a client that breaks on a correct service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefusalClass {
    /// `no-backend` — the host (or this build of the service) cannot do it.
    NoBackend,
    /// `no-privilege` — the host can, the caller's credentials cannot.
    NoPrivilege,
    /// `not-applicable` — the concept does not exist on this platform.
    NotApplicable,
    /// `disabled-by-policy` — it exists and is held back deliberately.
    DisabledByPolicy,
    /// `permissionDenied` — this connection's capability class is read-only.
    PermissionDenied,
    /// `notSupported` — the interface is served, the method is not.
    NotSupported,
    /// `notFound` — the thing named does not exist (a retired item, say).
    NotFound,
    /// `invalidArgument` — the caller asked something malformed.
    InvalidArgument,
    /// `unavailable` — temporarily cannot answer.
    Unavailable,
    /// A class token this build does not know. Carried verbatim.
    Open(String),
}

impl RefusalClass {
    /// Read the class out of the engine's reason text.
    ///
    /// The engine's convention is `"<class>: <sentence>"`; a message with no
    /// such prefix is honestly `NotSupported` with the whole text preserved
    /// (never a fabricated class).
    pub fn from_reason(reason: &str) -> Self {
        let token = reason.split_once(':').map(|(token, _)| token.trim());
        match token {
            Some("no-backend") => RefusalClass::NoBackend,
            Some("no-privilege") => RefusalClass::NoPrivilege,
            Some("not-applicable") => RefusalClass::NotApplicable,
            Some("disabled-by-policy") => RefusalClass::DisabledByPolicy,
            Some("permissionDenied") => RefusalClass::PermissionDenied,
            Some("notSupported") => RefusalClass::NotSupported,
            Some("notFound") => RefusalClass::NotFound,
            Some("invalidArgument") => RefusalClass::InvalidArgument,
            Some("unavailable") | Some("serviceUnavailable") => RefusalClass::Unavailable,
            Some(other) if !other.is_empty() => RefusalClass::Open(other.to_string()),
            _ => RefusalClass::NotSupported,
        }
    }

    /// The token the UI shows in the reason chip. Stable text, so a user can
    /// search an issue tracker for it.
    pub fn token(&self) -> &str {
        match self {
            RefusalClass::NoBackend => "no-backend",
            RefusalClass::NoPrivilege => "no-privilege",
            RefusalClass::NotApplicable => "not-applicable",
            RefusalClass::DisabledByPolicy => "disabled-by-policy",
            RefusalClass::PermissionDenied => "permissionDenied",
            RefusalClass::NotSupported => "notSupported",
            RefusalClass::NotFound => "notFound",
            RefusalClass::InvalidArgument => "invalidArgument",
            RefusalClass::Unavailable => "unavailable",
            RefusalClass::Open(token) => token,
        }
    }

    /// Whether this refusal is about *authority* rather than capability.
    ///
    /// The distinction drives the UI: a `PermissionDenied` gets a "reconnect
    /// with a control-capable connection" hint, while a `NoBackend` gets
    /// "start a service that serves a clipboard backend" — different fixes,
    /// so different words.
    pub fn is_authority(&self) -> bool {
        matches!(
            self,
            RefusalClass::PermissionDenied | RefusalClass::NoPrivilege
        )
    }
}

impl fmt::Display for RefusalClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

/// The service declined a verb, and said why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The contract method that was refused (`getHistory`, `setContent`, …).
    pub verb: String,
    /// The reason class the engine published.
    pub class: RefusalClass,
    /// The reason sentence, prefix stripped. Never empty: a refusal with no
    /// words is treated as a client fault by [`Refusal::from_message`].
    pub reason: String,
    /// The raw text the wire carried, kept for the "copy the exact reason"
    /// action. Evidence beats a paraphrase.
    pub raw: String,
}

/// The prefix capnp-rpc decorates a *remote* exception with. It is the
/// transport's wrapping, not the contract's vocabulary, so it is stripped
/// before the class is parsed.
const REMOTE_PREFIX: &str = "remote exception: ";

impl Refusal {
    /// Build a refusal from an error the wire delivered.
    pub fn from_error(verb: &str, error: &capnp::Error) -> Self {
        let raw = error.extra.clone();
        let stripped = raw.strip_prefix(REMOTE_PREFIX).unwrap_or(&raw);
        let class = RefusalClass::from_reason(stripped);
        let reason = match stripped.split_once(':') {
            Some((token, rest)) if is_known_token(token.trim()) => rest.trim().to_string(),
            _ => stripped.trim().to_string(),
        };
        Refusal {
            verb: verb.to_string(),
            class,
            reason,
            raw: stripped.to_string(),
        }
    }

    /// Build a refusal from a reason text that already carries its class
    /// prefix — the shape the contract's `T.Error.message` uses inside a
    /// `VoidResult`. Used when a *served* method answers "no" through the
    /// contract's own error struct rather than through an RPC exception.
    pub fn from_reason_text(verb: &str, message: &str) -> Self {
        let class = RefusalClass::from_reason(message);
        let reason = match message.split_once(':') {
            Some((token, rest)) if is_known_token(token.trim()) => rest.trim().to_string(),
            _ => message.trim().to_string(),
        };
        Refusal {
            verb: verb.to_string(),
            class,
            reason,
            raw: message.trim().to_string(),
        }
    }

    /// The sentence a user reads: class and reason, in the engine's words.
    pub fn headline(&self) -> String {
        if self.reason.is_empty() {
            format!(
                "{}: the service declined {} without a reason",
                self.class, self.verb
            )
        } else {
            format!("{}: {}", self.class, self.reason)
        }
    }
}

fn is_known_token(token: &str) -> bool {
    !matches!(
        RefusalClass::from_reason(&format!("{token}: x")),
        RefusalClass::Open(_)
    )
}

/// Everything that can go wrong on the client side.
#[derive(Debug)]
pub enum ClientError {
    /// No service was reachable at any candidate endpoint.
    NoService {
        /// Every endpoint that was tried, in order, with its outcome.
        attempts: Vec<(String, String)>,
    },
    /// The socket exists but the process can speak to it and the handshake
    /// did not complete.
    Transport(String),
    /// The service is reachable and does not serve the clipboard domain.
    DomainNotServed {
        /// The names the discovery interface *did* publish, so the user can
        /// see the difference between "wrong daemon" and "old daemon".
        published: Vec<String>,
    },
    /// The service declined the call, with the contract's own reason.
    Refused(Refusal),
    /// A call did not answer inside the client's patience.
    Timeout {
        /// The verb that timed out.
        verb: String,
        /// How long the client waited, in milliseconds.
        millis: u64,
    },
    /// The payload would exceed the client's own bound. This is a *client
    /// policy*, labelled as such wherever it is shown: the contract does not
    /// impose it and the service did not refuse it.
    PayloadTooLarge {
        /// Measured size the service reported.
        bytes: u64,
        /// The client's ceiling.
        limit: u64,
    },
    /// A capnp message did not decode: a schema/ABI problem, not a refusal.
    Decode(String),
}

impl ClientError {
    /// A short, stable machine-ish label for the UI's status line.
    pub fn label(&self) -> &'static str {
        match self {
            ClientError::NoService { .. } => "no-service",
            ClientError::Transport(_) => "transport",
            ClientError::DomainNotServed { .. } => "domain-not-served",
            ClientError::Refused(_) => "refused",
            ClientError::Timeout { .. } => "timeout",
            ClientError::PayloadTooLarge { .. } => "client-limit",
            ClientError::Decode(_) => "decode",
        }
    }

    /// The refusal, when this error *is* one.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            ClientError::Refused(refusal) => Some(refusal),
            _ => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::NoService { attempts } => {
                write!(
                    f,
                    "no smart service answered; tried {} endpoint(s)",
                    attempts.len()
                )
            }
            ClientError::Transport(detail) => write!(f, "transport: {detail}"),
            ClientError::DomainNotServed { published } => write!(
                f,
                "this service does not serve the clipboard domain (it publishes: {})",
                if published.is_empty() {
                    "nothing".to_string()
                } else {
                    published.join(", ")
                }
            ),
            ClientError::Refused(refusal) => f.write_str(&refusal.headline()),
            ClientError::Timeout { verb, millis } => {
                write!(f, "{verb} did not answer within {millis} ms")
            }
            ClientError::PayloadTooLarge { bytes, limit } => write!(
                f,
                "client-limit: {bytes} bytes is larger than this app will fetch ({limit} bytes)"
            ),
            ClientError::Decode(detail) => write!(f, "decode: {detail}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<capnp::Error> for ClientError {
    fn from(error: capnp::Error) -> Self {
        if error.kind == capnp::ErrorKind::Disconnected {
            ClientError::Transport(format!("the service disconnected: {}", error.extra))
        } else {
            ClientError::Refused(Refusal::from_error("call", &error))
        }
    }
}
