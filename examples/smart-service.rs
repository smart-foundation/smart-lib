//! `smart-service` — the smart-lib daemon: the Linux host's issuer of the
//! network contract's capability (`knowledge/05-AI-FIRST-SURFACE.md` §2).
//!
//! ```text
//! smart-service                              # unix socket at the unix transport's
//!                                           # own default address
//! SMART_SERVICE_SOCKET=/run/smart.sock smart-service
//! smart-service --socket /run/smart.sock     # an explicit path
//! smart-service --transport tcp --addr 127.0.0.1:7777
//! ```
//!
//! # What it does, and nothing more
//!
//! Bind the transport, accept a connection, **classify the peer from its host
//! credentials** (uid 0 / `CAP_NET_ADMIN` ⇒ Control; anything else, and any
//! peer with no identity, ⇒ ReadOnly), and serve the contract over that one
//! connection at that class.
//!
//! The wire is the contract's own: **capnp RPC**, over the transport's byte
//! stream. The daemon serves every domain that is enabled *and* has a
//! service implementation, each one registering its own discovery
//! descriptor; the foundation interfaces (discovery, ABI, capability
//! publication) are served on the same connection.
//!
//! There is no polkit, no elevation and no suid here — the daemon is not a
//! privilege broker (05 §2): a caller that lacks host privileges gets
//! host-truthful denials.
//!
//! The bound path and each connection's class are printed to **stderr** and
//! nothing else: diagnostics only, never on the wire.
//!
//! Law #8: the daemon itself changes no host state. Its mutating verbs
//! (`setNetworking`, `setRadioEnabled`, ...) write host state **only** on a
//! Control-class connection, and the live bring-back of those is founder-run.

#![cfg(all(feature = "rpc", feature = "std"))]

extern crate std;

use std::process::ExitCode;
use std::sync::Arc;

use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::{serve_connection, ServedDomain, Service};
use smart_lib::serve::classify;
use smart_lib::transport::registry::{TransportRegistry, TransportRequest};

#[cfg(feature = "domain-network")]
use smart_lib::serve::network::NetworkDomain;
#[cfg(feature = "domain-network")]
use smart_lib::platforms::linux::network::LinuxNetworkBackend;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scheme = flag_value(&args, "--transport").unwrap_or_else(|| "unix".to_string());
    let address = flag_value(&args, "--addr")
        .or_else(|| flag_value(&args, "--socket"))
        .or_else(|| std::env::var("SMART_SERVICE_SOCKET").ok());

    // The registry: the open set of transports. The daemon names a *scheme
    // token*; which implementation answers it is the registry's business.
    let registry = TransportRegistry::with_builtins();
    let Some(transport) = registry.lookup(&scheme) else {
        eprintln!(
            "smart-service: no transport is registered for scheme {scheme:?}; the \
             registered schemes are: {}",
            registry.schemes().join(", ")
        );
        return ExitCode::FAILURE;
    };
    let description = transport.description();
    let address = match address.or_else(|| description.default_address.map(String::from)) {
        Some(address) => address,
        None => {
            eprintln!(
                "smart-service: the {scheme:?} transport needs an address (--addr, or \
                 --socket for the unix transport)"
            );
            return ExitCode::FAILURE;
        }
    };

    // A stale socket file from a killed daemon must not keep the service
    // from binding: the file we are about to bind is ours.
    if scheme == "unix" {
        let _ = std::fs::remove_file(&address);
    }

    let listener = match registry.listen(&TransportRequest::new(&scheme, &address)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("smart-service: bind {scheme} at {address} failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    // The service: every domain that is enabled and has a service
    // implementation, each one registering its own discovery descriptor.
    let service = Arc::new(build_service());

    // The endpoint this service is reachable at, published in the contract's
    // own `ServiceEndpoint` (with the open transport token).
    let endpoints = vec![PublishedEndpoint {
        transport: scheme.clone(),
        address: address.clone(),
    }];
    service.set_endpoints(endpoints);

    eprintln!("smart-service: serving the contract on {scheme} at {address}");
    for domain in service.domains() {
        eprintln!(
            "smart-service:   domain {} (interface id 0x{:x}, service id 0x{:x})",
            domain.name(),
            domain.interface_id(),
            domain.service_id()
        );
    }

    // The accept loop: each connection is served at the class its peer's
    // credentials earned, on its own thread. A failed session is printed and
    // the daemon keeps accepting — one caller's fault is not the service's.
    loop {
        let channel = match listener.accept() {
            Ok(channel) => channel,
            Err(error) => {
                eprintln!("smart-service: accept failed: {error}");
                continue;
            }
        };
        let class = classify(channel.peer_credentials());
        let transport_name = channel.scheme();
        eprintln!("smart-service: {transport_name} connection served at capability class {class}");
        // One session at a time: the backend is the host's own state and is
        // not shared across threads (the old daemon's shape, kept).
        if let Err(error) = serve_connection(Arc::clone(&service), channel, class) {
            eprintln!("smart-service: session ended with {error}");
        }
    }
}

/// The service: the domains that are enabled and have a service
/// implementation, plus the foundation interfaces (discovery, ABI and the
/// capability publication), which every service serves.
///
/// The set is *data*, not a special case: each served domain contributes
/// one line here, and the daemon's accept loop, the discovery descriptors
/// and the ABI catalogue all follow from it. A domain whose service
/// implementation has not landed yet is simply not in the set — the daemon
/// says so on stderr rather than pretending to serve it.
fn build_service() -> Service {
    let mut service = Service::new();

    #[cfg(feature = "domain-network")]
    {
        // The backend: the Linux network cell over the live kernel-first
        // store and the counted NetworkManager seam. Building it reads host
        // state (netlink, the bus) and writes nothing (law #8).
        let backend = match LinuxNetworkBackend::connect() {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("smart-service: the network backend could not start: {error}");
                std::process::exit(1);
            }
        };
        let domain = Arc::new(NetworkDomain::new(Arc::new(backend)));
        // The ABI this service speaks, published: the schema's own id and
        // the generation it is spoken at.
        service.abi().publish(SupportedAbi {
            schema_id: domain.service_id(),
            name: "smart-protocol/network".to_string(),
            major: 0,
            minor: 1,
        });
        service = service.with_domain(domain);
    }

    service
}

/// The value of `--flag value` (or `--flag=value`).
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_string());
        }
    }
    None
}
