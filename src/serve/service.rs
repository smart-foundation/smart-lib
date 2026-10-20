//! The service: the contract interfaces it serves, and the per-connection
//! RPC session that exposes them.
//!
//! # The shape of a session
//!
//! ```text
//!   transport channel ──▶ byte stream ──▶ capnp RPC ──▶ interface router
//!        (the seam)        (no framing      (the contract's   (which contract
//!                           of this layer)   own call model)    interface the
//!                                                              caller named)
//! ```
//!
//! The router hands the call to the **generated** server of that interface,
//! which calls the engine's trait and fills the contract's own results
//! struct. There is no envelope of this layer's own anywhere on that path.
//!
//! # The capability class is decided once per connection
//!
//! The peer's host identity is read once, at accept time, and mapped into a
//! [`CapabilityClass`]. Every server built for that connection carries the
//! class, so the authority gate is a property of the connection, not
//! something a method re-decides.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use std::sync::{Arc, Mutex};

use capnp::capability::Client;
use capnp::message::ReaderOptions;
use capnp::private::capability::ClientHook;
use capnp_rpc::rpc_twoparty_capnp::Side;
use capnp_rpc::twoparty;
use capnp_rpc::RpcSystem;
use futures::executor::LocalPool;
use futures::future::Future;
use futures::task::LocalSpawnExt;

use crate::transport::channel::{ByteStream, Channel, TransportError};

use super::router::{InterfaceRouter, Route};
use super::stream::PumpedStream;
use super::CapabilityClass;

/// One command a domain publishes through the registry (cli.capnp's
/// `CommandRegistration`, minus what the engine fills in).
///
/// The presentation fields are the contract's own `CommandPresentation`
/// (`domain`, `summary`, `order`, `noArgsValid`), and the availability is
/// the domain's **own** matrix row — the registry never invents a reason.
#[derive(Clone, Debug)]
pub struct DomainCommand {
    /// The path the user types ("net status").
    pub path: &'static str,
    /// The interface the command calls, as the schema spells it ("Network").
    pub interface: &'static str,
    /// The method the command calls, as the schema spells it ("getStatus").
    pub method: &'static str,
    /// The one-line summary the registry publishes.
    pub summary: &'static str,
    /// The sort key within the domain (lower sorts first).
    pub order: u32,
    /// Whether a bare invocation (no arguments) is meaningful.
    pub no_args_valid: bool,
    /// Why the command cannot run right now, when it cannot: the matrix's
    /// reason class token and the row's own words.
    pub unavailable: Option<(&'static str, &'static str)>,
    /// The named degraded capability, when the row is `DEGRADED`: the command
    /// still answers, but not completely.
    pub degraded: Option<&'static str>,
    /// The registration's open metadata: the other contract methods one verb
    /// composes, under the stable `composes` key.
    pub metadata: Vec<(&'static str, &'static str)>,
}

impl DomainCommand {
    /// A command that answers, with the domain's own matrix row deciding
    /// whether it is available, degraded, or unavailable.
    pub fn new(
        path: &'static str,
        interface: &'static str,
        method: &'static str,
        summary: &'static str,
        order: u32,
        no_args_valid: bool,
    ) -> Self {
        Self {
            path,
            interface,
            method,
            summary,
            order,
            no_args_valid,
            unavailable: None,
            degraded: None,
            metadata: Vec::new(),
        }
    }

    /// The command, with the other contract methods the verb composes listed
    /// under the stable `composes` key.
    pub fn composing(
        path: &'static str,
        interface: &'static str,
        method: &'static str,
        summary: &'static str,
        order: u32,
        no_args_valid: bool,
        composes: &'static str,
    ) -> Self {
        Self {
            path,
            interface,
            method,
            summary,
            order,
            no_args_valid,
            unavailable: None,
            degraded: None,
            metadata: alloc::vec![("composes", composes)],
        }
    }

    /// The command, with the domain's **own** availability row deciding
    /// whether it can run, and what it is missing.
    ///
    /// The row is the engine's, read from the domain's availability matrix:
    /// the registry never invents a reason, and never publishes a command
    /// as available that the engine cannot answer.
    pub fn with_row(mut self, row: Option<super::availability::MatrixRow>) -> Self {
        if let Some(row) = row {
            self.unavailable = row.unavailable_class();
            self.degraded = row.degraded();
        }
        self
    }
}

/// A domain this service serves over the contract's own call model.
///
/// The interface id is the one the contract's **generated** code assigned to
/// the interface block — never a hand-written constant, and never an
/// ordinal this engine invented.
pub trait ServedDomain {
    /// The contract interface id, from the generated code.
    fn interface_id(&self) -> u64;

    /// The contract's file id for this domain's schema — what discovery
    /// publishes as the service id.
    fn service_id(&self) -> u64;

    /// The name discovery publishes for this service.
    fn name(&self) -> &'static str;

    /// The open category token discovery publishes.
    fn category(&self) -> &'static str {
        "system"
    }

    /// The capability ids a caller needs before this service is usable.
    fn required_capability_ids(&self) -> Vec<u64> {
        Vec::new()
    }

    /// The domain's own capability tokens (law #6), published in the
    /// discovery descriptor's open metadata.
    ///
    /// The contract's `requiredCapabilityIds` is a list of opaque
    /// `T.CapabilityHandle`s — numbers — while a domain's tokens are an open
    /// vocabulary of words. The numeric field therefore carries only the
    /// handles the engine really holds (the connection's class, published
    /// through the capability service), and the tokens travel in the
    /// descriptor's open metadata, where an open vocabulary belongs.
    fn capability_tokens(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// The commands this domain publishes through the registry
    /// (cli.capnp). The engine publishes them by NAME; the ordinal and the
    /// schema's node tree are resolved from the generated contract table and
    /// the schema tree, never written by hand.
    fn commands(&self) -> Vec<DomainCommand> {
        Vec::new()
    }

    /// The state this service is in, as the contract's `ServiceState`.
    ///
    /// The default is `Online`: a domain that serves its whole surface.
    /// A domain that answers `notSupported` for part of its matrix is
    /// `Degraded` — it serves most of the surface, and the missing part is
    /// named in the registry.
    fn state(&self) -> super::discovery::ServiceState {
        super::discovery::ServiceState::Online
    }

    /// This domain's schema node tree (the contract's own
    /// `CodeGeneratorRequest` for the schema), published in the registry's
    /// open metadata so a client that never compiled the schema can still
    /// address its methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        None
    }

    /// Build the server for one connection, at the class that connection
    /// earned.
    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook>;
}

/// The service: every contract interface it serves, and the state those
/// interfaces publish.
pub struct Service {
    domains: Vec<Arc<dyn ServedDomain>>,
    discovery: Arc<super::discovery::DiscoveryState>,
    abi: Arc<super::abi::AbiState>,
    capabilities: Arc<super::capability::CapabilityState>,
    registry: Arc<super::cli::RegistryState>,
    schema_trees: Arc<Mutex<Vec<(u64, &'static [u8])>>>,
}

impl Service {
    /// A service with no domains (the foundation interfaces only).
    pub fn new() -> Self {
        Self {
            domains: Vec::new(),
            discovery: Arc::new(super::discovery::DiscoveryState::new()),
            abi: Arc::new(super::abi::AbiState::new()),
            capabilities: Arc::new(super::capability::CapabilityState::new()),
            registry: Arc::new(super::cli::RegistryState::new()),
            schema_trees: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add a domain. The domain's discovery descriptor is registered with
    /// the service's discovery state, so a client can look it up; its
    /// commands are published in the registry, so a client can read the
    /// surface — and its schema's node tree, so a client that never
    /// compiled the schema can still address the methods.
    pub fn with_domain(mut self, domain: Arc<dyn ServedDomain>) -> Self {
        self.discovery
            .register_domain(domain.as_ref(), &self.discovery_endpoints());
        for command in domain.commands() {
            // The availability is the DOMAIN'S OWN matrix row: the class
            // token maps onto the contract's `AvailabilityReason`, and the
            // row's words ride the registration's open metadata.
            let unavailable = command.unavailable.map(|(class, reason)| {
                (
                    super::availability::reason_for(class),
                    super::availability::detail(class, reason),
                )
            });
            let metadata = command
                .metadata
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            self.registry.publish(super::cli::PublishedCommand {
                path: command.path.to_string(),
                schema_id: domain.service_id(),
                interface: command.interface.to_string(),
                method: command.method.to_string(),
                domain: domain.name().to_string(),
                summary: command.summary.to_string(),
                order: command.order,
                no_args_valid: command.no_args_valid,
                unavailable,
                degraded: command.degraded.map(|reason| reason.to_string()),
                metadata,
            });
        }
        if let Some(tree) = domain.schema_tree() {
            if let Ok(mut guard) = self.schema_trees.lock() {
                if !guard.iter().any(|(id, _)| *id == domain.service_id()) {
                    guard.push((domain.service_id(), tree));
                }
            }
        }
        self.domains.push(domain);
        self
    }

    /// The registry state (what the service publishes about its surface).
    pub fn registry(&self) -> &Arc<super::cli::RegistryState> {
        &self.registry
    }

    /// The endpoints this service publishes for its domains. Set by the
    /// daemon once it knows where it is bound.
    pub fn set_endpoints(&self, endpoints: Vec<super::discovery::PublishedEndpoint>) {
        self.discovery.set_endpoints(endpoints);
    }

    /// The discovery state (what the service publishes).
    pub fn discovery(&self) -> &Arc<super::discovery::DiscoveryState> {
        &self.discovery
    }

    /// The ABI state (what the service speaks).
    pub fn abi(&self) -> &Arc<super::abi::AbiState> {
        &self.abi
    }

    /// The capability state (what the service holds).
    pub fn capabilities(&self) -> &Arc<super::capability::CapabilityState> {
        &self.capabilities
    }

    /// The domains this service serves.
    pub fn domains(&self) -> &[Arc<dyn ServedDomain>] {
        &self.domains
    }

    /// The endpoints discovery publishes.
    fn discovery_endpoints(&self) -> Vec<super::discovery::PublishedEndpoint> {
        self.discovery.endpoints()
    }

    /// The bootstrap capability for one connection: the router over every
    /// contract interface this service serves.
    pub fn bootstrap(&self, class: CapabilityClass) -> Client {
        let mut routes = Vec::new();
        for domain in &self.domains {
            routes.push(Route {
                interface_id: domain.interface_id(),
                hook: domain.build(class),
            });
        }
        routes.push(Route {
            interface_id: super::discovery::INTERFACE_ID,
            hook: super::discovery::server(self.discovery.clone()),
        });
        routes.push(Route {
            interface_id: super::abi::INTERFACE_ID,
            hook: super::abi::server(self.abi.clone()),
        });
        routes.push(Route {
            interface_id: super::capability::INTERFACE_ID,
            hook: super::capability::server(self.capabilities.clone(), class),
        });
        routes.push(Route {
            interface_id: super::cli::INTERFACE_ID,
            hook: super::cli::server(self.registry.clone(), self.schema_trees.clone()),
        });
        InterfaceRouter::new(routes).as_client()
    }
}

impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a served connection ended.
#[derive(Clone, Debug)]
pub enum ServeError {
    /// The transport failed before or during the session.
    Transport(TransportError),
    /// The RPC session ended in failure.
    Rpc(capnp::Error),
}

impl core::fmt::Display for ServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ServeError::Transport(error) => write!(f, "transport failure: {error}"),
            ServeError::Rpc(error) => write!(f, "rpc failure: {error}"),
        }
    }
}

impl core::error::Error for ServeError {}

/// Serve one connection: the contract's own RPC over the channel's byte
/// stream, until the peer goes away.
///
/// The channel is **consumed**: from here on the transport's own framing is
/// not used on this connection, and the bytes on the wire are the contract's
/// capnp messages and nothing else.
pub fn serve_connection(
    service: Arc<Service>,
    channel: Box<dyn Channel>,
    class: CapabilityClass,
) -> Result<(), ServeError> {
    let stream = channel
        .into_byte_stream()
        .map_err(ServeError::Transport)?;
    let (reader, writer) = PumpedStream::new(stream).split();
    let network: Box<dyn capnp_rpc::VatNetwork<twoparty::VatId>> = Box::new(
        twoparty::VatNetwork::new(reader, writer, Side::Server, ReaderOptions::new()),
    );
    let rpc_system: RpcSystem<twoparty::VatId> =
        RpcSystem::new(network, Some(service.bootstrap(class)));
    let mut pool = LocalPool::new();
    pool.run_until(async move { rpc_system.await.map_err(ServeError::Rpc) })
}

/// A client session over a transport's byte stream: the contract's own call
/// model, from the caller's side.
///
/// The session owns the executor: a caller drives its calls with
/// [`ClientSession::run`], and the RPC system runs underneath.
pub struct ClientSession {
    pool: LocalPool,
    bootstrap: Box<dyn ClientHook>,
    stream: Arc<dyn ByteStream>,
}

impl ClientSession {
    /// Connect to a peer over a byte stream and take its bootstrap
    /// capability.
    pub fn connect(stream: Box<dyn ByteStream>) -> Result<Self, TransportError> {
        let pumped = PumpedStream::new(stream);
        let pumped_stream = pumped.stream();
        let (reader, writer) = pumped.split();
        let network: Box<dyn capnp_rpc::VatNetwork<twoparty::VatId>> = Box::new(
            twoparty::VatNetwork::new(reader, writer, Side::Client, ReaderOptions::new()),
        );
        let mut rpc_system: RpcSystem<twoparty::VatId> = RpcSystem::new(network, None);
        let bootstrap = rpc_system.bootstrap::<Box<dyn ClientHook>>(Side::Server);
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        spawner
            .spawn_local(async move {
                let _ = rpc_system.await;
            })
            .map_err(|error| TransportError::message(format!("cannot start the RPC session: {error:?}")))?;
        Ok(Self {
            pool,
            bootstrap,
            stream: pumped_stream,
        })
    }

    /// The peer's bootstrap capability. Any contract interface the peer
    /// serves is reachable through it: the interface id travels with the
    /// call, exactly as the contract's generated code says.
    pub fn bootstrap(&self) -> &dyn ClientHook {
        self.bootstrap.as_ref()
    }

    /// A client for one contract interface, built from the bootstrap.
    pub fn interface<C: capnp::capability::FromClientHook>(&self) -> C {
        C::new(self.bootstrap.clone())
    }

    /// Run a future to completion on this session's executor.
    pub fn run<T>(&mut self, future: impl Future<Output = T>) -> T {
        self.pool.run_until(future)
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        // The far side must see the end of the connection: the socket is
        // closed here, so the server's session ends too.
        self.stream.close();
    }
}
