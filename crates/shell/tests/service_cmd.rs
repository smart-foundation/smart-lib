//! The service command module over the scripted engine: the command surface's
//! data half, its rendering, the host-write gate on the lifecycle verbs, the
//! honest answers for what a host supervisor does not own, and the
//! `@service/*` selector tree. Nothing here touches the host (charter law #8).

mod common;

use clap::Parser;
use common::MockService;
use smart_lib::domains::service::ServiceBackend;
use shell::domains::service::{evaluate, selector_root, ServiceCmd, WriteGuard};
use shell::selector::{self, resolve};
use shell::Value;

fn cmd(argv: &[&str]) -> ServiceCmd {
    ServiceCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the supervisor's own service table: the scripted
/// services with their measured states, enablement and supervision.
#[test]
fn the_default_view_is_the_service_table() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service"]), &backend, refused()).unwrap();
    match value {
        Value::List(services) => {
            assert_eq!(services.len(), 5, "the scripted supervisor serves five");
            let nm = &services[0];
            assert_eq!(nm.field_str("name"), "NetworkManager");
            assert_eq!(nm.field_str("state"), "active");
            assert!(nm.field_bool("enabled"));
            assert!(nm.field_bool("supervised"));
            let sshd = &services[1];
            assert_eq!(sshd.field_str("state"), "inactive");
            assert_eq!(sshd.field_str("pid"), "0", "no pid is fabricated");
            let dhcpcd = &services[2];
            assert_eq!(dhcpcd.field_str("state"), "failed");
            assert!(!dhcpcd.field_str("state_reason").is_empty());
            let cold = &services[3];
            assert!(!cold.field_bool("supervised"));
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// A caller whose credentials cannot read the supervisor's state gets the
/// honest `unknown` with the measured reason — never a fabricated state and
/// never a fake empty table.
#[test]
fn a_caller_without_the_state_read_sees_unknown_with_a_reason() {
    let backend = MockService::default();
    backend.can_read_state.set(false);
    let value = evaluate(&cmd(&["service"]), &backend, refused()).unwrap();
    match value {
        Value::List(services) => {
            assert!(services
                .iter()
                .all(|s| s.field_str("state") == "unknown"));
            assert!(services
                .iter()
                .all(|s| s.field_str("state_reason").starts_with("no-privilege:")));
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// `service inspect <name>`: one service's state, and the honest answer for a
/// name the supervisor does not serve.
#[test]
fn inspect_reads_one_service_and_answers_honestly_when_absent() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service", "inspect", "NetworkManager"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("name"), "NetworkManager");
    assert_eq!(value.field_str("state"), "active");
    assert_eq!(value.field_str("pid"), "4821");
    assert!(value.field_bool("enabled"));

    let value = evaluate(&cmd(&["service", "inspect", "telepathy"]), &backend, refused()).unwrap();
    assert!(!value.field_bool("present"), "an unknown name is the answer, not a fault");
}

/// The lifecycle verbs are gate-routed: refused without `--force`, recorded
/// once accepted — and refused again by the engine when the caller's credentials
/// do not reach the supervisor's control path.
#[test]
fn the_lifecycle_verbs_are_gate_routed() {
    let backend = MockService::default();
    let err = evaluate(&cmd(&["service", "start", "sshd"]), &backend, refused()).unwrap_err();
    assert!(err.to_string().contains("CHANGES host state"), "{err}");
    assert!(backend.writes.borrow().is_empty(), "a refused write reaches no engine");

    evaluate(&cmd(&["service", "start", "sshd"]), &backend, allowed()).unwrap();
    evaluate(&cmd(&["service", "stop", "sshd"]), &backend, allowed()).unwrap();
    evaluate(&cmd(&["service", "restart", "sshd"]), &backend, allowed()).unwrap();
    assert_eq!(
        backend.writes.borrow().as_slice(),
        ["start sshd", "stop sshd", "restart sshd"],
        "each accepted write reaches the engine, in the caller's words"
    );

    // A caller without the control path is refused by the engine, with the
    // measured class — the write is never performed.
    backend.can_control.set(false);
    let err = evaluate(&cmd(&["service", "start", "sshd"]), &backend, allowed()).unwrap_err();
    assert!(err.to_string().contains("no-privilege"), "{err}");
}

/// The registry-config surface a host supervisor does not own answers with the
/// contract's own reason class, in a shape the shell can render.
#[test]
fn the_surfaces_a_supervisor_does_not_own_answer_with_their_reason_class() {
    let backend = MockService::default();
    for verb in ["metrics", "allocation", "check"] {
        let argv = format!("service {verb} NetworkManager");
        let argv: Vec<&str> = argv.split(' ').collect();
        let value = evaluate(&cmd(&argv), &backend, refused()).unwrap();
        let reason = value.field_str("error");
        assert!(
            !reason.is_empty(),
            "{verb} must answer with its measured reason"
        );
        assert!(reason.contains("not-applicable") || reason.contains("no-backend"), "{reason}");
    }

    // The gated control surface lists every verb with its own reason class.
    let value = evaluate(&cmd(&["service", "controls"]), &backend, refused()).unwrap();
    match value {
        Value::List(verbs) => {
            assert_eq!(verbs.len(), 7, "the seven ServiceControl contract verbs");
            assert_eq!(verbs[0].field_str("verb"), "setDependencies");
            assert_eq!(verbs[0].field_str("reason_class"), "no-backend");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let value = evaluate(&cmd(&["service", "controls", "setQuarantine"]), &backend, refused()).unwrap();
    match value {
        Value::List(verbs) => {
            assert_eq!(verbs.len(), 1);
            assert_eq!(verbs[0].field_str("verb"), "setQuarantine");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// `service logs` answers honestly that log reading is logging.capnp's surface:
/// a reason naming the contract split, never a fabricated log line.
#[test]
fn logs_answer_honestly() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service", "logs", "sshd"]), &backend, refused()).unwrap();
    let reason = value.field_str("reason");
    assert!(reason.contains("service.capnp OBSERVES"), "{reason}");
    assert!(reason.contains("logging.capnp"), "{reason}");
    assert!(reason.contains("sshd"), "{reason}");
}

/// `service health` renders the contract's own health view; `--quarantined`
/// is the honest empty on a host supervisor.
#[test]
fn health_reports_the_lifecycle_and_the_probe_unrun() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service", "health"]), &backend, refused()).unwrap();
    match value {
        Value::List(services) => {
            assert_eq!(services.len(), 5);
            assert!(!services[0].field_bool("last_check_passed"), "the probe is not run");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let value = evaluate(&cmd(&["service", "health", "--quarantined"]), &backend, refused()).unwrap();
    assert_eq!(value.field("total").as_f64(), Some(0.0));
}

/// The dependencies and the resolution are the supervisor's own facts.
#[test]
fn deps_and_resolve_are_the_supervisors_own_facts() {
    let backend = MockService::default();
    // The supervisor's own declaration: one edge for NetworkManager, none for
    // a service that declares none.
    let value = evaluate(&cmd(&["service", "deps", "NetworkManager"]), &backend, refused()).unwrap();
    match value {
        Value::List(deps) => {
            assert_eq!(deps.len(), 1);
            assert_eq!(deps[0].field_str("name"), "dbus", "the identity resolves back to its name");
            assert_eq!(deps[0].field_str("dependency_type"), "required");
            assert!(deps[0].field_bool("satisfied"), "the script says it is running");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let value = evaluate(&cmd(&["service", "deps", "sshd"]), &backend, refused()).unwrap();
    match value {
        Value::List(deps) => assert!(deps.is_empty(), "the script declares none"),
        other => panic!("expected a list, got {other:?}"),
    }
    let value = evaluate(&cmd(&["service", "resolve", "sshd"]), &backend, refused()).unwrap();
    assert!(value.field_bool("present"));
    let value = evaluate(&cmd(&["service", "resolve", "telepathy"]), &backend, refused()).unwrap();
    assert!(!value.field_bool("present"));
}

/// The bounded event stream: one subscription, one bounded pull, retired.
#[test]
fn watch_takes_one_bounded_pull() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service", "watch"]), &backend, refused()).unwrap();
    match value {
        Value::List(events) => assert!(!events.is_empty(), "the scripted supervisor has a transition"),
        other => panic!("expected a list, got {other:?}"),
    }
    let value = evaluate(&cmd(&["service", "watch", "sshd", "--count", "1"]), &backend, refused()).unwrap();
    match value {
        Value::List(events) => assert!(events.len() <= 1, "the caller's bound is honoured"),
        other => panic!("expected a list, got {other:?}"),
    }
}

/// An opaque identity is rendered exactly. The shell's value tree carries
/// numbers as `f64`, whose exact integer range ends at 2^53, and `T.ServiceId`
/// is a 64-bit hash the engine issues — so an id riding as a number would be a
/// number that is NOT the id (rounded), and a caller reading it back would
/// address a service the engine never issued. It therefore travels as its exact
/// decimal text, in the human and the JSON rendering alike.
#[test]
fn an_opaque_identity_is_rendered_exactly_never_rounded() {
    let backend = MockService::default();
    // The scripted ids are the engine's own kind: a 64-bit hash.
    let value = evaluate(&cmd(&["service", "resolve", "sshd"]), &backend, refused()).unwrap();
    let id = value.field_str("service_id");
    assert_eq!(id, backend.resolve("sshd").unwrap().0.get().to_string(), "{id}");
    assert!(!id.contains('.'), "an id is not a float: {id}");

    let json = shell::value::to_json(&value);
    assert!(
        json.contains(&format!("\"service_id\":\"{id}\"")),
        "the id is exact in JSON too: {json}"
    );
}

/// The `@service/*` selector tree is the same data the command renders, so a
/// selector and a command can never disagree.
#[test]
fn the_service_selector_tree_matches_the_command() {
    let backend = MockService::default();
    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@service/services").unwrap();
    match resolve(root, &sel) {
        Value::List(services) => assert_eq!(services.len(), 5),
        other => panic!("expected the service list, got {other:?}"),
    }

    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@service/supervisor/supervisor").unwrap();
    assert_eq!(
        resolve(root, &sel),
        Value::String("scripted".to_string())
    );

    let root = selector_root(&backend).unwrap();
    let sel = selector::parse("@service/services").unwrap();
    let services = resolve(root, &sel);
    let first = match &services {
        Value::List(items) => items[0].clone(),
        other => panic!("expected a list, got {other:?}"),
    };
    assert_eq!(first.field_str("name"), "NetworkManager");
}

/// The JSON rendering is contract-shaped over the owned value tree.
#[test]
fn json_output_is_contract_shaped() {
    let backend = MockService::default();
    let value = evaluate(&cmd(&["service", "resolve", "sshd"]), &backend, refused()).unwrap();
    let json = shell::value::to_json(&value);
    assert!(json.contains("\"present\":true"), "{json}");
}

/// The human aesthetic carries the honest answers: the reason a run state was
/// not readable is printed, not swallowed.
#[test]
fn the_human_rendering_names_the_reason_a_state_was_not_readable() {
    let backend = MockService::default();
    backend.can_read_state.set(false);
    let value = evaluate(&cmd(&["service"]), &backend, refused()).unwrap();
    let rendered =
        shell::domains::service::render::render_action(&cmd(&["service"]), &value, shell::OutputFormat::Human);
    assert!(rendered.contains("Services"), "{rendered}");
    assert!(
        rendered.contains("no-privilege"),
        "the reason travels into the table: {rendered}"
    );
}
