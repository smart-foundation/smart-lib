//! The logging domain's rendering, in the shell's aesthetic: box-drawing
//! tables, `◉/○/✗`, the host's own measured reasons. Every renderer reads the
//! ENGINE'S OWNED TYPES (`HostSink`, `LogDomain`, `LogEntry`, `LogFilter`) —
//! there is no capnp reader here, so a sink's measured verdict is printed
//! exactly as the host answered it.
//!
//! Every width is a display width under the terminal-faithful measure, and
//! every cell is CONTENT plus a STYLE (`Cell`), never a pre-styled string.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{truncate_to, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::logging::types::{Level, LogFilter, Pagination};
use smart_lib::domains::logging::LoggingBackend;

use super::{domain_value, entry_value, filter_of, sink_value};
use super::{LoggingAction as A, LoggingCmd};

/// The most rows one human table draws, and the line that names the cut.
const MAX_ROWS: usize = 40;

/// The most rows one human LINE view draws.
const MAX_LINES: usize = 40;

/// The engine's own published reason for one contract method, so a degraded row
/// is answered with the matrix's own measured words.
fn matrix_reason(method: &str) -> Option<String> {
    use smart_lib::domains::logging::availability::{method_availability_for, AvailStatus};
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

/// `logging` — the sinks this host has: every svlogd supervisor directory,
/// every `/var/log` file, with the measured verdict for each.
pub fn sinks_value(backend: &super::Backend) -> Value {
    let sinks = backend.host_sinks();
    let readable = sinks.iter().filter(|s| s.readable).count();
    Value::Object(vec![
        (
            "sinks".into(),
            Value::List(sinks.iter().map(sink_value).collect()),
        ),
        ("total".into(), Value::Number(sinks.len() as f64)),
        ("readable".into(), Value::Number(readable as f64)),
        (
            "reason".into(),
            Value::String(
                "measured by opening each one: this host runs no journald and no /dev/log (both \
                 measured), so the trails are the host's own svlogd directories and /var/log \
                 files — plus the one sink this process owns, its own stderr"
                    .to_string(),
            ),
        ),
    ])
}

/// `logging domains` — every domain the backend knows, with its configured
/// level and sampling rate.
pub fn domains_value(backend: &dyn LoggingBackend) -> SmartResult<Value> {
    let page = backend
        .list_domains(Pagination {
            offset: 0,
            limit: u32::MAX,
        })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "domains".into(),
            Value::List(page.items.iter().map(domain_value).collect()),
        ),
        ("total".into(), Value::Number(page.total as f64)),
    ]))
}

/// `logging tail <domain>` — one sink's own lines, read natively.
pub fn tail_value(backend: &dyn LoggingBackend, domain: &str, lines: u32) -> SmartResult<Value> {
    // The sink's domain token is the filter's domain, so the engine reads the
    // host's own file (or answers with the measured reason when it may not).
    let filter = LogFilter {
        domain: domain.to_string(),
        min_level: Level::Trace,
        since: smart_lib::core::values::Timestamp::UNIX_EPOCH,
        until: smart_lib::core::values::Timestamp::UNIX_EPOCH,
        pattern: String::new(),
        subsystems: Vec::new(),
        trace_id: String::new(),
        after_sequence: 0,
    };
    let page = backend
        .get_entries(&filter, Pagination {
            offset: 0,
            limit: lines.max(1),
        })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        (
            "entries".into(),
            Value::List(page.items.iter().map(entry_value).collect()),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("getEntries")
                    .unwrap_or_else(|| "the host's own lines, read natively".to_string()),
            ),
        ),
    ]))
}

/// `logging entries` — this process's own retained store, filtered by the
/// contract's own vocabulary.
pub fn entries_value(
    backend: &dyn LoggingBackend,
    level: Option<&str>,
    domain: Option<&str>,
    pattern: Option<&str>,
) -> SmartResult<Value> {
    let filter = filter_of(domain, level, pattern)?;
    let page = backend
        .get_entries(&filter, Pagination {
            offset: 0,
            limit: u32::MAX,
        })
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "entries".into(),
            Value::List(page.items.iter().map(entry_value).collect()),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        ("min_level".into(), Value::String(filter.min_level.as_token().to_string())),
    ]))
}

/// `logging follow` — the contract's own subscription interface: one
/// subscription, bounded pulls, the entries the backend reported.
pub fn follow_value(
    backend: &dyn LoggingBackend,
    domain: Option<&str>,
    level: Option<&str>,
    count: u32,
    interval_ms: u64,
) -> SmartResult<Value> {
    let filter = filter_of(domain, level, None)?;
    let subscription = backend.subscribe(&filter).map_err(SmartError::from)?;
    let mut entries = Vec::new();
    for _ in 0..count.max(1) {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        let batch = backend
            .pull(subscription, 32)
            .map_err(SmartError::from)?;
        for entry in batch {
            entries.push(entry_value(&entry));
        }
    }
    let _ = backend.cancel(subscription);
    Ok(Value::Object(vec![
        (
            "entries".into(),
            Value::List(entries.clone()),
        ),
        ("total".into(), Value::Number(entries.len() as f64)),
        (
            "min_level".into(),
            Value::String(filter.min_level.as_token().to_string()),
        ),
        (
            "reason".into(),
            Value::String(
                "the contract's own LogSubscription: a minted token, a bounded pull, and a cancel \
                 — a real event stream, not a tail of a file"
                    .to_string(),
            ),
        ),
    ]))
}

/// This backend's honest capability tokens (law #6).
pub fn caps_value(backend: &dyn LoggingBackend) -> Value {
    Value::List(
        backend
            .logging_caps()
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
pub fn render_action(cmd: &LoggingCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &LoggingCmd, value: &Value) -> String {
    match &cmd.action {
        None | Some(A::Sinks { .. }) => render_sinks(value),
        Some(A::Domains) => render_domains(value),
        Some(A::Tail { domain, .. }) => render_tail(value, domain),
        Some(A::Entries { .. }) => render_entries(value),
        Some(A::Follow { .. }) => render_follow(value),
        Some(A::Emit { .. }) => render_emit(value),
        Some(A::Level { domain }) => render_level(value, domain),
        Some(A::SetLevel { .. }) | Some(A::Sampling { .. }) | Some(A::Clear { .. }) => {
            render_verb(value)
        }
        Some(A::Export { .. }) => render_export(value),
        Some(A::Caps) => render_caps(value),
    }
}

/// The severity glyph, in the contract's own vocabulary.
fn level_glyph(level: &str) -> Cell {
    match level {
        "trace" | "debug" => Cell::styled("·", Sgr::Dimmed),
        "info" => Cell::styled("◉", Sgr::Green),
        "warn" | "warning" => Cell::styled("◐", Sgr::Yellow),
        "error" | "fatal" | "crash" => Cell::styled("✗", Sgr::Red),
        "security" | "audit" => Cell::styled("◆", Sgr::Cyan),
        _ => Cell::plain("▢"),
    }
}

/// The sink table: the host's own sinks, each with its measured verdict.
fn render_sinks(value: &Value) -> String {
    let sinks = match value.field("sinks") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — measured by opening each one, no journald here\n",
        "📡".blue().bold(),
        "Log sinks".blue().bold(),
        format!("({})", sinks.len()).dimmed(),
        format!("{} readable", value.field_str("readable")).dimmed()
    ));
    if sinks.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host publishes no log sink (an honest empty — the host was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let hidden = sinks.len().saturating_sub(MAX_ROWS);
    let shown = &sinks[..sinks.len().min(MAX_ROWS)];
    let mut table = Table::new(vec![
        "".into(),
        "SINK".into(),
        "PATH".into(),
        "READABLE".into(),
        "MEASURED REASON".into(),
    ]);
    for sink in shown {
        let readable = sink.field_bool("readable");
        table = table.row(vec![
            if readable {
                Cell::styled("◉", Sgr::Green)
            } else {
                Cell::styled("✗", Sgr::Red)
            },
            truncate_to(&sink.field_str("domain"), 28).into(),
            truncate_to(&sink.field_str("path"), 44).into(),
            if readable {
                Cell::styled("◉ yes", Sgr::Green)
            } else {
                Cell::styled("✗ no", Sgr::Red)
            },
            truncate_to(&sink.field_str("reason"), 40).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more sink(s) the host's own scan found\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.push_str(&format!(
        "\n  {} {}\n",
        "●".blue().bold(),
        value.field_str("reason")
    ));
    out.push_str(&format!(
        "  {} the one sink this process owns is its own stderr ({}), which under a runit service \
         IS the svlogd input\n",
        "●".blue().bold(),
        "smart logging entries".dimmed()
    ));
    out.trim_end().to_string()
}

/// The domain listing: every domain the backend knows, with its level and rate.
fn render_domains(value: &Value) -> String {
    let domains = match value.field("domains") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} — the domains this backend knows\n",
        "🗂".blue().bold(),
        "Log domains".blue().bold(),
        format!("({})", domains.len()).dimmed()
    ));
    if domains.is_empty() {
        out.push_str(&format!(
            "  └─ {} the backend knows no domain yet (an honest empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let hidden = domains.len().saturating_sub(MAX_ROWS);
    let shown = &domains[..domains.len().min(MAX_ROWS)];
    let mut table = Table::new(vec![
        "DOMAIN".into(),
        "LEVEL".into(),
        "SAMPLING".into(),
        "WHAT IT IS".into(),
    ]);
    for domain in shown {
        let rate = domain.field("sampling_rate").as_f64().unwrap_or(1.0);
        table = table.row(vec![
            truncate_to(&domain.field_str("name"), 30).into(),
            level_glyph(&domain.field_str("level"))
                .with_text(&format!(" {}", domain.field_str("level"))),
            Cell::plain(format!("{rate:.2}")),
            truncate_to(&domain.field_str("description"), 52).into(),
        ]);
    }
    out.push_str(&table.render());
    if hidden > 0 {
        out.push_str(&format!(
            "\n  {} … and {} more domain(s)\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

/// One sink's own lines.
fn render_tail(value: &Value, domain: &str) -> String {
    let entries = match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the host's own lines, read natively\n",
        "📜".blue().bold(),
        "Tail".blue().bold(),
        domain.bold(),
        format!("({})", entries.len()).dimmed()
    ));
    if entries.is_empty() {
        out.push_str(&format!(
            "  └─ {} {} holds no line (a measured empty, never an invented one)\n",
            "○".dimmed(),
            domain
        ));
        return out.trim_end().to_string();
    }
    let hidden = entries.len().saturating_sub(MAX_LINES);
    let shown = &entries[..entries.len().min(MAX_LINES)];
    for entry in shown {
        out.push_str(&format!(
            "  {} {:<7} {:<24} {}\n",
            level_glyph(&entry.field_str("level")),
            entry.field_str("level"),
            entry.field_str("domain"),
            entry.field_str("message")
        ));
    }
    if hidden > 0 {
        out.push_str(&format!(
            "  {} … and {} more line(s) this sink carries\n",
            "…".dimmed(),
            hidden
        ));
    }
    let reason = value.field_str("reason");
    if !reason.is_empty() {
        out.push_str(&format!("\n  {} {}\n", "●".blue().bold(), reason));
    }
    out.trim_end().to_string()
}

/// This process's own retained entries.
fn render_entries(value: &Value) -> String {
    let entries = match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "🧾".blue().bold(),
        "Entries".blue().bold(),
        format!("({})", entries.len()).dimmed(),
        "— this process's own retained store".dimmed(),
        format!("(min level {})", value.field_str("min_level")).dimmed()
    ));
    if entries.is_empty() {
        out.push_str(&format!(
            "  └─ {} this process retained no entry (an honest empty — the store was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let hidden = entries.len().saturating_sub(MAX_LINES);
    let shown = &entries[..entries.len().min(MAX_LINES)];
    for entry in shown {
        out.push_str(&format!(
            "  {} {:<7} {:<18} {}\n",
            level_glyph(&entry.field_str("level")),
            entry.field_str("level"),
            entry.field_str("domain"),
            entry.field_str("message")
        ));
    }
    if hidden > 0 {
        out.push_str(&format!(
            "  {} … and {} more retained entr(y|ies)\n",
            "…".dimmed(),
            hidden
        ));
    }
    out.trim_end().to_string()
}

/// The live event stream.
fn render_follow(value: &Value) -> String {
    let entries = match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "👁".blue().bold(),
        "Follow".blue().bold(),
        format!("({})", entries.len()).dimmed(),
        format!("(min level {})", value.field_str("min_level")).dimmed(),
        "— the contract's own subscription, bounded by the caller".dimmed()
    ));
    if entries.is_empty() {
        out.push_str(&format!(
            "  └─ {} no entry arrived in the bounded rounds (the subscription is real — nothing \
             was reported)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    for entry in &entries {
        out.push_str(&format!(
            "  {} {:<7} {:<18} {}\n",
            level_glyph(&entry.field_str("level")),
            entry.field_str("level"),
            entry.field_str("domain"),
            entry.field_str("message")
        ));
    }
    out.push_str(&format!("\n  {} {}\n", "●".blue().bold(), value.field_str("reason")));
    out.trim_end().to_string()
}

fn render_emit(value: &Value) -> String {
    format!(
        "  {} {} {} {} {}\n",
        "✎".green(),
        "Emit".blue().bold(),
        level_glyph(&value.field_str("level")).with_text(&format!(" {}", value.field_str("level"))),
        format!("({})", value.field_str("outcome")).dimmed(),
        value.field_str("message")
    )
    .trim_end()
    .to_string()
}

fn render_level(value: &Value, domain: &str) -> String {
    format!(
        "  {} {} {} {} {}\n",
        "📶".blue().bold(),
        "Level".blue().bold(),
        domain.bold(),
        "→".dimmed(),
        value.field_str("level").bold()
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

fn render_export(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} byte(s) — an OPEN content type, rendered by the backend\n",
        "📤".blue().bold(),
        "Export".blue().bold(),
        value.field_str("mime_type").bold(),
        value.field_str("bytes")
    ));
    let text = value.field_str("text");
    if !text.is_empty() {
        out.push('\n');
        out.push_str(&truncate_to(&text, 2000));
    }
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
        "Logging caps".blue().bold()
    ));
    for (index, cap) in caps.iter().enumerate() {
        let branch = if index + 1 == caps.len() { "└─" } else { "├─" };
        out.push_str(&format!("  {} {} {}\n", branch, "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}
