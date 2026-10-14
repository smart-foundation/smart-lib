//! The **/proc-derived supervision truth** — the supervision read path that
//! works for EVERY uid on a runit host, root or not.
//!
//! ## Why this module exists (the physical fact that shapes the runit cell)
//!
//! runit keeps a service's exact state in `/run/runit/supervise.<name>/stat` —
//! the supervisor's own words (`run: … (pid 1234) 5678s, normally up`). On this
//! host every `supervise.*` directory is `drwx------ root root`, so uid 1000
//! cannot read a single `stat` file (`sv status sshd` itself answers "unable to
//! open supervise/ok: access denied"). That is a **physical limit**, not a
//! reason to leave the state unknown.
//!
//! What every uid CAN read is the supervised process truth: a runit service's
//! `run` script executes under its own `runsv` process (the runsv processes are
//! root here, but `/proc` shows every process to every uid unless a `hidepid`
//! mount hides it), and the service's processes are the *children of that
//! runsv*. So the live state of a supervised service is derivable for every
//! uid:
//!
//! | question | /proc answer | who can read it |
//! |---|---|---|
//! | is a supervisor watching this service? | a `runsv <name>` process exists | every uid |
//! | is the service's process running? | runsv has a live child that is not its log process | every uid |
//! | which process holds it? | the single non-log child, or the service's own pid file | every uid |
//! | since when? | the process start time (stat field 22 + `/proc/stat` btime) | every uid |
//! | is it down? | runsv exists, no live service child | every uid |
//!
//! This module is that read, in one place, with every value measured from
//! `/proc` and no fabrication anywhere: a pid that `/proc` no longer lists is
//! reported as a pid `/proc` no longer lists, never as a running process.
//!
//! ## Law #8 (the host is read-only)
//!
//! Everything here is `read_dir` + `read` of `/proc` — the kernel's own
//! introspection directory. Nothing is written, no signal is sent, no process is
//! inspected that the kernel does not already publish world-readably.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::{Path, PathBuf};

/// The kernel's own process directory.
pub const DEFAULT_PROC_ROOT: &str = "/proc";

/// The kernel's fixed user-visible clock tick (`USER_HZ`), the unit of
/// `/proc/<pid>/stat`'s `starttime` and `/proc/uptime`. It is a kernel ABI
/// constant — every Linux arch exposes 100 — so a process's start time is
/// `btime + starttime / USER_HZ` without asking libc (the library links no C
/// dependency; `libc` is a dev-dependency only in this workspace).
pub const USER_HZ: u64 = 100;

/// A bound on one scan, so a host with a pathological process count cannot
/// turn a state read into an unbounded pull (the same bound discipline the
/// service domain applies to every listing).
const MAX_PROC_ENTRIES: usize = 65_536;

/// How many bytes of a service's own script this module reads when looking for
/// the program it `exec`s (its log script names its log process, which is how
/// the service's own process is told from its logger's).
const MAX_SCRIPT_BYTES: u64 = 8 * 1024;

/// One process as `/proc` publishes it: the fields a supervision read needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcEntry {
    /// The process id.
    pub pid: u64,
    /// The executable name (`comm`), without the kernel's parentheses.
    pub comm: String,
    /// The scheduler state letter (`R`, `S`, `D`, `Z`, `T`, …).
    pub state: char,
    /// The parent process id.
    pub ppid: u64,
    /// The process start time, in clock ticks since boot (`USER_HZ` units).
    pub starttime_ticks: u64,
}

impl ProcEntry {
    /// Whether the process is a live one: a zombie or a dead entry is a
    /// process that has exited and not been reaped or is being torn down — the
    /// supervisor's child that is *not running*.
    pub fn is_alive(&self) -> bool {
        !matches!(self.state, 'Z' | 'X')
    }
}

/// One bounded, consistent read of a `/proc` tree: every numeric entry's stat
/// fields, plus the boot clock from `/proc/stat`'s `btime`.
pub struct ProcScan {
    root: PathBuf,
    entries: Vec<ProcEntry>,
    /// The kernel's boot time, in epoch SECONDS (the unit `btime` writes).
    boot_epoch_s: u64,
}

impl ProcScan {
    /// Read one `/proc` tree. A missing or unreadable root yields an empty
    /// scan (not a fabricated one) — the caller answers honestly with it.
    pub fn scan(root: &Path) -> ProcScan {
        let mut entries: Vec<ProcEntry> = Vec::new();
        if let Ok(read) = std::fs::read_dir(root) {
            for entry in read.flatten() {
                if entries.len() >= MAX_PROC_ENTRIES {
                    break;
                }
                let Some(name) = entry.file_name().to_str().map(ToString::to_string) else {
                    continue;
                };
                // Only numeric entries are processes; the kernel's other procfs
                // files (`uptime`, `stat`, `meminfo`, …) are read by name.
                let Ok(pid) = name.parse::<u64>() else { continue };
                let Some(entry) = read_entry(root, pid) else { continue };
                entries.push(entry);
            }
        }
        ProcScan { root: root.to_path_buf(), entries, boot_epoch_s: read_btime(root) }
    }

    /// The host's `/proc` (the live one).
    pub fn host() -> ProcScan {
        ProcScan::scan(Path::new(DEFAULT_PROC_ROOT))
    }

    /// The root this scan read.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// One process, when `/proc` lists it.
    pub fn entry(&self, pid: u64) -> Option<&ProcEntry> {
        self.entries.iter().find(|e| e.pid == pid)
    }

    /// Whether `/proc` lists this pid and it is a live one.
    pub fn is_alive(&self, pid: u64) -> bool {
        self.entry(pid).is_some_and(ProcEntry::is_alive)
    }

    /// The live children of one parent, in pid order (a stable listing is part
    /// of the aesthetic and of reproducible answers).
    pub fn children_of(&self, ppid: u64) -> Vec<&ProcEntry> {
        let mut kids: Vec<&ProcEntry> =
            self.entries.iter().filter(|e| e.ppid == ppid).collect();
        kids.sort_unstable_by_key(|e| e.pid);
        kids
    }

    /// The first argument of one process's command line (`argv[0]`), which is
    /// the executable the service's own script names when it `exec`s it.
    pub fn argv0(&self, pid: u64) -> Option<String> {
        let text = std::fs::read(self.root.join(pid.to_string()).join("cmdline")).ok()?;
        let args = cmdline_args(&text);
        args.first().map(|a| program_name(a))
    }

    /// Every `runsv` supervisor process this tree shows, with the service name
    /// it supervises taken from its own command line (`runsv <name>`).
    ///
    /// The name is the supervisor's own word: on this host the arg is bare
    /// (`runsv sshd`); on a host that runs `runsv /etc/sv/<name>` it is a path
    /// and the basename is the service. An arg that names nothing (a bare
    /// `runsv`) is not a supervision relationship and is skipped.
    pub fn runsv_supervisors(&self) -> Vec<(u64, String)> {
        let mut out: Vec<(u64, String)> = Vec::new();
        for entry in self.entries.iter().filter(|e| e.comm == "runsv") {
            if let Some(name) = self.runsv_service_name(entry.pid) {
                out.push((entry.pid, name));
            }
        }
        out
    }

    /// The service name one `runsv` command line supervises.
    fn runsv_service_name(&self, pid: u64) -> Option<String> {
        let text = std::fs::read(self.root.join(pid.to_string()).join("cmdline")).ok()?;
        let args = cmdline_args(&text);
        // argv is `<runsv> <service dir | name>`; the last argument is the
        // service, exactly as the supervisor was invoked.
        let first = args.first()?;
        if program_name(first) != "runsv" {
            return None;
        }
        let last = args.last()?;
        if last.is_empty() || last.starts_with('-') {
            return None;
        }
        Some(program_name(last))
    }

    /// The epoch nanoseconds at which one process started, from the kernel's
    /// own two clocks: the boot time (`btime`) plus the process's tick count.
    /// A boot time this read could not get yields zero — "not observable",
    /// never a fabricated timestamp.
    pub fn start_epoch_ns(&self, entry: &ProcEntry) -> u64 {
        if self.boot_epoch_s == 0 {
            return 0;
        }
        self.boot_epoch_s
            .saturating_mul(1_000_000_000)
            .saturating_add(entry.starttime_ticks.saturating_mul(1_000_000_000 / USER_HZ))
    }

    /// Whether this scan actually read a process table (so "no supervisor" is
    /// a measured statement rather than an unread one).
    pub fn readable(&self) -> bool {
        self.root.is_dir()
    }
}

/// Read one `/proc/<pid>/stat` into the fields a supervision read needs.
///
/// The `comm` field is parenthesised and may itself contain spaces or
/// parentheses, so the line is split at its **last** `)` — the only correct
/// parse, and the one the kernel's own format guarantees.
fn read_entry(root: &Path, pid: u64) -> Option<ProcEntry> {
    let text = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let close = text.rfind(')')?;
    let comm = text
        .find('(')
        .map(|open| text[open + 1..close].to_string())
        .unwrap_or_default();
    // After the closing parenthesis: state, ppid, pgrp, … — field 22 overall,
    // so the 20th token here is `starttime`.
    let rest = &text[close + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next().and_then(|s| s.chars().next()).unwrap_or('?');
    let ppid = fields.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let starttime_ticks = fields
        .nth(17)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Some(ProcEntry { pid, comm, state, ppid, starttime_ticks })
}

/// The kernel's boot time in epoch seconds, from `/proc/stat`'s `btime` line.
fn read_btime(root: &Path) -> u64 {
    let Ok(text) = std::fs::read_to_string(root.join("stat")) else { return 0 };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            if let Ok(secs) = rest.trim().parse::<u64>() {
                return secs;
            }
        }
    }
    0
}

/// Split a `/proc/<pid>/cmdline` into its NUL-separated arguments, dropping the
/// trailing empty one the kernel appends.
fn cmdline_args(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    text.split('\0')
        .map(ToString::to_string)
        .filter(|a| !a.is_empty())
        .collect()
}

/// The basename of an executable path — the name `/proc` compares by, so a
/// service invoked as `/usr/bin/sshd` and one invoked as `sshd` compare equal.
fn program_name(path: &str) -> String {
    match path.rsplit('/').next() {
        Some(name) => name.to_string(),
        None => path.to_string(),
    }
}

/// The pid one service's own pid file names, with the path it was read from.
///
/// The pid-file locations are the two that exist on a runit host and are
/// world-readable when the daemon writes them as root: `/run/<name>.pid` and
/// `/run/<name>/pid`. A pid file that cannot be read (or names no number) is
/// simply not found — a pid is never guessed from the file's existence.
pub fn read_pid_file(pid_root: &Path, name: &str) -> Option<(PathBuf, u64)> {
    for relative in [format!("{name}.pid"), format!("{name}/pid")] {
        let path = pid_root.join(relative);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(pid) = text.split_whitespace().next().and_then(|t| t.parse::<u64>().ok())
            {
                return Some((path, pid));
            }
        }
    }
    None
}

/// The program a runit service script `exec`s, as the service's own script
/// declares it: the first word of the script's last `exec` line, basenamed.
///
/// This is how a service's log process is told from its service process: both
/// are children of the same `runsv`, but the log process's argv[0] is the
/// program the *log script* `exec`s (`exec vlogger -t sshd -p daemon`), while
/// the service process's argv[0] is the program the *run script* `exec`s. A
/// script that names no single exec'd program (a wrapper) yields `None`, and
/// the caller then cannot tell the two apart — which it says, rather than
/// guessing.
pub fn script_exec_program(script: &str) -> Option<String> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.split_whitespace().next() == Some("exec"))
        .next_back()
        .and_then(|line| line.split_whitespace().nth(1))
        .map(program_name)
}

/// Read one script file (bounded) and name the program it execs.
pub fn script_exec_program_of(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_SCRIPT_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    script_exec_program(&text)
}
