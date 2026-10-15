//! The clock domain's rendering, in the shell's aesthetic: box-drawing tables,
//! `◉/○/◐`, measured numbers. Every renderer reads the ENGINE'S OWNED TYPES
//! (`ClockInfo`, `ClockSyncInfo`, `ClockDomainInfo`, `TimerSpec`,
//! `TimerHandle`, `TimerEvent`, `Alarm`, `VirtualClockHandle`) — there is no
//! capnp reader here, so a resolution and a sync verdict are printed exactly as
//! the kernel and the host answered them.
//!
//! Every width is a display width under the terminal-faithful measure, and
//! every cell is CONTENT plus a STYLE (`Cell`), never a pre-styled string.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{truncate_to, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::core::values::{Duration, Timestamp};
use smart_lib::domains::clock::types::{Alarm, ClockDomainInfo, ClockInfo, TimerHandle, TimerSpec};

use super::{
    alarm_id, alarm_record, neutral_holiday, recurrence_of, timer_spec,
    virtual_config,
};
use super::{AlarmAction as AA, ClockAction as A, ClockCmd, TimerAction as TA, VirtualAction as VA};

/// The measured state of this host's clocks (the default view).
pub fn info_value(backend: &super::Backend) -> SmartResult<Value> {
    let info = backend.info().map_err(SmartError::from)?;
    Ok(clock_info_value(&info))
}

/// One `ClockInfo` as a record.
pub fn clock_info_value(info: &ClockInfo) -> Value {
    Value::Object(vec![
        (
            "monotonic".into(),
            Value::String(info.monotonic.as_nanos().to_string()),
        ),
        (
            "realtime".into(),
            Value::String(info.realtime.as_nanos().to_string()),
        ),
        (
            "resolution".into(),
            Value::String(info.resolution.as_nanos().to_string()),
        ),
        ("source".into(), Value::String(info.source_name.clone())),
        (
            "boot_time".into(),
            Value::String(info.boot_time.as_nanos().to_string()),
        ),
        (
            "uptime".into(),
            Value::String(
                info.realtime
                    .as_nanos()
                    .saturating_sub(info.boot_time.as_nanos())
                    .to_string(),
            ),
        ),
        (
            "sync".into(),
            Value::Object(vec![
                ("synced".into(), Value::Bool(info.sync.synced)),
                ("source".into(), Value::String(info.sync.source.clone())),
                ("stratum".into(), Value::Number(f64::from(info.sync.stratum))),
            ]),
        ),
        (
            "security".into(),
            Value::Object(vec![
                ("tampered".into(), Value::Bool(info.security.tampered)),
                ("secure_source".into(), Value::Bool(info.security.secure_source)),
                ("hardware_rng".into(), Value::Bool(info.security.hardware_rng)),
            ]),
        ),
        (
            "domains".into(),
            Value::List(info.domains.iter().map(domain_value).collect()),
        ),
    ])
}

/// One `ClockDomainInfo` as a record.
pub fn domain_value(domain: &ClockDomainInfo) -> Value {
    Value::Object(vec![
        ("domain".into(), Value::String(domain.domain.as_token().to_string())),
        ("name".into(), Value::String(domain.name.clone())),
        (
            "resolution".into(),
            Value::String(domain.resolution.as_nanos().to_string()),
        ),
        ("available".into(), Value::Bool(domain.available)),
    ])
}

/// The uptime: the kernel's own `btime` plus the monotonic base.
pub fn uptime_value(info: &ClockInfo) -> SmartResult<Value> {
    let uptime = info
        .realtime
        .as_nanos()
        .saturating_sub(info.boot_time.as_nanos());
    Ok(Value::Object(vec![
        ("uptime".into(), Value::String(uptime.to_string())),
        ("boot_time".into(), Value::String(info.boot_time.as_nanos().to_string())),
        ("source".into(), Value::String("btime".to_string())),
    ]))
}

/// The timer surface.
pub fn timer_value(
    backend: &super::Backend,
    action: &TA,
    guard: super::WriteGuard,
) -> SmartResult<Value> {
    match action {
        TA::Create {
            after_ms,
            interval_ms,
            label,
            clock,
        } => {
            let spec = timer_spec(*after_ms, *interval_ms, label, clock)?;
            let handle = backend.create_timer(&spec).map_err(SmartError::from)?;
            let spec_record = spec_value(&spec, &handle);
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("timer-create".into())),
                ("id".into(), Value::Number(handle.id.get() as f64)),
                ("label".into(), Value::String(handle.label.clone())),
                ("spec".into(), spec_record),
                (
                    "reason".into(),
                    Value::String(
                        "a real timerfd this process owns: timerfd_create + timerfd_settime, so a \
                         periodic timer that fired three times reports three"
                            .to_string(),
                    ),
                ),
            ]))
        }
        TA::Check { id } => {
            let handle = TimerHandle {
                id: smart_lib::core::ids::TimerId::new(*id),
                label: String::new(),
            };
            match backend.check_timer(&handle) {
                Ok(fired) => Ok(Value::Object(vec![
                    ("id".into(), Value::Number(*id as f64)),
                    ("fired".into(), Value::Bool(fired)),
                ])),
                // A timerfd belongs to the process that armed it, so a fresh
                // `smart` invocation holds none — the host's own notFound with
                // the reason beside it, never a fabricated fire.
                Err(e) => Ok(Value::Object(vec![
                    ("id".into(), Value::Number(*id as f64)),
                    ("fired".into(), Value::Bool(false)),
                    ("error".into(), Value::String(e.to_string())),
                    (
                        "reason".into(),
                        Value::String(
                            "a timerfd is held by the process that armed it, so a one-shot `smart` \
                             invocation holds no timer from an earlier one — arm one and check it in \
                             the same session"
                                .to_string(),
                        ),
                    ),
                ])),
            }
        }
        TA::List => {
            // The engine offers no list-timers verb, and this host does not
            // carry a timer-persistence store, so the rows a session armed are
            // visible to THAT session only. A caller with no session ledger
            // (the library path) is told the limit; the session surface
            // (`evaluate_session`) is the one that answers with rows.
            Ok(Value::Object(vec![
                ("timers".into(), Value::List(Vec::new())),
                ("total".into(), Value::Number(0.0)),
                (
                    "reason".into(),
                    Value::String(
                        "the engine exposes no list-timers verb and this host does not carry a \
                         timer-persistence store (its own persistTimer/listPersistedTimers are \
                         DEGRADED for exactly that reason), so the timers a process armed belong \
                         to that process's session: the REPL's `clock timer list`, or \
                         `smart clock timer list`, answers with the session's own rows plus the \
                         ones earlier sessions left in the shared ledger. This call carries no \
                         session ledger, so it names the limit rather than inventing rows."
                            .to_string(),
                    ),
                ),
            ]))
        }
        TA::Cancel { id } => {
            super::guard_host_write(guard, &format!("clock timer cancel {id}"))?;
            let handle = TimerHandle {
                id: smart_lib::core::ids::TimerId::new(*id),
                label: String::new(),
            };
            backend.cancel_timer(&handle).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("timer-cancel".into())),
                ("id".into(), Value::Number(*id as f64)),
            ]))
        }
    }
}

/// The session's timer ledger as a value: the rows this session armed (live,
/// checkable here), then the rows earlier processes left — each with the
/// measured ownership beside it. A timerfd belongs to its arming process, so a
/// firing never survives a process; the ledger names armings and owners, and
/// `clock timer check <id>` answers the kernel's own count inside the owning
/// session. Never a fabricated fire.
pub fn timer_list_value(session: &crate::session::ClockSession) -> Value {
    let rows = session.rows();
    let live = rows
        .iter()
        .filter(|r| r.ownership == crate::session::TimerOwnership::Live)
        .count();
    let owned = rows
        .iter()
        .filter(|r| r.ownership == crate::session::TimerOwnership::Owned)
        .count();
    let gone = rows
        .iter()
        .filter(|r| r.ownership == crate::session::TimerOwnership::Gone)
        .count();
    let timers = Value::List(
        rows.iter()
            .map(|row| {
                Value::Object(vec![
                    ("id".into(), Value::Number(row.record.id as f64)),
                    ("pid".into(), Value::Number(u64::from(row.record.pid) as f64)),
                    ("label".into(), Value::String(row.record.label.clone())),
                    (
                        "after_ms".into(),
                        Value::Number(row.record.after_ms as f64),
                    ),
                    (
                        "interval_ms".into(),
                        Value::Number(row.record.interval_ms as f64),
                    ),
                    ("clock".into(), Value::String(row.record.clock.clone())),
                    (
                        "armed_at".into(),
                        Value::String(row.record.armed_at_nanos.to_string()),
                    ),
                    (
                        "ownership".into(),
                        Value::String(
                            match row.ownership {
                                crate::session::TimerOwnership::Live => "live",
                                crate::session::TimerOwnership::Owned => "owned",
                                crate::session::TimerOwnership::Gone => "gone",
                            }
                            .to_string(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    Value::Object(vec![
        ("timers".into(), timers),
        ("total".into(), Value::Number(rows.len() as f64)),
        ("live".into(), Value::Number(live as f64)),
        ("owned".into(), Value::Number(owned as f64)),
        ("gone".into(), Value::Number(gone as f64)),
        (
            "reason".into(),
            Value::String(
                "a timerfd is held by the process that armed it: the rows above are the session's \
                 own ledger (live — checkable here with `clock timer check`), the rows earlier \
                 sessions left in the shared state file (owned — a live sibling holds the fd; \
                 gone — the owner exited and the fd died with it). The engine's own \
                 `persistTimer`/`listPersistedTimers` are DEGRADED precisely because this host \
                 does not carry a timer-persistence store, so the ledger records the FACT of a \
                 row and never claims a firing: a firing cannot outlive the process that owns \
                 the timer."
                    .to_string(),
            ),
        ),
    ])
}

/// The ledger's human table: ownership is the measured column.
fn render_timer_list(value: &Value) -> String {
    let rows = match value.field("timers") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    let counts = format!(
        "({} live · {} owned · {} gone)",
        value.field("live").as_f64().unwrap_or(0.0) as u64,
        value.field("owned").as_f64().unwrap_or(0.0) as u64,
        value.field("gone").as_f64().unwrap_or(0.0) as u64
    );
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "⏱".blue().bold(),
        "Timers".blue().bold(),
        counts.dimmed(),
        "— the session's ledger; a firing cannot outlive the process that owns the timer".dimmed()
    ));
    if rows.is_empty() {
        out.push_str(&format!(
            "  └─ {} this session armed no timer and the shared ledger holds none (an honest empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "ID".into(),
        "LABEL".into(),
        "EVERY".into(),
        "CLOCK".into(),
        "OWNER".into(),
        "STATE".into(),
    ]);
    for row in &rows {
        let ownership = row.field_str("ownership");
        let pid = row.field("pid").as_f64().unwrap_or(0.0) as u32;
        let (glyph, words) = match ownership.as_str() {
            "live" => (
                Cell::styled("◉", Sgr::Green),
                "live — checkable in this session".to_string(),
            ),
            "owned" => (
                Cell::styled("◐", Sgr::Yellow),
                format!("owned by pid {pid} (alive)"),
            ),
            _ => (
                Cell::styled("○", Sgr::Dimmed),
                format!("gone — pid {pid} exited, the fd died with it"),
            ),
        };
        let interval = row.field("interval_ms").as_f64().unwrap_or(0.0) as u64;
        let after = row.field("after_ms").as_f64().unwrap_or(0.0) as u64;
        table = table.row(vec![
            Cell::plain(row.field_str("id")),
            row.field_str("label").into(),
            Cell::plain(match (after, interval) {
                (0, 0) => "once".to_string(),
                (0, every) => format!("every {}", duration_label(every * 1_000_000)),
                (delay, 0) => format!("once, in {}", duration_label(delay * 1_000_000)),
                (delay, every) => format!(
                    "every {}, first in {}",
                    duration_label(every * 1_000_000),
                    duration_label(delay * 1_000_000)
                ),
            }),
            row.field_str("clock").into(),
            Cell::plain(pid.to_string()),
            glyph.with_text(&format!(" {words}")),
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!("\n  {} {}\n", "●".blue().bold(), value.field_str("reason")));
    out.trim_end().to_string()
}

/// One `TimerSpec` as a record.
fn spec_value(spec: &TimerSpec, handle: &TimerHandle) -> Value {
    Value::Object(vec![
        ("label".into(), Value::String(spec.label.clone())),
        (
            "interval".into(),
            Value::String(spec.interval.as_nanos().to_string()),
        ),
        ("clock".into(), Value::String(spec.clock_domain.as_token().to_string())),
        (
            "anchor".into(),
            match spec.anchor_type {
                super::TimerAnchor::Relative(after) => {
                    Value::String(after.as_nanos().to_string())
                }
                super::TimerAnchor::Absolute(at) => Value::String(at.as_nanos().to_string()),
                super::TimerAnchor::Zero => Value::String("zero".to_string()),
                super::TimerAnchor::Custom => Value::String("custom".to_string()),
            },
        ),
        ("periodic".into(), Value::Bool(spec.is_periodic())),
        ("handle".into(), Value::Number(handle.id.get() as f64)),
    ])
}

/// The virtual-clock surface: software, so a REAL surface.
pub fn virtual_value(backend: &super::Backend, action: &VA) -> SmartResult<Value> {
    match action {
        VA::Create { name, rate } => {
            let handle = backend
                .create_virtual_clock(&virtual_config(name, *rate))
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("virtual-create".into())),
                ("id".into(), Value::Number(handle.id as f64)),
                ("name".into(), Value::String(handle.name.clone())),
                ("rate".into(), Value::Number(f64::from(*rate))),
                (
                    "reason".into(),
                    Value::String(
                        "a virtual clock IS software, so this is a REAL surface rather than a \
                         degraded host read"
                            .to_string(),
                    ),
                ),
            ]))
        }
        VA::Advance { id, millis } => {
            let handle = smart_lib::domains::clock::types::VirtualClockHandle {
                id: *id,
                name: String::new(),
            };
            let now = backend
                .advance_virtual_clock(
                    &handle,
                    Duration::from_nanos(millis.saturating_mul(1_000_000)),
                )
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("virtual-advance".into())),
                ("id".into(), Value::Number(*id as f64)),
                ("now".into(), Value::String(now.as_nanos().to_string())),
            ]))
        }
    }
}

/// The alarm surface, acquired through `alarms @19`.
pub fn alarm_value(
    backend: &super::Backend,
    action: &AA,
    guard: super::WriteGuard,
) -> SmartResult<Value> {
    let alarms = backend.alarms().map_err(SmartError::from)?;
    match action {
        AA::List => {
            let page = alarms.list_alarms(super::all()).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                (
                    "alarms".into(),
                    Value::List(page.items.iter().map(alarm_record).collect()),
                ),
                ("total".into(), Value::Number(page.total as f64)),
                (
                    "reason".into(),
                    Value::String(
                        "this process's own alarm store, acquired through alarms @19 — holding \
                         the reference IS the authority"
                            .to_string(),
                    ),
                ),
            ]))
        }
        AA::Next => match alarms.get_next_alarm() {
            Ok(alarm) => Ok(alarm_record(&alarm)),
            Err(e) => Err(SmartError::from(e)),
        },
        AA::Create {
            label,
            after_ms,
            recurrence,
            snooze_ms,
        } => {
            // The fire time is resolved against a real reading, so a relative
            // alarm is measured against the host's clock rather than a guess.
            let now = backend.realtime_now().map_err(SmartError::from)?;
            let fire = Timestamp::from_nanos(
                now.as_nanos() + after_ms.saturating_mul(1_000_000),
            );
            let alarm = Alarm {
                id: alarm_id(0),
                label: label.clone(),
                fire_time: fire,
                recurrence: recurrence_of(recurrence),
                snooze_duration: Duration::from_nanos(snooze_ms.saturating_mul(1_000_000)),
                enabled: true,
                holiday_behavior: neutral_holiday(),
            };
            let id = alarms.create_alarm(&alarm).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("alarm-create".into())),
                ("id".into(), Value::Number(id.get() as f64)),
                ("label".into(), Value::String(label.clone())),
                ("fire_time".into(), Value::String(fire.as_nanos().to_string())),
                (
                    "recurrence".into(),
                    Value::String(recurrence.clone()),
                ),
            ]))
        }
        AA::Snooze { id, millis } => {
            let alarm = alarms.get_alarm(alarm_id(*id)).map_err(SmartError::from)?;
            alarms
                .snooze_alarm(
                    alarm_id(*id),
                    Duration::from_nanos(millis.saturating_mul(1_000_000)),
                )
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("alarm-snooze".into())),
                ("id".into(), Value::Number(*id as f64)),
                ("label".into(), Value::String(alarm.label.clone())),
                ("deferred_ms".into(), Value::Number(*millis as f64)),
            ]))
        }
        AA::Delete { id } => {
            super::guard_host_write(guard, &format!("clock alarm delete {id}"))?;
            alarms.delete_alarm(alarm_id(*id)).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("alarm-delete".into())),
                ("id".into(), Value::Number(*id as f64)),
            ]))
        }
    }
}

/// The timer event stream: the contract's own subscription, bounded by the
/// caller.
pub fn watch_value(
    backend: &super::Backend,
    count: u32,
    interval_ms: u64,
) -> SmartResult<Value> {
    let subscription = backend
        .subscribe_timers(&smart_lib::domains::clock::types::TimerFilter { labels: Vec::new() })
        .map_err(SmartError::from)?;
    let mut events = Vec::new();
    for _ in 0..count.max(1) {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        let batch = backend
            .pull_timers(subscription)
            .map_err(SmartError::from)?;
        for event in batch {
            events.push(Value::Object(vec![
                ("timer".into(), Value::Number(event.timer.id.get() as f64)),
                ("label".into(), Value::String(event.timer.label.clone())),
                (
                    "fired_at".into(),
                    Value::String(event.fired_at.as_nanos().to_string()),
                ),
                // The kernel's own expiration count, never a fabricated 1.
                ("count".into(), Value::Number(event.count as f64)),
            ]));
        }
    }
    let _ = backend.cancel_timers(subscription);
    Ok(Value::Object(vec![
        ("events".into(), Value::List(events.clone())),
        ("total".into(), Value::Number(events.len() as f64)),
        (
            "reason".into(),
            Value::String(
                "the contract's TimerSubscription: a minted token, a bounded pull, a cancel — and \
                 the count is the kernel's own timerfd expiration count"
                    .to_string(),
            ),
        ),
    ]))
}

/// This backend's honest capability tokens (law #6).
pub fn caps_value(backend: &super::Backend) -> Value {
    Value::List(
        backend
            .clock_caps()
            .into_iter()
            .map(|token| Value::String(token.to_string()))
            .collect(),
    )
}

// ═══ the aesthetic ════════════════════════════════════════════════════════

/// Render one command's value in the requested format.
pub fn render_action(cmd: &ClockCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &ClockCmd, value: &Value) -> String {
    match &cmd.action {
        None | Some(A::Info) => render_info(value),
        Some(A::Now) => render_now(value),
        Some(A::Monotonic) => render_monotonic(value),
        Some(A::Uptime) => render_uptime(value),
        Some(A::Resolution) => render_resolution(value),
        Some(A::Sync) => render_sync(value),
        Some(A::Security) => render_security(value),
        Some(A::Domains) => render_domains(value),
        Some(A::Quality { source }) => render_quality(value, source),
        Some(A::Drift { a, b }) => render_drift(value, a, b),
        Some(A::Sleep { millis }) => render_sleep(value, *millis),
        Some(A::Timer { action }) => render_timer(value, action),
        Some(A::SleepCancellable { millis, .. }) => render_sleep(value, *millis),
        Some(A::Virtual { action }) => render_virtual(value, action),
        Some(A::Alarm { action }) => render_alarm(value, action),
        Some(A::Watch { .. }) => render_watch(value),
        Some(A::Caps) => render_caps(value),
    }
}

/// The measured state of this host's clocks.
fn render_info(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — the kernel's own clocks\n",
        "🕰".blue().bold(),
        "Clock".blue().bold(),
        format!("({})", value.field_str("source")).dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Realtime    {} (CLOCK_REALTIME)\n",
        value.field_str("realtime")
    ));
    out.push_str(&format!(
        "  ├─ Monotonic   {} (CLOCK_MONOTONIC — immune to NTP and DST)\n",
        value.field_str("monotonic")
    ));
    out.push_str(&format!(
        "  ├─ Uptime      {} since boot (btime {})\n",
        duration_label(value.field("uptime").as_f64().unwrap_or(0.0) as u64),
        value.field_str("boot_time")
    ));
    out.push_str(&format!(
        "  ├─ Resolution  {} ns (clock_getres — never a fabricated precision)\n",
        value.field_str("resolution")
    ));
    let sync = value.field("sync");
    out.push_str(&format!(
        "  ├─ Sync        {} {} {}\n",
        if sync.field_bool("synced") {
            "◉".green().to_string()
        } else {
            "○".dimmed().to_string()
        },
        sync.field_str("source"),
        if sync.field_bool("synced") {
            String::new()
        } else {
            "(measured — see `clock sync`)".to_string()
        }
    ));
    let security = value.field("security");
    out.push_str(&format!(
        "  ├─ Security    tampered {} · secure source {} · hardware rng {}\n",
        yes_no(security.field_bool("tampered")),
        yes_no(security.field_bool("secure_source")),
        yes_no(security.field_bool("hardware_rng"))
    ));
    let domains = match value.field("domains") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    out.push_str(&format!(
        "  └─ Domains     {} served\n",
        domains.len()
    ));
    out.trim_end().to_string()
}

fn render_now(value: &Value) -> String {
    format!(
        "  {} {} {} {}\n",
        "🕐".blue().bold(),
        "Now".blue().bold(),
        value.field_str("realtime").bold(),
        "— the kernel's own CLOCK_REALTIME, no `date` fork".dimmed()
    )
    .trim_end()
    .to_string()
}

fn render_monotonic(value: &Value) -> String {
    format!(
        "  {} {} {} {}\n",
        "⏱".blue().bold(),
        "Monotonic".blue().bold(),
        value.field_str("monotonic").bold(),
        "— CLOCK_MONOTONIC, immune to NTP and DST".dimmed()
    )
    .trim_end()
    .to_string()
}

fn render_uptime(value: &Value) -> String {
    format!(
        "  {} {} {} {} — btime {} from /proc/stat\n",
        "⏳".blue().bold(),
        "Uptime".blue().bold(),
        duration_label(value.field("uptime").as_f64().unwrap_or(0.0) as u64).bold(),
        "".dimmed(),
        value.field_str("boot_time")
    )
    .trim_end()
    .to_string()
}

/// The resolutions the host publishes, one row per clock domain.
fn render_resolution(value: &Value) -> String {
    let Value::List(domains) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — clock_getres, measured\n",
        "📐".blue().bold(),
        "Resolution".blue().bold(),
        format!("({})", domains.len()).dimmed()
    ));
    let mut table = Table::new(vec![
        "DOMAIN".into(),
        "NAME".into(),
        "RESOLUTION".into(),
        "AVAILABLE".into(),
    ]);
    for domain in domains {
        table = table.row(vec![
            Cell::plain(domain.field_str("domain")),
            Cell::plain(domain.field_str("name")),
            Cell::plain(format!("{} ns", domain.field_str("resolution"))),
            yes_no(domain.field_bool("available")),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

/// The measured sync state, with the host's own words.
fn render_sync(value: &Value) -> String {
    let synced = value.field_bool("synced");
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "📡".blue().bold(),
        "Clock sync".blue().bold(),
        if synced {
            "◉ synchronized".green().to_string()
        } else {
            "○ not synchronized".dimmed().to_string()
        },
        "— measured, never assumed".dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Source      {} · stratum {}\n",
        value.field_str("source"),
        value.field_str("stratum")
    ));
    out.push_str(&format!(
        "  ├─ Evidence    {}\n",
        if value.field_str("evidence").is_empty() {
            "— nothing was measurable on this host".to_string()
        } else {
            value.field_str("evidence")
        }
    ));
    out.push_str(&format!(
        "  └─ Reading     last sync {} · offset {} ns\n",
        value.field_str("last_sync"),
        value.field_str("offset")
    ));
    out.trim_end().to_string()
}

fn render_security(value: &Value) -> String {
    format!(
        "  {} {} tampered {} · secure source {} · hardware rng {}\n",
        "🛡".blue().bold(),
        "Clock security".blue().bold(),
        yes_no(value.field_bool("tampered")),
        yes_no(value.field_bool("secure_source")),
        yes_no(value.field_bool("hardware_rng"))
    )
    .trim_end()
    .to_string()
}

fn render_domains(value: &Value) -> String {
    let Value::List(domains) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — what this host can serve\n",
        "🗄".blue().bold(),
        "Clock domains".blue().bold(),
        format!("({})", domains.len()).dimmed()
    ));
    let mut table = Table::new(vec![
        "DOMAIN".into(),
        "NAME".into(),
        "RESOLUTION".into(),
        "AVAILABLE".into(),
    ]);
    for domain in domains {
        table = table.row(vec![
            Cell::plain(domain.field_str("domain")),
            Cell::plain(domain.field_str("name")),
            Cell::plain(format!("{} ns", domain.field_str("resolution"))),
            yes_no(domain.field_bool("available")),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_quality(value: &Value, source: &str) -> String {
    format!(
        "  {} {} {} — accuracy {} ns · stability {} ns · reliability {} over {} sample(s)\n",
        "📊".blue().bold(),
        "Clock quality".blue().bold(),
        source.bold(),
        value.field_str("accuracy"),
        value.field_str("stability"),
        value.field_str("reliability"),
        value.field_str("samples")
    )
    .trim_end()
    .to_string()
}

fn render_drift(value: &Value, a: &str, b: &str) -> String {
    format!(
        "  {} {} {} → {} — drift {} ns · accuracy {} ns\n",
        "⇄".blue().bold(),
        "Clock drift".blue().bold(),
        a,
        b,
        value.field_str("drift"),
        value.field_str("accuracy")
    )
    .trim_end()
    .to_string()
}

fn render_sleep(value: &Value, millis: u64) -> String {
    format!(
        "  {} {} {} ms {} — nanosleep on the caller's own thread\n",
        "😴".blue().bold(),
        "Sleep".blue().bold(),
        millis,
        value.field_str("id")
    )
    .trim_end()
    .to_string()
}

/// The timer surface's own rendering.
fn render_timer(value: &Value, action: &TA) -> String {
    match action {
        TA::Create { .. } => {
            let spec = value.field("spec");
            let mut out = String::new();
            out.push_str(&format!(
                "  {} {} {} {} — a real timerfd this process owns\n",
                "⏲".blue().bold(),
                "Timer".blue().bold(),
                format!("#{}", value.field_str("id")).bold(),
                format!("({})", spec.field_str("label")).dimmed()
            ));
            // The anchor and interval travel as nanoseconds (the contract's
            // own `Duration`), so the shell scales them for a human.
            let anchor_ms = spec.field("anchor").as_f64().unwrap_or(0.0) / 1_000_000.0;
            let interval_ms = spec.field("interval").as_f64().unwrap_or(0.0) / 1_000_000.0;
            out.push_str(&format!(
                "  ├─ Anchor      {anchor_ms} ms from now ({})\n",
                spec.field_str("clock")
            ));
            out.push_str(&format!(
                "  ├─ Interval    {interval_ms} ms {}\n",
                if spec.field_bool("periodic") {
                    "(periodic)".to_string()
                } else {
                    "(one-shot)".to_string()
                }
            ));
            out.push_str(&format!("  └─ Source      {}\n", value.field_str("reason")));
            out.trim_end().to_string()
        }
        TA::Check { id } => {
            if !value.field_str("error").is_empty() {
                return format!(
                    "  {} {} {} {}\n  └─ {}\n",
                    "⏲".blue().bold(),
                    format!("Timer {id}").blue().bold(),
                    "✗".red(),
                    value.field_str("error"),
                    value.field_str("reason")
                )
                .trim_end()
                .to_string();
            }
            format!(
                "  {} {} {} {} — the kernel's own expiration count\n",
                "⏲".blue().bold(),
                format!("Timer {id}").blue().bold(),
                if value.field_bool("fired") {
                    "◉ fired".green().to_string()
                } else {
                    "○ not yet".dimmed().to_string()
                },
                "".dimmed()
            )
            .trim_end()
            .to_string()
        }
        TA::List => render_timer_list(value),
        TA::Cancel { .. } => format!(
            "  {} {} {} {}\n",
            "✎".green(),
            "Timer".blue().bold(),
            format!("#{} disarmed", value.field_str("id")).bold(),
            "— the engine's own path, accepted".dimmed()
        )
        .trim_end()
        .to_string(),
    }
}

fn render_virtual(value: &Value, action: &VA) -> String {
    match action {
        VA::Create { .. } => format!(
            "  {} {} {} {} rate {} — a virtual clock IS software, so this is REAL\n",
            "🖥".blue().bold(),
            "Virtual clock".blue().bold(),
            format!("#{}", value.field_str("id")).bold(),
            value.field_str("name").dimmed(),
            value.field_str("rate")
        )
        .trim_end()
        .to_string(),
        VA::Advance { id, millis } => format!(
            "  {} {} {} advanced {} ms → {}\n",
            "🖥".blue().bold(),
            format!("Virtual clock {id}").blue().bold(),
            "◉".green(),
            millis,
            value.field_str("now").bold()
        )
        .trim_end()
        .to_string(),
    }
}

/// The alarm surface: this process's own store.
fn render_alarm(value: &Value, action: &AA) -> String {
    match action {
        AA::List => {
            let alarms = match value.field("alarms") {
                Value::List(items) => items.clone(),
                _ => Vec::new(),
            };
            let mut out = String::new();
            out.push_str(&format!(
                "  {} {} {} — this process's own store, through alarms @19\n",
                "⏰".blue().bold(),
                "Alarms".blue().bold(),
                format!("({})", alarms.len()).dimmed()
            ));
            if alarms.is_empty() {
                out.push_str(&format!(
                    "  └─ {} the store holds no alarm (an honest empty — the store was read)\n",
                    "○".dimmed()
                ));
                return out.trim_end().to_string();
            }
            let mut table = Table::new(vec![
                "ID".into(),
                "LABEL".into(),
                "FIRE TIME".into(),
                "RECURRENCE".into(),
                "ENABLED".into(),
            ]);
            for alarm in &alarms {
                table = table.row(vec![
                    Cell::plain(alarm.field_str("id")),
                    truncate_to(&alarm.field_str("label"), 24).into(),
                    Cell::plain(alarm.field_str("fire_time")),
                    Cell::plain(alarm.field_str("recurrence")),
                    yes_no(alarm.field_bool("enabled")),
                ]);
            }
            out.push_str(&table.render());
            out.trim_end().to_string()
        }
        AA::Next => {
            if matches!(value, Value::Null) {
                return format!(
                    "  {} {} — the store holds no alarm\n",
                    "⏰".blue().bold(),
                    "Next alarm".blue().bold()
                );
            }
            format!(
                "  {} {} {} at {}\n",
                "⏰".blue().bold(),
                "Next alarm".blue().bold(),
                value.field_str("label").bold(),
                value.field_str("fire_time")
            )
            .trim_end()
            .to_string()
        }
        AA::Create { .. } => format!(
            "  {} {} {} {} — fires at {}\n",
            "⏰".blue().bold(),
            "Alarm".blue().bold(),
            format!("#{}", value.field_str("id")).bold(),
            value.field_str("label").dimmed(),
            value.field_str("fire_time")
        )
        .trim_end()
        .to_string(),
        AA::Snooze { .. } => format!(
            "  {} {} {} deferred {} ms\n",
            "⏰".blue().bold(),
            format!("Alarm {}", value.field_str("id")).blue().bold(),
            value.field_str("label"),
            value.field_str("deferred_ms")
        )
        .trim_end()
        .to_string(),
        AA::Delete { .. } => format!(
            "  {} {} {} {}\n",
            "✎".green(),
            "Alarm".blue().bold(),
            format!("#{} deleted", value.field_str("id")).bold(),
            "— the engine's own path, accepted".dimmed()
        )
        .trim_end()
        .to_string(),
    }
}

fn render_watch(value: &Value) -> String {
    let events = match value.field("events") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — the contract's own TimerSubscription\n",
        "👁".blue().bold(),
        "Timer watch".blue().bold(),
        format!("({})", events.len()).dimmed()
    ));
    if events.is_empty() {
        out.push_str(&format!(
            "  └─ {} no timer fired in the bounded rounds (the subscription is real — nothing was \
             reported)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "TIMER".into(),
        "LABEL".into(),
        "FIRED AT".into(),
        "COUNT".into(),
    ]);
    for event in &events {
        table = table.row(vec![
            Cell::plain(event.field_str("timer")),
            Cell::plain(event.field_str("label")),
            Cell::plain(event.field_str("fired_at")),
            // The kernel's own expiration count, never a fabricated 1.
            Cell::plain(event.field_str("count")),
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!("\n  {} {}\n", "●".blue().bold(), value.field_str("reason")));
    out.trim_end().to_string()
}

fn render_caps(value: &Value) -> String {
    let Value::List(caps) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — what this backend serves\n",
        "🔑".blue().bold(),
        "Clock caps".blue().bold()
    ));
    for (index, cap) in caps.iter().enumerate() {
        let branch = if index + 1 == caps.len() { "└─" } else { "├─" };
        out.push_str(&format!("  {} {} {}\n", branch, "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}

// ── the shell's own small helpers ─────────────────────────────────────────

/// A duration in the shell's own words: days, hours, minutes, seconds.
pub fn duration_label(nanos: u64) -> String {
    let secs = nanos / 1_000_000_000;
    let millis = (nanos % 1_000_000_000) / 1_000_000;
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else if secs > 0 {
        format!("{seconds}s")
    } else if millis > 0 {
        // Sub-second is where a one-shot timer's delay actually lives: a 10 ms
        // anchor labelled "0s" would be a rounding, not a presentation.
        format!("{millis} ms")
    } else {
        format!("{nanos} ns")
    }
}

fn yes_no(yes: bool) -> Cell {
    if yes {
        Cell::styled("◉", Sgr::Green)
    } else {
        Cell::styled("○", Sgr::Dimmed)
    }
}
