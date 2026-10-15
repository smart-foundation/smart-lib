//! The process domain's rendering, in the shell's aesthetic: box-drawing
//! tables, `◉/○/◐`, measured numbers. Every renderer reads the ENGINE'S OWNED
//! TYPES (`ProcessInfo`, `ProcessStatus`, `ResourceUsage`, `ThreadInfo`,
//! `MemoryMapEntry`, `ProcessGroup`, `Quota`, `ExitStatus`) — there is no capnp
//! reader here, and no `/proc` parsing outside the engine, so nothing can
//! disagree with what the kernel answered.
//!
//! Every width is a display width under the terminal-faithful measure, and
//! every cell is CONTENT plus a STYLE (`Cell`), never a pre-styled string.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{human_bytes, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::process::ProcessBackend;
use smart_lib::domains::process::types::{Page, ProcessInfo, ProcessStats, Quota, ResourceUsage};

use super::{exit_status_words, info_summary, table_filter, wait_timeout};
use super::{ProcessAction as A, ProcessCmd};

/// A 64-bit value that must not be rounded: `T.Timestamp` is nanoseconds since
/// the epoch, beyond `f64`'s exact integer range, so it travels as its exact
/// decimal text.
fn exact(value: u64) -> Value {
    Value::String(value.to_string())
}

/// The engine's own published reason for one contract method, so a degraded row
/// is answered with the matrix's own measured words.
fn matrix_reason(method: &str) -> Option<String> {
    use smart_lib::domains::process::availability::{method_availability_for, AvailStatus};
    let row = method_availability_for(method)?;
    let word = match row.status {
        AvailStatus::Real => "REAL",
        AvailStatus::Degraded => "DEGRADED",
        AvailStatus::Unavailable => "UNAVAILABLE",
    };
    Some(if row.reason.is_empty() {
        word.to_string()
    } else {
        format!("{word} — {}", row.reason)
    })
}

// ═══ the data half ═══════════════════════════════════════════════════════

/// `process` — the table: every process the host runs, with the kernel's own
/// measured memory and times. The memory is the kernel's own resident pages;
/// the time column is the kernel's own `utime+stime` counters scaled by the
/// host's own clock-tick rate (never the archive body's hardcoded pair).
pub fn table_value(cmd: &ProcessCmd, backend: &dyn ProcessBackend) -> SmartResult<Value> {
    let filter = table_filter(cmd.name.as_deref(), cmd.state.as_deref(), cmd.user)?;
    let page = backend.list_processes(&filter, super::all())?;
    let mut rows = Vec::with_capacity(page.items.len());
    for info in &page.items {
        // The accounting read is the kernel's own counters with no sampling
        // window (stat + statm + io), so the table's time and memory columns are
        // measured rather than guessed.
        let accounting = backend.get_accounting(info.pid).ok();
        rows.push(row_value(info, accounting.as_ref()));
    }
    Ok(Value::List(rows))
}

/// One table row: the engine's own `ProcessInfo` plus its measured accounting.
fn row_value(info: &ProcessInfo, accounting: Option<&ResourceUsage>) -> Value {
    let mut row = vec![
        ("pid".into(), Value::Number(info.pid.get() as f64)),
        ("ppid".into(), Value::Number(info.ppid.get() as f64)),
        ("name".into(), Value::String(info.name.clone())),
        ("command".into(), Value::String(info.command.clone())),
        ("state".into(), Value::String(info.state.as_token().to_string())),
        ("uid".into(), Value::Number(info.user.get() as f64)),
        ("threads".into(), Value::Number(f64::from(info.threads))),
        (
            "mem".into(),
            Value::Number(accounting.map(|a| a.memory_bytes).unwrap_or(info.memory_usage) as f64),
        ),
    ];
    // The kernel's own tick counters, scaled by the host's own clock-tick rate:
    // a measured duration, printed as one.
    if let Some(accounting) = accounting {
        row.push((
            "cpu_nanos".into(),
            Value::Number(accounting.cpu_time.as_nanos() as f64),
        ));
        row.push(("peak_mem".into(), Value::Number(accounting.peak_memory_bytes as f64)));
        row.push(("io_read".into(), Value::Number(accounting.io_read_bytes as f64)));
        row.push(("io_write".into(), Value::Number(accounting.io_write_bytes as f64)));
    }
    Value::Object(row)
}

/// One engine `ProcessInfo` as the selector tree's record.
pub fn process_value(info: &ProcessInfo) -> Value {
    info_summary(info)
}

/// `process <pid>` — the contract's whole read surface for one process: the
/// info, the measured status, the accounting, the limits, the threads, the
/// memory map's measured verdict and this uid's signal authority over it.
pub fn inspect_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let info = backend.get_process(pid).map_err(SmartError::from)?;
    let mut out = vec![
        ("pid".into(), Value::Number(info.pid.get() as f64)),
        ("ppid".into(), Value::Number(info.ppid.get() as f64)),
        ("name".into(), Value::String(info.name.clone())),
        ("command".into(), Value::String(info.command.clone())),
        ("state".into(), Value::String(info.state.as_token().to_string())),
        ("uid".into(), Value::Number(info.user.get() as f64)),
        ("threads".into(), Value::Number(f64::from(info.threads))),
        ("mem".into(), Value::Number(info.memory_usage as f64)),
        ("start_time".into(), exact(info.start_time.as_nanos())),
    ];
    // The status read is the measured one: the CPU fraction over the kernel's
    // own bounded window, the uptime from btime plus the process's own start.
    match backend.get_process_status(pid) {
        Ok(status) => {
            out.push(("uptime_nanos".into(), Value::Number(status.uptime.as_nanos() as f64)));
            // The engine's status read measures the CPU over the kernel's own
            // bounded window for the aggregate; the per-process measured value
            // is the kernel's own cumulative tick counter, carried here.
            out.push(("cpu_time_nanos".into(), Value::Number(status.cpu_time.as_nanos() as f64)));
            out.push((
                "cpu_time_nanos".into(),
                Value::Number(status.cpu_time.as_nanos() as f64),
            ));
            out.push(("status_memory_bytes".into(), Value::Number(status.memory_bytes as f64)));
            out.push((
                "exit_status".into(),
                Value::String(exit_status_words(&status.exit_status)),
            ));
        }
        Err(e) => out.push(("status_error".into(), Value::String(e.to_string()))),
    }
    match backend.get_accounting(pid) {
        Ok(accounting) => {
            out.push((
                "peak_mem".into(),
                Value::Number(accounting.peak_memory_bytes as f64),
            ));
            out.push(("io_read".into(), Value::Number(accounting.io_read_bytes as f64)));
            out.push(("io_write".into(), Value::Number(accounting.io_write_bytes as f64)));
        }
        Err(e) => out.push(("accounting_error".into(), Value::String(e.to_string()))),
    }
    // The measured verdicts: this uid's signal authority (the host's own
    // kill(pid, 0)), and this host's ptrace answer.
    match backend.measure_signal_privilege(pid, smart_lib::domains::process::types::SignalKind::Terminate) {
        Ok(allowed) => out.push(("can_signal".into(), Value::Bool(allowed))),
        Err(e) => out.push(("can_signal_error".into(), Value::String(e.to_string()))),
    }
    match backend.get_resource_limits(pid) {
        Ok(limits) => out.push(("limits".into(), quota_value(&limits))),
        Err(e) => out.push(("limits_error".into(), Value::String(e.to_string()))),
    }
    Ok(Value::Object(out))
}

/// The threads of one process: the kernel's own `task/<tid>/stat` rows.
pub fn threads_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let page = backend.get_threads(pid, super::all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "threads".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|thread| {
                        Value::Object(vec![
                            ("tid".into(), Value::Number(thread.id.get() as f64)),
                            ("name".into(), Value::String(thread.name.clone())),
                            ("state".into(), Value::String(thread.state.as_token().to_string())),
                            // The kernel exports no per-thread CPU fraction, so
                            // the engine reports 0 and says so rather than
                            // inventing one.
                            ("cpu".into(), Value::Number(f64::from(thread.cpu_usage))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getThreads")
                    .unwrap_or_else(|| "the kernel's own /proc/<pid>/task rows".to_string()),
            ),
        ),
    ]))
}

/// The environment of one process: the host's own NUL-separated pairs, with
/// the values held back when the caller asked for names only.
pub fn env_value(
    backend: &dyn ProcessBackend,
    pid: smart_lib::core::ids::ProcessId,
    names_only: bool,
) -> SmartResult<Value> {
    let env = backend.get_environment(pid).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "variables".into(),
            Value::List(
                env.iter()
                    .map(|(key, value)| {
                        Value::Object(vec![
                            ("key".into(), Value::String(key.clone())),
                            (
                                "value".into(),
                                // A secret does not belong on a screen that may
                                // be shared: the name stays, the value does not.
                                Value::String(if names_only {
                                    "⟨hidden⟩".to_string()
                                } else {
                                    value.clone()
                                }),
                            ),
                            ("hidden".into(), Value::Bool(names_only)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(env.len() as f64)),
        ("names_only".into(), Value::Bool(names_only)),
    ]))
}

/// The memory map of one process — readable for this uid's own processes only,
/// and EACCES for a foreign uid is the host's own measured answer.
pub fn maps_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let page = backend.get_memory_map(pid, super::all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "regions".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|region| {
                        Value::Object(vec![
                            ("start".into(), exact(region.start)),
                            ("end".into(), exact(region.end)),
                            ("size".into(), Value::Number(region.end.saturating_sub(region.start) as f64)),
                            (
                                "perms".into(),
                                Value::String(region.perms_word()),
                            ),
                            ("name".into(), Value::String(region.name.clone())),
                            ("backing".into(), Value::String(region.backing.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getMemoryMap").unwrap_or_else(|| {
                    "the kernel's own /proc/<pid>/maps rows, as this uid may read them".to_string()
                }),
            ),
        ),
    ]))
}

/// The resource limits of one process (`getResourceLimits`).
pub fn limits_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let limits = backend.get_resource_limits(pid).map_err(SmartError::from)?;
    Ok(quota_value(&limits))
}

/// One `Quota` as a record.
fn quota_value(limits: &Quota) -> Value {
    Value::Object(vec![
        (
            "memory_bytes".into(),
            Value::Number(limits.memory_bytes as f64),
        ),
        (
            "cpu_percent".into(),
            Value::Number(f64::from(limits.cpu_percent)),
        ),
        (
            "io_read_bytes_per_sec".into(),
            Value::Number(limits.io_read_bytes_per_sec as f64),
        ),
        (
            "io_write_bytes_per_sec".into(),
            Value::Number(limits.io_write_bytes_per_sec as f64),
        ),
        ("pids".into(), Value::Number(f64::from(limits.pids))),
        (
            "wall_clock_budget_nanos".into(),
            Value::Number(limits.wall_clock_budget.as_nanos() as f64),
        ),
    ])
}

/// The resource accounting of one process: the kernel's own counters.
pub fn accounting_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let usage = backend.get_accounting(pid).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "cpu_time_nanos".into(),
            Value::Number(usage.cpu_time.as_nanos() as f64),
        ),
        ("memory_bytes".into(), Value::Number(usage.memory_bytes as f64)),
        (
            "peak_memory_bytes".into(),
            Value::Number(usage.peak_memory_bytes as f64),
        ),
        ("io_read".into(), Value::Number(usage.io_read_bytes as f64)),
        ("io_write".into(), Value::Number(usage.io_write_bytes as f64)),
        (
            "network_rx_bytes".into(),
            Value::Number(usage.network_rx_bytes as f64),
        ),
        (
            "network_tx_bytes".into(),
            Value::Number(usage.network_tx_bytes as f64),
        ),
        (
            "acquired_at".into(),
            exact(usage.acquired_at.as_nanos()),
        ),
        (
            // The kernel publishes no per-process network counter this uid may
            // read, so the engine leaves it 0 and the reason says which.
            "reason".into(),
            Value::String(
                "cpu_time is the kernel's own utime+stime scaled by the host's clock-tick rate; \
                 memory is the kernel's own resident pages; per-process network counters are not \
                 published by any host file this uid may read, so they stay 0 rather than being \
                 invented"
                .to_string(),
            ),
        ),
    ]))
}

/// The kernel's own job-control groups.
pub fn groups_value(backend: &dyn ProcessBackend) -> SmartResult<Value> {
    let page: Page<smart_lib::domains::process::types::ProcessGroup> =
        backend.list_process_groups(super::all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "groups".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|group| {
                        Value::Object(vec![
                            ("group_id".into(), Value::Number(group.group_id.get() as f64)),
                            ("name".into(), Value::String(group.name.clone())),
                            ("session_id".into(), Value::Number(group.session_id as f64)),
                            (
                                "members".into(),
                                Value::Number(group.processes.len() as f64),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("listProcessGroups")
                    .unwrap_or_else(|| "the kernel's own pgrp/session rows, aggregated".to_string()),
            ),
        ),
    ]))
}

/// One group's members.
pub fn group_value(backend: &dyn ProcessBackend, id: u64) -> SmartResult<Value> {
    let group = backend
        .get_process_group(smart_lib::core::ids::ProcessGroupId::new(id))
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("group_id".into(), Value::Number(group.group_id.get() as f64)),
        ("name".into(), Value::String(group.name.clone())),
        ("session_id".into(), Value::Number(group.session_id as f64)),
        (
            "members".into(),
            Value::List(
                group
                    .processes
                    .iter()
                    .map(|pid| Value::Number(pid.get() as f64))
                    .collect(),
            ),
        ),
    ]))
}

/// The aggregate statistics, with the measured CPU over the kernel's own
/// bounded window.
pub fn stats_value(backend: &dyn ProcessBackend, user: smart_lib::core::ids::UserId) -> SmartResult<Value> {
    let stats: ProcessStats = backend.get_stats(user).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "user".into(),
            if user.get() == 0 {
                Value::String("every uid".into())
            } else {
                Value::Number(user.get() as f64)
            },
        ),
        (
            "total_processes".into(),
            Value::Number(f64::from(stats.total_processes)),
        ),
        ("active".into(), Value::Number(f64::from(stats.active))),
        ("blocked".into(), Value::Number(f64::from(stats.blocked))),
        ("suspended".into(), Value::Number(f64::from(stats.suspended))),
        ("terminated".into(), Value::Number(f64::from(stats.terminated))),
        (
            "total_cpu_usage".into(),
            Value::Number(f64::from(stats.total_cpu_usage)),
        ),
        (
            "total_memory_usage".into(),
            Value::Number(stats.total_memory_usage as f64),
        ),
        (
            "total_memory_human".into(),
            Value::String(human_bytes(stats.total_memory_usage)),
        ),
        (
            "reason".into(),
            Value::String(
                "the counts are the kernel's own enumeration aggregated; the aggregate CPU is \
                 measured across the kernel's own bounded 20 ms window; total memory is read from \
                 the kernel's own /proc/meminfo"
                .to_string(),
            ),
        ),
    ]))
}

/// This host's measured ptrace verdict, in the host's own words.
pub fn ptrace_value(backend: &dyn ProcessBackend) -> SmartResult<Value> {
    match backend.measure_ptrace_privilege() {
        Ok((allowed, reason)) => Ok(Value::Object(vec![
            ("allowed".into(), Value::Bool(allowed)),
            ("reason".into(), Value::String(reason)),
        ])),
        Err(e) => Ok(Value::Object(vec![
            ("error".into(), Value::String(e.to_string())),
        ])),
    }
}

/// The measured signal-delivery verdict for one pid: the host's own
/// `kill(pid, 0)` probe — a read, never a signal.
pub fn can_signal_value(backend: &dyn ProcessBackend, pid: smart_lib::core::ids::ProcessId) -> SmartResult<Value> {
    let kind = smart_lib::domains::process::types::SignalKind::Terminate;
    match backend.measure_signal_privilege(pid, kind) {
        Ok(allowed) => Ok(Value::Object(vec![
            ("pid".into(), Value::Number(pid.get() as f64)),
            ("signal".into(), Value::String(kind.as_token().to_string())),
            ("allowed".into(), Value::Bool(allowed)),
            (
                "reason".into(),
                Value::String(
                    "measured with the host's own kill(pid, 0) — signal 0 is the kernel's null \
                     signal, so the probe changes nothing"
                    .to_string(),
                ),
            ),
        ])),
        Err(e) => Err(SmartError::from(e)),
    }
}

/// A bounded wait for one process to exit (`waitpid` under the contract's own
/// "do not block").
pub fn wait_value(
    backend: &dyn ProcessBackend,
    pid: smart_lib::core::ids::ProcessId,
    timeout_ms: u64,
) -> SmartResult<Value> {
    match backend.wait(pid, wait_timeout(timeout_ms)) {
        Ok(status) => Ok(Value::Object(vec![
            ("pid".into(), Value::Number(pid.get() as f64)),
            ("timeout_ms".into(), Value::Number(timeout_ms as f64)),
            ("status".into(), Value::String(exit_status_words(&status))),
            ("exited".into(), Value::Bool(true)),
        ])),
        // The host's own answer is carried: a process that has not left (the
        // WNOHANG read), a timeout, or a pid this process may not wait on.
        Err(e) => Ok(Value::Object(vec![
            ("pid".into(), Value::Number(pid.get() as f64)),
            ("timeout_ms".into(), Value::Number(timeout_ms as f64)),
            ("exited".into(), Value::Bool(false)),
            ("reason".into(), Value::String(e.to_string())),
        ])),
    }
}

/// This backend's honest capability tokens (law #6).
pub fn caps_value(backend: &dyn ProcessBackend) -> Value {
    Value::List(
        backend
            .process_caps()
            .into_iter()
            .map(|token| Value::String(token.to_string()))
            .collect(),
    )
}

/// A gated write's answer once the gate accepted it.
pub fn verb_value(verb: &str, target: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("target".into(), Value::String(target.to_string())),
        ("changed".into(), Value::Bool(true)),
    ])
}

// ═══ the aesthetic ════════════════════════════════════════════════════════

/// Render one command's value in the requested format.
pub fn render_action(cmd: &ProcessCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &ProcessCmd, value: &Value) -> String {
    match &cmd.action {
        // `smart process <pid>` — the bare pid's inspect view, decided before
        // the table so a pid never renders as an empty table.
        _ if cmd.pid.is_some() => render_inspect(value, cmd.pid.unwrap_or(0)),
        None => render_table(value),
        Some(A::Inspect { pid }) => render_inspect(value, *pid),
        Some(A::Tree { .. }) => render_tree(value),
        Some(A::Threads { .. }) => render_threads(value),
        Some(A::Env { pid, names_only }) => render_env(value, *pid, *names_only),
        Some(A::Maps { .. }) => render_maps(value),
        Some(A::Limits { .. }) => render_limits(value),
        Some(A::Accounting { .. }) => render_accounting(value),
        Some(A::Groups) => render_groups(value),
        Some(A::Group { id }) => render_group(value, *id),
        Some(A::Stats { .. }) => render_stats(value),
        Some(A::Ptrace) => render_ptrace(value),
        Some(A::CanSignal { pid }) => render_can_signal(value, *pid),
        Some(A::Signal { .. }) | Some(A::Attach { .. }) => render_verb(value),
        Some(A::Wait { .. }) => render_wait(value),
        Some(A::Caps) => render_caps(value),
    }
}

/// The state glyph in the kernel's own state vocabulary.
fn state_glyph(state: &str) -> Cell {
    match state {
        "active" | "running" => Cell::styled("◉", Sgr::Green),
        "blocked" | "sleeping" | "disk-sleep" => Cell::styled("◐", Sgr::Yellow),
        "suspended" | "stopped" | "tracing-stop" => Cell::styled("◑", Sgr::Yellow),
        "terminated" | "zombie" | "dead" => Cell::styled("✗", Sgr::Red),
        _ => Cell::styled("○", Sgr::Dimmed),
    }
}

/// The kernel's own tick counters, scaled by the host's own clock-tick rate,
/// printed the way a process table prints time.
pub fn cpu_label(nanos: u64) -> String {
    let total_secs = nanos / 1_000_000_000;
    let hours = total_secs / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    let seconds = total_secs % 60;
    let centis = (nanos % 1_000_000_000) / 10_000_000;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else if total_secs >= 60 {
        format!("{minutes}:{seconds:02}.{centis:02}")
    } else if total_secs > 0 {
        format!("{seconds}.{centis:02}")
    } else {
        format!("0.{centis:02}")
    }
}

/// The process table: the kernel's own enumeration, every number measured.
///
/// The row count is the caller's: `process | take 5` asks for five, and this
/// renderer draws every row it is handed — the 40-row peephole that used to
/// cap 374 live processes to forty and print "334 more below the cut" is gone.
/// The WIDTH is the renderer's: every cell is bounded by the table's column
/// cap and the cut is marked (`pool_workqueue_rele…worker`, both ends kept),
/// with the full value in `-o json`.
fn render_table(value: &Value) -> String {
    let mut out = String::new();
    let Value::List(rows) = value else {
        return String::new();
    };
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "🧮".blue().bold(),
        "Processes".blue().bold(),
        format!("({})", value_length(value)).dimmed(),
        "— /proc/<pid> read natively, every value measured".dimmed(),
        "— every row the read found: `| take N` narrows it".dimmed(),
    ));
    if rows.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host runs no process matching that filter (an honest empty — /proc was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "PID".into(),
        "NAME".into(),
        "STATE".into(),
        "UID".into(),
        "MEM".into(),
        "CPU".into(),
        "THR".into(),
    ]);
    for row in rows {
        table = table.row(vec![
            Cell::plain(row.field_str("pid")),
            row.field_str("name").into(),
            state_glyph(&row.field_str("state")).with_text(&format!(" {}", row.field_str("state"))),
            Cell::plain(row.field_str("uid")),
            human_bytes(row.field("mem").as_f64().unwrap_or(0.0) as u64).into(),
            Cell::plain(cpu_label(row.field("cpu_nanos").as_f64().unwrap_or(0.0) as u64)),
            Cell::plain(row.field_str("threads")),
        ]);
    }
    // The NAME column's ceiling: a name longer than this is middle-cut with
    // the ellipsis that says so (both ends kept), the full value one door away
    // in `-o json` / `-o raw`. The ceiling is a WIDTH, never a row count — the
    // 40-row peephole that used to cap 374 processes is gone, and `| take N`
    // is how an operator narrows the list.
    out.push_str(&table.cap(1, 32).render());
    out.push_str(&format!(
        "\n  {} memory is the kernel's own resident pages; CPU is the kernel's own utime+stime \
         scaled by this host's clock-tick rate — never the archive body's hardcoded pair\n",
        "●".blue().bold()
    ));
    out.trim_end().to_string()
}

fn value_length(value: &Value) -> usize {
    match value {
        Value::List(items) => items.len(),
        Value::Object(pairs) => pairs.len(),
        _ => 0,
    }
}

/// One process's detail: the contract's whole read surface.
fn render_inspect(value: &Value, pid: u64) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "🔬".blue().bold(),
        "Process".blue().bold(),
        pid.to_string().bold(),
        format!("({})", value.field_str("name")).dimmed()
    ));
    let state = value.field_str("state");
    out.push_str(&format!(
        "  ├─ State       {} {}\n",
        state_glyph(&state),
        if state.is_empty() { "— the host did not answer".to_string() } else { state }
    ));
    out.push_str(&format!(
        "  ├─ Parent      {} · uid {} · {} thread(s)\n",
        value.field_str("ppid"),
        value.field_str("uid"),
        value.field_str("threads")
    ));
    out.push_str(&format!(
        "  ├─ Command     {}\n",
        if value.field_str("command").is_empty() {
            "—".to_string()
        } else {
            value.field_str("command")
        }
    ));
    out.push_str(&format!(
        "  ├─ Memory      {} current · {} peak\n",
        human_bytes(value.field("mem").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("peak_mem").as_f64().unwrap_or(0.0) as u64)
    ));
    out.push_str(&format!(
        "  ├─ CPU         {} s of the kernel's own utime+stime (scaled by this host's clock-tick rate)\n",
        cpu_label(value.field("cpu_time_nanos").as_f64().unwrap_or(0.0) as u64)
    ));
    let uptime = value.field("uptime_nanos").as_f64().unwrap_or(0.0) as u64;
    out.push_str(&format!(
        "  ├─ Uptime      {} · started {}\n",
        cpu_label(uptime),
        stamp_label(value.field_str("start_time"))
    ));
    out.push_str(&format!(
        "  ├─ I/O         {} read · {} written\n",
        human_bytes(value.field("io_read").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("io_write").as_f64().unwrap_or(0.0) as u64)
    ));
    match value.field("can_signal") {
        Value::Bool(allowed) => out.push_str(&format!(
            "  ├─ Signal      {} this uid {} deliver a terminate (measured with kill(pid, 0))\n",
            if *allowed { "◉".green().to_string() } else { "✗".red().to_string() },
            if *allowed { "may" } else { "may NOT" }
        )),
        _ => out.push_str(&format!(
            "  ├─ Signal      ✗ the host's kill(pid, 0) probe refused: {}\n",
            value.field_str("can_signal_error")
        )),
    }
    let limits = value.field("limits");
    if !matches!(limits, Value::Null) {
        // A 0-byte address-space limit is not a thing the host publishes, so a
        // zero reads as the host's own silence rather than as a limit of nothing.
        let memory = limits.field("memory_bytes").as_f64().unwrap_or(0.0) as u64;
        let pids = limits.field("pids").as_f64().unwrap_or(0.0);
        out.push_str(&format!(
            "  ├─ Limits      {} memory · {}% cpu · {} pid(s) (the host's own /proc/<pid>/limits)\n",
            if memory == 0 { "—".to_string() } else { human_bytes(memory) },
            limits.field("cpu_percent").as_f64().unwrap_or(0.0),
            if pids == 0.0 { "—".to_string() } else { pids.to_string() }
        ));
    }
    for (key, label) in [
        ("status_error", "Status"),
        ("accounting_error", "Accounting"),
        ("limits_error", "Limits"),
    ] {
        let reason = value.field_str(key);
        if !reason.is_empty() {
            out.push_str(&format!("  ├─ {:<11} ✗ {}\n", label, reason));
        }
    }
    out.push_str("  └─ Source     /proc/<pid>/{stat,status,cmdline,statm,io} + /proc/stat btime");
    out.trim_end().to_string()
}

/// The process tree: the engine's own walk, each node a real pid.
fn render_tree(value: &Value) -> String {
    match value {
        Value::Object(pairs) if pairs.iter().any(|(k, _)| k == "error") => {
            return format!("  {} tree — {}", "✗".red(), value.field_str("error"));
        }
        _ => {}
    }
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {}\n",
        "🌲".blue().bold(),
        "Process tree".blue().bold(),
        format!("(root pid {})", value.field_str("pid")).dimmed()
    ));
    let mut node = value.clone();
    let mut depth = 0usize;
    loop {
        let children = match node.field("children") {
            Value::List(items) => items.clone(),
            _ => Vec::new(),
        };
        out.push_str(&format!(
            "  {}{} {} {}\n",
            "  ".repeat(depth),
            if depth == 0 { "└─" } else { "├─" },
            Cell::plain(node.field_str("pid")).to_string().as_str(),
            node.field_str("name")
        ));
        if children.is_empty() {
            break;
        }
        node = children[0].clone();
        depth += 1;
        if depth > 16 {
            break;
        }
    }
    out.trim_end().to_string()
}

/// The kernel's own threads, with the per-thread CPU limit named.
fn render_threads(value: &Value) -> String {
    let mut out = String::new();
    let threads = match value.field("threads") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    out.push_str(&format!(
        "  {} {} {} — the kernel's own /proc/<pid>/task rows\n",
        "🧵".blue().bold(),
        "Threads".blue().bold(),
        format!("({})", threads.len()).dimmed()
    ));
    if threads.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host names no thread here ({})\n",
            "○".dimmed(),
            value.field_str("reason")
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "TID".into(),
        "NAME".into(),
        "STATE".into(),
        "CPU".into(),
    ]);
    for thread in &threads {
        table = table.row(vec![
            Cell::plain(thread.field_str("tid")),
            thread.field_str("name").into(),
            state_glyph(&thread.field_str("state"))
                .with_text(&format!(" {}", thread.field_str("state"))),
            Cell::plain("0".to_string()),
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!(
        "\n  {} the kernel exports no per-thread CPU fraction, so the column is 0 with the reason \
         carried rather than a guess\n",
        "●".blue().bold()
    ));
    out.trim_end().to_string()
}

/// The environment: the host's own pairs, values held back when asked.
fn render_env(value: &Value, pid: u64, names_only: bool) -> String {
    let mut out = String::new();
    let variables = match value.field("variables") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    out.push_str(&format!(
        "  {} {} {} {} — the host's own NUL-separated pairs\n",
        "🌍".blue().bold(),
        "Environment".blue().bold(),
        format!("(pid {pid}, {})", variables.len()).dimmed(),
        if names_only { "· names only" } else { "" }.dimmed()
    ));
    if variables.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host's own /proc/{pid}/environ reads empty\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["VARIABLE".into(), "VALUE".into()]);
    for variable in &variables {
        table = table.row(vec![
            Cell::plain(variable.field_str("key")),
            crate::widgets::truncate_to(&variable.field_str("value"), 56).into(),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

/// The memory map: the kernel's own rows, and the measured verdict when this
/// uid may not read them.
fn render_maps(value: &Value) -> String {
    match value.field("error") {
        Value::String(reason) if !reason.is_empty() => {
            return format!("  {} maps — {}", "✗".red(), reason);
        }
        _ => {}
    }
    let mut out = String::new();
    let regions = match value.field("regions") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    out.push_str(&format!(
        "  {} {} {} — the kernel's own /proc/<pid>/maps rows\n",
        "🗺".blue().bold(),
        "Memory map".blue().bold(),
        format!("({})", regions.len()).dimmed()
    ));
    if regions.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host names no region here ({})\n",
            "○".dimmed(),
            value.field_str("reason")
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "START".into(),
        "END".into(),
        "SIZE".into(),
        "PERMS".into(),
        "NAME".into(),
    ]);
    for region in &regions {
        table = table.row(vec![
            Cell::plain(region.field_str("start")),
            Cell::plain(region.field_str("end")),
            human_bytes(region.field("size").as_f64().unwrap_or(0.0) as u64).into(),
            Cell::plain(region.field_str("perms")),
            crate::widgets::truncate_to(&region.field_str("name"), 40).into(),
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!(
        "\n  {} {}\n",
        "●".blue().bold(),
        value.field_str("reason")
    ));
    out.trim_end().to_string()
}

fn render_limits(value: &Value) -> String {
    format!(
        "  {} {} {} memory · {}% cpu · {} io r/s · {} io w/s · {} pid(s)\n",
        "📐".blue().bold(),
        "Limits".blue().bold(),
        human_bytes(value.field("memory_bytes").as_f64().unwrap_or(0.0) as u64),
        value.field_str("cpu_percent"),
        human_bytes(value.field("io_read_bytes_per_sec").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("io_write_bytes_per_sec").as_f64().unwrap_or(0.0) as u64),
        value.field_str("pids")
    )
    .trim_end()
    .to_string()
}

/// The accounting: the kernel's own counters, with the two limits named.
fn render_accounting(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "📈".blue().bold(),
        "Accounting".blue().bold()
    ));
    out.push_str(&format!(
        "  ├─ CPU time   {} ({})\n",
        cpu_label(value.field("cpu_time_nanos").as_f64().unwrap_or(0.0) as u64),
        value.field_str("cpu_time_nanos")
    ));
    out.push_str(&format!(
        "  ├─ Memory     {} current · {} peak\n",
        human_bytes(value.field("memory_bytes").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("peak_memory_bytes").as_f64().unwrap_or(0.0) as u64)
    ));
    out.push_str(&format!(
        "  ├─ I/O        {} read · {} written\n",
        human_bytes(value.field("io_read_bytes").as_f64().unwrap_or(0.0) as u64),
        human_bytes(value.field("io_write_bytes").as_f64().unwrap_or(0.0) as u64)
    ));
    out.push_str(&format!("  └─ Source     {}\n", value.field_str("reason")));
    out.trim_end().to_string()
}

fn render_groups(value: &Value) -> String {
    let mut out = String::new();
    let groups = match value.field("groups") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    out.push_str(&format!(
        "  {} {} {} — the kernel's own pgrp/session rows\n",
        "👥".blue().bold(),
        "Process groups".blue().bold(),
        format!("({})", groups.len()).dimmed()
    ));
    if groups.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host runs no process group ({})\n",
            "○".dimmed(),
            value.field_str("reason")
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "PGID".into(),
        "NAME".into(),
        "SESSION".into(),
        "MEMBERS".into(),
    ]);
    for group in &groups {
        table = table.row(vec![
            Cell::plain(group.field_str("group_id")),
            Cell::plain(group.field_str("name")),
            Cell::plain(group.field_str("session_id")),
            Cell::plain(group.field_str("members")),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_group(value: &Value, id: u64) -> String {
    let members = match value.field("members") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    format!(
        "  {} {} {} member(s) in session {} — {}",
        "👥".blue().bold(),
        format!("Group {id}").blue().bold(),
        members.len(),
        value.field_str("session_id"),
        members
            .iter()
            .map(|pid| pid.display())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .trim_end()
    .to_string()
}

/// The aggregate statistics, with the measured CPU over the kernel's window.
fn render_stats(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — the kernel's own enumeration, aggregated\n",
        "📊".blue().bold(),
        "Process stats".blue().bold(),
        format!("({})", value.field_str("user")).dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Processes  {} total · {} active · {} blocked · {} suspended · {} terminated\n",
        value.field_str("total_processes"),
        value.field_str("active"),
        value.field_str("blocked"),
        value.field_str("suspended"),
        value.field_str("terminated")
    ));
    out.push_str(&format!(
        "  ├─ CPU        the engine's own aggregate over its measured window: {}\n",
        value.field("total_cpu_usage").as_f64().unwrap_or(0.0)
    ));
    out.push_str(&format!(
        "  {} the contract's `totalCpuUsage` window reading — the kernel's own counters, never the \
         archive body's hardcode\n",
        "●".blue().bold()
    ));
    out.push_str(&format!(
        "  └─ Memory     {} in use (the kernel's own /proc/meminfo)\n",
        value.field_str("total_memory_human")
    ));
    out.trim_end().to_string()
}

fn render_ptrace(value: &Value) -> String {
    let allowed = value.field_bool("allowed");
    format!(
        "  {} {} {} {}\n  └─ {}\n",
        "🧷".blue().bold(),
        "Ptrace".blue().bold(),
        if allowed { "◉ permitted".green().to_string() } else { "✗ refused".red().to_string() },
        "— measured, not assumed".dimmed(),
        value.field_str("reason")
    )
    .trim_end()
    .to_string()
}

fn render_can_signal(value: &Value, pid: u64) -> String {
    let allowed = value.field_bool("allowed");
    format!(
        "  {} {} {} {} — {}\n",
        "📡".blue().bold(),
        format!("pid {pid}").blue().bold(),
        if allowed {
            "◉ this uid may deliver a terminate".green().to_string()
        } else {
            "✗ this uid may NOT deliver a terminate".red().to_string()
        },
        "(measured)".dimmed(),
        value.field_str("reason")
    )
    .trim_end()
    .to_string()
}

/// A gated write's answer once the gate accepted it.
fn render_verb(value: &Value) -> String {
    format!(
        "  {} {} {} {}",
        "✎".green(),
        value.field_str("verb").bold(),
        value.field_str("target"),
        "— the engine's own host-mutating path, accepted".dimmed()
    )
    .trim_end()
    .to_string()
}

fn render_wait(value: &Value) -> String {
    if value.field_bool("exited") {
        format!(
            "  {} {} pid {} — {}\n",
            "⏳".blue().bold(),
            "Wait".blue().bold(),
            value.field_str("pid"),
            value.field_str("status").bold()
        )
        .trim_end()
        .to_string()
    } else {
        format!(
            "  {} {} pid {} — {}\n",
            "⏳".blue().bold(),
            "Wait".blue().bold(),
            value.field_str("pid"),
            value.field_str("reason")
        )
        .trim_end()
        .to_string()
    }
}

fn render_caps(value: &Value) -> String {
    let Value::List(caps) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — what this backend serves\n",
        "🔑".blue().bold(),
        "Process caps".blue().bold()
    ));
    for (index, cap) in caps.iter().enumerate() {
        let branch = if index + 1 == caps.len() { "└─" } else { "├─" };
        out.push_str(&format!("  {} {} {}\n", branch, "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}

/// A nanosecond timestamp as a wall clock, or an em dash when the host
/// reported no time (0 is never invented).
pub fn stamp_label(nanos_text: String) -> String {
    let nanos = nanos_of(&nanos_text);
    if nanos == 0 {
        return "—".to_string();
    }
    let secs = (nanos / 1_000_000_000) as i64;
    let millis = (nanos % 1_000_000_000) / 1_000_000;
    let civil = civil_from_secs(secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        civil.0, civil.1, civil.2, civil.3, civil.4, civil.5, millis
    )
}

fn nanos_of(text: &str) -> u64 {
    text.parse::<u64>().unwrap_or(0)
}

/// The contract's own civil kernel inverted, so the shell never reaches for
/// chrono's epoch math — the rule the time domain holds.
fn civil_from_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (seconds_of_day / 3_600) as u32,
        ((seconds_of_day % 3_600) / 60) as u32,
        (seconds_of_day % 60) as u32,
    )
}

// The tree the engine's own `getProcessTree` answers, walked for the aesthetic.
fn tree_children(node: &smart_lib::domains::process::types::ProcessTree) -> Value {
    Value::List(
        node.children
            .iter()
            .map(|child| {
                Value::Object(vec![
                    ("pid".into(), Value::Number(child.pid.get() as f64)),
                    ("name".into(), Value::String(child.name.clone())),
                    ("children".into(), tree_children(child)),
                ])
            })
            .collect(),
    )
}

/// `getProcessTree` as the shell's value tree.
pub fn tree_value(
    backend: &dyn ProcessBackend,
    pid: smart_lib::core::ids::ProcessId,
    depth: u32,
) -> SmartResult<Value> {
    let tree = backend.get_process_tree(pid, depth).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("pid".into(), Value::Number(tree.pid.get() as f64)),
        ("name".into(), Value::String(tree.name.clone())),
        ("children".into(), tree_children(&tree)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getProcessTree")
                    .unwrap_or_else(|| "the kernel's own ppid relationships".to_string()),
            ),
        ),
    ]))
}
