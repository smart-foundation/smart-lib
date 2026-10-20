//! The harness the six domain tests share.
//!
//! The tests drive the service the way a *contract-only* client would:
//! through the generated `Client` of the contract's own schema. If a test
//! could pass only by knowing something the contract does not say, it is a
//! bug in the harness, not a strength of the test.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use smart_lib::serve::service::{serve_connection, ClientSession, Service};
use smart_lib::transport::channel::{Channel, ChannelListener};
use smart_lib::serve::CapabilityClass;

/// A contract `Text` field, read losslessly.
pub fn text(reader: capnp::text::Reader<'_>) -> String {
    String::from_utf8_lossy(reader.as_bytes()).into_owned()
}

/// Read one entry of an open metadata map.
pub fn metadata_of(
    map: Option<smart_lib::schema::types_capnp::string_map::Reader<'_>>,
    key: &str,
) -> Option<String> {
    let map = map?;
    for entry in map.get_entries().into_iter().flatten() {
        let entry_key = text(entry.get_key().expect("the key"));
        if entry_key == key {
            return Some(text(entry.get_value().expect("the value")));
        }
    }
    None
}

/// A counter for unique socket paths.
static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique socket path for one test.
pub fn socket_path() -> String {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "/tmp/smart-lib-s1-{}-{counter}.sock",
        std::process::id()
    )
}

/// A client session over a unix socket, connected to the service.
fn connect_unix(path: &str) -> ClientSession {
    let channel = smart_lib::transport::unix::UnixChannel::connect(path).expect("connect");
    let stream = Box::new(channel)
        .into_byte_stream()
        .expect("byte stream");
    ClientSession::connect(stream).expect("session")
}

/// Run a contract-only client against the service.
///
/// The service is served **on this thread** (the backend is the host's own
/// state and is not shared across threads); the client runs on its own
/// thread, exactly as a separate process would.
pub fn run_client<T, F>(service: Arc<Service>, path: &str, class: CapabilityClass, body: F) -> T
where
    F: FnOnce(&mut ClientSession) -> T + Send + 'static,
    T: Send + 'static,
{
    let _ = std::fs::remove_file(path);
    let listener = smart_lib::transport::unix::UnixListener::bind(path).expect("bind");
    let (sender, receiver) = std::sync::mpsc::channel();
    let client_path = path.to_string();
    std::thread::spawn(move || {
        let mut session = connect_unix(&client_path);
        let _ = sender.send(body(&mut session));
    });
    let channel = listener.accept().expect("accept");
    let _ = serve_connection(service, channel, class);
    receiver.recv().expect("the client's answer")
}

/// The answer of one call: either the contract's own answer, or the
/// contract's own refusal.
///
/// The refusal must CARRY A REASON. The generated default — capnp's own
/// "method not implemented" — is never an acceptable answer from a served
/// domain: a method the engine cannot perform here answers `notSupported`
/// with the availability matrix's own reason, and a method the engine can
/// perform answers with the value.
pub fn assert_honest(what: &str, outcome: Result<(), capnp::Error>) {
    match outcome {
        Ok(()) => {}
        Err(error) => {
            let message = error.to_string();
            assert!(
                !message.contains("not implemented"),
                "{what} fell through to the generated default: {message}"
            );
            assert!(
                !message.is_empty(),
                "{what} refused without a reason: {message}"
            );
        }
    }
}

/// The method names a domain's registry publishes, from BOTH lists: the
/// commands that can run and the commands that cannot.
///
/// The coverage check needs both: a method the engine serves but the
/// surface does not name is a hidden method, and a hidden method is the
/// failure this batch exists to prevent.
pub fn published_method_names(
    available: Vec<String>,
    unavailable: Vec<String>,
) -> Vec<String> {
    let mut names: Vec<String> = available;
    names.extend(unavailable);
    names.sort();
    names.dedup();
    names
}

/// The reason a refusal carried, if it refused.
pub fn refusal_reason(outcome: Result<(), capnp::Error>) -> Option<String> {
    outcome.err().map(|error| error.to_string())
}
