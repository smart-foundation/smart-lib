//! The process command module over the scripted engine: the command surface's
//! data half, its human aesthetic (pixel-perfect under the terminal-faithful
//! measure), the host-write gate on the signal and attach verbs, the measured
//! verdicts the host answers, and the `@process/*` selector tree. Nothing here
//! touches the host (charter law #8): every read is served from the script.

#![cfg(feature = "domain-process")]

mod common;

use clap::Parser;
use common::{assert_borders_hold, MockProcess};
use shell::domains::process::{evaluate, render, selector_root, ProcessCmd, WriteGuard};
use shell::selector;
use shell::widgets::display_width;
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> ProcessCmd {
    ProcessCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}


/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the process table: the kernel's own measured rows.
#[test]
fn the_default_view_is_the_process_table() {
    let backend = MockProcess::default();
    let value = evaluate(&cmd(&["process"]), &backend, refused()).unwrap();
    match value {
        Value::List(rows) => {
            assert_eq!(rows.len(), 3, "the scripted host runs three");
            let runit = &rows[0];
            assert_eq!(runit.field_str("pid"), "1");
            assert_eq!(runit.field_str("name"), "runit");
            assert_eq!(runit.field_str("state"), "blocked");
            assert_eq!(runit.field_str("uid"), "0");
            assert_eq!(runit.field("mem").as_f64(), Some(92.0 * 1024.0));
            assert_eq!(runit.field("cpu_nanos").as_f64(), Some(1_050_000_000.0));
            assert_eq!(runit.field_str("threads"), "1");
            let smart = &rows[1];
            assert_eq!(smart.field_str("state"), "active");
            assert_eq!(smart.field("mem").as_f64(), Some(24.0 * 1024.0 * 1024.0));
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// A bare pid is the inspect view: the contract's whole read surface for one
/// process, with the measured verdicts beside it.
#[test]
fn a_bare_pid_is_the_inspect_view() {
    let backend = MockProcess::default();
    let value = evaluate(&cmd(&["process", "42"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("name"), "smart");
    assert_eq!(value.field_str("command"), "target/debug/smart service");
    assert_eq!(value.field_str("ppid"), "1");
    assert_eq!(value.field("mem").as_f64(), Some(24.0 * 1024.0 * 1024.0));
    assert_eq!(value.field("peak_mem").as_f64(), Some(24.0 * 1024.0 * 1024.0));
    assert_eq!(value.field_bool("can_signal"), true);
    assert_eq!(value.field("uptime_nanos").as_f64(), Some(90_000_000_000.0));
    assert_eq!(value.field("io_read").as_f64(), Some(4096.0));
    assert_eq!(value.field("limits").field("memory_bytes").display(), "8589934592", "the hosts own limits, walked one level at a time");

    // A pid the host does not run is the engine's own notFound, with its reason.
    let missing = evaluate(&cmd(&["process", "9999"]), &backend, refused());
    assert!(missing.is_err());
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("/proc/9999"));
}

/// The `--name`/`--state`/`--user` filters reach the engine's own filter, so
/// the table is the host's own enumeration narrowed by the kernel's rule.
#[test]
fn filters_reach_the_engines_own_filter() {
    let backend = MockProcess::default();
    let by_name = evaluate(&cmd(&["process", "--name", "smart"]), &backend, refused()).unwrap();
    match &by_name {
        Value::List(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].field_str("name"), "smart");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    // uid 0 is the engine's own "every uid" reading, so a named uid narrows.
    let by_user = evaluate(&cmd(&["process", "--user", "1000"]), &backend, refused()).unwrap();
    match &by_user {
        Value::List(rows) => assert_eq!(rows.len(), 1, "this uid's one process"),
        other => panic!("expected a list, got {other:?}"),
    }
    let every = evaluate(&cmd(&["process", "--user", "0"]), &backend, refused()).unwrap();
    match &every {
        Value::List(rows) => assert_eq!(rows.len(), 3, "uid 0 is every uid"),
        other => panic!("expected a list, got {other:?}"),
    }
    let by_state = evaluate(&cmd(&["process", "--state", "active"]), &backend, refused()).unwrap();
    match &by_state {
        Value::List(rows) => assert_eq!(rows.len(), 1),
        other => panic!("expected a list, got {other:?}"),
    }
}

/// The threads, the environment and the memory map are the kernel's own rows,
/// and a foreign uid's answer is the host's measured EACCES.
#[test]
fn the_per_process_reads_answer_with_the_hosts_own_words() {
    let backend = MockProcess::default();
    let threads = evaluate(&cmd(&["process", "threads", "42"]), &backend, refused()).unwrap();
    assert_eq!(threads.field_str("total"), "4");
    assert_eq!(list_of(&threads, "threads")[0].field_str("tid"), "4200");

    // The environment is the host's own NUL pairs; `--names-only` holds the
    // values back rather than putting a secret on a shared screen.
    let env = evaluate(&cmd(&["process", "env", "42"]), &backend, refused()).unwrap();
    assert_eq!(list_of(&env, "variables")[0].field_str("key"), "LANG");
    assert_eq!(list_of(&env, "variables")[0].field_str("value"), "en_ZA.UTF-8");
    let names = evaluate(&cmd(&["process", "env", "42", "--names-only"]), &backend, refused()).unwrap();
    assert_eq!(list_of(&names, "variables")[0].field_str("value"), "⟨hidden⟩");
    assert!(names.field_bool("names_only"));

    // A foreign uid's environ is the host's own refusal.
    let refused_env = evaluate(&cmd(&["process", "env", "1"]), &backend, refused());
    assert!(refused_env.is_err());
    assert!(refused_env
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));

    // The memory map's rows carry the kernel's own permission word.
    let maps = evaluate(&cmd(&["process", "maps", "42"]), &backend, refused()).unwrap();
    assert_eq!(list_of(&maps, "regions")[0].field_str("perms"), "r--p");
    let foreign = evaluate(&cmd(&["process", "maps", "1"]), &backend, refused());
    assert!(foreign.is_err());
}

/// The measured verdicts: the host's own `kill(pid, 0)` for signal authority
/// and its ptrace answer.
#[test]
fn the_privilege_verdicts_are_measured_not_assumed() {
    let backend = MockProcess::default();
    let can = evaluate(&cmd(&["process", "can-signal", "42"]), &backend, refused()).unwrap();
    assert!(can.field_bool("allowed"));
    assert!(can.field_str("reason").contains("kill(pid, 0)"));

    let ptrace = evaluate(&cmd(&["process", "ptrace"]), &backend, refused()).unwrap();
    assert!(!ptrace.field_bool("allowed"));
    assert!(ptrace.field_str("reason").contains("ptrace_scope"));

    let backend = MockProcess {
        signal_privilege: false,
        ..MockProcess::default()
    };
    let cannot = evaluate(&cmd(&["process", "can-signal", "777"]), &backend, refused()).unwrap();
    assert!(!cannot.field_bool("allowed"));
}

/// The host-mutating verbs refuse without the accepted write, name what they
/// would have changed, and reach the engine's own path with it.
#[test]
fn the_mutating_verbs_are_gated() {
    let backend = MockProcess::default();
    let refused_signal = evaluate(&cmd(&["process", "signal", "42"]), &backend, refused());
    assert!(refused_signal.is_err());
    assert!(refused_signal
        .unwrap_err()
        .to_string()
        .contains("CHANGES host state"));
    let refused_attach = evaluate(&cmd(&["process", "attach", "42"]), &backend, refused());
    assert!(refused_attach.is_err());
    assert!(
        backend.writes.borrow().is_empty(),
        "nothing reached the engine: {:?}",
        backend.writes.borrow()
    );

    let value = evaluate(&cmd(&["process", "signal", "42", "--signal", "kill"]), &backend, allowed()).unwrap();
    assert_eq!(value.field_str("signal"), "kill");
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "signal(42, kill)"), "{writes:?}");
}

/// The wait surface is the contract's own bounded `waitpid`: a status when the
/// process has left, and the host's own reason when it has not.
#[test]
fn the_wait_surface_answers_with_the_hosts_own_words() {
    let backend = MockProcess::default();
    let value = evaluate(&cmd(&["process", "wait", "42"]), &backend, refused()).unwrap();
    assert!(value.field_bool("exited"));
    assert_eq!(value.field_str("status"), "exited 0");
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "wait(42)"), "{writes:?}");
}

/// The aggregates are the kernel's own enumeration, and the engine's own
/// window reading is labelled as one rather than restated in a unit the
/// engine's arithmetic does not carry.
#[test]
fn the_aggregates_are_measured() {
    let backend = MockProcess::default();
    let stats = evaluate(&cmd(&["process", "stats"]), &backend, refused()).unwrap();
    assert_eq!(stats.field_str("total_processes"), "3");
    assert_eq!(stats.field_str("active"), "1");
    assert_eq!(stats.field_str("blocked"), "2");
    assert_eq!(stats.field("total_cpu_usage").as_f64(), Some(1.5));
    assert_eq!(stats.field_str("total_memory_human"), "24.1 MiB");
    let rendered = render::render_action(&cmd(&["process", "stats"]), &stats, OutputFormat::Human);
    assert!(rendered.contains("the engine's own aggregate"), "{rendered}");
    assert!(!rendered.contains("cores busy"), "the shell does not restate a unit the engine does not carry: {rendered}");
}

/// The groups, the tree and the caps complete the contract's read surface.
#[test]
fn the_rest_of_the_read_surface_is_wired() {
    let backend = MockProcess::default();
    let groups = evaluate(&cmd(&["process", "groups"]), &backend, refused()).unwrap();
    assert_eq!(list_of(&groups, "groups")[0].field("members").as_f64(), Some(3.0));

    let tree = evaluate(&cmd(&["process", "tree", "1"]), &backend, refused()).unwrap();
    assert_eq!(tree.field_str("name"), "runit");
    assert_eq!(list_of(&tree, "children")[0].field_str("name"), "smart");
    assert_eq!(list_of(&tree, "children")[1].field_str("name"), "kworker/0:1");

    let caps = evaluate(&cmd(&["process", "caps"]), &backend, refused()).unwrap();
    assert_eq!(caps.display(), "[process.read, process.subscribe, process.signal]");

    let accounting = evaluate(&cmd(&["process", "accounting", "42"]), &backend, refused()).unwrap();
    assert_eq!(accounting.field("cpu_time_nanos").as_f64(), Some(350_000_000.0));
    assert_eq!(accounting.field("io_read").as_f64(), Some(4096.0));
}

/// The selector tree is the same values the command surface renders.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockProcess::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(list_of(&root, "processes")[0].field_str("name"), "runit");
    assert_eq!(root.field("stats").field_str("total_processes"), "3");
    assert_eq!(list_of(&root, "caps")[0].display(), "process.read");

    let all = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@process/processes").unwrap(),
    );
    match all {
        Value::List(rows) => assert_eq!(rows.len(), 3),
        other => panic!("expected a list, got {other:?}"),
    }
    let one = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@process/processes/1/mem").unwrap(),
    );
    assert_eq!(one.display(), "25165824");
}

/// The human table is pixel-perfect: its borders hold under the
/// terminal-faithful measure (W/F=2, ambiguous=1, ANSI stripped).
#[test]
fn the_table_is_pixel_perfect() {
    let backend = MockProcess::default();
    for argv in [
        &["process"][..],
        &["process", "--name", "smart"][..],
        &["process", "threads", "42"][..],
    ] {
        let value = evaluate(&cmd(argv), &backend, refused()).unwrap();
        let rendered = render::render_action(&cmd(argv), &value, OutputFormat::Human);
        assert_borders_hold(&rendered);
    }
    // A long name is marked as cut rather than wrapping the row.
    let backend = MockProcess {
        rows: vec![common::MockProcessRow {
            pid: 5,
            ppid: 1,
            name: "a-process-name-far-longer-than-the-column-and-then-some-more".to_string(),
            command: "a-process-name-far-longer-than-the-column-and-then-some-more".to_string(),
            state: smart_lib::domains::process::types::ExecutionState::Active,
            uid: 1000,
            threads: 1,
            memory_usage: 1024,
            cpu_nanos: 0,
        }],
        ..MockProcess::default()
    };
    let value = evaluate(&cmd(&["process"]), &backend, refused()).unwrap();
    let rendered = render::render_action(&cmd(&["process"]), &value, OutputFormat::Human);
    assert!(rendered.contains('…'), "the cut is marked: {rendered}");
    assert_borders_hold(&rendered);
    assert!(
        rendered
            .lines()
            .filter(|line| line.contains('│'))
            .all(|line| display_width(line) <= 100),
        "a table row never runs away: {rendered}"
    );
    // Both ends survive: the middle cut keeps the head AND the tail, and the
    // NAME column holds the whole width the cut was given.
    let cut = rendered
        .lines()
        .find(|line| line.contains("…"))
        .expect("a row carries the cut");
    let name_cell = cut.split('│').nth(2).expect("the NAME column");
    assert!(name_cell.starts_with(" a-process"), "the head survives: {cut}");
    assert!(
        name_cell.trim_end().ends_with("-more"),
        "the tail survives: {cut}"
    );
    // The full name is one door away, never lost: the JSON shape carries it whole.
    let json = render::render_action(&cmd(&["process"]), &value, OutputFormat::Json);
    assert!(
        json.contains("a-process-name-far-longer-than-the-column-and-then-some-more"),
        "the uncut value survives: {json}"
    );
}

/// The machine forms carry the same values the human table draws.
#[test]
fn json_and_raw_carry_the_same_values() {
    let backend = MockProcess::default();
    let value = evaluate(&cmd(&["process"]), &backend, refused()).unwrap();
    let json = render::render_action(&cmd(&["process"]), &value, OutputFormat::Json);
    assert!(json.contains("\"name\":\"runit\""), "{json}");
    assert!(json.contains("\"cpu_nanos\":1050000000"), "{json}");
    let raw = render::render_action(&cmd(&["process"]), &value, OutputFormat::Raw);
    assert!(raw.contains("runit"), "{raw}");
}

/// The CPU label: the kernel's own tick counters, scaled by the host's own
/// clock-tick rate, printed the way a process table prints time.
#[test]
fn the_cpu_label_is_the_measured_duration() {
    assert_eq!(render::cpu_label(0), "0.00");
    assert_eq!(render::cpu_label(1_050_000_000), "1.05");
    assert_eq!(render::cpu_label(65_400_000_000), "1:05.40");
    assert_eq!(render::cpu_label(3_725_000_000_000), "1:02:05");
}
