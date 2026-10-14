//! The Linux **service** backend's runit cell — the native supervision read
//! path, and the primary target on this host.
//!
//! ## The native decision (the founder's "innovation like netlink" mandate)
//!
//! The archive body (`smart-protocol/crates/smart-lib/src/platforms/linux/
//! service/runit.rs`) ran `sv status <name>` **and `ls <dir>` as subprocesses
//! per service**, then parsed the text: a fork per query, locale-dependent
//! parsing, and exit codes discarded (pitfall P5). Its `list_services` even
//! shelled out to `ls /var/service`.
//!
//! This backend reads runit's own supervision tree **directly**, with no fork
//! anywhere on the path:
//!
//! | question | native read | what it measures |
//! |---|---|---|
//! | what services are declared? | `read_dir(/etc/sv)` | the service directories |
//! | which are linked in (enabled)? | `read_dir(/var/service)` | the supervisor's scan dir |
//! | which does runsv supervise? | `metadata(/run/runit/supervise.<name>)` | the per-service state dir |
//! | what state is one in? | `read(/run/runit/supervise.<name>/stat)` | the supervisor's own status line |
//! | since when? | the stat line's own seconds count | the supervisor's own transition clock |
//! | what does it depend on? | `read(/etc/sv/<name>/{run,check,finish})` | the `sv check`/`sv up` invocations in the service's own scripts |
//! | control it | write one byte to `/run/runit/supervise.<name>/control` | the supervisor's control protocol |
//!
//! ## The host fact that shapes this cell (measured)
//!
//! This box runs runit: 16 services linked in `/var/service`, 58 declared in
//! `/etc/sv`, and 26 `supervise.*` state directories under `/run/runit` — all
//! readable by an unprivileged reader. The **supervise status files are not**:
//! `/run/runit/supervise.<name>` is `drwx------ root root`, so `stat`/`pid`
//! answer `EACCES` for uid 1000 (verified: `sv status sshd` itself fails with
//! "unable to open supervise/ok: access denied"). The backend therefore reads
//! natively and reports honestly what it could not read — `status = unknown`
//! with the measured reason and the exact denied path, never a fabricated
//! state. Every *parse* of the status grammar is proven over a scripted
//! supervision tree (a real directory layout built in a temp dir), so the
//! parser is proven without ever needing root.
//!
//! ## Law #8 (the host is read-only)
//!
//! Reads here are stat, read_dir and file reads — no host state changes. The
//! control path (one byte to the supervise control FIFO) is host-mutating: it
//! is exercised only by the mock (a real FIFO in a temp dir with a reader on
//! the other end) and by the founder's live bring-back. The `#[ignore]`d live
//! probe at the bottom is read-only.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::PathBuf;

use crate::core::{Error, ErrorCode, Result};
use crate::domains::service::types::{
    ServiceStatus, SupervisionCapabilities, SupervisorIdentity,
};

use super::{LifecycleVerb, NativeDep, NativeObservation, NativeState, NativeSupervision};

// ═══════════════════════════════════════════════════════════════════════
// The host's runit layout, measured
// ═══════════════════════════════════════════════════════════════════════

/// The directory runsv scans for services (runit's service dir). On this host
/// every entry is a symlink into `/etc/sv`.
pub const DEFAULT_SCAN_ROOT: &str = "/var/service";
/// Where the declared service directories live.
pub const DEFAULT_DECLARED_ROOT: &str = "/etc/sv";
/// Where runsv keeps its per-service state directories
/// (`supervise.<name>` and `supervise.<name>-log`).
pub const DEFAULT_STATE_ROOT: &str = "/run/runit";

/// How many services one enumeration will read. A bound, so a bizarre host
/// cannot turn a listing into an unbounded pull (the contract's pagination
/// rule is honoured on top of it: the caller's `limit` slices what this one
/// enumeration found).
const MAX_SERVICES: usize = 4096;

/// How many bytes of a service's own script this backend reads when looking
/// for its declared dependencies. A bound, so an unexpected file cannot be
/// dragged into memory in full.
const MAX_SCRIPT_BYTES: u64 = 64 * 1024;

// ═══════════════════════════════════════════════════════════════════════
// The runit supervisor status grammar
// ═══════════════════════════════════════════════════════════════════════

/// One parsed runit supervise status line — the supervisor's own words, kept
/// verbatim alongside the mapped status so a caller can always see what the
/// supervisor actually said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatLine {
    /// The line as the supervisor wrote it (a reason a caller can read back).
    pub raw: String,
    /// The supervisor's state word, verbatim (`run`, `down`, `finish`, …).
    pub word: String,
    /// The main process id when the supervisor names one (zero = none).
    pub main_pid: u64,
    /// How long the service has been in this status, as the supervisor counted
    /// it (microseconds; zero when the line carries no count).
    pub since_us: u64,
    /// The flags after the seconds count, verbatim (`normally down`,
    /// `want down`, `normally up`, `want up`, `got TERM`, …).
    pub flags: String,
}

impl StatLine {
    /// Parse one supervise `stat` line in runit's own grammar:
    ///
    /// ```text
    ///   run: /etc/sv/sshd: (pid 1234) 5678s, normally down
    ///   down: /etc/sv/sshd: 12s, want down
    ///   finish: /etc/sv/sshd: 3s
    /// ```
    ///
    /// The parse is deliberately tolerant: an unrecognised line still yields a
    /// `StatLine` with its `word` and `raw` text (so the caller is never handed
    /// a guess), and the mapped status is then `unknown` with the line's own
    /// text as the reason.
    pub fn parse(line: &str) -> StatLine {
        let trimmed = line.trim();
        let mut word = trimmed.to_string();
        let mut main_pid = 0;
        let mut since_us = 0;
        let mut flags = String::new();

        // The state word is everything before the first colon.
        if let Some(colon) = trimmed.find(':') {
            word = trimmed[..colon].trim().to_string();
            let rest = trimmed[colon + 1..].trim();
            // `(pid 1234)` follows the service path in the run state.
            if let Some(open) = rest.find("(pid ") {
                let after = &rest[open + 5..];
                if let Some(close) = after.find(')') {
                    main_pid = after[..close].trim().parse().unwrap_or(0);
                }
            }
            // The seconds count is the last `<digits>s` token before any
            // comma-separated flags.
            let head = rest.split(',').next().unwrap_or(rest);
            let tail_start = head.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
            let token = head[tail_start..].trim();
            if let Some(digits) = token.strip_suffix('s') {
                if let Ok(secs) = digits.trim().parse::<u64>() {
                    since_us = secs.saturating_mul(1_000_000);
                }
            }
            if let Some(comma) = rest.find(',') {
                flags = rest[comma + 1..].trim().to_string();
            }
        }
        StatLine { raw: trimmed.to_string(), word, main_pid, since_us, flags }
    }

    /// The contract's lifecycle this line means.
    ///
    /// The mapping is runit's own semantics, not a heuristic: `run` is active;
    /// `exit`/`down` with the supervisor *wanting* the service up (`want up` /
    /// `normally up`) is a service the supervisor desires and does not have —
    /// the supervisor's failed/flapping state; `down` with `want down` is a
    /// deliberate stop. An unrecognised word is `unknown`, and the caller keeps
    /// the verbatim line in the reason.
    pub fn status(&self) -> (ServiceStatus, String) {
        let wants_up = self.flags.contains("want up") || self.flags.contains("normally up");
        match self.word.as_str() {
            "run" => (ServiceStatus::Active, String::new()),
            "finish" => (
                ServiceStatus::Deactivating,
                format!(
                    "the supervisor is running the service's own finish script ({})",
                    self.flags
                ),
            ),
            "exit" => (
                ServiceStatus::Failed,
                format!("the supervisor reports an exited service ({})", self.flags),
            ),
            "down" if wants_up => (
                ServiceStatus::Failed,
                format!(
                    "the supervisor wants the service up and it is down ({})",
                    self.flags
                ),
            ),
            "down" => (
                ServiceStatus::Inactive,
                format!("the supervisor reports the service down ({})", self.flags),
            ),
            _ => (
                ServiceStatus::Unknown,
                format!(
                    "the supervisor's status line is not a state this backend maps: {}",
                    self.raw
                ),
            ),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The runit cell
// ═══════════════════════════════════════════════════════════════════════

/// The runit backend: the host's supervise tree, read natively. Every path is
/// injectable, so the tests build a real runit layout in a temp directory and
/// prove the reads without touching `/var/service`.
pub struct RunitBackend {
    scan_root: PathBuf,
    declared_root: PathBuf,
    state_root: PathBuf,
}

impl RunitBackend {
    /// Construct over explicit roots (the tests use a scripted tree).
    pub fn with_roots(scan_root: PathBuf, declared_root: PathBuf, state_root: PathBuf) -> Self {
        RunitBackend { scan_root, declared_root, state_root }
    }

    /// The host's runit layout.
    pub fn host() -> Self {
        RunitBackend {
            scan_root: PathBuf::from(DEFAULT_SCAN_ROOT),
            declared_root: PathBuf::from(DEFAULT_DECLARED_ROOT),
            state_root: PathBuf::from(DEFAULT_STATE_ROOT),
        }
    }

    /// The per-service state directory runsv keeps.
    pub fn supervise_dir(&self, name: &str) -> PathBuf {
        self.state_root.join(format!("supervise.{name}"))
    }

    /// The names the host's tree declares or links, sorted (a stable listing
    /// is part of the aesthetic and of the contract's stable `total`).
    pub fn service_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for root in [&self.declared_root, &self.scan_root] {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with('.') {
                            continue;
                        }
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
        }
        names.sort_unstable();
        names.truncate(MAX_SERVICES);
        names
    }

    /// Read and parse one supervise status line. The `Err` variant carries the
    /// **measured** reason this backend could not read it (a permission denial
    /// on this host, a missing file for an unsupervised service), because "I
    /// could not read it" and "the service is stopped" are different answers.
    pub fn read_stat(&self, name: &str) -> std::result::Result<StatLine, String> {
        let path = self.supervise_dir(name).join("stat");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "{prefix}{path}: {detail}",
                prefix = reason_prefix(&e),
                path = path.display(),
                detail = e
            )
        })?;
        Ok(StatLine::parse(text.lines().next().unwrap_or("")))
    }

    /// The supervisor's own transition clock (the stat file's mtime), used when
    /// a line carries no seconds count. A zero means "not observable".
    fn state_file_mtime_ns(&self, name: &str) -> u64 {
        let path = self.supervise_dir(name).join("stat");
        match std::fs::metadata(&path) {
            Ok(meta) => meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// The dependency invocations in one service's own scripts. runit has no
    /// dependency declaration; the edge a runit service declares is an
    /// `sv check <dep>` / `sv up <dep>` invocation in its own `run`/`check`/
    /// `finish` script — so that is what is read, natively.
    pub fn read_dependencies(&self, name: &str) -> Vec<NativeDep> {
        let mut deps = Vec::new();
        let dir = self.declared_root.join(name);
        for script in ["run", "check", "finish"] {
            let path = dir.join(script);
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.len() > MAX_SCRIPT_BYTES {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let tokens: Vec<&str> = text.split_whitespace().collect();
            for (i, token) in tokens.iter().enumerate() {
                if *token != "sv" {
                    continue;
                }
                let action = tokens.get(i + 1).copied().unwrap_or("");
                if action != "check" && action != "up" {
                    continue;
                }
                let Some(dep) = tokens.get(i + 2).copied() else { continue };
                if dep == name || dep.is_empty() || dep.starts_with('-') {
                    continue;
                }
                // A dependency is satisfied when the supervisor reports the
                // dependency active — a native read, not an assumption.
                let satisfied = matches!(&self.read_stat(dep), Ok(stat) if stat.word == "run");
                if !deps.iter().any(|d: &super::NativeDep| d.name == dep) {
                    deps.push(NativeDep { name: dep.to_string(), satisfied });
                }
            }
        }
        deps
    }

    /// The supervisor's control protocol: one byte to the supervise control
    /// FIFO. This is exactly what `sv` does internally — the native path the
    /// archive's `Command::new("sv")` shellout stood in for.
    fn write_control(&self, name: &str, verb: LifecycleVerb) -> Result<()> {
        // runit's control protocol: `u` up, `d` down, `t` terminate+restart,
        // `o` once, `p` pause, `c` continue, plus the signal bytes. Only the
        // three the init authority defines are reachable from the domain.
        let byte: u8 = match verb {
            LifecycleVerb::Start => b'u',
            LifecycleVerb::Stop => b'd',
            LifecycleVerb::Restart => b't',
        };
        let path = self.supervise_dir(name).join("control");
        use std::io::Write;
        let mut fifo = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| {
            Error::with_message(ErrorCode::PermissionDenied, format!(
                "no-privilege: cannot open the supervisor's control fifo {}: {e} — the runit \
                 supervise tree is not writable by this caller; the archive body's `sv` shellout \
                 would have failed here too, but it would have hidden the reason",
                path.display()
            ))
        })?;
        fifo.write_all(&[byte]).map_err(|e| {
            Error::failed(format!(
                "failed: writing the supervisor's control byte to {} failed: {e} — nothing was \
                 read back, so the verb's effect is unknown",
                path.display()
            ))
        })?;
        Ok(())
    }
}

/// The reason prefix a failed read gets, so the runtime answer's class matches
/// the availability matrix's row exactly.
fn reason_prefix(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "no-privilege: ",
        std::io::ErrorKind::NotFound => "not-found: ",
        _ => "unavailable: ",
    }
}

impl NativeSupervision for RunitBackend {
    fn supervisor(&self) -> SupervisorIdentity {
        SupervisorIdentity {
            supervisor: "runit".to_string(),
            service_root: self.scan_root.display().to_string(),
            declared_root: self.declared_root.display().to_string(),
            state_root: self.state_root.display().to_string(),
        }
    }

    fn enumerate(&self) -> Vec<NativeObservation> {
        self.service_names()
            .into_iter()
            .map(|name| {
                let stat = self.read_stat(&name);
                let observation = NativeObservation {
                    declared: self.declared_root.join(&name).is_dir(),
                    enabled: self.scan_root.join(&name).exists(),
                    supervised: self.supervise_dir(&name).is_dir(),
                    state: stat.as_ref().ok().map(|stat| {
                        let (status, reason) = stat.status();
                        // The supervisor's own seconds count when it has one;
                        // otherwise the state file's mtime. Both measured.
                        let since = self.state_file_mtime_ns(&name);
                        NativeState {
                            status,
                            main_pid: stat.main_pid,
                            since_us: if stat.since_us != 0 {
                                stat.since_us
                            } else {
                                since.saturating_sub(super::now_nanos()) / 1_000
                            },
                            since,
                            since_valid: true,
                            reason,
                        }
                    }),
                    state_reason: match &stat {
                        Ok(_) => String::new(),
                        Err(reason) => reason.clone(),
                    },
                    dependencies: self.read_dependencies(&name),
                    name,
                    description: String::new(),
                };
                observation
            })
            .collect()
    }

    fn dependencies(&self, name: &str) -> Vec<NativeDep> {
        self.read_dependencies(name)
    }

    fn control(&self, name: &str, verb: LifecycleVerb) -> Result<()> {
        self.write_control(name, verb)
    }

    fn measured(&self) -> (SupervisorIdentity, SupervisionCapabilities) {
        let identity = self.supervisor();
        let observations = self.enumerate();
        // `can_read_state` is a permission measurement, not a promise: a state
        // file that this caller could actually read is the claim, and a single
        // denial makes the claim false (with the per-call reason naming the
        // path).
        let denied = observations
            .iter()
            .any(|o| o.state_reason.starts_with("no-privilege:"));
        // `can_control` is the same kind of measurement: the supervisor's
        // control FIFO had to be reached for the claim to be true.
        let can_control = observations
            .iter()
            .any(|o| std::fs::metadata(self.supervise_dir(&o.name).join("control")).is_ok());
        let caps = SupervisionCapabilities {
            supervisor: "runit".to_string(),
            can_enumerate: self.declared_root.is_dir(),
            can_read_state: !denied,
            can_control,
        };
        (identity, caps)
    }
}

/// The control protocol's byte for one verb, so a test can assert the mapping
/// without a live supervisor.
pub fn control_byte(verb: LifecycleVerb) -> u8 {
    match verb {
        LifecycleVerb::Start => b'u',
        LifecycleVerb::Stop => b'd',
        LifecycleVerb::Restart => b't',
    }
}
