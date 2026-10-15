//! `logging` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`LogEntry`, `LogFilter`,
//! `LogDomain`, `LogOutcome`, `HostSink`, `SubscriptionId`) and reads this
//! host's own sinks natively — the svlogd supervisor directories under
//! `/var/service/*/log`, the files under `/var/log`, and this process's own
//! stderr (which under a runit service IS the svlogd input). There is no
//! `journalctl` fork, no `sv` fork, no `dmesg` fork and no journal parsing,
//! because this host has no journal: **journald is absent, no `/dev/log`, no
//! auditd** — all measured, and each absence is answered with the host's own
//! measured reason instead of an empty table.
//!
//! ## The severity vocabulary is the contract's own
//!
//! Eleven levels, one-for-one with `T.LogLevel`, and the filter's default
//! minimum is the wire reading of an absent `minLevel` — ordinal 0, `trace`,
//! never `info` (which would silently drop every debug entry a caller never
//! said it wanted dropped).
//!
//! ## Per-sink honesty
//!
//! Every sink is measured by opening it: an EACCES on a root-only svlogd
//! directory is named with the host's own error words, and a service that
//! declares a log but is not running answers ENOENT. Never a silent skip, never
//! a fabricated line.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::logging::types::{
    Level, LogDomain, LogEntry, LogFilter, Pagination,
};
use smart_lib::domains::logging::LoggingBackend;
use smart_lib::platforms::linux::logging::HostSink;

/// The engine seam this shell speaks to: the contract's own
/// [`LoggingBackend`], plus the sink surface the Linux backend publishes on top
/// of it — the measured verdict per sink, so the shell can render a sink's path
/// and its reason in their own columns rather than as prose inside a
/// description string.
pub trait LoggingSeam: LoggingBackend + SinkSurface {}
impl LoggingSeam for smart_lib::platforms::linux::logging::LinuxLoggingBackend {}

/// The engine seam this shell speaks to — the per-domain backend trait object.
pub type Backend = dyn LoggingSeam;

/// The host's own sink measurements, as the Linux backend measured them.
pub trait SinkSurface {
    /// Every sink this host has, with the measured verdict for each.
    fn host_sinks(&self) -> Vec<HostSink>;
    /// Re-measure the sinks (a read, safe at any time).
    fn refresh_host_sinks(&self);
}

impl SinkSurface for smart_lib::platforms::linux::logging::LinuxLoggingBackend {
    fn host_sinks(&self) -> Vec<HostSink> {
        smart_lib::platforms::linux::logging::LinuxLoggingBackend::host_sinks(self)
    }
    fn refresh_host_sinks(&self) {
        smart_lib::platforms::linux::logging::LinuxLoggingBackend::refresh_host_sinks(self)
    }
}

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart logging` — the log sinks this host actually has, read natively.
#[derive(Parser, Debug)]
pub struct LoggingCmd {
    #[command(subcommand)]
    pub action: Option<LoggingAction>,
}

#[derive(Subcommand, Debug)]
pub enum LoggingAction {
    /// The sinks this host has (the default view): every svlogd directory,
    /// every `/var/log` file, with the measured verdict for each.
    Sinks {
        /// Re-measure the sinks before rendering (a fresh read of the host).
        #[arg(long)]
        refresh: bool,
    },
    /// One domain's level and sampling rate (`getLevel`), and every domain the
    /// backend knows (`listDomains`).
    Domains,
    /// Read one sink's own lines (`getEntries` on the sink's domain token).
    Tail {
        /// The sink's domain token: a `/var/log` file's name, or
        /// `service:<name>` for a service's svlogd directory.
        domain: String,
        /// How many lines to take.
        #[arg(long, default_value = "20")]
        lines: u32,
    },
    /// This process's own retained entries, filtered by the contract's own
    /// severity vocabulary (`getEntries`).
    Entries {
        /// Only entries at or above this level.
        #[arg(long)]
        level: Option<String>,
        /// Only this domain's entries.
        #[arg(long)]
        domain: Option<String>,
        /// Only entries whose message contains this text.
        #[arg(long)]
        pattern: Option<String>,
    },
    /// A live event stream (`subscribe` + bounded pulls) — the contract's own
    /// subscription interface, not a tail -f of a file.
    Follow {
        /// Only this domain's events.
        #[arg(long)]
        domain: Option<String>,
        /// Only events at or above this level.
        #[arg(long)]
        level: Option<String>,
        /// How many pull rounds to take before stopping.
        #[arg(long, default_value = "3")]
        count: u32,
        /// The pause between pulls, in milliseconds.
        #[arg(long, default_value = "500")]
        interval: u64,
    },
    /// Write one entry through the contract's hot path (host-touching — gated:
    /// the one sink a hosted process owns is its own stderr, which under a
    /// runit service IS the svlogd input).
    Emit {
        /// The severity, in the contract's own vocabulary.
        level: String,
        /// The domain token (empty is this process's own).
        #[arg(long, default_value = "")]
        domain: String,
        /// The message.
        #[arg(required = true, num_args = 1..)]
        message: Vec<String>,
    },
    /// Read one domain's configured level (`getLevel`).
    Level {
        /// The domain token.
        domain: String,
    },
    /// Configure one domain's level (`setLevel` — this backend's own
    /// configuration, gated).
    SetLevel {
        /// The domain token.
        domain: String,
        /// The severity.
        level: String,
    },
    /// Declare a sampling rate (`setSamplingRate` — gated): a producer that
    /// will drop 90% of debug entries says so instead of a consumer
    /// discovering it statistically.
    Sampling {
        /// The domain token.
        domain: String,
        /// The rate, a fraction in 0.0..=1.0.
        #[arg(long)]
        set: Option<f32>,
    },
    /// Clear one domain's retained entries (`clear` — this process's own
    /// store; a host log file this caller does not own is refused).
    Clear {
        /// The domain token (empty is every domain this process retained).
        domain: String,
    },
    /// Export the retained entries in an OPEN content type (`export`).
    Export {
        /// Only this domain's entries.
        #[arg(long)]
        domain: Option<String>,
        /// The content type, e.g. `application/x-ndjson`.
        #[arg(long, default_value = "text/plain")]
        mime: String,
    },
    /// This backend's honest capability tokens.
    Caps,
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used. Logging's own
/// configuration verbs (`setLevel`, `setSamplingRate`) write THIS backend's
/// state, and `emit` writes the one stream this process owns; all are gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Parse a severity token through the contract's own vocabulary (an unknown
/// token is the contract's `custom` arm, never a guessed severity).
pub(crate) fn level_of(token: &str) -> SmartResult<Level> {
    let level = Level::from_token(token);
    if matches!(level, Level::Custom) {
        return Err(SmartError::invalid_argument(format!(
            "a severity is trace/debug/info/warn/error/fatal/silent/crash/security/audit, got {token:?}"
        )));
    }
    Ok(level)
}

/// The filter the read surface builds, in the contract's own fields.
pub(crate) fn filter_of(
    domain: Option<&str>,
    level: Option<&str>,
    pattern: Option<&str>,
) -> SmartResult<LogFilter> {
    Ok(LogFilter {
        domain: domain.unwrap_or_default().to_string(),
        min_level: match level {
            Some(token) => level_of(token)?,
            // The wire reading of an absent `minLevel`: ordinal 0, `trace` —
            // never `info`, which would silently drop the debug entries.
            None => Level::Trace,
        },
        since: smart_lib::core::values::Timestamp::UNIX_EPOCH,
        until: smart_lib::core::values::Timestamp::UNIX_EPOCH,
        pattern: pattern.unwrap_or_default().to_string(),
        subsystems: Vec::new(),
        trace_id: String::new(),
        after_sequence: 0,
    })
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &LoggingCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None => Ok(render::sinks_value(backend)),
        Some(LoggingAction::Sinks { refresh }) => {
            if *refresh {
                backend.refresh_host_sinks();
            }
            Ok(render::sinks_value(backend))
        }
        Some(LoggingAction::Domains) => render::domains_value(backend),
        Some(LoggingAction::Tail { domain, lines }) => render::tail_value(backend, domain, *lines),
        Some(LoggingAction::Entries {
            level,
            domain,
            pattern,
        }) => render::entries_value(backend, level.as_deref(), domain.as_deref(), pattern.as_deref()),
        Some(LoggingAction::Follow {
            domain,
            level,
            count,
            interval,
        }) => render::follow_value(backend, domain.as_deref(), level.as_deref(), *count, *interval),
        Some(LoggingAction::Emit {
            level,
            domain,
            message,
        }) => {
            guard_host_write(
                guard,
                &format!("logging emit {level}"),
                "the one sink a hosted process owns is its own stderr — under a runit service \
                 that stream IS the svlogd input, so an entry lands in a host log file",
            )?;
            let level = level_of(level)?;
            let text = message.join(" ");
            let outcome = backend.log(level, domain, &text).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("level".into(), Value::String(level.as_token().to_string())),
                ("domain".into(), Value::String(domain.clone())),
                ("message".into(), Value::String(text)),
                ("outcome".into(), Value::String(outcome.as_token().to_string())),
                (
                    "reason".into(),
                    Value::String(
                        "accepted and retained in this process's own bounded store, and written \
                         to the one sink this process owns"
                            .to_string(),
                    ),
                ),
            ]))
        }
        Some(LoggingAction::Level { domain }) => {
            let level = backend.get_level(domain).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("domain".into(), Value::String(domain.clone())),
                ("level".into(), Value::String(level.as_token().to_string())),
            ]))
        }
        Some(LoggingAction::SetLevel { domain, level }) => {
            guard_host_write(
                guard,
                &format!("logging set-level {domain}"),
                "configuring a domain's level changes what this backend retains",
            )?;
            let level = level_of(level)?;
            backend.set_level(domain, level).map_err(SmartError::from)?;
            Ok(render::verb_value("set-level", domain))
        }
        Some(LoggingAction::Sampling { domain, set }) => {
            let Some(rate) = set else {
                let domains = backend.list_domains(all()).map_err(SmartError::from)?;
                let current = domains
                    .items
                    .iter()
                    .find(|d| d.name == *domain)
                    .map(|d| d.sampling_rate);
                return Ok(Value::Object(vec![
                    ("domain".into(), Value::String(domain.clone())),
                    (
                        "sampling_rate".into(),
                        match current {
                            Some(rate) => Value::Number(f64::from(rate)),
                            None => Value::Null,
                        },
                    ),
                ]));
            };
            guard_host_write(
                guard,
                &format!("logging sampling {domain}"),
                "declaring a sampling rate changes what this backend retains",
            )?;
            backend
                .set_sampling_rate(domain, *rate)
                .map_err(SmartError::from)?;
            Ok(render::verb_value("sampling", &format!("{domain} @ {rate}")))
        }
        Some(LoggingAction::Clear { domain }) => {
            guard_host_write(
                guard,
                &format!("logging clear {domain}"),
                "clearing refuses to truncate a host log file this caller does not own — the \
                 engine answers with the measured reason",
            )?;
            backend.clear(domain).map_err(SmartError::from)?;
            Ok(render::verb_value("clear", domain))
        }
        Some(LoggingAction::Export { domain, mime }) => {
            let filter = filter_of(domain.as_deref(), None, None)?;
            let bytes = backend.export(&filter, mime).map_err(SmartError::from)?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            Ok(Value::Object(vec![
                ("mime_type".into(), Value::String(mime.clone())),
                ("bytes".into(), Value::Number(bytes.len() as f64)),
                ("text".into(), Value::String(text)),
            ]))
        }
        Some(LoggingAction::Caps) => Ok(render::caps_value(backend)),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &LoggingCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

pub mod render;

/// The host-write gate (law #8, the C1 shape): a mutating verb proceeds only
/// when the caller accepted the write, with the reason what it would touch.
fn guard_host_write(guard: WriteGuard, verb: &str, what: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES host state ({what}). In the REPL confirm the \
         prompt, or pass --force once you accept the write (the live bring-back proof is \
         founder-run)."
    )))
}

/// The logging selector tree, rendered from the engine's owned types — the same
/// values the command surface renders, so a selector and a command can never
/// disagree. `@logging/sinks`, `@logging/domains`, `@logging/caps` resolve from
/// here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("sinks".into(), render::sinks_value(backend)),
        ("domains".into(), render::domains_value(backend)?),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// One `LogEntry` as a record.
pub(crate) fn entry_value(entry: &LogEntry) -> Value {
    Value::Object(vec![
        ("sequence".into(), Value::Number(entry.sequence as f64)),
        ("timestamp".into(), Value::String(entry.timestamp.as_nanos().to_string())),
        ("level".into(), Value::String(entry.level.as_token().to_string())),
        ("domain".into(), Value::String(entry.domain.clone())),
        ("message".into(), Value::String(entry.message.clone())),
        ("subsystem".into(), Value::Number(entry.subsystem.get() as f64)),
        ("metadata".into(), pairs_value(&entry.metadata)),
    ])
}

/// One `LogDomain` as a record.
pub(crate) fn domain_value(domain: &LogDomain) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(domain.name.clone())),
        ("level".into(), Value::String(domain.level.as_token().to_string())),
        (
            "sampling_rate".into(),
            Value::Number(f64::from(domain.sampling_rate)),
        ),
        ("description".into(), Value::String(domain.description.clone())),
    ])
}

/// One `HostSink` as a record — the measured verdict, per sink.
pub(crate) fn sink_value(sink: &HostSink) -> Value {
    Value::Object(vec![
        ("domain".into(), Value::String(sink.domain.clone())),
        ("path".into(), Value::String(sink.path.clone())),
        ("readable".into(), Value::Bool(sink.readable)),
        (
            "reason".into(),
            Value::String(if sink.readable {
                String::new()
            } else {
                sink.measured_reason.clone()
            }),
        ),
    ])
}

/// The contract's `StringMap` as records, so a pipe can address a pair.
fn pairs_value(pairs: &[(String, String)]) -> Value {
    Value::List(
        pairs
            .iter()
            .map(|(key, value)| {
                Value::Object(vec![
                    ("key".into(), Value::String(key.clone())),
                    ("value".into(), Value::String(value.clone())),
                ])
            })
            .collect(),
    )
}

/// Re-export for the shell and main: the clap shape.
pub use self::{LoggingAction as Action, LoggingCmd as Cmd};
