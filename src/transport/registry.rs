//! The **open** transport registry: a transport is a registered
//! implementation, never an enum variant.
//!
//! # Why this file exists
//!
//! The tree used to name its transports in a closed enum
//! (`TransportKind::InProcess | Shmem | UnixSocket | Tcp`). That enum *was*
//! the hardcoded-paradigm anti-pattern the contract forbids
//! (`endpoint.capnp:9-24`: a transport set with exactly one outlet per
//! paradigm is not a protocol). A second implementation — the founder's own,
//! a 2046 one, a test double — could not be added without editing the
//! engine's shared files.
//!
//! Here a transport is:
//!
//! * a **scheme token** — an open string, the same kind of open token the
//!   contract's `ServiceEndpoint.transport` is (`discovery.capnp:122-130`);
//! * a **registered implementation** — anything that implements
//!   [`TransportImpl`], registered into a [`TransportRegistry`].
//!
//! Adding a fifth transport therefore touches **no** protocol crate and no
//! shared engine file: the implementation registers itself, and everything
//! that speaks "a transport" already speaks "a scheme token".
//!
//! # The unknown scheme is answered, never rejected
//!
//! A scheme nobody registered is an *unavailable-with-reason* answer
//! ([`TransportError::UnknownScheme`]): the caller is told which schemes
//! exist. It is never a crash, never a silent fallback to some other
//! transport, and never a "rejected" protocol error — the request was
//! well-formed; this engine simply does not serve that paradigm.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use crate::transport::channel::{
    CapabilityTransfer, Channel, ChannelListener, TransportError,
};

/// An open transport scheme token ("unix", "tcp", or a 2046 one).
///
/// The set is open by construction: a scheme names a registered
/// implementation, never an enum variant. The contract's own
/// `ServiceEndpoint.transport` is the same kind of open token
/// (`discovery.capnp:122-130`).
pub type Scheme = str;

/// The scheme token the `AF_UNIX` transport answers to.
pub const UNIX_SCHEME: &str = "unix";
/// The scheme token the TCP transport answers to.
pub const TCP_SCHEME: &str = "tcp";
/// The scheme token the same-process transport answers to.
pub const INPROCESS_SCHEME: &str = "inprocess";
/// The scheme token the shared-arena transport answers to.
pub const SHMEM_SCHEME: &str = "shmem";

/// What a registered transport declares about itself.
///
/// Every field is a *declaration* (charter law #6: published, never
/// guessed). A transport that cannot carry a capability says so here
/// instead of faking the transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportDescription {
    /// The scheme token this implementation answers to.
    pub scheme: &'static str,
    /// The framing it actually uses, as declared text.
    pub framing: &'static str,
    /// How a capability reference moves here.
    pub capability_transfer: CapabilityTransfer,
    /// Whether the transport carries a byte stream a foreign protocol
    /// (capnp RPC) can ride.
    pub byte_stream: bool,
    /// The address this transport binds by default, when the caller names
    /// none. `None` means the caller must name one.
    pub default_address: Option<&'static str>,
    /// The honest gap, named. Empty only when there is none.
    pub note: &'static str,
}

/// A transport selection: the open scheme token, the address in that
/// scheme's own vocabulary, and any state the scheme must be established
/// with first (a bootstrapped arena, a same-process peer).
///
/// The state is opaque to the registry: the scheme's own implementation
/// interprets it, and a missing or ill-typed state is that scheme's
/// honest refusal — never a silent substitution.
pub struct TransportRequest {
    /// The open scheme token.
    pub scheme: String,
    /// The address, in the scheme's own vocabulary (a path, a `host:port`,
    /// an arena name).
    pub address: String,
    /// Pre-established state, when the scheme needs it.
    pub state: Option<Box<dyn Any + Send>>,
}

impl TransportRequest {
    /// A request that names only a scheme and an address.
    pub fn new(scheme: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            address: address.into(),
            state: None,
        }
    }

    /// Attach the pre-established state this scheme needs.
    pub fn with_state(mut self, state: Box<dyn Any + Send>) -> Self {
        self.state = Some(state);
        self
    }
}

/// The one shape a transport implementation has.
///
/// An implementation is a *value*, not a variant: a test double, a
/// loopback, and a 2046 transport are all "a thing that answers to a
/// scheme token".
pub trait TransportImpl: Send + Sync {
    /// The scheme token this implementation answers to.
    fn scheme(&self) -> &'static str;

    /// What this transport declares about itself.
    fn description(&self) -> TransportDescription;

    /// Dial the far side.
    fn connect(&self, request: &TransportRequest) -> Result<Box<dyn Channel>, TransportError>;

    /// Bind and listen.
    fn listen(&self, request: &TransportRequest) -> Result<Box<dyn ChannelListener>, TransportError>;
}

/// The registry: the open set of transports.
///
/// The registry is a *value*, not a global: a test can build one with a
/// fake scheme, a daemon builds one with the built-ins plus whatever the
/// host provides, and a library user can pass their own set around without
/// touching engine state.
#[derive(Clone, Default)]
pub struct TransportRegistry {
    entries: Vec<Arc<dyn TransportImpl>>,
}

impl TransportRegistry {
    /// An empty registry (no transport answers yet).
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The registry with the tree's own transports in it: `inprocess`,
    /// `shmem`, and — when the host sockets are compiled — `unix` and
    /// `tcp`. The four register *themselves*; nothing here names a
    /// paradigm in a required position.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(
            crate::transport::inprocess::InProcessTransport,
        ));
        registry.register(Arc::new(crate::transport::shmem::ShmemTransport));
        #[cfg(all(unix, feature = "std"))]
        registry.register(Arc::new(crate::transport::unix::UnixTransport));
        #[cfg(feature = "std")]
        registry.register(Arc::new(crate::transport::tcp::TcpTransport));
        registry
    }

    /// Register an implementation. Registering the same scheme twice keeps
    /// the first registration: the set is append-only and the earlier
    /// answer stays the honest one.
    pub fn register(&mut self, transport: Arc<dyn TransportImpl>) {
        let scheme = transport.scheme();
        if self.entries.iter().any(|entry| entry.scheme() == scheme) {
            return;
        }
        self.entries.push(transport);
    }

    /// The implementation registered for a scheme, or `None`.
    pub fn lookup(&self, scheme: &str) -> Option<&Arc<dyn TransportImpl>> {
        self.entries.iter().find(|entry| entry.scheme() == scheme)
    }

    /// Every registered scheme token, sorted.
    pub fn schemes(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .iter()
            .map(|entry| entry.scheme().to_string())
            .collect();
        names.sort();
        names
    }

    /// Dial through a registered scheme.
    ///
    /// An unknown scheme is an *unavailable-with-reason* answer that names
    /// the schemes that do exist — never a crash and never a silent
    /// fallback to a different paradigm.
    pub fn connect(&self, request: &TransportRequest) -> Result<Box<dyn Channel>, TransportError> {
        match self.lookup(&request.scheme) {
            Some(transport) => transport.connect(request),
            None => Err(self.unknown_scheme(&request.scheme)),
        }
    }

    /// Bind through a registered scheme (same unknown-scheme answer).
    pub fn listen(
        &self,
        request: &TransportRequest,
    ) -> Result<Box<dyn ChannelListener>, TransportError> {
        match self.lookup(&request.scheme) {
            Some(transport) => transport.listen(request),
            None => Err(self.unknown_scheme(&request.scheme)),
        }
    }

    /// The honest answer for a scheme nobody registered.
    fn unknown_scheme(&self, scheme: &str) -> TransportError {
        TransportError::UnknownScheme {
            scheme: scheme.to_string(),
            known: self.schemes(),
        }
    }
}

impl core::fmt::Debug for TransportRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TransportRegistry")
            .field("schemes", &self.schemes())
            .finish()
    }
}
