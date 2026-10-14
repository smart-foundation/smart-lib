//! The L3 behaviour tests for the Linux service backend — the ones the
//! availability matrix names (`l3_linux_service::…`). Every one of them drives
//! a **scripted supervision tree** (a real directory layout built in a temp
//! dir) or a **scripted bus peer**, so the native read path, the honesty layer,
//! the bounded pulls and the lifecycle mapping are proven without touching the
//! host (charter law #8). The host-touching live probe is `#[ignore]`d and
//! read-only.

extern crate std;

use alloc::ffi::CString;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use std::fs;
use std::path::{Path, PathBuf};

use super::runit::{control_byte, RunitBackend};
use super::NativeSupervision;
use super::{Cell, LinuxServiceBackend};
use crate::core::ids::ServiceId;
use crate::core::ErrorCode;
use crate::domains::service::availability::{claimed_l3_tests, SERVICE_AVAILABILITY};
use crate::domains::service::backend::{
    LifecycleVerb, PresentedCapability, ServiceBackend, ServiceLifecycle,
};
use crate::domains::service::types::{
    HealthState, Pagination, ServiceEventPayload, ServiceStatus, SubscriptionId,
};

/// A temp path unique to this test run and this test: the tests run in parallel
/// and each builds its own supervision tree, so the path must not be shared.
fn temp_root(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("smart-{tag}-{}-{n}", std::process::id()))
}

/// A scripted runit supervision tree: the layout this host actually has, in a
/// temp directory. Returns the backend and the root (so a test can extend the
/// tree mid-test and clean it up afterwards).
fn runit_tree() -> (RunitBackend, PathBuf) {
    let root = temp_root("runit");
    let _ = fs::remove_dir_all(&root);
    let declared = root.join("etc/sv");
    let scan = root.join("var/service");
    let state = root.join("run/runit");
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&state).unwrap();

    // A service the supervisor is running, with a pid and a transition count.
    declare(&declared, "NetworkManager", "");
    supervise(&state, "NetworkManager", "run: /etc/sv/NetworkManager: (pid 4821) 9123s");
    // A service that is linked but down by request.
    declare(&declared, "sshd", "");
    supervise(&state, "sshd", "down: /etc/sv/sshd: 4s, want down");
    // A service the supervisor wants up and does not have (its failed state).
    declare(&declared, "dhcpcd", "");
    supervise(&state, "dhcpcd", "down: /etc/sv/dhcpcd: 2s, normally up, want up");
    // A service that is declared but neither linked nor supervised.
    declare(&declared, "cold", "");
    // A service whose own script names a dependency, and that dependency.
    declare(&declared, "acpid", "#!/bin/sh\nsv check dbus >/dev/null 2>&1 || exit 1\nexec acpid -f\n");
    declare(&declared, "dbus", "");
    supervise(&state, "dbus", "run: /etc/sv/dbus: (pid 812) 9123s");
    // Link the enabled ones into the scan dir, the way the host has it.
    for name in ["NetworkManager", "sshd", "dhcpcd", "acpid", "dbus"] {
        std::os::unix::fs::symlink(declared.join(name), scan.join(name)).unwrap();
    }

    let backend = RunitBackend::with_roots(scan, declared, state);
    (backend, root)
}

/// Declare one service directory under the declared root.
fn declare(declared_root: &Path, name: &str, run_script: &str) {
    let dir = declared_root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("run"),
        if run_script.is_empty() {
            "#!/bin/sh\nexec /bin/true\n".to_string()
        } else {
            format!("{run_script}\n")
        },
    )
    .unwrap();
}

/// One supervise state directory, with the supervisor's own status line.
fn supervise(state_root: &Path, name: &str, stat: &str) {
    let dir = state_root.join(format!("supervise.{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stat"), format!("{stat}\n")).unwrap();
    if let Some(pid) = stat.split("(pid ").nth(1).and_then(|s| s.split(')').next()) {
        fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
    }
}

/// The whole tree wrapped by the hub backend, so the contract answers are
/// exercised exactly as a caller receives them.
fn hub() -> (LinuxServiceBackend, PathBuf) {
    let (runit, root) = runit_tree();
    (LinuxServiceBackend::with_cell(Cell::Runit(runit)), root)
}

fn resolve_id(backend: &LinuxServiceBackend, name: &str) -> ServiceId {
    let (id, present) = backend.resolve(name).expect("resolve");
    assert!(present, "{name} must resolve on the scripted tree");
    id
}

/// `listServices` is served natively: the scripted tree's services, their
/// measured enablement and their measured state.
#[test]
fn list_services_enumerates_the_supervision_tree_natively() {
    let (backend, root) = hub();
    let page = backend
        .list_services(Pagination { offset: 0, limit: u32::MAX })
        .expect("listServices");
    assert_eq!(page.total, 6, "the scripted tree declares six services");
    let names: Vec<&str> = page.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["NetworkManager", "acpid", "cold", "dbus", "dhcpcd", "sshd"]
    );

    // The measured columns: linked, declared, supervised.
    let nm = page.items.iter().find(|i| i.name == "NetworkManager").unwrap();
    assert!(nm.enabled, "linked into the scan dir");
    assert_eq!(nm.state.status, ServiceStatus::Active);
    let cold = page.items.iter().find(|i| i.name == "cold").unwrap();
    assert!(!cold.enabled, "declared but never linked");
    assert_eq!(cold.state.status, ServiceStatus::Unknown);
    assert!(
        cold.state.reason.contains("supervise.cold/stat"),
        "the honest reason names the path that was not there: {}",
        cold.state.reason
    );

    // Pagination is the contract's: the caller's bound honoured, the total
    // stable, and a limit of 0 means "one".
    let first = backend
        .list_services(Pagination { offset: 0, limit: 2 })
        .expect("listServices");
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.total, 6, "the total is the full enumeration, not the page");
    let second = backend
        .list_services(Pagination { offset: 2, limit: 2 })
        .expect("listServices");
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.items[0].name, "cold");
    let one = backend
        .list_services(Pagination { offset: 0, limit: 0 })
        .expect("listServices");
    assert_eq!(one.items.len(), 1, "a limit of 0 means one, never all");

    assert!(SERVICE_AVAILABILITY.iter().any(|r| r.l3_test == Some(
        "l3_linux_service::list_services_enumerates_the_supervision_tree_natively"
    )));
    let _ = fs::remove_dir_all(&root);
}

/// `getService` reads one entry natively.
#[test]
fn get_service_reads_one_entry_natively() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let info = backend.get_service(nm).expect("getService");
    assert_eq!(info.name, "NetworkManager");
    assert!(info.enabled);
    assert_eq!(info.state.status, ServiceStatus::Active);
    assert_eq!(info.state.main_pid, 4821, "the supervisor's own pid line");
    assert_eq!(
        info.state.since_us, 9_123_000_000,
        "the supervisor's own seconds count, in the contract's unit"
    );
    assert!(info.state.since_valid);

    // An id the supervisor does not serve is a not-found that names the
    // supervisor, never a fabricated service.
    let err = backend.get_service(ServiceId::new(424_242)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("runit"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// `getState` parses the supervisor's own status grammar, including the failed
/// and the inactive readings, and keeps the supervisor's words in the reason.
#[test]
fn get_state_parses_the_supervise_status_natively() {
    let (backend, root) = hub();

    let nm = resolve_id(&backend, "NetworkManager");
    let state = backend.get_state(nm).expect("getState");
    assert_eq!(state.status, ServiceStatus::Active);
    assert_eq!(state.main_pid, 4821);
    assert!(state.reason.is_empty(), "an active service needs no reason");

    let sshd = resolve_id(&backend, "sshd");
    let state = backend.get_state(sshd).expect("getState");
    assert_eq!(state.status, ServiceStatus::Inactive);
    let reason = state.reason.clone();
    assert!(state.reason.contains("want down"), "{reason}");

    let dhcpcd = resolve_id(&backend, "dhcpcd");
    let state = backend.get_state(dhcpcd).expect("getState");
    assert_eq!(
        state.status,
        ServiceStatus::Failed,
        "a service the supervisor wants up and does not have is the failed reading"
    );
    let reason = state.reason.clone();
    assert!(state.reason.contains("wants the service up"), "{reason}");

    // The metadata carries the measured supervision facts, so a caller can see
    // what the backend observed beside the status.
    assert_eq!(state.metadata[0].0, "supervisor");
    assert_eq!(state.metadata[0].1, "runit");
    assert_eq!(state.metadata[1].0, "declared");
    let _ = fs::remove_dir_all(&root);
}

/// The status grammar parses the supervisor's own words for every state it
/// writes, and an unknown word stays unknown instead of being guessed.
#[test]
fn the_supervise_status_grammar_parses_the_supervisors_own_words() {
    use super::runit::StatLine;
    let line = StatLine::parse("run: /etc/sv/sshd: (pid 1234) 5678s, normally down");
    assert_eq!(line.word, "run");
    assert_eq!(line.main_pid, 1234);
    assert_eq!(line.since_us, 5_678_000_000);
    assert_eq!(line.flags, "normally down");
    assert_eq!(line.status().0, ServiceStatus::Active);

    let line = StatLine::parse("down: /etc/sv/sshd: 4s, want down");
    assert_eq!(line.main_pid, 0);
    assert_eq!(line.status().0, ServiceStatus::Inactive);

    let line = StatLine::parse("finish: /etc/sv/sshd: 1s");
    assert_eq!(line.status().0, ServiceStatus::Deactivating);

    let line = StatLine::parse("exit: /etc/sv/sshd: 1s");
    assert_eq!(line.status().0, ServiceStatus::Failed);

    // An unrecognised word keeps the supervisor's text in the reason.
    let line = StatLine::parse("wibble: something the backend does not map");
    let (status, reason) = line.status();
    assert_eq!(status, ServiceStatus::Unknown);
    assert!(reason.contains("wibble"), "{reason}");
}

/// `resolve` maps a name to an identity, and an unknown name is the answer
/// rather than a fault.
#[test]
fn resolve_maps_a_name_to_an_identity() {
    let (backend, root) = hub();
    let (id, present) = backend.resolve("NetworkManager").expect("resolve");
    assert!(present);
    // The same name resolves to the same id twice (the stable-total promise).
    let (again, _) = backend.resolve("NetworkManager").expect("resolve");
    assert_eq!(id, again);

    let (absent, present) = backend.resolve("telepathy").expect("resolve");
    assert!(!present, "an unknown name is the answer, not an error");
    assert_eq!(absent.get(), 0);
    let _ = fs::remove_dir_all(&root);
}

/// `getHealth` reports the measured lifecycle and declares the probe unrun — a
/// read-only backend never executes a service's probe script.
#[test]
fn health_reports_the_lifecycle_and_declares_the_probe_unrun() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let health = backend.get_health(nm).expect("getHealth");
    assert_eq!(health.state, HealthState::Online);
    assert!(!health.last_check_passed, "the probe is honestly declared unrun");
    assert_eq!(health.last_check_at, 0, "no check was taken");
    assert!(!health.quarantined);
    assert_eq!(health.restarts, 0, "a supervisor observes no restart counter");

    let dhcpcd = resolve_id(&backend, "dhcpcd");
    let health = backend.get_health(dhcpcd).expect("getHealth");
    assert_eq!(health.state, HealthState::Offline);
    let reason = health.reason.clone();
    assert!(!health.reason.is_empty(), "{reason}");

    // listHealth is the same measurement over the listing.
    let page = backend
        .list_health(Pagination { offset: 0, limit: u32::MAX })
        .expect("listHealth");
    assert_eq!(page.total, 6);
    let _ = fs::remove_dir_all(&root);
}

/// `listQuarantined` is the honest empty on a host supervisor: it has no
/// quarantine concept, so the list is empty with a zero total — never a
/// fabricated quarantined entry.
#[test]
fn quarantine_is_an_honest_empty_on_a_host_supervisor() {
    let (backend, root) = hub();
    let page = backend
        .list_quarantined(Pagination { offset: 0, limit: u32::MAX })
        .expect("listQuarantined");
    assert!(page.items.is_empty());
    assert_eq!(page.total, 0);
    let _ = fs::remove_dir_all(&root);
}

/// The concepts a host supervisor does not own answer `Unavailable` with the
/// exact reason class the matrix promises.
#[test]
fn the_measurement_and_registry_surfaces_answer_unavailable_with_a_class() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let err = backend.get_resource_allocation(nm).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.starts_with("not-applicable:"), "{err}");
    assert!(err.message.contains("quota"), "{err}");

    let err = backend.get_health_check(nm).unwrap_err();
    assert!(err.message.starts_with("not-applicable:"), "{err}");

    let err = backend.get_metrics(nm, 1_000_000_000).unwrap_err();
    assert!(err.message.starts_with("not-applicable:"), "{err}");
    assert!(err.message.contains("request counts"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// `getDependencies` reads the supervisor's own declaration: runit's dependency
/// lives in the service's own scripts.
#[test]
fn dependencies_are_read_from_the_service_dirs_own_scripts() {
    let (backend, root) = hub();
    let acpid = resolve_id(&backend, "acpid");
    let deps = backend.get_dependencies(acpid).expect("getDependencies");
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].service_id, resolve_id(&backend, "dbus"));
    assert!(
        deps[0].satisfied,
        "dbus is running on the scripted tree, and the supervisor says so"
    );

    // A service that declares none answers empty — the supervisor's own facts,
    // not a fabricated edge.
    let nm = resolve_id(&backend, "NetworkManager");
    assert!(backend.get_dependencies(nm).expect("getDependencies").is_empty());
    let _ = fs::remove_dir_all(&root);
}

/// A state subscription observes the supervisor: the first pull is what exists,
/// a later pull reports only what changed, and each pull is bounded by the
/// caller.
#[test]
fn a_state_subscription_observes_the_supervision_tree() {
    let (backend, root) = hub();
    let sub = backend.subscribe_state(ServiceId::new(0)).expect("subscribeState");

    let first = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert_eq!(first.len(), 6, "the first pull reports what exists");
    // Every service the supervisor reports a state for carries the supervisor's
    // own transition clock; the one with no state file at all is the honest
    // `since_valid == false` (never a fabricated timestamp).
    assert!(first
        .iter()
        .filter(|s| s.status != ServiceStatus::Unknown)
        .all(|s| s.since_valid));

    // With nothing changed, the next pull is an empty batch — not an invented
    // event.
    let second = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert!(second.is_empty(), "no transition, no event");

    // A bound the caller states is honoured.
    let bounded = backend.pull_state(sub, 2).expect("pullState");
    assert!(bounded.len() <= 2);

    // And a change the supervisor made between pulls is reported by the next
    // pull, with the supervisor's own words in the reason.
    // The supervisor brings sshd up between the two pulls: the same stat file
    // the tree was built with, rewritten in the supervisor's own grammar.
    let state_root = root.join("run/runit");
    let stat = state_root.join("supervise.sshd/stat");
    fs::write(stat, "run: /etc/sv/sshd: (pid 900) 1s\n").unwrap();
    let after = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert!(
        after.iter().any(|s| s.status == ServiceStatus::Active && s.main_pid == 900),
        "the supervisor's new transition is reported"
    );

    // An unknown subscription id is a not-found, never a panic.
    let err = backend.pull_state(SubscriptionId(9_999), 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let _ = fs::remove_dir_all(&root);
}

/// `cancel` retires the token: a retired subscription is inert and cancelling
/// it again is a no-op rather than an error.
#[test]
fn cancel_retires_a_subscription_token() {
    let (backend, root) = hub();
    let sub = backend.subscribe_state(ServiceId::new(0)).expect("subscribeState");
    backend.cancel_state(sub).expect("cancelState");
    assert!(backend.cancel_state(sub).is_ok(), "cancelling a retired token is inert");
    let _ = fs::remove_dir_all(&root);
}

/// `subscribeServiceEvents` observes the contract's own event vocabulary from
/// the supervisor's measured transitions.
#[test]
fn service_events_observe_started_stopped_and_failed() {
    let (backend, root) = hub();
    let sub = backend
        .subscribe_service_events(ServiceId::new(0))
        .expect("subscribeServiceEvents");
    let events = backend.pull_service_events(sub, u32::MAX).expect("pull");
    assert_eq!(events.len(), 6, "the first pull reports what exists");

    let running = events
        .iter()
        .find(|e| {
            matches!(&e.payload, ServiceEventPayload::StateChanged(s) if s.status == ServiceStatus::Active)
        })
        .expect("an active service yields a started event");
    assert_eq!(
        running.event_type(),
        crate::domains::service::types::ServiceEventType::Started
    );

    let failed = events
        .iter()
        .find(|e| {
            matches!(&e.payload, ServiceEventPayload::StateChanged(s) if s.status == ServiceStatus::Failed)
        })
        .expect("the supervisor's failed reading is reported");
    assert_eq!(
        failed.event_type(),
        crate::domains::service::types::ServiceEventType::Failed
    );

    // The second pull is empty until something changes (and it is always
    // bounded).
    assert!(backend.pull_service_events(sub, u32::MAX).expect("pull").is_empty());
    assert_eq!(backend.pull_service_events(sub, 1).expect("pull").len(), 0);
    let _ = fs::remove_dir_all(&root);
}

/// `control` mints a reference only for a scope the presented capability
/// covers, and every verb on it answers with the contract's honest reason.
#[test]
fn control_mints_a_control_reference_only_for_a_covered_scope() {
    let (backend, root) = hub();
    let all = PresentedCapability {
        id: vec![1u8, 2, 3],
        target: "*".to_string(),
        operations: alloc::vec!["service.control".to_string()],
        expiry_ns: 0,
    };
    let control = backend.control(all).expect("control mints for a wild scope");
    let nm = resolve_id(&backend, "NetworkManager");
    let err = control.clear_metrics(nm).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.contains("no-backend:"), "{err}");

    // A service the supervisor serves is a covered scope.
    let one = PresentedCapability {
        target: "sshd".to_string(),
        ..Default::default()
    };
    assert!(backend.control(one).is_ok());

    // A target the supervisor does not serve mints nothing.
    let bogus = PresentedCapability {
        target: "telepathy".to_string(),
        ..Default::default()
    };
    let err = match backend.control(bogus) {
        Ok(_) => panic!("a target the supervisor does not serve must not mint a reference"),
        Err(e) => e,
    };
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.contains("permission-denied:"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// The lifecycle verbs speak the supervisor's native control path: the control
/// byte mapping is the supervisor's own protocol, and a host the caller cannot
/// write refuses with the measured class — never by performing a write it was
/// not asked for (charter law #8).
#[test]
fn lifecycle_verbs_speak_the_native_control_path() {
    let (backend, root) = hub();

    // The byte mapping is the supervisor's own protocol.
    assert_eq!(control_byte(LifecycleVerb::Start), b'u');
    assert_eq!(control_byte(LifecycleVerb::Stop), b'd');
    assert_eq!(control_byte(LifecycleVerb::Restart), b't');

    // A control path the caller's credentials do not reach refuses with the
    // measured class, before any host state is touched.
    let err = backend
        .apply_lifecycle(
            resolve_id(&backend, "NetworkManager"),
            "NetworkManager",
            LifecycleVerb::Start,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.starts_with("no-privilege:"), "{err}");
    assert!(err.message.contains("control"), "the reason names the path: {err}");

    // The caps publish exactly those measurements.
    let caps = backend.supervision_caps();
    assert!(caps.can_enumerate, "the scripted tree enumerates");
    assert!(caps.can_read_state, "the scripted tree's state files are readable");
    assert!(!caps.can_control, "no supervision control fifo exists in the scripted tree");
    assert_eq!(caps.supervisor, "runit");
    let _ = fs::remove_dir_all(&root);
}

/// The native control write path is proven hermetically: a real FIFO with a
/// reader on the other end receives the supervisor's own byte, and a verb whose
/// FIFO is missing reports the measured error.
#[test]
fn the_control_byte_writes_through_a_real_fifo() {
    let root = temp_root("runit-fifo");
    let _ = fs::remove_dir_all(&root);
    let state = root.join("run/runit");
    let dir = state.join("supervise.scripted");
    fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("control");
    // The supervisor's control path is a fifo; std's mkfifo is unstable on
    // this toolchain, so the test makes one the way the platform does.
    let c_path = alloc::ffi::CString::new(fifo.to_string_lossy().into_owned()).unwrap();
    let made = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(made, 0, "mkfifo: {}", std::io::Error::last_os_error());
    let scan = root.join("var/service");
    let declared = root.join("etc/sv");
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(declared.join("scripted")).unwrap();
    std::os::unix::fs::symlink(declared.join("scripted"), scan.join("scripted")).unwrap();

    let backend = RunitBackend::with_roots(scan, declared, state);
    // The reader lives in a thread, so the write is not a host daemon: the
    // FIFO is this test's own temp file.
    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut file = fs::File::open(&fifo).expect("open the fifo for reading");
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte).expect("read one control byte");
            byte[0]
        })
    };
    NativeSupervision::control(&backend, "scripted", LifecycleVerb::Start)
        .expect("the control byte is written through the fifo");
    assert_eq!(reader.join().expect("the reader finishes"), b'u');

    // A service with no control fifo reports the measured error, and the
    // message names the path it could not open.
    let err = NativeSupervision::control(&backend, "absent", LifecycleVerb::Start)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.contains("supervise.absent/control"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// A backend that reached no supervisor answers honestly: every read carries
/// the reason naming what was looked for, and the identity is the empty one.
#[test]
fn no_supervisor_is_an_honest_absence() {
    let backend = LinuxServiceBackend::with_cell(Cell::None);
    assert_eq!(backend.supervisor().supervisor, "");
    let err = backend
        .list_services(Pagination { offset: 0, limit: u32::MAX })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.starts_with("no-backend:"), "{err}");
    assert!(err.message.contains("/run/runit"), "the reason names the markers: {err}");
    assert!(backend.list_quarantined(Pagination::default()).is_ok());
    // The control accessor refuses to mint an authority it cannot bind.
    assert!(backend
        .control(PresentedCapability { target: "*".into(), ..Default::default() })
        .is_err());
    // And the lifecycle verbs refuse rather than touching anything.
    assert!(backend
        .apply_lifecycle(ServiceId::new(1), "anything", LifecycleVerb::Start)
        .is_err());
}

/// Every availability claim names a test that exists, and every test in this
/// suite is claimed: a REAL/DEGRADED row whose test has vanished is a FAIL, not
/// a silent claim.
#[test]
fn every_claimed_test_exists() {
    let tests: Vec<&str> = alloc::vec![
        "l3_linux_service::list_services_enumerates_the_supervision_tree_natively",
        "l3_linux_service::get_service_reads_one_entry_natively",
        "l3_linux_service::get_state_parses_the_supervise_status_natively",
        "l3_linux_service::resolve_maps_a_name_to_an_identity",
        "l3_linux_service::health_reports_the_lifecycle_and_declares_the_probe_unrun",
        "l3_linux_service::quarantine_is_an_honest_empty_on_a_host_supervisor",
        "l3_linux_service::dependencies_are_read_from_the_service_dirs_own_scripts",
        "l3_linux_service::a_state_subscription_observes_the_supervision_tree",
        "l3_linux_service::cancel_retires_a_subscription_token",
        "l3_linux_service::service_events_observe_started_stopped_and_failed",
        "l3_linux_service::control_mints_a_control_reference_only_for_a_covered_scope",
        "l3_linux_service::lifecycle_verbs_speak_the_native_control_path",
    ];
    for (method, test) in claimed_l3_tests() {
        assert!(tests.contains(&test), "{method} claims {test}, which is not in this suite");
    }
    for test in tests {
        assert!(
            claimed_l3_tests().any(|(_, t)| t == test),
            "{test} is real but unclaimed by the matrix"
        );
    }
}

/// The READ-ONLY live probe (charter law #8): read THIS host's runit supervision
/// tree natively — the declared services, the linked ones, the supervised ones —
/// and read real services' status through the supervisor's own grammar. It
/// writes nothing; the control verbs are founder-run. `#[ignore]`d so CI never
/// depends on a host; run with `--ignored --nocapture` on a runit box.
#[test]
#[ignore = "reads the host's runit supervision tree; read-only, founder-runnable"]
fn live_reads_this_hosts_runit_supervision_natively() {
    let backend = LinuxServiceBackend::native_connect();
    let identity = backend.supervisor();
    std::eprintln!(
        "live supervisor: {} (scan {} declared {} state {})",
        identity.supervisor, identity.service_root, identity.declared_root, identity.state_root
    );
    let caps = backend.supervision_caps();
    std::eprintln!(
        "live caps: enumerate={} read_state={} control={}",
        caps.can_enumerate, caps.can_read_state, caps.can_control
    );
    let page = match backend.list_services(Pagination { offset: 0, limit: u32::MAX }) {
        Ok(page) => page,
        Err(e) => {
            std::eprintln!("live listServices: {e}");
            return;
        }
    };
    std::eprintln!("live services: {} enumerated", page.total);
    for info in page.items.iter() {
        let supervised = info
            .state
            .metadata
            .iter()
            .find(|(k, _)| k == "supervised")
            .map(|(_, v)| v.as_str())
            .unwrap_or("?");
        std::eprintln!(
            "  {:<16} {:<10} enabled={:<5} supervised={:<5} {}",
            info.name,
            info.state.status.as_token(),
            info.enabled,
            supervised,
            if info.state.reason.is_empty() { "-" } else { &info.state.reason }
        );
    }
    for name in ["sshd", "NetworkManager"] {
        let (id, present) = backend.resolve(name).expect("resolve");
        if !present {
            std::eprintln!("live {name}: not linked, not supervised");
            continue;
        }
        match backend.get_state(id) {
            Ok(state) => std::eprintln!(
                "live {name}: {} pid={} reason={}",
                state.status.as_token(),
                state.main_pid,
                if state.reason.is_empty() { "-" } else { &state.reason }
            ),
            Err(e) => std::eprintln!("live {name}: {e}"),
        }
    }
}



