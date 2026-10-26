//! `smart-service` — the daemon that makes the contract **live on a desktop**.
//!
//! # Why this binary exists
//!
//! The engine shipped its daemon as an example: `cargo run --example
//! smart-service --features …`. That is a build instruction, not a product. On a
//! real machine the clipboard domain has to be *running* — a daemon started by the
//! session, finding its socket by convention, recording copies while nobody is
//! looking, and still being there tomorrow. This binary is that daemon:
//!
//! * **one command, no flags** — binds the unix transport's own default address,
//!   so a client that follows the same convention finds it with no configuration;
//! * **durable history** — the clipboard backend is wired to a `database.kv` store
//!   under the user's state directory, so `getHistory` survives a restart;
//! * **live capture** — the selection watcher thread records copies made by other
//!   applications, which is the difference between a clipboard history and a log
//!   of our own writes;
//! * **concurrent sessions** — one connection per client, each on its own thread,
//!   because a desktop app holds a long-lived session and the shell still wants to
//!   ask a question;
//! * **honest startup** — everything it decided is printed once to stderr, and
//!   anything it could not do is printed *with the reason*, never swallowed.
//!
//! # What it serves
//!
//! | domain | source | notes |
//! |---|---|---|
//! | `clipboard` | the native seat (Wayland data-control or X11 ICCCM) | reads/writes the real selection; the history is this backend's ledger, archived to the database |
//! | `database` | `$XDG_STATE_HOME/smart/database` | the same journal the clipboard history is stored in, so a client can query it directly |
//! | `discovery`, `abi`, `capability` | the foundation | always served, on the same connection |
//!
//! A domain the host cannot serve is **not** silently dropped: the clipboard
//! backend falls back to the seat-less seam and the availability matrix says why.
//!
//! # Usage
//!
//! ```text
//! smart-service                        # $XDG_RUNTIME_DIR/smart/service.sock (or /tmp/smart-service.sock)
//! smart-service --socket /run/smart.sock
//! smart-service --state-dir ~/.local/state/smart --no-watch
//! smart-service --print-address        # what a client should dial, then exit
//! ```
//!
//! # Law #8
//!
//! The watcher only *reads* the selection; nothing here writes host state on
//! startup. The mutating verbs are reachable only on a Control-class connection,
//! exactly as the served domain enforces.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use smart_lib::domains::clipboard::archive::HistoryArchive;
use smart_lib::domains::clipboard::backend::ClipboardBackend;
use smart_lib::platforms::linux::clipboard::LinuxClipboardBackend;
use smart_lib::platforms::linux::clipboard_watch::ClipboardWatcher;
use smart_lib::platforms::linux::database::LinuxDatabaseBackend;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::clipboard::ClipboardDomain;
use smart_lib::serve::clipboard_archive::DatabaseArchive;
use smart_lib::serve::database::DatabaseDomain;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::service::{serve_connection, Service, ServedDomain};
use smart_lib::serve::classify;
use smart_lib::transport::registry::{TransportRegistry, TransportRequest};

/// The service's name in discovery, and the name a client looks up.
const SERVICE_LABEL: &str = "smart-service";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|argument| argument == "--help" || argument == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|argument| argument == "--version" || argument == "-V") {
        println!("{SERVICE_LABEL} {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let scheme = flag_value(&args, "--transport").unwrap_or_else(|| "unix".to_string());
    let address = flag_value(&args, "--addr")
        .or_else(|| flag_value(&args, "--socket"))
        .or_else(|| std::env::var("SMART_SERVICE_SOCKET").ok());
    let watch = !args.iter().any(|argument| argument == "--no-watch");
    let history = !args.iter().any(|argument| argument == "--no-history");
    let quiet = args.iter().any(|argument| argument == "--quiet");

    // ── the transport ────────────────────────────────────────────────────
    let registry = TransportRegistry::with_builtins();
    let Some(transport) = registry.lookup(&scheme) else {
        eprintln!(
            "{SERVICE_LABEL}: no transport is registered for scheme {scheme:?}; the registered \
             schemes are: {}",
            registry.schemes().join(", ")
        );
        return ExitCode::FAILURE;
    };
    let description = transport.description();
    let address = match address.or_else(|| description.default_address.map(String::from)) {
        Some(address) => address,
        None => {
            eprintln!(
                "{SERVICE_LABEL}: the {scheme:?} transport needs an address (--addr, or --socket \
                 for the unix transport)"
            );
            return ExitCode::FAILURE;
        }
    };
    if args.iter().any(|argument| argument == "--print-address") {
        println!("{scheme}://{address}");
        return ExitCode::SUCCESS;
    }

    // ── durable history ──────────────────────────────────────────────────
    //
    // The archive is the reason a restart is not a data loss. It is opened before
    // the listener so a broken state directory is a startup error the operator
    // sees, rather than a surprise at the first copy.
    let database = if history {
        let root = state_root(&args);
        match LinuxDatabaseBackend::persistent(&root) {
            Ok(database) => {
                note(quiet, &format!("history store: {}", root.display()));
                Some(Arc::new(database))
            }
            Err(error) => {
                eprintln!(
                    "{SERVICE_LABEL}: durable history is OFF: the store at {} could not be \
                     opened ({error}). The clipboard still works; its history will not survive \
                     this process.",
                    root.display()
                );
                None
            }
        }
    } else {
        note(quiet, "history store: disabled (--no-history)");
        None
    };

    // ── the clipboard backend ────────────────────────────────────────────
    let mut backend = match LinuxClipboardBackend::native_connect() {
        Ok(backend) => {
            let (transport, can_read, can_write) = backend.clipboard_caps();
            note(
                quiet,
                &format!("clipboard: native {transport} seat (read={can_read}, write={can_write})"),
            );
            backend
        }
        Err(error) => {
            // Honest degradation, not a refusal to run: the ledger, the history
            // and the contract all still answer, and every live-selection read
            // says exactly why it cannot.
            eprintln!(
                "{SERVICE_LABEL}: the live selection is UNAVAILABLE on this seat: {error}"
            );
            LinuxClipboardBackend::solid()
        }
    };
    if let Some(database) = database.as_ref() {
        let archive: Arc<dyn HistoryArchive> =
            Arc::new(DatabaseArchive::new(Arc::clone(database)));
        backend = backend.attach_archive(archive);
        note(quiet, "history: attached to database.kv (the ledger is restored at startup)");
    }

    let clipboard = Arc::new(Mutex::new(backend));

    // ── live capture ─────────────────────────────────────────────────────
    let watcher = if watch {
        match ClipboardWatcher::spawn(Arc::clone(&clipboard)) {
            Ok(watcher) => {
                note(quiet, "capture: watching the selection for other clients' copies");
                Some(watcher)
            }
            Err(reason) => {
                eprintln!(
                    "{SERVICE_LABEL}: live capture is OFF: {reason}. Copies this service makes \
                     are still recorded; copies other applications make are not."
                );
                None
            }
        }
    } else {
        note(quiet, "capture: disabled (--no-watch)");
        None
    };

    // ── the listener ─────────────────────────────────────────────────────
    if scheme == "unix" {
        // A stale socket from a daemon that was killed must not keep this one
        // from binding: the path we are about to bind is ours.
        let _ = std::fs::remove_file(&address);
    }
    let listener = match registry.listen(&TransportRequest::new(&scheme, &address)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("{SERVICE_LABEL}: bind {scheme} at {address} failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let endpoints = vec![PublishedEndpoint::new(&scheme, &address)];
    eprintln!("{SERVICE_LABEL}: serving the contract on {scheme} at {address}");
    for domain in build_service(&clipboard, database.as_ref(), &endpoints).domains() {
        note(
            quiet,
            &format!(
                "domain {} (interface 0x{:x}, schema 0x{:x})",
                domain.name(),
                domain.interface_id(),
                domain.service_id()
            ),
        );
    }

    // ── the accept loop ──────────────────────────────────────────────────
    //
    // One thread per connection. Each thread builds its *own* `Service` — the
    // harness's state is per-connection — while the clipboard backend and the
    // database are shared, because the host's selection and its history are one
    // object no matter how many clients ask about them.
    loop {
        let channel = match listener.accept() {
            Ok(channel) => channel,
            Err(error) => {
                eprintln!("{SERVICE_LABEL}: accept failed: {error}");
                continue;
            }
        };
        let class = classify(channel.peer_credentials());
        let clipboard = Arc::clone(&clipboard);
        let database = database.clone();
        let endpoints = endpoints.clone();
        note(quiet, &format!("connection accepted at class {class}"));
        let spawned = std::thread::Builder::new()
            .name("smart-service-session".into())
            .spawn(move || {
                let service = build_service(&clipboard, database.as_ref(), &endpoints);
                if let Err(error) = serve_connection(Arc::new(service), channel, class) {
                    eprintln!("{SERVICE_LABEL}: session ended with {error}");
                }
                // A copy the archive refused is not a lost clipboard, but it is
                // lost history: say so once per session rather than nowhere.
                let guard = clipboard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(error) = guard.archive_error() {
                    eprintln!("{SERVICE_LABEL}: history store: {error}");
                }
            });
        if let Err(error) = spawned {
            // The connection is dropped rather than served on the accept loop:
            // one client's session must never block the next caller.
            eprintln!("{SERVICE_LABEL}: cannot serve this connection: {error}");
        }
    }

    // Unreachable in practice (the loop only ends with the process), but the
    // watcher is stopped explicitly so a future graceful-shutdown path cannot
    // forget it.
    #[allow(unreachable_code)]
    {
        drop(watcher);
        ExitCode::SUCCESS
    }
}

/// The service: the domains this daemon serves, each with its ABI row.
///
/// Built per connection on purpose: discovery, capability and ABI state are the
/// harness's, while the backends are shared. That is what lets two clients have
/// two sessions over one clipboard.
fn build_service(
    clipboard: &Arc<Mutex<LinuxClipboardBackend>>,
    database: Option<&Arc<LinuxDatabaseBackend>>,
    endpoints: &[PublishedEndpoint],
) -> Service {
    let mut service = Service::new();
    service.set_endpoints(endpoints.to_vec());

    let domain = Arc::new(ClipboardDomain::new(Arc::clone(clipboard)));
    service.abi().publish(SupportedAbi {
        schema_id: domain.service_id(),
        name: "smart-protocol/clipboard".to_string(),
        major: 0,
        minor: 1,
    });
    service = service.with_domain(domain);

    if let Some(database) = database {
        let domain = Arc::new(DatabaseDomain::new(Arc::clone(database)));
        service.abi().publish(SupportedAbi {
            schema_id: domain.service_id(),
            name: "smart-protocol/database".to_string(),
            major: 0,
            minor: 1,
        });
        service = service.with_domain(domain);
    }

    service
}

/// The state directory durable data lives in: `--state-dir`, then
/// `$XDG_STATE_HOME/smart`, then `$HOME/.local/state/smart`.
fn state_root(args: &[String]) -> std::path::PathBuf {
    if let Some(explicit) = flag_value(args, "--state-dir") {
        return std::path::PathBuf::from(explicit);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return std::path::PathBuf::from(xdg).join("smart").join("database");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| std::ffi::OsString::from("."));
    std::path::PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("smart")
        .join("database")
}

/// Print a startup decision unless the operator asked for silence.
fn note(quiet: bool, line: &str) {
    if !quiet {
        eprintln!("{SERVICE_LABEL}: {line}");
    }
}

/// The value of `--flag value` (or `--flag=value`).
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == flag {
            return iter.next().cloned();
        }
        if let Some(value) = argument.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn print_help() {
    println!(
        "\
{SERVICE_LABEL} {version} — the smart-protocol daemon for this desktop session.

USAGE
  {label} [OPTIONS]

OPTIONS
  --socket <path>        the unix socket to bind (default: the transport's own
                         default — $XDG_RUNTIME_DIR/smart/service.sock, falling
                         back to /tmp/smart-service.sock)
  --addr <address>       an address for any transport (alias of --socket)
  --transport <scheme>   the transport scheme (default: unix; \"tcp\" is registered)
  --state-dir <path>     where durable data lives
                         (default: $XDG_STATE_HOME/smart/database)
  --no-history           do not open the history store: the ledger stays in memory
  --no-watch             do not watch the selection: only this service's own
                         copies are recorded
  --quiet                suppress the startup report (errors still print)
  --print-address        print the address a client should dial, then exit
  -h, --help             this text
  -V, --version          the version

WHAT IT SERVES
  clipboard   the live selection over the native seat, plus a history that is
              archived to database.kv and survives a restart
  database    the same store, so a client can query it directly
  discovery, abi, capability   the foundation, on every connection

WHAT IT WILL NOT DO
  It does not fork a shellout (no wl-paste, no xclip), it does not paste into
  your applications, and it does not prompt for privilege. Mutating verbs are
  reachable only on a Control-class connection (uid 0 / CAP_NET_ADMIN); every
  other peer is served read-only, and told so.

FILES
  <state-dir>/database.journal   the durable store (one journal, compacted)
  <socket>                       the unix socket (removed when this process binds)

Start it from your session with the packaged unit:
  systemctl --user enable --now smart-service.service
",
        version = env!("CARGO_PKG_VERSION"),
        label = SERVICE_LABEL
    );
}
