#![cfg(feature = "domain-clock")]
//! The clock command module over the scripted engine: the command surface's
//! data half, its human aesthetic (pixel-perfect under the terminal-faithful
//! measure), the host-write gate on the removal verbs, the measured sync words,
//! and the `@clock/*` selector tree. Nothing here touches the host (charter
//! law #8): every read is served from the script.

mod common;

use clap::Parser;
use common::{assert_borders_hold, MockClock};
use shell::domains::clock::{evaluate, render, selector_root, ClockCmd, WriteGuard};
use shell::selector;
use shell::widgets::display_width;
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> ClockCmd {
    ClockCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the measured state of this host's clocks: the kernel's
/// own readings, the resolutions, the sync verdict and the security state.
#[test]
fn the_default_view_is_the_measured_clock_state() {
    let backend = MockClock::default();
    let value = evaluate(&cmd(&["clock"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("source"), "tsc");
    assert_eq!(value.field_str("realtime"), "1700000000000000000");
    assert_eq!(value.field_str("monotonic"), "900000000000");
    assert_eq!(value.field_str("resolution"), "1");
    assert_eq!(value.field_str("boot_time"), "1699999000000000000");
    // 1000 seconds of uptime: btime plus the monotonic base.
    assert_eq!(value.field_str("uptime"), "1000000000000");
    assert!(!value.field("sync").field_bool("synced"), "sync is never assumed");
    assert_eq!(value.field("sync").field_str("source"), "chrony");
    assert!(value.field("security").field_bool("hardware_rng"));
    assert_eq!(list_of(&value, "domains").len(), 2);

    let rendered = render::render_action(&cmd(&["clock"]), &value, OutputFormat::Human);
    assert_borders_hold(&rendered);
    assert!(rendered.contains("CLOCK_REALTIME"), "{rendered}");
}

/// `clock sync` answers with the host's own measured words: a daemon that runs
/// is a measured fact, and the state this uid cannot read is named rather than
/// invented.
#[test]
fn the_sync_verdict_carries_the_hosts_own_words() {
    let backend = MockClock::default();
    let value = evaluate(&cmd(&["clock", "sync"]), &backend, refused()).unwrap();
    assert!(!value.field_bool("synced"));
    assert_eq!(value.field_str("source"), "chrony");
    assert_eq!(value.field_str("stratum"), "0", "never an invented stratum");
    assert!(value.field_str("evidence").contains("pid 711 runs chronyd"));
    assert!(value.field_str("evidence").contains("root-only"));
}

/// The resolution and domain reads are the host's own measured answers.
#[test]
fn the_resolution_and_domain_reads_are_measured() {
    let backend = MockClock::default();
    let resolution = evaluate(&cmd(&["clock", "resolution"]), &backend, refused()).unwrap();
    match &resolution {
        Value::List(domains) => {
            assert_eq!(domains.len(), 2);
            assert_eq!(domains[0].field_str("domain"), "realtime");
            assert_eq!(domains[0].field_str("resolution"), "1");
            assert!(domains[0].field_bool("available"));
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let uptime = evaluate(&cmd(&["clock", "uptime"]), &backend, refused()).unwrap();
    assert_eq!(uptime.field_str("uptime"), "1000000000000");
    assert_eq!(uptime.field_str("source"), "btime");

    let drift = evaluate(&cmd(&["clock", "drift", "realtime", "monotonic"]), &backend, refused()).unwrap();
    assert_eq!(drift.field_str("drift"), "12");
    assert_eq!(drift.field_str("accuracy"), "1");

    let quality = evaluate(&cmd(&["clock", "quality", "tsc"]), &backend, refused()).unwrap();
    assert_eq!(quality.field_str("samples"), "32");
    assert_eq!(quality.field_str("reliability"), "0.99");
}

/// Arming a timer is a real `timerfd` this process owns; checking it answers
/// with the kernel's own verdict, and a timer this process does not hold is
/// the host's own notFound with the reason beside it.
#[test]
fn a_timer_is_a_real_timerfd_this_process_owns() {
    let backend = MockClock::default();
    let created = evaluate(
        &cmd(&["clock", "timer", "create", "--after-ms", "50", "--label", "live"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(created.field_str("id"), "1");
    assert_eq!(created.field_str("label"), "live");
    assert_eq!(created.field("spec").field("anchor").as_f64(), Some(50_000_000.0));
    assert!(!created.field("spec").field("periodic").as_bool().unwrap());
    assert!(created.field_str("reason").contains("timerfd"));

    // The periodic case states its interval.
    let periodic = evaluate(
        &cmd(&[
            "clock", "timer", "create", "--after-ms", "10", "--interval-ms", "20", "--label", "tick",
        ]),
        &backend,
        refused(),
    )
    .unwrap();
    assert!(periodic.field("spec").field("periodic").as_bool().unwrap());
    assert_eq!(periodic.field("spec").field("interval").as_f64(), Some(20_000_000.0));

    // A timer this process armed is checkable — the kernel's own answer.
    let checked = evaluate(&cmd(&["clock", "timer", "check", "1"]), &backend, refused()).unwrap();
    assert!(checked.field_bool("fired"));

    // A timer this process never armed is the host's own notFound with the
    // reason, never a fabricated fire.
    let missing = evaluate(&cmd(&["clock", "timer", "check", "99"]), &backend, refused()).unwrap();
    assert!(!missing.field_bool("fired"));
    assert!(missing.field_str("reason").contains("held by the process that armed it"));
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w.starts_with("timer(1, live)")), "{writes:?}");
}

/// The timer event stream is the contract's own subscription, and the count is
/// the kernel's own expiration count — a periodic timer that fired three times
/// reports three.
#[test]
fn the_timer_stream_reports_the_kernels_own_count() {
    let backend = MockClock::default();
    let value = evaluate(&cmd(&["clock", "watch", "--count", "2", "--interval", "1"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total"), "2");
    assert_eq!(list_of(&value, "events")[0].field_str("count"), "3");
    assert_eq!(list_of(&value, "events")[0].field_str("label"), "scripted");
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "subscribe-timers"), "{writes:?}");
    assert!(writes.iter().any(|w| w == "cancel-timers"), "{writes:?}");
}

/// The alarm surface is this process's own store, acquired through `alarms @19`
/// — and REMOVAL is the gated half.
#[test]
fn the_alarm_surface_is_this_processs_own_store() {
    let backend = MockClock::default();
    let created = evaluate(
        &cmd(&["clock", "alarm", "create", "--label", "wake", "--after-ms", "60000"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(created.field_str("id"), "1");
    assert_eq!(created.field_str("label"), "wake");
    assert_eq!(created.field_str("fire_time"), "1700000060000000000");

    let snoozed = evaluate(&cmd(&["clock", "alarm", "snooze", "1", "5000"]), &backend, refused()).unwrap();
    assert_eq!(snoozed.field_str("verb"), "alarm-snooze");

    let listed = evaluate(&cmd(&["clock", "alarm", "list"]), &backend, refused()).unwrap();
    assert_eq!(listed.field_str("total"), "1");
    assert_eq!(list_of(&listed, "alarms")[0].field_str("recurrence"), "once");

    let next = evaluate(&cmd(&["clock", "alarm", "next"]), &backend, refused()).unwrap();
    assert_eq!(next.field_str("label"), "wake");

    // The removal half is gated: without the accepted write it refuses and
    // names the verb, and the engine is never asked to remove anything.
    let refused_delete = evaluate(&cmd(&["clock", "alarm", "delete", "1"]), &backend, refused());
    assert!(refused_delete.is_err());
    assert!(refused_delete
        .unwrap_err()
        .to_string()
        .contains("CHANGES state"));
    let still = evaluate(&cmd(&["clock", "alarm", "list"]), &backend, refused()).unwrap();
    assert_eq!(still.field_str("total"), "1", "the alarm is still there");

    // The timer's removal half is gated the same way.
    let refused_cancel = evaluate(&cmd(&["clock", "timer", "cancel", "1"]), &backend, refused());
    assert!(refused_cancel.is_err());
    assert!(backend
        .writes
        .borrow()
        .iter()
        .all(|w| !w.starts_with("cancel-timer(") && !w.starts_with("cancel(")));
}

/// The virtual clock is software, so it is a REAL surface: create and advance
/// both answer with the engine's own values.
#[test]
fn the_virtual_clock_is_real_because_it_is_software() {
    let backend = MockClock::default();
    let created = evaluate(
        &cmd(&["clock", "virtual", "create", "--name", "test-clock", "--rate", "2.0"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert_eq!(created.field_str("name"), "test-clock");
    assert_eq!(created.field("rate").as_f64(), Some(2.0));
    assert!(created.field_str("reason").contains("REAL"));

    let advanced = evaluate(&cmd(&["clock", "virtual", "advance", "1", "1000"]), &backend, refused()).unwrap();
    assert_eq!(advanced.field_str("now"), "1700000001000000000");
}

/// The sleep surface is the caller's own thread, and a cancellable sleep's
/// cancel half is gated.
#[test]
fn the_sleep_surface_runs_on_the_callers_own_thread() {
    let backend = MockClock::default();
    let value = evaluate(&cmd(&["clock", "sleep", "20"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("verb"), "sleep");
    assert!(backend.writes.borrow().iter().any(|w| w == "sleep"));

    let cancellable = evaluate(
        &cmd(&["clock", "sleep-cancellable", "20"]),
        &backend,
        refused(),
    )
    .unwrap();
    assert!(!cancellable.field_bool("cancelled"));
    assert_eq!(cancellable.field_str("id"), "5");

    let refused_cancel = evaluate(
        &cmd(&["clock", "sleep-cancellable", "20", "--cancel-after", "5"]),
        &backend,
        refused(),
    );
    assert!(refused_cancel.is_err());
    assert!(refused_cancel
        .unwrap_err()
        .to_string()
        .contains("CHANGES state"));

    let cancelled = evaluate(
        &cmd(&["clock", "sleep-cancellable", "20", "--cancel-after", "5"]),
        &backend,
        allowed(),
    )
    .unwrap();
    assert!(cancelled.field_bool("cancelled"));
    assert!(backend.writes.borrow().iter().any(|w| w == "cancel-sleep"));
}

/// The physical limits are named: timer persistence has no host store here.
#[test]
fn the_physical_limits_are_named_not_faked() {
    let backend = MockClock::default();
    let timers = evaluate(&cmd(&["clock", "timer", "list"]), &backend, refused()).unwrap();
    assert_eq!(timers.field_str("total"), "0");
    assert!(timers.field_str("reason").contains("this host does not carry"));
}

/// The selector tree is the same values the command surface renders.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockClock::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(root.field("info").field_str("source"), "tsc");
    assert_eq!(root.field("info").field("sync").field_str("source"), "chrony");
    assert_eq!(list_of(&root, "caps")[0].display(), "clock.realtime");

    let uptime = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@clock/info/uptime").unwrap(),
    );
    assert_eq!(uptime.display(), "1000000000000");
}

/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

/// The human renderings are pixel-perfect.
#[test]
fn the_renderings_are_pixel_perfect() {
    let backend = MockClock::default();
    for argv in [
        &["clock"][..],
        &["clock", "resolution"][..],
        &["clock", "domains"][..],
        &["clock", "alarm", "list"][..],
    ] {
        let value = evaluate(&cmd(argv), &backend, refused()).unwrap();
        let rendered = render::render_action(&cmd(argv), &value, OutputFormat::Human);
        assert!(
            rendered
                .lines()
                .filter(|line| line.contains('│'))
                .all(|line| display_width(line) <= 140),
            "a row never runs away: {rendered}"
        );
    }
    assert_borders_hold(&render::render_action(
        &cmd(&["clock", "resolution"]),
        &evaluate(&cmd(&["clock", "resolution"]), &backend, refused()).unwrap(),
        OutputFormat::Human,
    ));
}

/// The duration label is the shell's own words for a measured duration. A
/// sub-second duration keeps its own unit: a one-shot timer's 10 ms anchor
/// labelled "0s" would be a rounding presented as a measurement.
#[test]
fn the_duration_label_is_the_shells_own_words() {
    assert_eq!(render::duration_label(0), "0 ns");
    assert_eq!(render::duration_label(10_000), "10000 ns");
    assert_eq!(render::duration_label(10_000_000), "10 ms");
    assert_eq!(render::duration_label(1_500_000_000), "1s");
    assert_eq!(render::duration_label(90_000_000_000), "1m 30s");
    assert_eq!(render::duration_label(3_725_000_000_000), "1h 2m 5s");
    assert_eq!(render::duration_label(183_000_000_000_000), "2d 2h 50m");
}
