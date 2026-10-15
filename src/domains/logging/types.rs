//! Owned value types the **logging** domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen `logging.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam.
//!
//! ## The severity vocabulary is the contract's own
//!
//! [`Level`] mirrors `T.LogLevel` one-for-one, including the `custom` outlet for
//! a level nobody has named. There is deliberately no `unknown` severity: a
//! backend that cannot tell what a line's severity was reports it as `custom`
//! and carries the raw token in the entry's metadata, rather than guessing
//! `info` — a guessed severity is a filter that silently drops the truth.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::{EventId, SubsystemId};
use crate::core::values::Timestamp;
use crate::schema::{logging_capnp, types_capnp};

/// The contract's severity vocabulary (`T.LogLevel`), as an enum in Rust so the
/// eleven levels are distinct typed values the compiler cannot collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// The most verbose level.
    Trace,
    /// Debug-level detail.
    Debug,
    /// Informational (the usual default).
    Info,
    /// A warning.
    Warn,
    /// An error.
    Error,
    /// A fatal condition.
    Fatal,
    /// Silent: nothing is emitted.
    Silent,
    /// A crash.
    Crash,
    /// A security-relevant event.
    Security,
    /// An audit-relevant event.
    Audit,
    /// A level this schema was frozen before (the open outlet).
    Custom,
}

impl Level {
    /// The contract ordinal (the value that never moves).
    pub const fn ordinal(self) -> u16 {
        match self {
            Level::Trace => 0,
            Level::Debug => 1,
            Level::Info => 2,
            Level::Warn => 3,
            Level::Error => 4,
            Level::Fatal => 5,
            Level::Silent => 6,
            Level::Crash => 7,
            Level::Security => 8,
            Level::Audit => 9,
            Level::Custom => 10,
        }
    }

    /// The open token the CLI renders.
    pub const fn as_token(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Fatal => "fatal",
            Level::Silent => "silent",
            Level::Crash => "crash",
            Level::Security => "security",
            Level::Audit => "audit",
            Level::Custom => "custom",
        }
    }

    /// Parse a token. An unrecognised token maps to `Custom` — unknown-tolerant,
    /// never a guessed severity.
    pub fn from_token(token: &str) -> Self {
        match token {
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "info" => Level::Info,
            "warn" | "warning" => Level::Warn,
            "error" | "err" => Level::Error,
            "fatal" | "crit" | "critical" => Level::Fatal,
            "silent" => Level::Silent,
            "crash" | "emerg" | "emergency" => Level::Crash,
            "security" => Level::Security,
            "audit" | "notice" => Level::Audit,
            _ => Level::Custom,
        }
    }

    /// Write into the generated `LogLevel` field.
    pub fn to_generated(self) -> types_capnp::LogLevel {
        use types_capnp::LogLevel as G;
        match self {
            Level::Trace => G::Trace,
            Level::Debug => G::Debug,
            Level::Info => G::Info,
            Level::Warn => G::Warn,
            Level::Error => G::Error,
            Level::Fatal => G::Fatal,
            Level::Silent => G::Silent,
            Level::Crash => G::Crash,
            Level::Security => G::Security,
            Level::Audit => G::Audit,
            Level::Custom => G::Custom,
        }
    }

    /// Read out of the generated `LogLevel`.
    pub fn from_generated(level: types_capnp::LogLevel) -> Self {
        use types_capnp::LogLevel as G;
        match level {
            G::Trace => Level::Trace,
            G::Debug => Level::Debug,
            G::Info => Level::Info,
            G::Warn => Level::Warn,
            G::Error => Level::Error,
            G::Fatal => Level::Fatal,
            G::Silent => Level::Silent,
            G::Crash => Level::Crash,
            G::Security => Level::Security,
            G::Audit => Level::Audit,
            G::Custom => Level::Custom,
        }
    }
}

impl Default for Level {
    /// The neutral severity: `info`, which is what a caller gets when it states
    /// no minimum. Never `silent`, which would hide everything by default.
    fn default() -> Self {
        Level::Info
    }
}

/// Which way an entry went (`LogOutcome`) — the contract's own refusal to
/// swallow the outcome of `log @0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogOutcome {
    /// The entry was accepted and retained.
    Accepted,
    /// Accepted by the sampling policy and not retained (declared, so a producer
    /// can reason about it).
    Sampled,
    /// Not retained for another reason; `reason` says which.
    Dropped(&'static str),
}

impl LogOutcome {
    /// The open token the CLI renders.
    pub const fn as_token(self) -> &'static str {
        match self {
            LogOutcome::Accepted => "accepted",
            LogOutcome::Sampled => "sampled",
            LogOutcome::Dropped(_) => "dropped",
        }
    }
}

/// One retained log entry (`LogEntry`).
///
/// `sequence` is the contract's own gap-detection lever: a consumer that pulls
/// 1, 2, 4 knows an entry was dropped. Two producers' sequences are NOT
/// comparable — correlate on (domain, sequence), never on sequence alone.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// The entry id.
    pub id: EventId,
    /// Monotonic within this producer's boot epoch.
    pub sequence: u64,
    /// When it was recorded.
    pub timestamp: Timestamp,
    /// Its severity.
    pub level: Level,
    /// The domain that produced it (empty means the producer's own).
    pub domain: String,
    /// The message.
    pub message: String,
    /// Opaque source location: file/line for a compiled language, nothing for a
    /// JIT. Backend-decided, never parsed.
    pub source: Vec<u8>,
    /// The producing subsystem (zero when the producer is not a kernel one).
    pub subsystem: SubsystemId,
    /// Open key/value metadata — where a backend records what it could not put
    /// in a typed field (the raw severity token of a host line, for instance).
    pub metadata: Vec<(String, String)>,
    /// Correlation across services.
    pub correlation_id: String,
    /// Distributed-trace correlation.
    pub trace_id: String,
    /// The span this entry belongs to.
    pub span_id: String,
    /// The parent span.
    pub parent_span_id: String,
}

impl LogEntry {
    /// Write into a generated `LogEntry` builder.
    pub fn write_to(&self, builder: &mut logging_capnp::log_entry::Builder<'_>) {
        builder.reborrow().init_id().set_id(self.id.get());
        builder.set_sequence(self.sequence);
        builder.reborrow().init_timestamp().set_nanoseconds(self.timestamp.as_nanos());
        builder.set_level(self.level.to_generated());
        builder.set_domain(&self.domain);
        builder.set_message(&self.message);
        if !self.source.is_empty() {
            builder.set_source(&self.source);
        }
        builder.reborrow().init_subsystem().set_id(self.subsystem.get());
        let mut map = builder.reborrow().init_metadata();
        let mut entries = map.reborrow().init_entries(self.metadata.len() as u32);
        for (i, (key, value)) in self.metadata.iter().enumerate() {
            let mut entry = entries.reborrow().get(i as u32);
            entry.set_key(key);
            entry.set_value(value);
        }
        builder.set_correlation_id(&self.correlation_id);
        builder.set_trace_id(&self.trace_id);
        builder.set_span_id(&self.span_id);
        builder.set_parent_span_id(&self.parent_span_id);
    }

    /// Read from a generated `LogEntry` reader.
    pub fn from_reader(reader: &logging_capnp::log_entry::Reader<'_>) -> Self {
        let mut metadata = Vec::new();
        if let Ok(map) = reader.get_metadata() {
            if let Ok(entries) = map.get_entries() {
                for entry in entries {
                    metadata.push((text_field(entry.get_key()), text_field(entry.get_value())));
                }
            }
        }
        LogEntry {
            id: EventId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
            sequence: reader.get_sequence(),
            timestamp: reader
                .get_timestamp()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            level: reader
                .get_level()
                .map(Level::from_generated)
                .unwrap_or(Level::Custom),
            domain: text_field(reader.get_domain()),
            message: text_field(reader.get_message()),
            source: reader.get_source().map(|d| d.to_vec()).unwrap_or_default(),
            subsystem: SubsystemId::new(reader.get_subsystem().map(|s| s.get_id()).unwrap_or(0)),
            metadata,
            correlation_id: text_field(reader.get_correlation_id()),
            trace_id: text_field(reader.get_trace_id()),
            span_id: text_field(reader.get_span_id()),
            parent_span_id: text_field(reader.get_parent_span_id()),
        }
    }
}

/// A bounded query over retained entries (`LogFilter`).
///
/// Every field is an AND of the ones that are set: an empty `domain` means all
/// domains, a zero `minLevel` means the most verbose bound, and a zero
/// `afterSequence` means no lower bound (the gap-detection lever).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    /// Empty = all domains.
    pub domain: String,
    /// Entries at or above this level only.
    pub min_level: Level,
    /// Entries at or after this instant (0 = unbounded below).
    pub since: Timestamp,
    /// Entries before this instant (0 = unbounded above).
    pub until: Timestamp,
    /// Plain substring match on the message. Not a query language.
    pub pattern: String,
    /// Restrict to these subsystems (empty = all).
    pub subsystems: Vec<u64>,
    /// Only entries from this trace.
    pub trace_id: String,
    /// Only entries after this sequence (0 = no lower bound).
    pub after_sequence: u64,
}

/// A zeroed filter: everything matches, and the level bound is the contract's
/// own reading of an absent `minLevel` — the enumerant at ordinal 0, `trace`
/// (the most verbose). NOT `Level::default()`'s `info`: that would silently
/// drop every debug entry a caller never said it wanted dropped.
impl Default for LogFilter {
    fn default() -> Self {
        LogFilter {
            domain: String::new(),
            min_level: Level::Trace,
            since: Timestamp::UNIX_EPOCH,
            until: Timestamp::UNIX_EPOCH,
            pattern: String::new(),
            subsystems: Vec::new(),
            trace_id: String::new(),
            after_sequence: 0,
        }
    }
}

impl LogFilter {
    /// The filter's own rule: does this entry pass?
    pub fn matches(&self, entry: &LogEntry) -> bool {
        if !self.domain.is_empty() && entry.domain != self.domain {
            return false;
        }
        if entry.level < self.min_level {
            return false;
        }
        if self.since.as_nanos() != 0 && entry.timestamp.as_nanos() < self.since.as_nanos() {
            return false;
        }
        if self.until.as_nanos() != 0 && entry.timestamp.as_nanos() > self.until.as_nanos() {
            return false;
        }
        if !self.pattern.is_empty() && !entry.message.contains(&self.pattern) {
            return false;
        }
        if !self.subsystems.is_empty() && !self.subsystems.contains(&entry.subsystem.get()) {
            return false;
        }
        if !self.trace_id.is_empty() && entry.trace_id != self.trace_id {
            return false;
        }
        if self.after_sequence != 0 && entry.sequence <= self.after_sequence {
            return false;
        }
        true
    }

    /// Read from a generated `LogFilter` reader.
    pub fn from_reader(reader: &logging_capnp::log_filter::Reader<'_>) -> Self {
        let subsystems = reader
            .get_subsystems()
            .map(|list| list.iter().map(|s| s.get_id()).collect())
            .unwrap_or_default();
        LogFilter {
            domain: text_field(reader.get_domain()),
            min_level: reader
                .get_min_level()
                .map(Level::from_generated)
                .unwrap_or(Level::Trace),
            since: reader
                .get_since()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            until: reader
                .get_until()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            pattern: text_field(reader.get_pattern()),
            subsystems,
            trace_id: text_field(reader.get_trace_id()),
            after_sequence: reader.get_after_sequence(),
        }
    }
}

/// One domain's configured state (`LogDomain`), including the sampling rate so a
/// consumer can see what it is missing.
#[derive(Clone, Debug, PartialEq)]
pub struct LogDomain {
    /// The domain name.
    pub name: String,
    /// Its configured level.
    pub level: Level,
    /// A description.
    pub description: String,
    /// The configured sampling rate (1.0 = everything is retained).
    pub sampling_rate: f32,
}

impl LogDomain {
    /// Write into a generated `LogDomain` builder.
    pub fn write_to(&self, builder: &mut logging_capnp::log_domain::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_level(self.level.to_generated());
        builder.set_description(&self.description);
        builder.set_sampling_rate(self.sampling_rate);
    }
}

/// An opaque subscription handle, minted by `subscribe @2`. Holding it IS the
/// authority to pull (the contract's own rule: no subscription id is presented).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Pagination, mirroring `T.Pagination`: `limit == 0` means "one", never "all".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// Where the page starts.
    pub offset: u32,
    /// How many entries the caller will take.
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule.
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A one-page window over an unbounded list, with the stable `total`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The entries this page carries.
    pub items: Vec<T>,
    /// The total the caller may page through.
    pub total: u64,
}

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty —
/// the same equivalence the writers emit, so a round-trip stays total).
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

#[allow(dead_code)]
fn assert_generated_types_linked() {
    // Naming (not constructing) the generated types proves the domain feature
    // compiled the logging schema.
    let _: Option<logging_capnp::log_entry::Owned> = None;
}
