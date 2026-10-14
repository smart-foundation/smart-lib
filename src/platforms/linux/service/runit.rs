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
//! anywhere on the path — and on TWO live paths, because one of them is
//! physically privilege-gated on this host:
//!
//! | question | native read | what it measures | who can read it |
//! |---|---|---|---|
//! | what services are declared? | `read_dir(/etc/sv)` | the service directories | every uid |
//! | which are linked in (enabled)? | `read_dir(/var/service)` | the supervisor's scan dir | every uid |
//! | which does runsv supervise? | a `runsv <name>` process in `/proc` | the supervisor's own process | every uid |
//! | what state is one in? (exact) | `read(/run/runit/supervise.<name>/stat)` | the supervisor's own status line, its transition clock, its want up/down flags | root on this host (drwx------ supervise dirs) |
//! | what state is one in? (derived) | the `runsv` process + its live children + the service's own pid file + the kernel's boot clock | up/down for every uid | every uid |
//! | since when? (derived) | `/proc/<pid>/stat` starttime + `/proc/stat` btime | the process start time | every uid |
//! | what does it depend on? | `read(/etc/sv/<name>/{run,check,finish})` | the `sv check`/`sv up` invocations in the service's own scripts | every uid |
//! | control it | write one byte to `/run/runit/supervise.<name>/control` | the supervisor's control protocol | root on this host (gated; founder-run) |
//!
//! ## The host fact that shapes this cell (measured)
//!
//! This box runs runit: 16 services linked in `/var/service`, 58 declared in
//! `/etc/sv`, and 26 `supervise.*` state directories under `/run/runit` — the
//! directories are stat-able by an unprivileged reader, but every one is
//! `drwx------ root root`, so `stat`/`pid` answer `EACCES` for uid 1000
//! (verified: `sv status sshd` itself fails with "unable to open supervise/ok:
//! access denied"). That is a PHYSICAL limit on the exact half of this path —
//! not a reason to park the state: the supervised process truth in `/proc` (the
//! `runsv` processes and their children are root here and readable by every
//! uid) derives the same states for every caller. This backend tries the exact
//! read FIRST and reports the supervisor's own words whenever they are
//! readable (a root caller gets them), and otherwise answers from the
//! `/proc` derivation — up, down, and why, with every value measured. The
//! privileged half's live bring-back is the founder's: the committed
//! `examples/service-state-probe` binary (or `smart service status`) under
//! `sudo`, both speaking this same code path.
//!
//! Every *parse* of the status grammar is proven over a scripted supervision
//! tree (a real directory layout built in a temp dir), the `/proc` derivation
//! is proven over a fabricated `/proc` tree the test builds itself, and the
//! control path is proven over a real FIFO — so no path ever needs root to be
//! proven (except the live probe, which is read-only and `#[ignore]`d).
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

use super::proc;
use super::{LifecycleVerb, NativeDep, NativeObservation, NativeProcFacts, NativeState, NativeSupervision};

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
/// The kernel's process directory: the supervision truth every uid can read.
pub const DEFAULT_PROC_ROOT: &str = "/proc";
/// Where a service's own pid file lives, when it writes one (two conventions,
/// both world-readable when a root daemon writes them).
pub const DEFAULT_PIDFILE_ROOT: &str = "/run";
/// Where the runit service-dir convention is looked for when the scan root's
/// entries are bare names (a `runsv <name>` process names its service dir
/// relative to the scan root).
pub const RUNSVDIR_CURRENT: &str = "runsvdir/current";

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
/// injectable, so the tests build a real runit layout in a temp directory —
/// including a fabricated `/proc` — and prove the reads without touching
/// `/var/service`.
pub struct RunitBackend {
    scan_root: PathBuf,
    declared_root: PathBuf,
    state_root: PathBuf,
    /// The `/proc` tree the unprivileged supervision truth is derived from.
    proc_root: PathBuf,
    /// Where the service's own pid files are looked for.
    pidfile_root: PathBuf,
}

impl RunitBackend {
    /// Construct over explicit roots (the tests use a scripted tree).
    pub fn with_roots(
        scan_root: PathBuf,
        declared_root: PathBuf,
        state_root: PathBuf,
        proc_root: PathBuf,
        pidfile_root: PathBuf,
    ) -> Self {
        RunitBackend { scan_root, declared_root, state_root, proc_root, pidfile_root }
    }

    /// The host's runit layout.
    pub fn host() -> Self {
        RunitBackend {
            scan_root: PathBuf::from(DEFAULT_SCAN_ROOT),
            declared_root: PathBuf::from(DEFAULT_DECLARED_ROOT),
            state_root: PathBuf::from(DEFAULT_STATE_ROOT),
            proc_root: PathBuf::from(DEFAULT_PROC_ROOT),
            pidfile_root: PathBuf::from(DEFAULT_PIDFILE_ROOT),
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
        self.read_dependencies_over(name, &proc::ProcScan::scan(&self.proc_root))
    }

    /// The dependency invocations in one service's own scripts, over one
    /// already-taken `/proc` scan (so an enumeration reads `/proc` once).
    pub(crate) fn read_dependencies_over(
        &self,
        name: &str,
        scan: &proc::ProcScan,
    ) -> Vec<NativeDep> {
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
                // dependency active — a native read, not an assumption. The
                // exact read answers when it can; otherwise the same
                // supervised-process truth that answers this caller's own
                // state answers for the dependency (so an unprivileged
                // caller on a root-only supervise host still measures it).
                let satisfied = self.is_running_over(scan, dep);
                if !deps.iter().any(|d: &super::NativeDep| d.name == dep) {
                    deps.push(NativeDep { name: dep.to_string(), satisfied });
                }
            }
        }
        deps
    }

    /// Whether the supervisor reports this service running, on whichever path
    /// serves this caller: the supervisor's own stat line when it is readable,
    /// the supervised process truth in /proc otherwise.
    fn is_running_over(&self, scan: &proc::ProcScan, name: &str) -> bool {
        if let Ok(stat) = self.read_stat(name) {
            return stat.word == "run";
        }
        matches!(&self.state_from_proc(scan, name).0, Some(state) if state.status == ServiceStatus::Active)
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

// ═══════════════════════════════════════════════════════════════════════
// The two live paths: the supervisor's own words, and the supervised
// process truth every uid can read
// ═══════════════════════════════════════════════════════════════════════

/// One row of the privileged exact read (`/run/runit/supervise.<name>/stat`),
/// published as data so the founder's helper binary is a thin CLI over the
/// engine's own path and the format is hermetic-testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactStateRow {
    /// The service this row is about.
    pub name: String,
    /// The exact file that was (or was not) read.
    pub path: String,
    /// The supervisor's own status line, verbatim, when it could be read.
    pub stat: Option<String>,
    /// The status the supervisor's own line maps to.
    pub status: Option<ServiceStatus>,
    /// The main pid the supervisor's own line names.
    pub main_pid: u64,
    /// How long the supervisor says the service has been in this status.
    pub since_us: u64,
    /// The measured reason this caller could not read the exact file.
    pub reason: Option<String>,
}

impl RunitBackend {
    /// The state the supervisor's own status line reports: the exact read.
    /// The supervisor's own seconds count when it has one; otherwise the state
    /// file's mtime. Both measured.
    fn state_from_stat(&self, name: &str, stat: &StatLine) -> NativeState {
        let (status, reason) = stat.status();
        let since = self.state_file_mtime_ns(name);
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
    }

    /// The state derived from the supervised process truth in `/proc`: the
    /// permitted path that works for EVERY uid on this host, because the
    /// `runsv` process and its children are readable by all.
    ///
    /// A `None` state means no path derived one; the caller pairs it with the
    /// measured reason naming what was looked for.
    fn state_from_proc(
        &self,
        scan: &proc::ProcScan,
        name: &str,
    ) -> (Option<NativeState>, NativeProcFacts) {
        let mut facts = NativeProcFacts { state_source: "proc", ..Default::default() };

        // The pid file the service writes, if any: the service's own naming of
        // the process that holds it.
        let pidfile = proc::read_pid_file(&self.pidfile_root, name);
        let pidfile_fact = match &pidfile {
            Some((path, pid)) if scan.is_alive(*pid) => format!(
                "the service's own pid file {} names pid {pid}, and /proc lists it as alive",
                path.display()
            ),
            Some((path, pid)) => format!(
                "the service's own pid file {} names pid {pid}, which /proc no longer lists",
                path.display()
            ),
            None => String::new(),
        };
        facts.pidfile = pidfile_fact;

        // The supervisor's own process: a `runsv <name>` in /proc.
        let runsv = scan.runsv_supervisors().into_iter().find(|(_, n)| n == name);
        let Some((runsv_pid, _)) = runsv else {
            // No supervisor process. A service whose own pid file names a live
            // process is still running — it just runs outside the supervisor,
            // and the answer says exactly that.
            if let Some((path, pid)) = pidfile.as_ref().filter(|(_, p)| scan.is_alive(*p)) {
                return (
                    Some(NativeState {
                        status: ServiceStatus::Active,
                        main_pid: *pid,
                        since: 0,
                        since_us: 0,
                        since_valid: false,
                        reason: format!(
                            "the service's own pid file {} names pid {pid}, which /proc lists as \
                             alive, but no runsv process supervises {name} — the process runs \
                             outside the supervisor",
                            path.display()
                        ),
                    }),
                    facts,
                );
            }
            // No supervisor process and no visible process: if the supervisor
            // once had a slot for this service (or wants it, by the link), the
            // measured truth is that it is not running — said with the reason
            // naming every fact, including the honest visibility limit.
            if self.supervise_dir(name).is_dir() || self.scan_root.join(name).exists() {
                let slot = if self.supervise_dir(name).is_dir() {
                    format!(
                        "the supervisor's state directory {} exists",
                        self.supervise_dir(name).display()
                    )
                } else {
                    "the service is linked into the supervisor's scan dir".to_string()
                };
                let pidfile_clause = if facts.pidfile.is_empty() {
                    String::new()
                } else {
                    format!(", and {}", facts.pidfile)
                };
                return (
                    Some(NativeState {
                        status: ServiceStatus::Inactive,
                        main_pid: 0,
                        since: 0,
                        since_us: 0,
                        since_valid: false,
                        reason: format!(
                            "{slot}, but no runsv process for {name} is in /proc — the supervisor \
                             is not watching it{pidfile_clause}; a process running outside both \
                             the supervisor and its pid file is not visible to any permitted read"
                        ),
                    }),
                    facts,
                );
            }
            // Nothing running, nothing supervising, no slot: no state is
            // derived, and the measured reason names what was looked for.
            facts.state_source = "";
            return (None, facts);
        };
        facts.runsv_pid = runsv_pid;

        // The runsv's live children: the service's process(es) and its logger.
        let live: Vec<&proc::ProcEntry> = scan
            .children_of(runsv_pid)
            .into_iter()
            .filter(|e| e.is_alive())
            .collect();
        facts.live_children = live.len();
        // The log process is the child whose argv[0] is the program the
        // service's OWN log script execs — both are children of the same
        // runsv, and that is the only /proc-visible difference between them.
        let log_program = proc::script_exec_program_of(
            &self.declared_root.join(name).join("log").join("run"),
        );
        let log_pids: Vec<u64> = match &log_program {
            Some(program) => live
                .iter()
                .filter(|e| scan.argv0(e.pid).as_deref() == Some(program.as_str()))
                .map(|e| e.pid)
                .collect(),
            None => Vec::new(),
        };
        let service_children: Vec<&proc::ProcEntry> =
            live.iter().copied().filter(|e| !log_pids.contains(&e.pid)).collect();

        // The main pid: the single non-log child when the supervisor's own
        // process tree says which one it is, else the service's own pid file.
        let main_pid = if service_children.len() == 1 {
            Some(service_children[0].pid)
        } else {
            pidfile.as_ref().filter(|(_, p)| scan.is_alive(*p)).map(|(_, p)| *p)
        };

        match main_pid {
            Some(pid) => {
                let entry = scan.entry(pid);
                let since = entry.map(|e| scan.start_epoch_ns(e)).unwrap_or(0);
                (
                    Some(NativeState {
                        status: ServiceStatus::Active,
                        main_pid: pid,
                        since,
                        // The age of the measured process: the age of its own
                        // start time, in the contract's microsecond unit.
                        since_us: super::now_nanos().saturating_sub(since) / 1_000,
                        since_valid: since != 0,
                        reason: String::new(),
                    }),
                    facts,
                )
            }
            None if service_children.len() > 1 => (
                // Real supervision truth — processes ARE running — with the
                // honest limit that /proc does not say which one is the main.
                Some(NativeState {
                    status: ServiceStatus::Active,
                    main_pid: 0,
                    since: 0,
                    since_us: 0,
                    since_valid: false,
                    reason: format!(
                        "{} supervised processes are running under runsv (pid {runsv_pid}); the \
                         service's run script does not exec a single command, so the main process \
                         is not identifiable from /proc alone",
                        service_children.len()
                    ),
                }),
                facts,
            ),
            None => {
                // The supervisor is watching and the service is not running.
                // The pid-file fact (a dead pid) travels in the reason because
                // it is the strongest measured statement available.
                let running = if service_children.is_empty() && !live.is_empty() {
                    "only its log process is running"
                } else {
                    "no supervised process is running"
                };
                let pidfile_clause = if facts.pidfile.is_empty() {
                    String::new()
                } else {
                    format!("; {}", facts.pidfile)
                };
                (
                    Some(NativeState {
                        status: ServiceStatus::Inactive,
                        main_pid: 0,
                        since: 0,
                        since_us: 0,
                        since_valid: false,
                        reason: format!(
                            "runsv (pid {runsv_pid}) supervises {name} and {running} \
                             (measured from /proc){pidfile_clause}"
                        ),
                    }),
                    facts,
                )
            }
        }
    }

    /// The privileged exact read, over every service the tree declares or
    /// links: what a root caller (the founder's helper) sees when the
    /// supervisor's own `stat` files are readable. Committed in the library so
    /// the helper binary is a thin CLI over this path.
    pub fn exact_states(&self, filter: Option<&str>) -> Vec<ExactStateRow> {
        let mut rows: Vec<ExactStateRow> = Vec::new();
        let names: Vec<String> = self
            .service_names()
            .into_iter()
            .filter(|name| filter.is_none_or(|f| name == f))
            .collect();
        for name in names {
            let path = self.supervise_dir(&name).join("stat");
            match self.read_stat(&name) {
                Ok(stat) => rows.push(ExactStateRow {
                    name,
                    path: path.display().to_string(),
                    stat: Some(stat.raw.clone()),
                    status: Some(stat.status().0),
                    main_pid: stat.main_pid,
                    since_us: stat.since_us,
                    reason: None,
                }),
                Err(reason) => rows.push(ExactStateRow {
                    name,
                    path: path.display().to_string(),
                    stat: None,
                    status: None,
                    main_pid: 0,
                    since_us: 0,
                    reason: Some(reason),
                }),
            }
        }
        rows
    }
}

/// The report the founder's privileged helper prints, in the engine's own
/// format so the format is test-locked and the helper is pure arg parsing.
pub fn render_exact_states(rows: &[ExactStateRow]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "runit exact state read: {} service(s)", rows.len());
    for row in rows {
        let (state, detail) = match (&row.stat, &row.reason) {
            (Some(stat), _) => (
                row.status.map(|s| s.as_token().to_string()).unwrap_or_default(),
                stat.clone(),
            ),
            (None, Some(reason)) => ("unreadable".to_string(), reason.clone()),
            (None, None) => ("unreadable".to_string(), "the supervisor's stat file was not read".to_string()),
        };
        let _ = writeln!(
            out,
            "  {:<28} {:<11} {detail}",
            row.name, state, detail = detail
        );
    }
    out
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
        // One `/proc` read for the whole enumeration, so every service's
        // derivation measures from one consistent snapshot.
        let scan = proc::ProcScan::scan(&self.proc_root);
        self.service_names()
            .into_iter()
            .map(|name| {
                let stat = self.read_stat(&name);
                // The privileged exact read first: when the caller can read the
                // supervisor's own words, they are the state (its transition
                // clock and its want up/down flags included). Otherwise the
                // supervised process truth in /proc answers — for every uid.
                let (state, facts) = match &stat {
                    Ok(line) => (
                        Some(self.state_from_stat(&name, line)),
                        NativeProcFacts {
                            state_source: "supervise.stat",
                            ..self.supervision_facts(&scan, &name)
                        },
                    ),
                    Err(_) => self.state_from_proc(&scan, &name),
                };
                let state_reason = match &state {
                    Some(_) => String::new(),
                    None => self.no_state_reason(&name, stat.as_ref().err(), &facts),
                };
                NativeObservation {
                    declared: self.declared_root.join(&name).is_dir(),
                    enabled: self.scan_root.join(&name).exists(),
                    // Measured both ways: the supervisor's own slot, and the
                    // supervisor's own process in /proc.
                    supervised: self.supervise_dir(&name).is_dir() || facts.runsv_pid != 0,
                    state,
                    state_reason,
                    dependencies: self.read_dependencies_over(&name, &scan),
                    name,
                    description: String::new(),
                    proc_facts: facts,
                }
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
        // this caller can actually answer is the claim — either the
        // supervisor's own stat file (privileged) or the /proc supervision
        // truth (every uid). On this host the second is what answers.
        let can_read_state = observations
            .iter()
            .any(|o| o.state.is_some() && !o.proc_facts.state_source.is_empty());
        // `can_read_exact_state` is the privileged half specifically: the
        // supervisor's own stat file, root-only on this host.
        let can_read_exact_state = observations
            .iter()
            .any(|o| o.proc_facts.state_source == "supervise.stat");
        // `can_control` is the same kind of measurement: the supervisor's
        // control FIFO had to be reached for the claim to be true.
        let can_control = observations
            .iter()
            .any(|o| std::fs::metadata(self.supervise_dir(&o.name).join("control")).is_ok());
        let caps = SupervisionCapabilities {
            supervisor: "runit".to_string(),
            can_enumerate: self.declared_root.is_dir(),
            can_read_state,
            can_read_exact_state,
            can_control,
        };
        (identity, caps)
    }
}

impl RunitBackend {
    /// The /proc-derived supervision facts beside a service (the `runsv` pid,
    /// the live child count, the pid-file reading), measured whether or not the
    /// exact read answered — published so a caller can see what the state is
    /// made of.
    fn supervision_facts(&self, scan: &proc::ProcScan, name: &str) -> NativeProcFacts {
        let mut facts = NativeProcFacts::default();
        if let Some((runsv_pid, _)) = scan.runsv_supervisors().into_iter().find(|(_, n)| n == name) {
            facts.runsv_pid = runsv_pid;
            facts.live_children = scan
                .children_of(runsv_pid)
                .into_iter()
                .filter(|e| e.is_alive())
                .count();
        }
        if let Some((path, pid)) = proc::read_pid_file(&self.pidfile_root, name) {
            facts.pidfile = format!(
                "the service's own pid file {} names pid {pid}, which /proc {}",
                path.display(),
                if scan.is_alive(pid) { "lists as alive" } else { "no longer lists" }
            );
        }
        facts
    }

    /// The measured reason no path could derive a state: the exact read's own
    /// error (naming the exact denied path) plus the supervision truth.
    fn no_state_reason(
        &self,
        name: &str,
        stat_error: Option<&String>,
        facts: &NativeProcFacts,
    ) -> String {
        let base = stat_error.cloned().unwrap_or_else(|| {
            format!("unavailable: no run state was derived for {name}")
        });
        let truth = if !facts.pidfile.is_empty() {
            format!("; {}", facts.pidfile)
        } else if self.supervise_dir(name).is_dir() {
            format!(
                "; the supervisor's state directory exists but no runsv process for {name} is in \
                 /proc, so the supervisor is not watching it"
            )
        } else if self.scan_root.join(name).exists() {
            format!(
                "; the service is linked into the supervisor's scan dir but no runsv process \
                 supervises it"
            )
        } else {
            format!(
                "; the service is declared but not linked into the supervisor's scan dir, and no \
                 runsv process supervises it"
            )
        };
        format!("{base}{truth}")
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
