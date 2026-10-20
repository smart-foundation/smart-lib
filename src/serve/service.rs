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
use alloc::vec::Vec;

use std::sync::Arc;

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
}

impl Service {
    /// A service with no domains (the foundation interfaces only).
    pub fn new() -> Self {
        Self {
            domains: Vec::new(),
            discovery: Arc::new(super::discovery::DiscoveryState::new()),
            abi: Arc::new(super::abi::AbiState::new()),
            capabilities: Arc::new(super::capability::CapabilityState::new()),
        }
    }

    /// Add a domain. The domain's discovery descriptor is registered with
    /// the service's discovery state, so a client can look it up.
    pub fn with_domain(mut self, domain: Arc<dyn ServedDomain>) -> Self {
        self.discovery
            .register_domain(domain.as_ref(), &self.discovery_endpoints());
        self.domains.push(domain);
        self
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
