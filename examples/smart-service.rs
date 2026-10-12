//! `smart-service` — the smart-lib daemon: the Linux host's issuer of the
//! network contract's capability (`knowledge/05-AI-FIRST-SURFACE.md` §2).
//!
//! ```text
//! smart-service                              # unix socket at /tmp/smart-service.sock
//! SMART_SERVICE_SOCKET=/run/smart.sock smart-service
//! smart-service --socket /run/smart.sock     # an explicit path
//! smart-service --transport tcp --addr 127.0.0.1:7777
//! ```
//!
//! What it does, and nothing more: bind the transport, accept a connection,
//! **classify the peer from its host credentials** (uid 0 / `CAP_NET_ADMIN`
//! ⇒ Control; anything else, and any peer with no identity, ⇒ ReadOnly), and
//! serve the network backend over that one connection at that class. There
//! is no polkit, no elevation and no suid here — the daemon is not a
//! privilege broker (05 §2): a caller that lacks host privileges gets
//! host-truthful denials.
//!
//! The bound path and each connection's class are printed to **stderr** and
//! nothing else: diagnostics only, never on the wire.
//!
//! Law #8: the daemon itself changes no host state. Its mutating verbs
//! (`setNetworking`, `setRadioEnabled`, ...) write host state **only** on a
//! Control-class connection, and the live bring-back of those is founder-run.
//! Every test of this harness path is mock-verified
//! (`src/serve/tests.rs`, including the UnixSocket end-to-end session).

#![cfg(all(unix, feature = "domain-network", feature = "std"))]

extern crate std;

use std::process::ExitCode;

use smart_lib::platforms::linux::network::LinuxNetworkBackend;
use smart_lib::serve::{classify, serve, orderly_close};
use smart_lib::transport::channel::{Channel, ChannelListener};
use smart_lib::transport::tcp::TcpListener;
use smart_lib::transport::unix::UnixListener;

// The daemon is a plain consumer of the harness's public API now that
// `build.rs` wires the `serve` wave module: `smart_lib::serve` carries the
// connection loop and the uid-peer capability policy, and nothing here
// reaches into the harness internals.

/// The default unix socket path (`$SMART_SERVICE_SOCKET` overrides it).
const DEFAULT_SOCKET: &str = "/tmp/smart-service.sock";

/// The bound listener, whichever transport it came from — the accept loop
/// names neither.
enum Bound {
    Unix(UnixListener),
    Tcp(TcpListener),
}

impl Bound {
    fn accept(&self) -> Result<Box<dyn Channel>, smart_lib::transport::channel::TransportError> {
        match self {
            Bound::Unix(listener) => listener.accept(),
            Bound::Tcp(listener) => listener.accept(),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let transport_kind = flag_value(&args, "--transport").unwrap_or_else(|| "unix".to_string());
    let socket = flag_value(&args, "--socket")
        .or_else(|| std::env::var("SMART_SERVICE_SOCKET").ok())
        .unwrap_or_else(|| DEFAULT_SOCKET.to_string());
    let addr = flag_value(&args, "--addr");

    let (listener, bound) = match transport_kind.as_str() {
        "unix" => {
            // A stale socket file from a killed daemon must not keep the
            // service from binding: the file we are about to bind is ours.
            let _ = std::fs::remove_file(&socket);
            match UnixListener::bind(&socket) {
                Ok(listener) => (Bound::Unix(listener), socket.clone()),
                Err(error) => {
                    eprintln!("smart-service: bind {socket} failed: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        "tcp" => match &addr {
            Some(addr) => match TcpListener::bind(addr) {
                Ok(listener) => {
                    let bound = match listener.local_addr() {
                        Ok(bound) => bound,
                        Err(error) => {
                            eprintln!("smart-service: local_addr failed: {error}");
                            return ExitCode::FAILURE;
                        }
                    };
                    (Bound::Tcp(listener), bound)
                }
                Err(error) => {
                    eprintln!("smart-service: bind {addr} failed: {error}");
                    return ExitCode::FAILURE;
                }
            },
            None => {
                eprintln!("smart-service: --transport tcp needs --addr host:port");
                return ExitCode::FAILURE;
            }
        },
        other => {
            eprintln!("smart-service: unknown --transport {other:?} (unix|tcp)");
            return ExitCode::FAILURE;
        }
    };

    // The backend: the Linux network cell over the live kernel-first store
    // and the counted NetworkManager seam. Building it reads host state
    // (netlink, the bus) and writes nothing (law #8).
    let backend = match LinuxNetworkBackend::connect() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("smart-service: the network backend could not start: {error}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("smart-service: serving network on {transport_kind} at {bound}");

    // The accept loop: one session at a time, each served at the class its
    // peer's credentials earned. A failed session is printed and the daemon
    // keeps accepting — one caller's fault is not the service's.
    loop {
        let channel = match listener.accept() {
            Ok(channel) => channel,
            Err(error) => {
                eprintln!("smart-service: accept failed: {error}");
                continue;
            }
        };
        let class = classify(channel.peer_credentials());
        let kind = channel.kind();
        eprintln!(
            "smart-service: {} connection served at capability class {class}",
            kind.as_str()
        );
        if let Err(error) = serve(&backend, &*channel, class) {
            eprintln!("smart-service: session ended with {error}");
        }
        // The harness borrows its channel, so the daemon owns the close: an
        // orderly shutdown frame reaches the peer instead of a dropped fd.
        let _ = channel.close(orderly_close());
    }
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
