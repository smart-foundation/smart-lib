//! The proof that matters for this crate: a **contract-only client** driving a
//! **contract-compliant service**, with no engine type crossing either way.
//!
//! The test builds a tiny backend, serves it through the engine's own service
//! harness on a unix socket, and then talks to it using *only* this crate's
//! `Session` + `ClipboardClient` — the same code the app runs. The engine is a
//! `dev-dependency`; no shipped binary links it.
//!
//! What the assertions check:
//!
//! 1. discovery answers with the clipboard descriptor, and the published
//!    schema id is the one this crate compiled against (the app's drift check
//!    would fire here if it were wrong);
//! 2. `getHistory` returns the contract's units (nanoseconds, with 0 meaning
//!    "no clock read") and the MIME list in the service's order;
//! 3. `getPreview` distinguishes "served" from "unknown" — the second is a
//!    first-class answer, not an error;
//! 4. `setContent` round-trips: the backend observes the bytes the client
//!    offered, which is the whole point of the write path;
//! 5. a read-only connection's `setContent` is refused with
//!    `permissionDenied`, and the refusal *keeps its class* through this
//!    crate's error mapping.
//!
//! Run with:
//!
//! ```text
//! SMART_SCHEMA_DIR=/path/to/smart-protocol-official/schema \
//!   cargo test -p smart-clipboard-proto --test contract_roundtrip
//! ```

use std::future::Future;
use std::sync::{Arc, Mutex};

use smart_clipboard_proto::{
    Candidate, ClientError, ClipboardClient, EndpointSource, ItemId, PreviewState, Query,
    RefusalClass, ServiceLocation,
};
use smart_lib::core::Result as EngineResult;
use smart_lib::domains::clipboard::{
    ClipboardBackend, ClipboardContent, ClipboardId, ClipboardItem, ClipboardPreview,
    ClipboardSizeLimits, ClipboardSourceId, ContentOutcome, Page as EnginePage, Pagination,
    PreviewOutcome,
};
use smart_lib::serve::clipboard::ClipboardDomain;
use smart_lib::serve::service::{serve_connection, Service};
use smart_lib::serve::CapabilityClass;
use smart_lib::transport::unix::UnixListener;

/// A backend with two entries and no host access at all: the test is about
/// the wire, not about a selection.
#[derive(Default)]
struct Ledger {
    written: Mutex<Option<ClipboardContent>>,
}

impl Ledger {
    fn item(id: u64, nanos: u64, mime: &str, pinned: bool) -> ClipboardItem {
        ClipboardItem {
            id: ClipboardId::new(id),
            timestamp: nanos,
            mime_types: vec![mime.to_string()],
            tags: Vec::new(),
            pinned,
            source_id: ClipboardSourceId::new(1),
            size_bytes: 42,
            text: String::new(),
        }
    }
}

impl ClipboardBackend for Ledger {
    fn get_history(&self, _page: Pagination) -> EngineResult<EnginePage<ClipboardItem>> {
        Ok(EnginePage {
            items: vec![
                Self::item(1, 1_700_000_000_000_000_000, "text/plain", false),
                // A timestamp of 0 is the engine's "clock could not be read";
                // the client must surface it as unknown, not as 1970.
                Self::item(2, 0, "image/png", true),
            ],
            total: 2,
        })
    }

    fn get_preview(&self, item_id: ClipboardId) -> EngineResult<PreviewOutcome> {
        if item_id.get() == 1 {
            Ok(PreviewOutcome::Preview(ClipboardPreview {
                text_preview: "hello from the ledger".to_string(),
                available: true,
                original_size: 42,
                preview_mime_type: "text/plain".to_string(),
            }))
        } else {
            Ok(PreviewOutcome::Unavailable)
        }
    }

    fn set_content(&self, content: ClipboardContent) -> EngineResult<()> {
        *self.written.lock().expect("the mock's lock") = Some(content);
        Ok(())
    }

    fn clear_content(&self) -> EngineResult<()> {
        *self.written.lock().expect("the mock's lock") = None;
        Ok(())
    }

    fn get_content(&self, _mime_type: &str) -> EngineResult<ContentOutcome> {
        match self.written.lock().expect("the mock's lock").clone() {
            Some(content) => Ok(ContentOutcome::Content(content)),
            None => Ok(ContentOutcome::Empty),
        }
    }

    fn get_size_limits(&self) -> EngineResult<ClipboardSizeLimits> {
        Ok(ClipboardSizeLimits {
            max_item_size: 0,
            max_history_size: 0,
            max_history_count: 0,
        })
    }
}

/// Serve one backend on a socket and return the path and the backend handle.
fn serve(class: CapabilityClass) -> (String, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::default());
    let service =
        Arc::new(Service::new().with_domain(Arc::new(ClipboardDomain::new(Arc::clone(&ledger)))));
    let path = format!(
        "/tmp/smart-clipboard-proto-{}-{}.sock",
        std::process::id(),
        match class {
            CapabilityClass::Control => "control",
            CapabilityClass::ReadOnly => "readonly",
        }
    );
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    std::thread::spawn(move || {
        let channel = listener.accept().expect("accept");
        let _ = serve_connection(service, channel, class);
    });
    (path, ledger)
}

/// Drive one client body on the current thread's LocalSet.
///
/// `capnp-rpc`'s client futures are `!Send`, so every client lives inside a
/// `LocalSet` on a current-thread runtime — which is exactly how the app's
/// worker thread is built, so a test that passes here passes there.
fn block_on<F: Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    tokio::task::LocalSet::new().block_on(&runtime, future)
}

async fn connect(address: &str) -> smart_clipboard_proto::session::Session {
    let candidate = Candidate {
        scheme: "unix".to_string(),
        address: address.to_string(),
        source: EndpointSource::Argument,
    };
    smart_clipboard_proto::session::Session::connect(&candidate)
        .await
        .expect("the service must answer")
}

#[test]
fn discovery_publishes_the_clipboard_domain_and_the_compiled_schema_id() {
    let (path, _ledger) = serve(CapabilityClass::Control);
    let service = block_on(async {
        let session = connect(&path).await;
        session.service.clone()
    });
    assert_eq!(service.name, "clipboard");
    assert!(
        service.schema_matches_app,
        "the served schema id ({:#x}) must match the one this crate compiled ({:#x})",
        service.schema_id,
        smart_clipboard_proto::contract::EXPECTED_CLIPBOARD_SCHEMA_ID
    );
    assert!(service.methods.len() >= 17);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn history_carries_the_contract_units_and_an_unknown_clock_stays_unknown() {
    let (path, _ledger) = serve(CapabilityClass::Control);
    let page = block_on(async {
        let mut session = connect(&path).await;
        let mut client = ClipboardClient::new(&mut session);
        client.page(&Query::first("")).await
    })
    .expect("getHistory must answer");
    assert_eq!(page.total, 2);
    assert_eq!(
        page.items[0].captured_at_ns,
        Some(1_700_000_000_000_000_000)
    );
    assert_eq!(
        page.items[1].captured_at_ns, None,
        "0 is 'clock unreadable', never 1970"
    );
    assert_eq!(page.items[1].mimes, vec!["image/png".to_string()]);
    assert!(page.items[1].pinned);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_preview_that_does_not_exist_is_an_answer_not_an_error() {
    let (path, _ledger) = serve(CapabilityClass::Control);
    let (served, missing) = block_on(async {
        let mut session = connect(&path).await;
        let mut client = ClipboardClient::new(&mut session);
        let served = client.preview(ItemId(1)).await;
        let missing = client.preview(ItemId(2)).await;
        (served, missing)
    });
    match served {
        PreviewState::Ready { text, .. } => assert_eq!(text, "hello from the ledger"),
        other => panic!("expected a served preview, got {other:?}"),
    }
    assert!(matches!(missing, PreviewState::Unavailable { .. }));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn set_content_reaches_the_backend_with_the_bytes_the_client_offered() {
    let (path, ledger) = serve(CapabilityClass::Control);
    block_on(async {
        let mut session = connect(&path).await;
        let mut client = ClipboardClient::new(&mut session);
        client
            .set_content("text/plain", b"offered by the app")
            .await
    })
    .expect("a control connection may write");
    let written = ledger
        .written
        .lock()
        .expect("the mock's lock")
        .clone()
        .expect("the backend saw the write");
    assert_eq!(written.mime_type, "text/plain");
    assert_eq!(written.data, b"offered by the app");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_read_only_connection_is_refused_and_the_class_survives_the_mapping() {
    let (path, ledger) = serve(CapabilityClass::ReadOnly);
    let outcome = block_on(async {
        let mut session = connect(&path).await;
        let mut client = ClipboardClient::new(&mut session);
        client.set_content("text/plain", b"nope").await
    });
    match outcome {
        Err(ClientError::Refused(refusal)) => {
            assert_eq!(refusal.class, RefusalClass::PermissionDenied);
            assert!(!refusal.reason.is_empty(), "a refusal must carry words");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(
        ledger.written.lock().expect("the mock's lock").is_none(),
        "the backend was never reached"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_worker_reports_every_candidate_it_tried() {
    use smart_clipboard_proto::{BackendEvent, Command, Worker};

    // No service is listening at either address, so the worker must walk the
    // list and report each attempt — the connection panel's whole content.
    let location = ServiceLocation {
        candidates: vec![
            Candidate {
                scheme: "unix".to_string(),
                address: format!("/tmp/smart-clipboard-absent-{}.sock", std::process::id()),
                source: EndpointSource::Argument,
            },
            Candidate {
                scheme: "unix".to_string(),
                address: format!(
                    "/tmp/smart-clipboard-absent-too-{}.sock",
                    std::process::id()
                ),
                source: EndpointSource::TransportDefault,
            },
        ],
    };
    let (worker, events) = Worker::spawn(location);

    let mut dialled = 0;
    let mut failed = false;
    while let Ok(event) = events.recv_blocking() {
        match event {
            BackendEvent::DialFailed { .. } => dialled += 1,
            BackendEvent::ConnectFailed { attempts, .. } => {
                assert_eq!(attempts.len(), 2);
                failed = true;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(dialled, 2);
    assert!(failed, "the worker must end the walk with ConnectFailed");
    worker.send(Command::Shutdown);
}
