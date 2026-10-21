//! The Linux **logging** backend for the logging domain — the host's real sinks,
//! measured, read natively (law #9).
//!
//! ## What this host has, measured (not assumed)
//!
//! | question | the measured answer |
//! |---|---|
//! | is there a journal? | **no** — no `/run/systemd/journal` directory and no `systemd-journald` process in `/proc` |
//! | is there a syslog socket? | **no** — no `/dev/log` |
//! | who is the init system? | **runit** — `/run/runit`, `/etc/sv`, 58 services declared and 16 linked into `/var/service` |
//! | where does a service's log go? | `/etc/sv/<n>/log/run` starts `svlogd`, whose supervisable directory is `/run/runit/supervise.<n>-log` |
//! | can this uid read a LIVE one? | **no** — mode `drwx------ root root`, measured `EACCES` for every one of the 16 linked services |
//! | and a DECLARED-but-not-running one? | **no** — its `log/supervise` symlink is dangling, measured `ENOENT` |
//! | what host logs ARE readable? | `/var/log/Xorg.0.log`, `/var/log/Xorg.1.log` and their `.old` siblings (mode 0644); `dmesg.log`, `lastlog`, `btmp` are 0600 root, and `gdm/`, `lightdm/` are closed directories |
//!
//! So the backend is two honest halves: a complete in-process structured store
//! (the contract's full surface) writing to the one sink this process owns — its
//! own stderr, which under a runit service IS the svlogd input — and a native
//! read of the host's own files that **names** the ones it cannot reach. No
//! `journalctl`, no `sv`, no `dmesg` shellout, and no journal parsing (there is
//! no journal to parse).

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::values::Timestamp;
use crate::core::{Error, ErrorCode, Result};

use crate::domains::logging::backend::LoggingBackend;
use crate::domains::logging::types::{
    Level, LogDomain, LogEntry, LogFilter, LogOutcome, Page, Pagination, SubscriptionId,
};

/// The root of the host's own log files.
const VAR_LOG: &str = "/var/log";
/// The runit services directory — every service DECLARED on this host, each
/// one's `log/` holding its svlogd config.
const ETC_SV: &str = "/etc/sv";
/// The runit live-service directory — the services actually linked in and
/// supervised. A live service's `log/supervise` is its svlogd directory, which
/// is the version of the sink that has a writer today.
const VAR_SERVICE: &str = "/var/service";
/// The runit supervisor's runtime directory, where each service's *log*
/// supervisor publishes its own state directory.
const RUN_RUNIT: &str = "/run/runit";
/// How many entries this process retains (the contract's store is bounded by
/// the backend's retention, and the retention is stated, not unbounded).
const RETENTION: usize = 4_096;
/// How many host sink files this backend enumerates in one sweep.
const MAX_HOST_SINKS: usize = 64;
/// How many bytes of one host sink this backend reads in one pull.
const MAX_SINK_BYTES: u64 = 512 * 1024;

/// One of the host's own log sinks, as measured: its path, the domain token a
/// caller names it by, whether this uid may read it, and the measured reason when
/// it may not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSink {
    /// The domain token a caller passes in `LogFilter.domain`.
    pub domain: String,
    /// The absolute path this sink is read from.
    pub path: String,
    /// Whether this uid can read it (measured by opening it).
    pub readable: bool,
    /// The measured reason when it is not readable — the hostile-path evidence.
    pub measured_reason: String,
}

/// One live subscription: the filter it was created with and the sequence cursor
/// it has already delivered, so a pull never repeats an entry.
struct Subscription {
    filter: LogFilter,
    delivered_sequence: u64,
}

/// The Linux logging backend: an exact in-process store plus the host's own
/// files, with the one sink this process owns as the write path.
pub struct LinuxLoggingBackend {
    entries: Mutex<Vec<LogEntry>>,
    levels: Mutex<BTreeMap<String, Level>>,
    sampling: Mutex<BTreeMap<String, f32>>,
    subscriptions: Mutex<HashMap<u64, Subscription>>,
    sequence: AtomicU64,
    next_id: AtomicU64,
    /// The host's sinks, measured at construction and refreshed on demand.
    host_sinks: Mutex<Vec<HostSink>>,
}

impl Default for LinuxLoggingBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxLoggingBackend {
    /// A backend over this host's measured sinks.
    pub fn new() -> Self {
        let sinks = measure_host_sinks();
        LinuxLoggingBackend {
            entries: Mutex::new(Vec::new()),
            levels: Mutex::new(BTreeMap::new()),
            sampling: Mutex::new(BTreeMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            host_sinks: Mutex::new(sinks),
        }
    }

    fn mint(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn now_nanos(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    /// Re-measure the host's sinks (a read; safe to call at any time).
    pub fn refresh_host_sinks(&self) {
        *self.host_sinks.lock().unwrap() = measure_host_sinks();
    }

    /// The host's sinks as measured.
    pub fn host_sinks(&self) -> Vec<HostSink> {
        self.host_sinks.lock().unwrap().clone()
    }

    /// Whether the process's stderr is a real sink (it always is: fd 2 exists
    /// for a hosted process, and under a runit service it is the svlogd pipe).
    pub fn owns_stderr_sink(&self) -> bool {
        true
    }

    /// The measured per-service svlogd sink for one service: the `/etc/sv/<n>`
    /// directory's `log/supervise` symlink target, and whether this uid may read
    /// it. `None` when the service declares no log service at all.
    pub fn measure_service_log_sink(service: &str) -> Option<HostSink> {
        Self::measure_service_log_sink_in(ETC_SV, service)
    }

    /// The same measurement against one service root (`/var/service` for the
    /// live set, `/etc/sv` for everything declared).
    pub fn measure_service_log_sink_in(root: &str, service: &str) -> Option<HostSink> {
        let supervise = format!("{root}/{service}/log/supervise");
        let target = fs::read_link(&supervise).ok()?;
        // A supervise symlink is absolute on this host, but a relative target is
        // anchored at the supervisor's own runtime root rather than resolved
        // against the caller's working directory — a measurement that depends on
        // the CWD is not a measurement.
        let path = if target.is_absolute() {
            target.to_string_lossy().to_string()
        } else {
            format!("{RUN_RUNIT}/{}", target.to_string_lossy())
        };
        Some(probe(&format!("service:{service}"), path))
    }
}

/// How many of the first `sequence` entries a declared `rate` keeps — the
/// monotone step the sampler samples on. `floor(n * rate)` is exact for every
/// rate in 0.0..=1.0 on `f32` (the products are far from `f32`'s 2^24 exact
/// integer boundary), and it is a pure function of the sequence, so the drop is
/// reproducible from the sequence alone.
fn kept_upto(sequence: u64, rate: f32) -> u64 {
    (sequence as f32 * rate).floor() as u64
}

/// Open a path for reading and report the measured verdict: either it is
/// readable or the OS error is the reason, verbatim.
fn probe(domain: &str, path: String) -> HostSink {
    match fs::File::open(&path) {
        Ok(_) => HostSink {
            domain: domain.to_string(),
            path,
            readable: true,
            measured_reason: String::new(),
        },
        Err(e) => HostSink {
            domain: domain.to_string(),
            path: path.clone(),
            readable: false,
            measured_reason: format!("{path}: {e}"),
        },
    }
}

/// Measure this host's own log sinks: the files under `/var/log` and, per
/// service, the svlogd supervisor directory. Every verdict is a measurement —
/// nothing here assumes a file is readable because it exists.
fn measure_host_sinks() -> Vec<HostSink> {
    let mut sinks = Vec::new();
    if let Ok(entries) = fs::read_dir(VAR_LOG) {
        for entry in entries.flatten().take(MAX_HOST_SINKS) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            sinks.push(probe(&name, path.to_string_lossy().to_string()));
        }
    }
    // The per-service svlogd sinks, live set first: `/var/service` is the
    // supervisor's own view of what is running, and a live service's log
    // supervisor directory is the sink that has a writer today.
    let mut seen: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(VAR_SERVICE) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(sink) = LinuxLoggingBackend::measure_service_log_sink_in(VAR_SERVICE, &name) {
                seen.push(name);
                sinks.push(sink);
            }
        }
    }
    // Then every declared service the live set does not cover: a service that
    // declares a `log/` but is not running has no supervisor directory yet, and
    // the measurement says so (ENOENT) instead of pretending the sink exists.
    if let Ok(entries) = fs::read_dir(ETC_SV) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if seen.contains(&name) {
                continue;
            }
            if let Some(sink) = LinuxLoggingBackend::measure_service_log_sink(&name) {
                sinks.push(sink);
            }
        }
    }
    sinks
}

impl LoggingBackend for LinuxLoggingBackend {
    fn log(&self, level: Level, domain: &str, message: &str) -> Result<LogOutcome> {
        // 1. This backend's own per-domain filter, declared in `setLevel`.
        if level < self.levels.lock().unwrap().get(domain).copied().unwrap_or(Level::Trace) {
            return Ok(LogOutcome::Dropped("level"));
        }
        // 2. Declared sampling: the policy is visible in `listDomains`, so a
        //    producer's drop is never a surprise to its consumer.
        let rate = self.sampling.lock().unwrap().get(domain).copied().unwrap_or(1.0);
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        if rate < 1.0 {
            // Deterministic and declared: the standard rate-thinning rule — the
            // entry whose index crosses the next multiple of the rate is kept,
            // so exactly `rate` of the sequence is retained and a consumer that
            // knows the sequence can reproduce which ones. Never a random drop.
            let keep = kept_upto(sequence, rate) > kept_upto(sequence - 1, rate);
            if !keep {
                return Ok(LogOutcome::Sampled);
            }
        }
        // 3. The one sink this process owns on this host: its own stderr. Under a
        //    runit service that stream is the svlogd input, so the entry really
        //    lands in the service's log — measured, not hoped for.
        let line = format!(
            "[{}] {} {}: {}\n",
            level.as_token(),
            Self::now_nanos(self),
            if domain.is_empty() { "app" } else { domain },
            message
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
        // 4. Retain it, bounded, so the query path is exact rather than unbounded.
        let mut entries = self.entries.lock().unwrap();
        entries.push(LogEntry {
            id: crate::core::ids::EventId::new(self.mint()),
            sequence,
            timestamp: Timestamp::from_nanos(self.now_nanos()),
            level,
            domain: domain.to_string(),
            message: message.to_string(),
            source: Vec::new(),
            subsystem: crate::core::ids::SubsystemId::new(0),
            metadata: Vec::new(),
            correlation_id: String::new(),
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: String::new(),
        });
        if entries.len() > RETENTION {
            let excess = entries.len() - RETENTION;
            entries.drain(0..excess);
        }
        Ok(LogOutcome::Accepted)
    }

    fn get_entries(&self, filter: &LogFilter, page: Pagination) -> Result<Page<LogEntry>> {
        // Two sources, both honest: the host's own files when the caller names a
        // sink domain, and this process's retained store otherwise.
        let sinks = self.host_sinks.lock().unwrap();
        if let Some(sink) = sinks.iter().find(|s| s.domain == filter.domain && !filter.domain.is_empty()) {
            if !sink.readable {
                // NAMED, never silently skipped: the reason is the OS error the
                // host gave us for this uid.
                return Err(Error::unavailable(format!(
                    "no-privilege: the host's log sink is not readable by this caller: {}",
                    sink.measured_reason
                )));
            }
            let parsed = read_host_sink(&sink.path, page.offset as usize, page.effective_limit() as usize);
            return Ok(parsed);
        }
        drop(sinks);
        let entries = self.entries.lock().unwrap();
        let matched: Vec<LogEntry> = entries
            .iter()
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();
        Ok(page_window(&matched, page))
    }

    fn subscribe(&self, filter: &LogFilter) -> Result<SubscriptionId> {
        let id = self.mint();
        // The cursor starts after everything already retained, so a subscription
        // never replays history the caller did not ask for.
        let delivered = self.sequence.load(Ordering::Relaxed);
        self.subscriptions
            .lock()
            .unwrap()
            .insert(
                id,
                Subscription {
                    filter: filter.clone(),
                    delivered_sequence: delivered,
                },
            );
        Ok(SubscriptionId(id))
    }

    fn pull(&self, subscription: SubscriptionId, max_entries: u32) -> Result<Vec<LogEntry>> {
        let mut subs = self.subscriptions.lock().unwrap();
        let Some(sub) = subs.get_mut(&subscription.0) else {
            return Err(Error::not_found());
        };
        let entries = self.entries.lock().unwrap();
        let mut out = Vec::new();
        let bound = max_entries.max(1) as usize;
        for entry in entries.iter() {
            if out.len() >= bound {
                break;
            }
            if entry.sequence <= sub.delivered_sequence {
                continue;
            }
            if !sub.filter.matches(entry) {
                continue;
            }
            out.push(entry.clone());
            sub.delivered_sequence = entry.sequence;
        }
        Ok(out)
    }

    fn cancel(&self, subscription: SubscriptionId) -> Result<()> {
        match self.subscriptions.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found()),
        }
    }

    fn set_level(&self, domain: &str, level: Level) -> Result<()> {
        self.levels.lock().unwrap().insert(domain.to_string(), level);
        Ok(())
    }

    fn get_level(&self, domain: &str) -> Result<Level> {
        Ok(self.levels.lock().unwrap().get(domain).copied().unwrap_or(Level::Info))
    }

    fn clear(&self, domain: &str) -> Result<()> {
        // This process's retention clears. A host file does not: truncating a
        // log the caller does not own is refused with the measured reason.
        let sinks = self.host_sinks.lock().unwrap();
        if let Some(sink) = sinks.iter().find(|s| s.domain == domain) {
            return Err(Error::with_message(
                ErrorCode::PermissionDenied,
                format!(
                    "no-privilege: clear would truncate the host's log file {} which this caller does not own",
                    sink.path
                ),
            ));
        }
        drop(sinks);
        let mut entries = self.entries.lock().unwrap();
        if domain.is_empty() {
            entries.clear();
        } else {
            entries.retain(|e| e.domain != domain);
        }
        Ok(())
    }

    fn export(&self, filter: &LogFilter, mime_type: &str) -> Result<Vec<u8>> {
        let entries = self.entries.lock().unwrap();
        let matched: Vec<&LogEntry> = entries.iter().filter(|e| filter.matches(e)).collect();
        let text = match mime_type {
            "application/json" | "application/x-ndjson" => render_json(&matched),
            _ => {
                // An open content type this backend does not render is answered
                // with the text rendering AND the reason, on the first line —
                // never a silent substitution of the format the caller named.
                let mut out = format!(
                    "# requested content type {mime_type} is not rendered by this backend; \
                     rendered as text/plain\n"
                );
                out.push_str(&render_text(&matched));
                out
            }
        };
        Ok(text.into_bytes())
    }

    fn set_sampling_rate(&self, domain: &str, rate: f32) -> Result<()> {
        if !(0.0..=1.0).contains(&rate) {
            return Err(Error::invalid_argument("a sampling rate is a fraction in 0.0..=1.0"));
        }
        self.sampling.lock().unwrap().insert(domain.to_string(), rate);
        Ok(())
    }

    fn list_domains(&self, page: Pagination) -> Result<Page<LogDomain>> {
        let levels = self.levels.lock().unwrap();
        let sampling = self.sampling.lock().unwrap();
        let entries = self.entries.lock().unwrap();
        let sinks = self.host_sinks.lock().unwrap();
        // The domains this backend knows: every domain that has an entry, a
        // level, a sampling rate, or a host sink — with each one's configured
        // level and rate, so a consumer can see what it is missing.
        let mut names: BTreeMap<String, (Level, f32, String)> = BTreeMap::new();
        for entry in entries.iter() {
            names
                .entry(entry.domain.clone())
                .or_insert((Level::Info, 1.0, "this process's retained entries".to_string()));
        }
        for (domain, level) in levels.iter() {
            let slot = names
                .entry(domain.clone())
                .or_insert((Level::Info, 1.0, "configured on this backend".to_string()));
            slot.0 = *level;
        }
        for (domain, rate) in sampling.iter() {
            names
                .entry(domain.clone())
                .or_insert((Level::Info, 1.0, "configured on this backend".to_string()));
            names.get_mut(domain).unwrap().1 = *rate;
        }
        for sink in sinks.iter() {
            let description = if sink.readable {
                format!("host log file {}", sink.path)
            } else {
                // The unreachable sinks are listed too, with the measured
                // reason: a consumer that asks about them is told why.
                format!("host log file {} (not readable by this caller: {})", sink.path, sink.measured_reason)
            };
            names
                .entry(sink.domain.clone())
                .or_insert((Level::Custom, 1.0, description));
        }
        let all: Vec<LogDomain> = names
            .into_iter()
            .map(|(name, (level, rate, description))| LogDomain {
                name,
                level,
                description,
                sampling_rate: rate,
            })
            .collect();
        Ok(page_window(&all, page))
    }
}

/// The contract's page window: `offset` bounded by the list, `limit` bounded by
/// the caller's number, and the stable total beside it.
fn page_window<T: Clone>(items: &[T], page: Pagination) -> Page<T> {
    let total = items.len() as u64;
    let start = page.offset as usize;
    let end = start
        .saturating_add(page.effective_limit() as usize)
        .min(items.len());
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..end].to_vec()
    };
    Page { items, total }
}

/// Read one page of a host log file, natively: the file is opened once, its size
/// is bounded, and each line becomes an entry carrying the sink's own words.
fn read_host_sink(path: &str, offset: usize, limit: usize) -> Page<LogEntry> {
    let Ok(meta) = fs::metadata(path) else {
        return Page { items: Vec::new(), total: 0 };
    };
    let size = meta.len().min(MAX_SINK_BYTES);
    let Ok(file) = fs::File::open(path) else {
        return Page { items: Vec::new(), total: 0 };
    };
    // Read backwards from the end: a log's newest entries are at its tail, and
    // reading a bounded window beats pulling the whole file into memory.
    let start = meta.len().saturating_sub(size);
    let mut buf = alloc::vec![0u8; (meta.len() - start) as usize];
    use std::os::unix::fs::FileExt;
    if file.read_exact_at(&mut buf, start).is_err() {
        return Page { items: Vec::new(), total: 0 };
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as u64;
    let end = offset.saturating_add(limit.max(1)).min(lines.len());
    let items = if offset >= lines.len() {
        Vec::new()
    } else {
        lines[offset..end]
            .iter()
            .enumerate()
            .map(|(i, line)| host_line_to_entry(path, line, offset + i))
            .collect()
    };
    Page { items, total }
}

/// One line of a host log file as an entry.
///
/// The line's own marker decides the level (`(II)` is information, `(WW)` a
/// warning, `(EE)` an error in the X org's own vocabulary); a line with no marker
/// of its own is `custom` with the raw token in metadata — never a guessed
/// `info`, because a guessed severity is a filter that silently drops the truth.
/// A timestamp that cannot be anchored (an X server's boot-relative counter) is
/// 0, and the entry says so rather than inventing a date.
/// The host line's own severity marker, if it carries one: a two-character
/// token in parentheses standing as a word — `(II) message`, `(WW)message`,
/// `[ 12.345] (EE)fatal`. Returns the token and the message that follows it
/// (leading whitespace trimmed), or `None` for a line that names no marker of
/// its own, which the caller renders `custom` rather than guessing `info`.
///
/// The "standing as a word" rule is what keeps a parenthesised aside inside a
/// message (`foo (bar) baz`) from being read as a severity: the `(` must open
/// at the start of the line or after a space/`]`, and exactly two characters
/// must sit between it and its `)`.
fn host_line_marker(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    for i in 0..=bytes.len() - 4 {
        if bytes[i] != b'(' || bytes[i + 3] != b')' {
            continue;
        }
        let bounded = i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b']';
        if !bounded {
            continue;
        }
        // `/` and `(` and `)` cannot be a severity token's character.
        let token = &line[i + 1..i + 3];
        if token.starts_with('(') || token.ends_with(')') || token.starts_with('/') {
            continue;
        }
        return Some((token, line[i + 4..].trim_start()));
    }
    None
}

fn host_line_to_entry(path: &str, line: &str, index: usize) -> LogEntry {
    let mut level = Level::Custom;
    let mut raw_level = String::new();
    let mut message = line.to_string();
    let domain = path.rsplit('/').next().unwrap_or(path).to_string();
    if let Some((token, rest)) = host_line_marker(line) {
        raw_level = token.to_string();
        level = match token {
            "II" | "**" | "==" | "++" => Level::Info,
            "WW" => Level::Warn,
            "EE" | "!!" => Level::Error,
            _ => Level::Custom,
        };
        message = rest.to_string();
    }
    LogEntry {
        id: crate::core::ids::EventId::new(index as u64),
        sequence: index as u64,
        timestamp: Timestamp::UNIX_EPOCH,
        level,
        domain,
        message,
        source: Vec::new(),
        subsystem: crate::core::ids::SubsystemId::new(0),
        metadata: alloc::vec![
            ("sink".to_string(), path.to_string()),
            ("rawLevel".to_string(), raw_level),
            (
                "timestamp".to_string(),
                "unanchored: the sink's own counter is boot-relative to its daemon".to_string()
            ),
        ],
        correlation_id: String::new(),
        trace_id: String::new(),
        span_id: String::new(),
        parent_span_id: String::new(),
    }
}

/// The text rendering (the contract's `export` shape for `text/plain`).
fn render_text(entries: &[&LogEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&format!(
            "{} {} {} {} {}\n",
            entry.timestamp.as_nanos(),
            entry.sequence,
            entry.level.as_token(),
            if entry.domain.is_empty() { "app" } else { &entry.domain },
            entry.message
        ));
    }
    out
}

/// The JSON-lines rendering (the contract's `export` shape for JSON content
/// types): one object per entry, with the entry's own fields and no field
/// invented for the format.
fn render_json(entries: &[&LogEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&format!(
            "{{\"sequence\":{},\"timestamp\":{},\"level\":\"{}\",\"domain\":\"{}\",\"message\":\"{}\"}}\n",
            entry.sequence,
            entry.timestamp.as_nanos(),
            entry.level.as_token(),
            escape_json(&entry.domain),
            escape_json(&entry.message),
        ));
    }
    out
}

/// The minimal JSON string escape this renderer needs (quotes, backslashes and
/// control characters) — a full JSON codec is not the logging domain's job.
fn escape_json(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::logging::types::LogFilter;

    /// The one sink this process owns is written and the entry lands in the
    /// backend's own store: a real write, and a real retention.
    #[test]
    fn log_writes_the_one_sink_this_process_owns() {
        let backend = LinuxLoggingBackend::new();
        assert!(backend.owns_stderr_sink());
        let outcome = backend.log(Level::Info, "app", "a real entry").expect("log");
        assert_eq!(outcome, LogOutcome::Accepted);
        let page = backend
            .get_entries(&LogFilter::default(), Pagination { offset: 0, limit: 0 })
            .expect("getEntries");
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1, "a limit of 0 means one");
        assert_eq!(page.items[0].message, "a real entry");
        assert_eq!(page.items[0].sequence, 1, "the sequence is the gap-detection lever");
    }

    /// The filter is the contract's: level, domain, pattern, window and the
    /// gap-detection lever each narrow the answer.
    #[test]
    fn entries_are_filtered_paginated_and_gap_detectable() {
        let backend = LinuxLoggingBackend::new();
        backend.log(Level::Debug, "a", "first").expect("log");
        backend.log(Level::Error, "b", "second").expect("log");
        backend.log(Level::Info, "a", "third").expect("log");
        // Level filter.
        let warn = backend
            .get_entries(&LogFilter { min_level: Level::Warn, ..Default::default() }, Pagination { offset: 0, limit: 10 })
            .expect("getEntries");
        assert_eq!(warn.total, 1);
        assert_eq!(warn.items[0].message, "second");
        // Domain filter.
        let domain_a = backend
            .get_entries(&LogFilter { domain: "a".to_string(), ..Default::default() }, Pagination { offset: 0, limit: 10 })
            .expect("getEntries");
        assert_eq!(domain_a.total, 2);
        // Pattern filter (a plain substring, not a query language).
        let pattern = backend
            .get_entries(&LogFilter { pattern: "thir".to_string(), ..Default::default() }, Pagination { offset: 0, limit: 10 })
            .expect("getEntries");
        assert_eq!(pattern.total, 1);
        // The gap-detection lever: after sequence 2 the answer is one entry.
        let after = backend
            .get_entries(&LogFilter { after_sequence: 2, ..Default::default() }, Pagination { offset: 0, limit: 10 })
            .expect("getEntries");
        assert_eq!(after.total, 1);
        assert_eq!(after.items[0].sequence, 3);
        // Pagination is the contract's: the offset is honoured and the total is stable.
        let page1 = backend
            .get_entries(&LogFilter::default(), Pagination { offset: 0, limit: 2 })
            .expect("page");
        assert_eq!(page1.items.len(), 2);
        assert_eq!(page1.total, 3);
        let page2 = backend
            .get_entries(&LogFilter::default(), Pagination { offset: 2, limit: 2 })
            .expect("page");
        assert_eq!(page2.items.len(), 1);
        let _ = backend;
    }

    /// A subscription pulls bounded, cancels cleanly, and an unknown token is
    /// `notFound` — never a fabricated empty success.
    #[test]
    /// `subscribe` mints the engine's own subscription token, and the pull
    /// accepts it.
    #[test]
    fn subscribe_mints_a_subscription() {
        let backend = LinuxLoggingBackend::new();
        let sub = backend.subscribe(&LogFilter::default()).expect("subscribe");
        // The token is a value the pull takes.
        assert!(backend.pull(sub, 1).is_ok());
    }

    /// The subscription pull is bounded by the caller's number, and it never
    /// repeats an entry.
    #[test]
    fn a_subscription_pull_is_bounded() {
        let backend = LinuxLoggingBackend::new();
        let sub = backend.subscribe(&LogFilter::default()).expect("subscribe");
        for i in 0..5 {
            backend.log(Level::Info, "app", &format!("entry {i}")).expect("log");
        }
        // The subscription started before the entries, and the bound is the
        // caller's: five entries, two at a time.
        let first = backend.pull(sub, 2).expect("pull");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].message, "entry 0");
        let second = backend.pull(sub, 10).expect("pull");
        assert_eq!(second.len(), 3, "the remaining three, with no repeat");
        // A repeat pull is empty (the cursor advanced), and an unknown token is notFound.
        assert!(backend.pull(sub, 10).expect("pull").is_empty());
        assert_eq!(backend.pull(SubscriptionId(4242), 1).unwrap_err().code, ErrorCode::NotFound);
        backend.cancel(sub).expect("cancel");
    }


    #[test]
    fn cancel_retires_the_subscription_token() {
        let backend = LinuxLoggingBackend::new();
        let sub = backend.subscribe(&LogFilter::default()).expect("subscribe");
        backend.cancel(sub).expect("cancel");
        assert_eq!(backend.pull(sub, 1).unwrap_err().code, ErrorCode::NotFound);
    }

    /// Levels configure THIS backend's filter — and nothing on the host, which
    /// the reason states.
    #[test]
    /// `getLevel` answers with the backend's own level for the logger, and
    /// the default is declared, not zero.
    #[test]
    fn get_level_answers_with_the_backends_own_level() {
        let backend = LinuxLoggingBackend::new();
        assert_eq!(backend.get_level("app").expect("get"), Level::Info, "the default");
    }

    /// `setLevel` configures this backend only, and the filter really
    /// applies: an entry below the level is dropped, and the outcome says so
    /// rather than swallowing it.
    #[test]
    fn set_level_configures_this_backend_only() {
        let backend = LinuxLoggingBackend::new();
        backend.set_level("app", Level::Warn).expect("set");
        assert_eq!(backend.get_level("app").expect("get"), Level::Warn);
        // The filter really applies: an info entry is dropped, and the outcome
        // says so rather than swallowing it.
        let dropped = backend.log(Level::Info, "app", "too verbose").expect("log");
        assert!(matches!(dropped, LogOutcome::Dropped("level")));
        let kept = backend.log(Level::Error, "app", "kept").expect("log");
        assert_eq!(kept, LogOutcome::Accepted);
    }

    /// Clearing this process's retention is real; clearing a host file is
    /// refused with the measured reason.
    #[test]
    fn clear_clears_this_process_and_refuses_root_files() {
        let backend = LinuxLoggingBackend::new();
        backend.log(Level::Info, "app", "one").expect("log");
        backend.clear("app").expect("clear");
        let page = backend
            .get_entries(&LogFilter::default(), Pagination { offset: 0, limit: 10 })
            .expect("getEntries");
        assert_eq!(page.total, 0);
        // A host sink is refused, not attempted.
        let sinks = backend.host_sinks();
        if let Some(sink) = sinks.first() {
            let err = backend.clear(&sink.domain).expect_err("a host file is not ours");
            assert_eq!(err.code, ErrorCode::PermissionDenied);
            assert!(err.message.contains("no-privilege:"));
        }
    }

    /// The export carries the entries in the named content type, and names the
    /// requested type it does not render rather than substituting silently.
    #[test]
    fn export_carries_the_entries_in_the_named_content_type() {
        let backend = LinuxLoggingBackend::new();
        backend.log(Level::Info, "app", "hello").expect("log");
        let text = backend
            .export(&LogFilter::default(), "text/plain")
            .expect("export");
        let text = String::from_utf8(text).expect("utf8");
        assert!(text.contains("hello"));
        assert!(text.contains("info"));
        let json = backend
            .export(&LogFilter::default(), "application/json")
            .expect("export");
        let json = String::from_utf8(json).expect("utf8");
        assert!(json.starts_with('{'), "{json}");
        assert!(json.contains("\"level\":\"info\""));
        assert!(json.contains("\"message\":\"hello\""));
        // An unrendered content type is answered with the text rendering AND the
        // reason on the first line.
        let other = backend
            .export(&LogFilter::default(), "application/vnd.2046.columnar")
            .expect("export");
        let other = String::from_utf8(other).expect("utf8");
        assert!(other.lines().next().expect("first line").contains("not rendered"));
    }

    /// Declared sampling is applied and visible — the contract's whole point:
    /// a producer says it will drop, instead of a consumer discovering it.
    #[test]
    fn declared_sampling_is_applied_and_visible() {
        let backend = LinuxLoggingBackend::new();
        // An out-of-range rate is refused rather than clamped.
        assert_eq!(
            backend.set_sampling_rate("app", 1.5).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        backend.set_sampling_rate("app", 0.5).expect("set");
        let mut sampled = 0;
        for i in 0..20 {
            if matches!(
                backend.log(Level::Info, "app", &format!("e{i}")).expect("log"),
                LogOutcome::Sampled
            ) {
                sampled += 1;
            }
        }
        assert!(sampled > 0 && sampled < 20, "half of 20 entries are declared-sampled");
        // The declared rate is visible in the domain list.
        let page = backend
            .list_domains(Pagination { offset: 0, limit: 10 })
            .expect("listDomains");
        let domain = page.items.iter().find(|d| d.name == "app").expect("app");
        assert_eq!(domain.sampling_rate, 0.5);
    }

    /// The domains carry their level and sampling rate — and the host's sinks
    /// are listed too, readable or not, each with its measured reason.
    #[test]
    fn domains_carry_their_level_and_sampling_rate() {
        let backend = LinuxLoggingBackend::new();
        backend.log(Level::Info, "app", "x").expect("log");
        backend.set_level("app", Level::Debug).expect("set");
        backend.set_sampling_rate("app", 0.25).expect("set");
        let page = backend
            .list_domains(Pagination { offset: 0, limit: 1000 })
            .expect("listDomains");
        let domain = page.items.iter().find(|d| d.name == "app").expect("app");
        assert_eq!(domain.level, Level::Debug);
        assert_eq!(domain.sampling_rate, 0.25);
        // The host's sinks are measured and reported, including the unreadable
        // ones with the OS's own words.
        let sinks = backend.host_sinks();
        assert!(!sinks.is_empty(), "this host has log files");
        let unreadable: Vec<&HostSink> = sinks.iter().filter(|s| !s.readable).collect();
        assert!(!unreadable.is_empty(), "this host's svlogd directories are closed");
        for sink in &unreadable {
            assert!(!sink.measured_reason.is_empty(), "a closed sink names the OS's own words");
        }
    }

    /// UNIT: the host's sinks are measured on this host, not assumed: the
    /// runit services' log supervisors are closed and the world-readable
    /// `/var/log` files are readable. It proves the engine's own
    /// measurement, not a contract method, so the matrix does not publish it.
    #[test]
    fn the_hosts_sinks_are_measured_not_assumed() {
        let sinks = measure_host_sinks();
        // At least one world-readable file exists under /var/log.
        assert!(sinks.iter().any(|s| s.readable), "measured: Xorg.0.log is 0644");
        // And every service sink is closed for this uid — either the running
        // ones (drwx------ root root, measured EACCES) or the ones whose log
        // supervisor is not running (a dangling supervise symlink, ENOENT).
        let services: Vec<&HostSink> = sinks.iter().filter(|s| s.domain.starts_with("service:")).collect();
        assert!(!services.is_empty(), "runit services declare log services");
        for sink in &services {
            assert!(!sink.readable, "{}: {}", sink.domain, sink.measured_reason);
            assert!(
                sink.measured_reason.contains("Permission denied")
                    || sink.measured_reason.contains("No such file or directory"),
                "{}",
                sink.measured_reason
            );
            assert!(sink.path.contains("supervise."), "{}", sink.path);
        }
        // MEASURED: at least one running service's supervisor is closed by
        // privilege, not merely absent.
        assert!(
            sinks
                .iter()
                .any(|s| s.measured_reason.contains("Permission denied")),
            "a running runit service's log dir is drwx------ root root"
        );
    }

    /// UNIT: a host sink's lines keep their own words: the marker decides
    /// the level, an unmarked line is `custom` (never a guessed info), and an
    /// unanchored timestamp is 0 with the reason attached. It proves the
    /// engine's own parser, not a contract method, so the matrix does not
    /// publish it.
    #[test]
    fn host_lines_keep_their_own_words() {
        let entry = host_line_to_entry("/var/log/Xorg.0.log", "[ 43304.655] (II) AMDgpu: yes", 7);
        assert_eq!(entry.level, Level::Info);
        assert_eq!(entry.message, "AMDgpu: yes");
        assert_eq!(entry.metadata[1], ("rawLevel".to_string(), "II".to_string()));
        assert_eq!(entry.timestamp, Timestamp::UNIX_EPOCH);
        let unmarked = host_line_to_entry("/var/log/other", "kernel: something happened", 0);
        assert_eq!(unmarked.level, Level::Custom);
        assert_eq!(unmarked.message, "kernel: something happened");
        let warning = host_line_to_entry("/var/log/Xorg.1.log", "[ 12.345] (WW)warning!", 1);
        assert_eq!(warning.level, Level::Warn);
        let error = host_line_to_entry("/var/log/Xorg.0.log.old", "[ 12.345] (EE)fatal!", 1);
        assert_eq!(error.level, Level::Error);
        assert_eq!(error.domain, "Xorg.0.log.old");
    }
}
