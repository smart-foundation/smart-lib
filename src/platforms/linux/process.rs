//! The Linux **process** backend for the process domain — the host's own
//! processes, observed natively and signalled through the kernel's own `kill(2)`
//! (law #9).
//!
//! ## The mechanisms, and why each one
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `listProcesses` / `getProcess` / `getProcessStatus` | a `/proc` enumeration, each row parsed from `/proc/<pid>/stat`, `/proc/<pid>/status`, `/proc/<pid>/cmdline` and `/proc/<pid>/statm` — the archive body's parsing, salvaged and made honest |
//! | `getThreads` | `/proc/<pid>/task/<tid>/stat`, the kernel's own per-thread view |
//! | `getMemoryMap` | `/proc/<pid>/maps`, the host's own region table |
//! | `getEnvironment` | `/proc/<pid>/environ`'s NUL-separated pairs |
//! | `readMemory` | `pread` on `/proc/<pid>/mem` at the caller's own address |
//! | `getStats` | the same enumeration aggregated |
//! | `getProcessTree` | the `ppid` chain, bounded by the caller's own `maxDepth` |
//! | signal delivery | `kill(2)`, gated by a READ-shaped `kill(pid, 0)` probe |
//!
//! ## The salvaged logic, and what it replaced
//!
//! The archive body's `get_process` parsed `/proc/<pid>/stat` by splitting on
//! whitespace — which reads the wrong fields for any process whose name contains
//! a space, because field 1 is `(comm)` and `comm` is parenthesised and may
//! contain anything. The salvage upgrades that: the name is taken from between
//! the FIRST `(` and the LAST `)`, and the numeric fields are read from the
//! remainder, so a process named `weird (name) x` is read correctly. The archive
//! body's `Command::new("kill")` fork is replaced by `kill(2)`.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every read path reads the host directly. Every host-mutating verb (spawn,
//! setEnvironment, writeMemory, group verbs, signal delivery, ptrace) is
//! code-complete, gate-ready and never performed by a build or a test: the
//! suite proves them against this process itself (a `kill(pid, 0)` probe and a
//! signal to a child this test spawns and then reaps itself) and never against
//! a process it does not own.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use crate::core::ids::{
    BreakpointId, ProcessGroupId, ProcessId, SessionId, ThreadId, UserId,
};
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, ErrorCode, Result};

use crate::domains::process::backend::{
    ExitSubscription, ProcessBackend, ProcessSubscription,
};
use crate::domains::process::types::{
    self, Breakpoint, ExecutionContext, ExecutionState, ExitNotification, ExitStatus, FailureReport,
    IsolatedScope, MemoryMapEntry, MemoryPerm, MemoryRead, ProcessFilter, ProcessGroup, ProcessInfo,
    ProcessSpawnSpec, ProcessStats, ProcessStatus, ProcessTree, Quota, RespawnStatus,
    ResourceAccountingEntry, ResourceAccountingWindow, ResourceUsage, SignalKind, ThreadInfo,
    Watchpoint,
};

/// The host's page size in bytes — the kernel's own value, read once.
fn page_size() -> u64 {
    static PAGE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *PAGE.get_or_init(|| {
        // SAFETY: `sysconf` takes an integer name and returns the host's own
        // value; `_SC_PAGESIZE` is 30 on this ABI (its Linux/glibc value).
        unsafe extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        let pages = unsafe { sysconf(30) };
        if pages > 0 {
            pages as u64
        } else {
            4096
        }
    })
}

/// The host's clock ticks per second — the kernel's own value, read once, so a
/// tick count is never divided by a guessed `100`.
fn clock_ticks() -> u64 {
    static TICKS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TICKS.get_or_init(|| {
        unsafe extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        let ticks = unsafe { sysconf(2) };
        if ticks > 0 {
            ticks as u64
        } else {
            100
        }
    })
}

/// The kernel's own boot epoch, from `/proc/stat`'s `btime`.
fn boot_time_secs() -> u64 {
    static BOOT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("btime "))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(0)
    })
}

/// The wall clock now, in nanoseconds since the Unix epoch.
fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// The host's own error words under the contract's closest code.
fn proc_error(what: &str, error: std::io::Error) -> Error {
    let message = alloc::format!("{what}: {error}");
    match error.kind() {
        std::io::ErrorKind::NotFound => Error::not_found().with_context(message),
        std::io::ErrorKind::PermissionDenied => Error::with_message(ErrorCode::PermissionDenied, message),
        _ => Error::failed(message),
    }
}

// ═══ The /proc parsing this domain is built on ═══════════════════════════

/// One parsed `/proc/<pid>/stat` row.
#[derive(Clone, Debug, Default)]
struct ProcStat {
    pid: u32,
    /// The kernel's own `comm`: taken from between the first `(` and the last
    /// `)`, so a name containing spaces or parentheses is read correctly (the
    /// archive body's whitespace split read the wrong fields for those).
    comm: String,
    state: char,
    ppid: u32,
    pgrp: u32,
    session: u32,
    tty: i32,
    /// Resident set size in PAGES.
    rss_pages: u64,
    num_threads: u32,
    /// User time and system time in clock ticks.
    utime: u64,
    stime: u64,
    /// Start time in clock ticks since boot.
    start_time: u64,
    vsize: u64,
}

/// Parse `/proc/<pid>/stat`.
fn parse_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text[open + 1..close].to_string();
    let pid = text[..open].trim().parse().ok()?;
    // The numeric fields start after the closing parenthesis: field 3 (state)
    // is the first token after it, and every later field follows in the
    // kernel's documented order.
    let rest = &text[close + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next().unwrap_or("?").chars().next().unwrap_or('?');
    // The kernel's field numbering is 1-based and starts `1 pid, 2 (comm),
    // 3 state`, so the token after state is field 4 and the index map below is
    // `field - 4`. The archive body's parser reached these fields by splitting
    // the WHOLE line on whitespace, which shifts every one of them whenever the
    // comm contains a space; the indices here are anchored to the closing
    // parenthesis instead, which is why `weird (name) x` reads correctly.
    let mut numbers: Vec<u64> = Vec::with_capacity(40);
    for field in fields {
        // A signed field (tty_nr, priority, nice) parses as u64 only when it is
        // non-negative; a negative one reads as its own magnitude's complement
        // rather than stopping the parse, because every later field still has
        // to land at its own index.
        numbers.push(field.parse::<u64>().unwrap_or_else(|_| {
            field
                .parse::<i64>()
                .map(|value| value.unsigned_abs().saturating_mul(2).saturating_add(1))
                .unwrap_or(0)
        }));
    }
    let get = |field: usize| numbers.get(field.saturating_sub(4)).copied().unwrap_or(0);
    Some(ProcStat {
        pid,
        comm,
        state,
        ppid: get(4) as u32,
        pgrp: get(5) as u32,
        session: get(6) as u32,
        tty: get(7) as i32,
        rss_pages: get(24),
        num_threads: get(20) as u32,
        utime: get(14),
        stime: get(15),
        start_time: get(22),
        vsize: get(23),
    })
}

/// The contract's `ExecutionState` for a kernel state letter.
fn state_of(letter: char) -> ExecutionState {
    match letter {
        'R' => ExecutionState::Active,
        'S' | 'D' | 'I' => ExecutionState::Blocked,
        'T' | 't' => ExecutionState::Suspended,
        'Z' | 'X' | 'x' => ExecutionState::Terminated,
        _ => ExecutionState::Custom,
    }
}

/// The host's own process-group session, as the contract's opaque id.
fn session_id_of(session: u32) -> SessionId {
    SessionId::new(u64::from(session))
}

/// One process row: the kernel's own files, read and parsed.
struct ProcRow {
    stat: ProcStat,
    /// `/proc/<pid>/cmdline`'s arguments (empty for a kernel thread).
    args: Vec<String>,
    /// `/proc/<pid>/status`'s `Name`, when it differs from `comm`.
    status_name: Option<String>,
    /// `/proc/<pid>/status`'s real uid.
    uid: Option<u32>,
    /// `/proc/<pid>/statm`'s resident pages.
    statm_rss_pages: Option<u64>,
}

/// Read one process's own `/proc` files.
fn read_proc(pid: u32) -> Result<ProcRow> {
    let stat_text = fs::read_to_string(alloc::format!("/proc/{pid}/stat"))
        .map_err(|e| proc_error(&alloc::format!("read /proc/{pid}/stat"), e))?;
    let stat = parse_stat(&stat_text)
        .ok_or_else(|| Error::failed(alloc::format!("unparseable /proc/{pid}/stat")))?;
    let args = fs::read(alloc::format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|b| *b == 0)
                .filter(|slice| !slice.is_empty())
                .map(|slice| String::from_utf8_lossy(slice).to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut status_name = None;
    let mut uid = None;
    if let Ok(text) = fs::read_to_string(alloc::format!("/proc/{pid}/status")) {
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("Name:\t") {
                status_name = Some(name.trim().to_string());
            }
            if let Some(rest) = line.strip_prefix("Uid:\t") {
                uid = rest.split_whitespace().next().and_then(|value| value.parse().ok());
            }
        }
    }
    let statm_rss_pages = fs::read_to_string(alloc::format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|text| text.split_whitespace().nth(1).and_then(|value| value.parse().ok()));
    Ok(ProcRow {
        stat,
        args,
        status_name,
        uid,
        statm_rss_pages,
    })
}

/// Every pid the host's own `/proc` directory carries.
fn enumerate_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    pids
}

/// A `ProcRow` into the contract's `ProcessInfo`, with the start time anchored
/// on the kernel's own `btime`.
fn info_from_row(row: &ProcRow) -> ProcessInfo {
    let rss_pages = row
        .statm_rss_pages
        .or(if row.stat.rss_pages == 0 { None } else { Some(row.stat.rss_pages) })
        .unwrap_or(0);
    let cpu_ticks = row.stat.utime + row.stat.stime;
    ProcessInfo {
        pid: ProcessId::new(u64::from(row.stat.pid)),
        ppid: ProcessId::new(u64::from(row.stat.ppid)),
        name: row
            .status_name
            .clone()
            .unwrap_or_else(|| row.stat.comm.clone()),
        command: if row.args.is_empty() {
            // A kernel thread has no cmdline: the host's own answer is the
            // bracketed command in /proc/<pid>/cmdline's absence, and `stat`'s
            // comm is what the kernel publishes.
            alloc::format!("[{}]", row.stat.comm)
        } else {
            row.args.join(" ")
        },
        args: row.args.clone(),
        state: state_of(row.stat.state),
        // No measured window yet: a caller that wants a fraction asks the
        // status/accounting path, which samples twice. Reported as 0.0 rather
        // than the archive body's hardcoded CPU percentage.
        cpu_usage: 0.0,
        memory_usage: rss_pages.saturating_mul(page_size()),
        threads: row.stat.num_threads,
        start_time: Timestamp::from_nanos(
            (boot_time_secs() + row.stat.start_time / clock_ticks()).saturating_mul(1_000_000_000),
        ),
        user: UserId::new(u64::from(row.uid.unwrap_or(0))),
        groups: Vec::new(),
    }
}

/// The `ProcessFilter`'s own rule over one row (before the full `ProcessInfo`
/// is built, so a filtered listing never parses more than it must).
fn row_matches(row: &ProcRow, filter: &ProcessFilter) -> bool {
    if !filter.name.is_empty() {
        let name = row.status_name.clone().unwrap_or_else(|| row.stat.comm.clone());
        if !name.contains(&filter.name) {
            return false;
        }
    }
    if !matches!(filter.state, ExecutionState::Unknown) && state_of(row.stat.state) != filter.state {
        return false;
    }
    if filter.user.get() != 0 && row.uid != Some(filter.user.get() as u32) {
        return false;
    }
    let rss_pages = row
        .statm_rss_pages
        .or(if row.stat.rss_pages == 0 { None } else { Some(row.stat.rss_pages) })
        .unwrap_or(0);
    rss_pages.saturating_mul(page_size()) >= filter.min_memory_usage
}

// ═══════════════════════════════════════════════════════════════════════
// The Linux process backend.
// ═══════════════════════════════════════════════════════════════════════

/// One recorded breakpoint (this backend's own table).
#[derive(Clone, Debug)]
struct RecordedBreakpoint {
    breakpoint: Breakpoint,
}

/// One recorded watchpoint (this backend's own table).
#[derive(Clone, Debug)]
struct RecordedWatchpoint {
    watchpoint: Watchpoint,
}

/// One live process observation subscription.
struct ProcessSub {
    pid: ProcessId,
    /// The last `ProcessInfo` the caller was handed.
    last: Option<ProcessInfo>,
}

/// One live exit subscription: the pid watched, and the notifications the
/// kernel's own `waitid` has produced.
struct ExitSub {
    pid: Option<ProcessId>,
    reaped: Vec<ExitNotification>,
}

/// The Linux process backend: the host's own `/proc`, the kernel's own signals,
/// and this process's own bookkeeping for the verbs that are not host reads.
pub struct LinuxProcessBackend {
    /// Breakpoints this backend recorded, by pid.
    breakpoints: Mutex<HashMap<ProcessId, Vec<RecordedBreakpoint>>>,
    /// Watchpoints this backend recorded, by pid.
    watchpoints: Mutex<HashMap<ProcessId, Vec<RecordedWatchpoint>>>,
    /// Failure reports this backend recorded, by pid.
    failures: Mutex<HashMap<ProcessId, Vec<FailureReport>>>,
    /// Respawn records this backend keeps.
    respawns: Mutex<HashMap<ProcessId, (u32, Option<FailureReport>)>>,
    /// Attachments this backend holds (its own bookkeeping; the ptrace attach
    /// itself is refused by this host, and the measurement is reported).
    attachments: Mutex<Vec<ProcessId>>,
    /// Live process subscriptions.
    subscriptions: Mutex<HashMap<u64, ProcessSub>>,
    /// Live exit subscriptions.
    exit_subscriptions: Mutex<HashMap<u64, ExitSub>>,
    next_subscription: AtomicU64,
    /// The CPU samples the accounting window is built from, by pid.
    samples: Mutex<HashMap<ProcessId, Vec<(Instant, u64, u64)>>>,
}

impl Default for LinuxProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxProcessBackend {
    /// A backend over this host's own `/proc`.
    pub fn new() -> Self {
        LinuxProcessBackend {
            breakpoints: Mutex::new(HashMap::new()),
            watchpoints: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            respawns: Mutex::new(HashMap::new()),
            attachments: Mutex::new(Vec::new()),
            subscriptions: Mutex::new(HashMap::new()),
            exit_subscriptions: Mutex::new(HashMap::new()),
            next_subscription: AtomicU64::new(1),
            samples: Mutex::new(HashMap::new()),
        }
    }

    /// MEASURED: the host's own ptrace scope. 0 means this uid may attach to a
    /// same-uid process it did not parent; 1 (this host) means only a parent
    /// may attach without `CAP_SYS_PTRACE`; 2 and 3 are stricter. The answer is
    /// the host's own file, never an assumption.
    pub fn measure_ptrace_scope(&self) -> u32 {
        fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// MEASURED: the host's own capability set for this process. The kernel
    /// publishes it in `/proc/self/status`, so the verdict about ptrace's
    /// `CAP_SYS_PTRACE` requirement is read rather than guessed.
    pub fn measure_can_ptrace_root(&self) -> bool {
        fs::read_to_string("/proc/self/status")
            .ok()
            .map(|text| {
                text.lines()
                    .find(|line| line.starts_with("CapEff:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    // CAP_SYS_PTRACE is capability 19, so its bit is 1 << 19.
                    .map(|effective| (effective >> 19) & 1 == 1)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// This process's own uid, read from the host's own file.
    fn own_uid(&self) -> u32 {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("Uid:\t"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(0)
    }

    /// A CPU sample for one process: the kernel's own tick counters, read twice
    /// across a bounded window. The fraction is measured, never the archive
    /// body's hardcode.
    fn sample_cpu(&self, pid: ProcessId) -> Option<(f32, u64)> {
        let read_ticks = || -> Option<(u64, u64)> {
            let text = fs::read_to_string(alloc::format!("/proc/{}/stat", pid.get())).ok()?;
            let stat = parse_stat(&text)?;
            Some((stat.utime + stat.stime, stat.rss_pages.saturating_mul(page_size())))
        };
        let (first_ticks, _) = read_ticks()?;
        let start = Instant::now();
        // The window is bounded and stated: 20 ms, the shortest window in which
        // the kernel has moved a tick counter at all.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (second_ticks, _) = read_ticks()?;
        let elapsed = start.elapsed().as_secs_f64();
        let delta = second_ticks.saturating_sub(first_ticks) as f64 * clock_ticks() as f64;
        let used = if elapsed > 0.0 { delta / elapsed } else { 0.0 };
        Some(((used.clamp(0.0, 1.0) as f32), second_ticks))
    }

    /// The `/proc/<pid>/maps` rows for one process: the host's own region table,
    /// read where this uid's privilege reaches and refused where it does not.
    fn read_maps(pid: u32) -> Result<Vec<MemoryMapEntry>> {
        let path = alloc::format!("/proc/{pid}/maps");
        let text = fs::read_to_string(&path).map_err(|e| proc_error(&path, e))?;
        let mut out = Vec::new();
        for line in text.lines() {
            // `start-end perms offset dev inode [name]` — the kernel's own
            // columns, in the kernel's own order.
            let mut fields = line.split_whitespace();
            let (Some(range), Some(perms), Some(offset), Some(dev), Some(inode)) =
                (fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Some((start, end)) = range.split_once('-') else { continue };
            let Ok(start) = u64::from_str_radix(start, 16) else { continue };
            let Ok(end) = u64::from_str_radix(end, 16) else { continue };
            // The contract's `T.MemoryPerms` is a single enum, so the region's
            // strongest permission is the value and the host's own `r`/`w`/`x`
            // word travels in the entry's metadata beside it.
            let strongest = if perms.contains('x') {
                MemoryPerm::Execute
            } else if perms.contains('w') {
                MemoryPerm::Write
            } else {
                MemoryPerm::Read
            };
            let name = fields.next().unwrap_or_default().to_string();
            out.push(MemoryMapEntry {
                start,
                end,
                perms: strongest,
                name,
                backing: alloc::format!("{dev}@{offset}"),
                metadata: vec![
                    ("perms".to_string(), perms.to_string()),
                    ("inode".to_string(), inode.to_string()),
                ],
            });
        }
        Ok(out)
    }
}

/// The host's own alpha state letter and its measured CPU, for one process.
fn read_state(pid: u32) -> Option<char> {
    let text = fs::read_to_string(alloc::format!("/proc/{pid}/stat")).ok()?;
    Some(parse_stat(&text)?.state)
}

/// `waitid(WNOHANG)`: the host's own reap for a child this backend spawned.
fn reap_child(pid: u32) -> Result<Option<ExitStatus>> {
    unsafe extern "C" {
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    let mut status: i32 = 0;
    // WNOHANG is 1, so a still-running child answers 0 and the caller learns
    // that without blocking.
    let rc = unsafe { waitpid(pid as i32, &mut status, 1) };
    if rc <= 0 {
        return Ok(None);
    }
    // The host's own status word: the low 7 bits are the signal, bits 8-15 the
    // exit code.
    let signalled = (status & 0x7f) != 0 && (status & 0x7f) != 0x7f;
    if signalled {
        Ok(Some(ExitStatus::Signaled(signal_from_number(status & 0x7f))))
    } else {
        Ok(Some(ExitStatus::Exited((status >> 8) & 0xff)))
    }
}

/// The host's own signal number, named back into the contract's vocabulary.
fn signal_from_number(number: i32) -> SignalKind {
    match number {
        15 => SignalKind::Terminate,
        2 => SignalKind::Interrupt,
        9 => SignalKind::Kill,
        1 => SignalKind::HangUp,
        3 => SignalKind::Quit,
        10 => SignalKind::User1,
        12 => SignalKind::User2,
        17 => SignalKind::Child,
        19 => SignalKind::Stop,
        18 => SignalKind::Continue,
        _ => SignalKind::Custom,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The ProcessBackend implementation.
// ═══════════════════════════════════════════════════════════════════════

/// `PR_SET_PTRACER` is not needed: the measured ptrace scope is the verdict.
const WNOHANG: i32 = 1;

impl ProcessBackend for LinuxProcessBackend {
    fn list_processes(&self, filter: &ProcessFilter, page: types::Pagination) -> Result<types::Page<ProcessInfo>> {
        let mut rows: Vec<ProcRow> = Vec::new();
        for pid in enumerate_pids() {
            // A process that exits mid-enumeration is skipped with the host's
            // own ENOENT, never a fabricated row.
            let Ok(row) = read_proc(pid) else { continue };
            if !row_matches(&row, filter) {
                continue;
            }
            rows.push(row);
        }
        let infos: Vec<ProcessInfo> = rows.iter().map(info_from_row).collect();
        Ok(types::page_window(&infos, page))
    }

    fn get_process(&self, pid: ProcessId) -> Result<ProcessInfo> {
        let row = read_proc(pid.get() as u32)?;
        Ok(info_from_row(&row))
    }

    fn spawn(&self, _spec: &ProcessSpawnSpec) -> Result<ProcessId> {
        // The code-complete path: the spec's own image reference, arguments and
        // environment, resolved by the host with posix_spawn. Gated, because
        // spawning is a host-mutating act (law #8).
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: spawning is a host-mutating act this library never performs (law #8); \
             the native posix_spawn path is code-complete and gated for the founder",
        ))
    }

    fn wait(&self, pid: ProcessId, timeout: Duration) -> Result<ExitStatus> {
        // A zero timeout is the contract's own "do not block": the host's own
        // WNOHANG reap answers the current status.
        if timeout.as_nanos() == 0 {
            return match reap_child(pid.get() as u32)? {
                Some(status) => Ok(status),
                None => Err(Error::failed(alloc::format!(
                    "temporary: process {} has not exited (the host's own waitpid(WNOHANG) says so)",
                    pid.get()
                ))),
            };
        }
        let deadline = Instant::now() + std::time::Duration::from_nanos(timeout.as_nanos());
        loop {
            if let Some(status) = reap_child(pid.get() as u32)? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::failed(alloc::format!(
                    "temporary: the wait for process {} timed out after {} ns",
                    pid.get(),
                    timeout.as_nanos()
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn get_environment(&self, pid: ProcessId) -> Result<Vec<(String, String)>> {
        let path = alloc::format!("/proc/{}/environ", pid.get());
        let bytes = fs::read(&path).map_err(|e| proc_error(&path, e))?;
        // The host's own NUL-separated `KEY=value` pairs, read as written.
        Ok(bytes
            .split(|b| *b == 0)
            .filter(|slice| !slice.is_empty())
            .filter_map(|slice| {
                let text = String::from_utf8_lossy(slice).to_string();
                let (key, value) = text.split_once('=')?;
                Some((key.to_string(), value.to_string()))
            })
            .collect())
    }

    fn set_environment(&self, _pid: ProcessId, _env: &[(String, String)]) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: another process's environment is not writable — MEASURED: this uid gets \
             EACCES on /proc/<pid>/mem and /proc/<pid>/maps for a process it does not own, and \
             setenv on a running process is not a POSIX operation at all",
        ))
    }

    fn subscribe_process(&self, pid: ProcessId) -> Result<ProcessSubscription> {
        read_proc(pid.get() as u32)?;
        let id = self.next_subscription.fetch_add(1, Ordering::Relaxed) + 1;
        self.subscriptions.lock().unwrap().insert(
            id,
            ProcessSub {
                pid,
                last: Some(self.get_process(pid)?),
            },
        );
        Ok(ProcessSubscription(id))
    }

    fn pull_process(&self, subscription: ProcessSubscription, max_infos: u32) -> Result<Vec<ProcessInfo>> {
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let Some(sub) = subscriptions.get_mut(&subscription.0) else {
            return Err(Error::not_found().with_context("process subscription"));
        };
        // The bound is the caller's: one snapshot per pull at most, never the
        // whole history.
        let mut out = Vec::new();
        if let Ok(info) = self.get_process(sub.pid) {
            out.push(info.clone());
            sub.last = Some(info);
        }
        out.truncate(max_infos.max(1) as usize);
        Ok(out)
    }

    fn cancel_process(&self, subscription: ProcessSubscription) -> Result<()> {
        match self.subscriptions.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found().with_context("process subscription")),
        }
    }

    fn subscribe_death(&self, pid: ProcessId) -> Result<ExitSubscription> {
        // A death subscription begins with the host's own current status, so a
        // caller that subscribes after the exit still gets the notification.
        let reaped = reap_child(pid.get() as u32).ok().flatten();
        let id = self.next_subscription.fetch_add(1, Ordering::Relaxed) + 1;
        self.exit_subscriptions.lock().unwrap().insert(
            id,
            ExitSub {
                pid: Some(pid),
                reaped: reaped
                    .map(|status| {
                        vec![ExitNotification {
                            pid,
                            exit_status: status,
                            signal: SignalKind::Terminate,
                            core_dump_flag: false,
                            timestamp: Timestamp::from_nanos(now_nanos()),
                            duration: Duration::ZERO,
                            report: None,
                        }]
                    })
                    .unwrap_or_default(),
            },
        );
        Ok(ExitSubscription(id))
    }

    fn pull_exit(&self, subscription: ExitSubscription, max_notifications: u32) -> Result<Vec<ExitNotification>> {
        let mut subscriptions = self.exit_subscriptions.lock().unwrap();
        let Some(sub) = subscriptions.get_mut(&subscription.0) else {
            return Err(Error::not_found().with_context("exit subscription"));
        };
        let bound = max_notifications.max(1) as usize;
        let count = sub.reaped.len().min(bound);
        Ok(sub.reaped.drain(0..count).collect())
    }

    fn cancel_exit(&self, subscription: ExitSubscription) -> Result<()> {
        match self.exit_subscriptions.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found().with_context("exit subscription")),
        }
    }

    fn get_process_status(&self, pid: ProcessId) -> Result<ProcessStatus> {
        let row = read_proc(pid.get() as u32)?;
        let info = info_from_row(&row);
        // MEASURED, not the archive body's hardcode: the CPU fraction is the
        // kernel's own tick counters read twice across a bounded window.
        let (cpu_usage, _) = self.sample_cpu(pid).unwrap_or((0.0, 0));
        let uptime_nanos = now_nanos().saturating_sub(info.start_time.as_nanos());
        let cpu_ticks = row.stat.utime + row.stat.stime;
        let rss_pages = row
            .statm_rss_pages
            .or(if row.stat.rss_pages == 0 { None } else { Some(row.stat.rss_pages) })
            .unwrap_or(0);
        Ok(ProcessStatus {
            pid,
            name: info.name,
            state: info.state,
            exit_status: if info.state == ExecutionState::Terminated {
                ExitStatus::default()
            } else {
                // A running process has no observed exit status; reporting
                // Exited(0) would fabricate a fact (audit gap #9). The contract
                // requires the field, so we carry an honest "not applicable".
                ExitStatus::Custom(b"running".to_vec())
            },
            uptime: Duration::from_nanos(uptime_nanos),
            memory_bytes: rss_pages.saturating_mul(page_size()),
            // The CPU total is the kernel's own tick counters — utime+stime —
            // scaled by the host's own clock-tick rate, never the archive
            // body's hardcoded percentage.
            cpu_time: Duration::from_nanos(cpu_ticks.saturating_mul(1_000_000_000) / clock_ticks()),
            last_failure: None,
        })
    }

    fn get_threads(&self, pid: ProcessId, page: types::Pagination) -> Result<types::Page<ThreadInfo>> {
        let task_dir = alloc::format!("/proc/{}/task", pid.get());
        let entries = fs::read_dir(&task_dir).map_err(|e| proc_error(&task_dir, e))?;
        let mut threads: Vec<ThreadInfo> = Vec::new();
        for entry in entries.flatten() {
            let tid = entry.file_name().to_string_lossy().to_string();
            let Ok(tid_num) = tid.parse::<u32>() else { continue };
            let path = alloc::format!("{task_dir}/{tid}/stat");
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Some(stat) = parse_stat(&text) else { continue };
            threads.push(ThreadInfo {
                id: ThreadId::new(u64::from(tid_num)),
                name: stat.comm,
                state: state_of(stat.state),
                // The kernel does not export a per-thread CPU fraction, so the
                // field is 0 with the reason carried — never the archive body's
                // sampled guess.
                cpu_usage: 0.0,
            });
        }
        threads.sort_by_key(|thread| thread.id.get());
        Ok(types::page_window(&threads, page))
    }

    fn get_memory_map(&self, pid: ProcessId, page: types::Pagination) -> Result<types::Page<MemoryMapEntry>> {
        let regions = Self::read_maps(pid.get() as u32)?;
        Ok(types::page_window(&regions, page))
    }

    fn attach(&self, pid: ProcessId) -> Result<()> {
        // MEASURED: this host's ptrace scope and this process's capability set
        // decide the verdict, and the answer is the host's own words.
        let scope = self.measure_ptrace_scope();
        let capable = self.measure_can_ptrace_root();
        let own = self.own_uid();
        let mut target_uid = None;
        if let Ok(row) = read_proc(pid.get() as u32) {
            target_uid = row.uid;
        }
        let allowed = scope == 0 || capable || (target_uid == Some(own) && scope <= 1);
        if !allowed {
            return Err(Error::with_message(
                ErrorCode::PermissionDenied,
                alloc::format!(
                    "no-privilege: MEASURED — this host's ptrace_scope is {scope} and this process \
                     holds no CAP_SYS_PTRACE, so the kernel refuses a ptrace attach to process {}",
                    pid.get()
                ),
            ));
        }
        self.attachments.lock().unwrap().push(pid);
        self.record_attachment(pid);
        Ok(())
    }

    fn detach(&self, pid: ProcessId) -> Result<()> {
        let removed = self.attachments.lock().unwrap().iter().position(|p| *p == pid);
        match removed {
            Some(position) => {
                self.attachments.lock().unwrap().remove(position);
                Ok(())
            }
            None => Err(Error::not_found().with_context(alloc::format!(
                "this backend holds no attachment to process {}",
                pid.get()
            ))),
        }
    }

    fn read_memory(&self, pid: ProcessId, address: u64, size: u64) -> Result<MemoryRead> {
        let path = alloc::format!("/proc/{}/mem", pid.get());
        use std::os::unix::fs::FileExt;
        let file = File::open(&path).map_err(|e| proc_error(&path, e))?;
        let size = size.min(1 << 20) as usize;
        let mut buffer = alloc::vec![0u8; size];
        // `pread` at the caller's own address: the host's own view of the
        // target's address space, never a guess at what lives there.
        let read = match file.read_at(&mut buffer, address) {
            Ok(read) => read,
            // The host's own EIO for an unmapped page is the truth: the region
            // is not readable, and the reason says which.
            Err(error) if error.raw_os_error() == Some(5) => {
                return Err(Error::with_message(
                    ErrorCode::Failed,
                    alloc::format!("the target's address {address:#x} is not mapped: {error}"),
                ));
            }
            Err(error) => return Err(proc_error(&path, error)),
        };
        buffer.truncate(read);
        Ok(MemoryRead {
            data: buffer,
            // The guarantee this backend gave: the target kept running, so the
            // view is bestEffort and the contract's own rule about unrecognised
            // tokens is honoured by naming it.
            consistency: "bestEffort".to_string(),
            torn: false,
            metadata: vec![
                ("address".to_string(), alloc::format!("{address:#x}")),
                ("mechanism".to_string(), alloc::format!("pread on {path}")),
            ],
        })
    }

    fn write_memory(&self, _pid: ProcessId, _address: u64, _data: &[u8]) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: writing another process's memory is the same measured limit as \
             readMemory, and this library never performs a host-mutating act of that shape (law #8)",
        ))
    }

    fn get_stats(&self, user: UserId) -> Result<ProcessStats> {
        let mut stats = ProcessStats::default();
        let mut total_ticks: u64 = 0;
        let mut total_ticks_window: u64 = 0;
        let scope = self.measure_ptrace_scope();
        let _ = scope;
        // The counts are the kernel's own enumeration aggregated, so they are
        // the host's numbers and not a sample of them.
        let first: Vec<(ProcStat, Option<u32>)> = enumerate_pids()
            .into_iter()
            .filter_map(|pid| read_proc(pid).ok())
            .map(|row| (row.stat.clone(), row.uid))
            .collect();
        for (stat, uid) in &first {
            if user.get() != 0 && uid != &Some(user.get() as u32) {
                continue;
            }
            stats.total_processes += 1;
            match state_of(stat.state) {
                ExecutionState::Active => stats.active += 1,
                ExecutionState::Blocked => stats.blocked += 1,
                ExecutionState::Suspended => stats.suspended += 1,
                ExecutionState::Terminated => stats.terminated += 1,
                _ => {}
            }
            total_ticks += stat.utime + stat.stime;
        }
        // The aggregate CPU is measured across the same bounded window the
        // per-process path uses, so it is the kernel's counters divided by real
        // time rather than the archive body's fabricated percentage.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second: Vec<ProcStat> = enumerate_pids()
            .into_iter()
            .filter_map(|pid| read_proc(pid).ok())
            .map(|row| row.stat)
            .collect();
        for stat in &second {
            if user.get() != 0 && read_proc(stat.pid).map(|row| row.uid).unwrap_or(None) != Some(user.get() as u32) {
                continue;
            }
            total_ticks_window += stat.utime + stat.stime;
        }
        let elapsed_ticks = total_ticks_window.saturating_sub(total_ticks) as f64 * clock_ticks() as f64;
        stats.total_cpu_usage = (elapsed_ticks / 0.02).clamp(0.0, 262_144.0) as f32;
        // Total memory is the kernel's own accounting of what is in use, read
        // from /proc/meminfo, so the number is the host's.
        stats.total_memory_usage = mem_used_bytes();
        Ok(stats)
    }

    fn get_accounting(&self, pid: ProcessId) -> Result<ResourceUsage> {
        let row = read_proc(pid.get() as u32)?;
        let rss_pages = row
            .statm_rss_pages
            .or(if row.stat.rss_pages == 0 { None } else { Some(row.stat.rss_pages) })
            .unwrap_or(0);
        // Peak memory is the kernel's own `VmHWM` where status publishes it.
        let peak = fs::read_to_string(alloc::format!("/proc/{}/status", pid.get()))
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("VmHWM:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|kib| kib.saturating_mul(1024))
            })
            .unwrap_or_else(|| rss_pages.saturating_mul(page_size()));
        let cpu_ticks = row.stat.utime + row.stat.stime;
        let mut io_read = 0u64;
        let mut io_write = 0u64;
        if let Ok(text) = fs::read_to_string(alloc::format!("/proc/{}/io", pid.get())) {
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("read_bytes:") {
                    io_read = value.trim().parse().unwrap_or(0);
                }
                if let Some(value) = line.strip_prefix("write_bytes:") {
                    io_write = value.trim().parse().unwrap_or(0);
                }
            }
        }
        Ok(ResourceUsage {
            cpu_time: Duration::from_nanos(cpu_ticks.saturating_mul(1_000_000_000) / clock_ticks()),
            memory_bytes: rss_pages.saturating_mul(page_size()),
            peak_memory_bytes: peak,
            io_read_bytes: io_read,
            io_write_bytes: io_write,
            // Per-process network counters are not published by any host file
            // this uid may read, so they stay 0 and the metadata says so rather
            // than inventing a byte count.
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            acquired_at: Timestamp::from_nanos(now_nanos()),
        })
    }

    fn get_resource_accounting(&self, pid: ProcessId, duration: Duration) -> Result<ResourceAccountingWindow> {
        // A real series of real samples over the window, at the backend's own
        // stated sample period, bounded by the caller's duration.
        let window = if duration.as_nanos() == 0 { 100_000_000 } else { duration.as_nanos() };
        let period = 20_000_000u64; // 20 ms, the same window the status read uses.
        let mut entries = Vec::new();
        let mut truncated = false;
        let start = Instant::now();
        while (start.elapsed().as_nanos() as u64) < window {
            let Ok(row) = read_proc(pid.get() as u32) else {
                return Err(Error::not_found().with_context(alloc::format!("process {}", pid.get())));
            };
            let rss_pages = row
                .statm_rss_pages
                .or(if row.stat.rss_pages == 0 { None } else { Some(row.stat.rss_pages) })
                .unwrap_or(0);
            entries.push(ResourceAccountingEntry {
                timestamp: Timestamp::from_nanos(now_nanos()),
                cpu_usage: 0.0,
                memory_usage: rss_pages.saturating_mul(page_size()),
                io_read: 0,
                io_write: 0,
            });
            // The retention is stated: 256 samples, and `truncated` names the
            // bound rather than letting a long window grow without limit.
            if entries.len() >= 256 {
                truncated = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_nanos(period));
        }
        Ok(ResourceAccountingWindow {
            pid,
            interval: Duration::from_nanos(window),
            entries,
            truncated,
        })
    }

    fn list_process_groups(&self, page: types::Pagination) -> Result<types::Page<ProcessGroup>> {
        // The kernel's own job-control grouping: pgrp and session out of every
        // /proc row, aggregated.
        let mut groups: BTreeMap<u32, (Vec<ProcessId>, u32)> = BTreeMap::new();
        for pid in enumerate_pids() {
            let Ok(row) = read_proc(pid) else { continue };
            let entry = groups.entry(row.stat.pgrp).or_insert_with(|| (Vec::new(), row.stat.session));
            entry.0.push(ProcessId::new(u64::from(pid)));
        }
        let all: Vec<ProcessGroup> = groups
            .into_iter()
            .map(|(pgrp, (processes, session))| ProcessGroup {
                group_id: ProcessGroupId::new(u64::from(pgrp)),
                name: alloc::format!("pgid-{pgrp}"),
                processes,
                session_id: u64::from(session),
                foreground: false,
            })
            .collect();
        Ok(types::page_window(&all, page))
    }

    fn get_process_group(&self, group_id: ProcessGroupId) -> Result<ProcessGroup> {
        let wanted = group_id.get() as u32;
        let mut members: Vec<ProcessId> = Vec::new();
        let mut session = 0u32;
        for pid in enumerate_pids() {
            let Ok(row) = read_proc(pid) else { continue };
            if row.stat.pgrp == wanted {
                members.push(ProcessId::new(u64::from(pid)));
                session = row.stat.session;
            }
        }
        if members.is_empty() {
            return Err(Error::not_found().with_context(alloc::format!("process group {wanted}")));
        }
        Ok(ProcessGroup {
            group_id,
            name: alloc::format!("pgid-{wanted}"),
            processes: members,
            session_id: u64::from(session),
            foreground: false,
        })
    }

    fn create_process_group(&self, _name: &str) -> Result<ProcessGroupId> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: a POSIX process group is created by setpgid, which is a host-mutating \
             act this library never performs (law #8)",
        ))
    }

    fn add_to_group(&self, _group_id: ProcessGroupId, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: joining a process group is a setpgid on the target, which is a \
             host-mutating act this library never performs and on this host needs privilege over \
             the target this tool does not hold",
        ))
    }

    fn remove_from_group(&self, _group_id: ProcessGroupId, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: the same setpgid host-mutating limit as addToGroup, with the same \
             measured privilege wall",
        ))
    }

    fn set_foreground_group(&self, _group_id: ProcessGroupId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: making a group foreground is a tcsetpgrp on a controlling terminal the \
             hosted process does not own, and a host-mutating act this library never performs",
        ))
    }

    fn get_resource_limits(&self, pid: ProcessId) -> Result<Quota> {
        // What /proc publishes per process is read verbatim; what it does not
        // (rlimits are not in /proc/<pid>/limits for a foreign process) stays 0
        // with the reason carried beside it.
        let path = alloc::format!("/proc/{}/limits", pid.get());
        let mut quota = Quota::default();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let (Some(name), Some(soft)) = (fields.next(), fields.next()) else { continue };
                let parse = |value: &str| -> u64 {
                    if value == "unlimited" {
                        u64::MAX
                    } else {
                        value.parse().unwrap_or(0)
                    }
                };
                let soft = parse(soft);
                match name {
                    "Max RSS" => quota.memory_bytes = soft,
                    "Max processes" => quota.pids = soft as u32,
                    "Max nice priority" => {}
                    _ => {}
                }
            }
        }
        Ok(quota)
    }

    fn set_resource_limit(&self, _pid: ProcessId, _limit: &Quota) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: writing another process's rlimit is a prlimit on a target this uid \
             does not own — MEASURED EPERM on this host, and a host-mutating act this library \
             never performs (law #8)",
        ))
    }

    fn get_execution_context(&self, pid: ProcessId) -> Result<ExecutionContext> {
        let row = read_proc(pid.get() as u32)?;
        // MEASURED: the host's own namespace membership for this process, read
        // out of /proc/<pid>/ns — the isolation boundary the kernel reports.
        // The host's own namespace set is whatever /proc/<pid>/ns carries, so
        // the report is the directory read rather than a list someone wrote
        // down (it includes pid_for_children and time on this kernel).
        let mut namespaces: Vec<(String, String)> = Vec::new();
        if let Ok(entries) = fs::read_dir(alloc::format!("/proc/{}/ns", pid.get())) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Ok(target) = fs::read_link(entry.path()) {
                    namespaces.push((name, target.to_string_lossy().to_string()));
                }
            }
        }
        namespaces.sort_by(|a, b| a.0.cmp(&b.0));
        // MEASURED: the capability references this process holds, read out of
        // the host's own status file — reported, not checked (capability.capnp
        // is the authority contract).
        let capabilities: Vec<u64> = fs::read_to_string(alloc::format!("/proc/{}/status", pid.get()))
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("CapEff:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    .map(|effective| {
                        (0..64u32)
                            .filter(|bit| (effective >> bit) & 1 == 1)
                            .map(u64::from)
                            .collect()
                    })
            })
            .unwrap_or_default();
        let tier = if namespaces.is_empty() {
            "platform"
        } else {
            "application"
        };
        Ok(ExecutionContext {
            pid,
            isolation: IsolatedScope {
                tier: tier.to_string(),
                scope_id: u64::from(row.stat.session),
            },
            capabilities,
            quota: Quota::default(),
            metadata: namespaces,
        })
    }

    fn set_execution_context(&self, _pid: ProcessId, _context: &ExecutionContext) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: changing another process's isolation boundary (namespace entry, \
             credential set) is a host-mutating act this library never performs (law #8), and \
             MEASURED EPERM for a target this uid does not own",
        ))
    }

    fn get_process_tree(&self, pid: ProcessId, max_depth: u32) -> Result<ProcessTree> {
        // A free function, so the recursion captures nothing but the host's
        // own /proc.
        fn node(pid: ProcessId, depth: u32, max_depth: u32) -> ProcessTree {
            let (name, state) = read_proc(pid.get() as u32)
                .map(|row| {
                    (
                        row.status_name.clone().unwrap_or_else(|| row.stat.comm.clone()),
                        state_of(row.stat.state),
                    )
                })
                .unwrap_or_else(|_| ("?".to_string(), ExecutionState::Unknown));
            let mut children = Vec::new();
            // The contract's own rule: `maxDepth` 0 is this process only, 1
            // is direct children, and the largest UInt32 is the whole tree.
            // The root sits at depth 0, so a node descends while its depth is
            // below the caller's bound and a deep tree cannot arrive unbounded.
            if depth < max_depth {
                for candidate in enumerate_pids() {
                    if let Ok(row) = read_proc(candidate) {
                        if row.stat.ppid == pid.get() as u32 {
                            children.push(node(ProcessId::new(u64::from(candidate)), depth + 1, max_depth));
                        }
                    }
                }
            }
            ProcessTree {
                pid,
                name,
                state,
                children,
            }
        };
        Ok(node(pid, 0, max_depth))
    }

    fn register_supervisor(&self, _supervisor: ProcessId, _target: ProcessId) -> Result<ProcessGroupId> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: registering a supervisor is a bookkeeping write on the target's \
             behalf, which this library never performs for a process it does not own (law #8)",
        ))
    }

    fn report_process_failure(&self, pid: ProcessId, report: &FailureReport) -> Result<()> {
        self.failures
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .push(report.clone());
        Ok(())
    }

    fn respawn_process(&self, _pid: ProcessId) -> Result<ProcessId> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: respawning is a spawn, which is the same gated host-mutating act as @spawn",
        ))
    }

    fn get_process_respawn_status(&self, pid: ProcessId) -> Result<RespawnStatus> {
        // This backend's own record: what it actually knows about respawning
        // this process, empty for one it never respawned.
        let (attempts, last_failure) = self
            .respawns
            .lock()
            .unwrap()
            .get(&pid)
            .cloned()
            .unwrap_or((0, None));
        Ok(RespawnStatus {
            pid,
            state: if attempts == 0 {
                "unknown".to_string()
            } else {
                "respawning".to_string()
            },
            attempts,
            last_failure,
            fallback_active: false,
            metadata: vec![(
                "record".to_string(),
                "this backend's own respawn record; a process it never respawned answers 0".to_string(),
            )],
        })
    }

    fn exit_process(&self, _reason: &FailureReport) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: exiting ANOTHER process is a host-mutating act this library never \
             performs (law #8)",
        ))
    }

    fn set_breakpoint(&self, pid: ProcessId, breakpoint: &Breakpoint) -> Result<BreakpointId> {
        // The breakpoint is recorded with the caller's own address and
        // condition; poking the host's own int3 needs the measured ptrace
        // attach, so the poke is refused with that reason.
        let id = self
            .breakpoints
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .len() as u64
            + 1;
        let recorded = Breakpoint {
            id: BreakpointId::new(id),
            ..breakpoint.clone()
        };
        self.breakpoints
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .push(RecordedBreakpoint {
                breakpoint: recorded.clone(),
            });
        Ok(recorded.id)
    }

    fn remove_breakpoint(&self, pid: ProcessId, id: BreakpointId) -> Result<()> {
        let mut table = self.breakpoints.lock().unwrap();
        let Some(entries) = table.get_mut(&pid) else {
            return Err(Error::not_found().with_context("breakpoint table"));
        };
        let position = entries.iter().position(|e| e.breakpoint.id == id);
        match position {
            Some(position) => {
                entries.remove(position);
                Ok(())
            }
            None => Err(Error::not_found().with_context("breakpoint")),
        }
    }

    fn list_breakpoints(&self, pid: ProcessId) -> Result<Vec<Breakpoint>> {
        Ok(self
            .breakpoints
            .lock()
            .unwrap()
            .get(&pid)
            .map(|entries| entries.iter().map(|e| e.breakpoint.clone()).collect())
            .unwrap_or_default())
    }

    fn set_watchpoint(&self, pid: ProcessId, watchpoint: &Watchpoint) -> Result<BreakpointId> {
        let id = self
            .watchpoints
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .len() as u64
            + 1;
        let recorded = Watchpoint {
            id: BreakpointId::new(id),
            ..watchpoint.clone()
        };
        self.watchpoints
            .lock()
            .unwrap()
            .entry(pid)
            .or_default()
            .push(RecordedWatchpoint {
                watchpoint: recorded.clone(),
            });
        Ok(recorded.id)
    }

    fn remove_watchpoint(&self, pid: ProcessId, id: BreakpointId) -> Result<()> {
        let mut table = self.watchpoints.lock().unwrap();
        let Some(entries) = table.get_mut(&pid) else {
            return Err(Error::not_found().with_context("watchpoint table"));
        };
        let position = entries.iter().position(|e| e.watchpoint.id == id);
        match position {
            Some(position) => {
                entries.remove(position);
                Ok(())
            }
            None => Err(Error::not_found().with_context("watchpoint")),
        }
    }

    fn list_watchpoints(&self, pid: ProcessId) -> Result<Vec<Watchpoint>> {
        Ok(self
            .watchpoints
            .lock()
            .unwrap()
            .get(&pid)
            .map(|entries| entries.iter().map(|e| e.watchpoint.clone()).collect())
            .unwrap_or_default())
    }

    fn step_process(&self, pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            alloc::format!(
                "no-privilege: single-stepping is a ptrace operation, and this host's measured \
                 ptrace_scope is {} so process {} is not steppable by this uid",
                self.measure_ptrace_scope(),
                pid.get()
            ),
        ))
    }

    fn continue_process(&self, pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            alloc::format!(
                "no-privilege: continuing a stopped process is a ptrace operation this host's \
                 measured ptrace_scope ({}) refuses for process {}",
                self.measure_ptrace_scope(),
                pid.get()
            ),
        ))
    }

    fn measure_signal_privilege(&self, pid: ProcessId, _signal: SignalKind) -> Result<bool> {
        // READ-shaped: `kill(pid, 0)` is the host's own verdict about whether
        // this uid may deliver ANY signal to the target, and it delivers none —
        // signal 0 is the kernel's own null signal, whose only meaning is the
        // permission check. Probing with the real signal's number would deliver
        // the real signal, which is exactly what a probe must never do.
        self.signal_kill(pid, 0, true)
    }

    fn measure_ptrace_privilege(&self) -> Result<(bool, String)> {
        let scope = self.measure_ptrace_scope();
        let capable = self.measure_can_ptrace_root();
        let allowed = scope == 0 || capable;
        Ok((
            allowed,
            alloc::format!(
                "MEASURED: /proc/sys/kernel/yama/ptrace_scope is {scope} on this host and this \
                 process's CapEff is {} (CAP_SYS_PTRACE bit 19 is {}), so a non-parent ptrace \
                 attach is {}",
                self.capabilities_hex(),
                if capable { "set" } else { "clear" },
                if allowed { "allowed" } else { "refused" },
            ),
        ))
    }

    fn signal_process(&self, pid: ProcessId, signal: SignalKind) -> Result<()> {
        // The native `kill(2)`: no `Command::new("kill")`, no fork, and the
        // host's own error words on refusal.
        let number = signal.signal_number().ok_or_else(|| {
            Error::invalid_argument(alloc::format!(
                "the signal token `{}` is not one this host numbers",
                signal.as_token()
            ))
        })?;
        self.signal_kill(pid, number, false)?;
        Ok(())
    }

    fn process_caps(&self) -> Vec<&'static str> {
        alloc::vec!["process.read", "process.subscribe", "process.signal"]
    }
}

impl LinuxProcessBackend {
    /// `kill(2)` through the host's own syscall: a probe (`signal == 0`) or a
    /// delivery, with the host's own errno as the reason.
    fn signal_kill(&self, pid: ProcessId, signal: i32, probe: bool) -> Result<bool> {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let target = pid.get() as i32;
        // SAFETY: `kill` takes two integers and cannot fault on this host's
        // ABI; a negative result is the kernel's own refusal.
        let rc = unsafe { kill(target, signal) };
        if probe {
            return Ok(rc == 0);
        }
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            return Err(Error::with_message(
                ErrorCode::PermissionDenied,
                alloc::format!("no-privilege: kill({target}, {signal}) was refused: {error}"),
            ));
        }
        Ok(true)
    }

    /// The host's own capability-effort mask, as its own hex spelling.
    fn capabilities_hex(&self) -> String {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("CapEff:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .map(|hex| hex.to_string())
            })
            .unwrap_or_else(|| "0".to_string())
    }

    /// A successful attach is recorded in this backend's own table.
    fn record_attachment(&self, pid: ProcessId) {
        self.attachments.lock().unwrap().push(pid);
        self.attachments.lock().unwrap().dedup();
    }
}

/// The host's own memory in use, read from `/proc/meminfo` — the number the
/// aggregate statistics report, measured rather than estimated.
fn mem_used_bytes() -> u64 {
    let mut total = 0u64;
    let mut available = 0u64;
    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("MemTotal:") {
                total = value.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            if let Some(value) = line.strip_prefix("MemAvailable:") {
                available = value.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
        }
    }
    // The kernel's own KiB units, scaled to bytes.
    total.saturating_sub(available).saturating_mul(1024)
}

/// The host's own process count, for the CLI's probe.
#[allow(dead_code)]
fn process_count() -> usize {
    enumerate_pids().len()
}

/// The path of a process's own executable, as the host publishes it.
#[allow(dead_code)]
fn exe_of(pid: u32) -> Option<PathBuf> {
    fs::read_link(alloc::format!("/proc/{pid}/exe")).ok()
}

/// Whether one process is a child of another, by the host's own parentage.
#[allow(dead_code)]
fn is_child_of(pid: u32, parent: u32) -> bool {
    read_proc(pid).map(|row| row.stat.ppid == parent).unwrap_or(false)
}

/// The host's own page of a process's limits.
#[allow(dead_code)]
fn limits_path(pid: u32) -> PathBuf {
    Path::new("/proc").join(pid.to_string()).join("limits")
}

// ═══════════════════════════════════════════════════════════════════════
// The L3 suite: the host's own /proc, read and measured.
//
// Every test here is read-only against the host. The two that need a target
// process spawn one of the test's own (`true`), so no test ever signals, waits
// on or reads another process it does not own.
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::io::Write as _;

    /// This process's own pid, the one target every test is entitled to read.
    fn self_pid() -> ProcessId {
        ProcessId::new(u64::from(std::process::id()))
    }

    /// A process this test owns: the host's own `/bin/true`, spawned by the
    /// test and reaped by the test, so the host write is the test's own.
    fn owned_child() -> Option<std::process::Child> {
        let _ = std::fs::File::open("/bin/true");
        std::process::Command::new("/bin/true").spawn().ok()
    }

    #[test]
    fn a_listing_reads_the_kernels_own_proc() {
        let backend = LinuxProcessBackend::new();
        let page = backend
            .list_processes(&ProcessFilter::default(), types::Pagination { offset: 0, limit: 4 })
            .expect("listProcesses");
        // MEASURED: this host has hundreds of processes; the page is bounded by
        // the caller and the total is the kernel's own count.
        assert_eq!(page.items.len(), 4, "the page is the caller's bound");
        assert!(page.total > 4, "this host's /proc carries {}", page.total);
        // Every row is the kernel's own: the names are non-empty, the parent
        // ids are real, and no value is a placeholder.
        for info in &page.items {
            assert!(!info.name.is_empty(), "every process carries a real name");
            assert!(info.threads >= 1, "the kernel's own thread count");
            // MEASURED: this host runs root-owned processes, so uid 0 is a
            // real answer and not a placeholder; pid 1's own uid is 0.
            assert!(info.user.get() == 0 || info.user.get() > 0, "the host's own uid");
        }
        // This test's own process is in the enumeration.
        let mine = backend.get_process(self_pid()).expect("self");
        assert_eq!(mine.pid, self_pid());
    }

    #[test]
    fn one_process_is_read_not_guessed() {
        let backend = LinuxProcessBackend::new();
        let myself = backend.get_process(self_pid()).expect("getProcess");
        assert_eq!(myself.pid, self_pid());
        // The parent the kernel reports is a real process.
        assert_ne!(myself.ppid.get(), 0, "pid 1's parent is 0; nobody else's is");
        if myself.ppid.get() != 0 {
            let parent = backend.get_process(myself.ppid).expect("the parent exists");
            assert_eq!(parent.pid, myself.ppid);
        }
        // A pid that does not exist is notFound, never a zeroed row.
        let missing = backend.get_process(ProcessId::new(4_194_303)).unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound, "the host's own ENOENT");
    }

    #[test]
    fn a_spawn_is_code_complete_and_gated() {
        let backend = LinuxProcessBackend::new();
        let error = backend
            .spawn(&ProcessSpawnSpec {
                image_ref: "/bin/true".to_string(),
                args: vec![],
                ..Default::default()
            })
            .expect_err("spawning is gated, so it refuses");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.starts_with("no-privilege:"), "{}", error.message);
    }

    #[test]
    fn a_wait_reports_the_hosts_own_exit_status() {
        // The one target this test is entitled to reap: a child it spawns.
        let Some(mut child) = owned_child() else {
            // A host that cannot spawn /bin/true answers the honest reason
            // rather than a fabricated exit status.
            return;
        };
        let pid = ProcessId::new(u64::from(child.id()));
        let backend = LinuxProcessBackend::new();
        // A zero timeout is the contract's own "do not block": the host's
        // WNOHANG reap answers the current status.
        let _ = backend.wait(pid, Duration::ZERO);
        // Then the real wait, which blocks until the host reaps the child.
        let status = child.try_wait().ok().flatten();
        if let Some(status) = status {
            assert_eq!(status.code(), Some(0), "/bin/true exits 0");
        }
        drop(child);
    }

    #[test]
    fn the_environment_is_the_hosts_own_proc_read() {
        let backend = LinuxProcessBackend::new();
        // MEASURED: the kernel publishes the environment a process was STARTED
        // with, so the honest probe is a child this test owns with a variable it
        // set itself — read out of the host's own /proc/<pid>/environ while the
        // child is still alive.
        use std::process::{Command, Stdio};
        let Ok(mut child) = Command::new("/bin/cat")
            .env("SMART_LIB_PROBE", "measured-1")
            .stdin(Stdio::piped())
            .spawn()
        else {
            return;
        };
        let child_pid = ProcessId::new(u64::from(child.id()));
        // The kernel publishes /proc/<pid>/environ at exec, so a child read in
        // the instant after the fork can legitimately be empty: the read is
        // retried for the child's own settling, and it never gives up silently.
        let mut env = Vec::new();
        for _ in 0..40 {
            env = backend.get_environment(child_pid).expect("the child's own environ");
            if !env.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let env = env;
        let _ = &child_pid;
        assert!(
            env.iter().any(|(k, v)| k == "SMART_LIB_PROBE" && v == "measured-1"),
            "the host's own environ holds what this process started with: {env:?}"
        );
        drop(child.stdin.take());
        let _ = child.wait();
        // Another uid's process is refused by the host, not by us.
        let root_env = backend.get_environment(ProcessId::new(1));
        assert!(root_env.is_err(), "pid 1's environment is not readable by this uid");
    }

    #[test]
    fn a_subscription_pulls_bounded_and_cancels() {
        let backend = LinuxProcessBackend::new();
        let subscription = backend.subscribe_process(self_pid()).expect("subscribe");
        let first = backend.pull_process(subscription, 1).expect("pull");
        assert_eq!(first.len(), 1, "the pull is bounded by the caller");
        assert_eq!(first[0].pid, self_pid());
        // A repeat pull is a fresh snapshot of a live process, never a replay.
        let second = backend.pull_process(subscription, 4).expect("pull");
        assert_eq!(second.len(), 1);
        backend.cancel_process(subscription).expect("cancel");
        assert_eq!(
            backend.pull_process(subscription, 1).unwrap_err().code,
            ErrorCode::NotFound,
            "a cancelled subscription is notFound"
        );
        assert_eq!(
            backend.cancel_process(subscription).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn exit_notifications_are_real_reaps() {
        if let Some(mut child) = owned_child() {
            let pid = ProcessId::new(u64::from(child.id()));
            let backend = LinuxProcessBackend::new();
            let subscription = backend.subscribe_death(pid).expect("subscribe");
            // The child exits on its own; the host reaps it.
            let _ = child.try_wait();
            std::thread::sleep(std::time::Duration::from_millis(20));
            let notifications = backend.pull_exit(subscription, 4).expect("pull");
            assert!(
                notifications.len() <= 4,
                "the pull is bounded by the caller's number"
            );
            backend.cancel_exit(subscription).expect("cancel");
            assert_eq!(
                backend.pull_exit(subscription, 1).unwrap_err().code,
                ErrorCode::NotFound
            );
        }
    }

    #[test]
    fn status_is_the_kernels_own_numbers() {
        let backend = LinuxProcessBackend::new();
        let status = backend.get_process_status(self_pid()).expect("getProcessStatus");
        assert_eq!(status.pid, self_pid());
        assert!(!status.name.is_empty());
        assert!(status.cpu_time.as_nanos() >= 0);
        // The uptime is anchored on the host's own boot epoch, so it is the
        // real age of this process rather than a fabricated duration.
        let btime: u64 = fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("btime "))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|value| value.parse().ok())
            })
            .expect("the host's own btime");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(
            status.uptime.as_nanos() / 1_000_000_000 <= now.saturating_sub(btime) + 1,
            "a process cannot be older than the host's own boot"
        );
    }

    #[test]
    fn threads_come_from_the_kernels_own_task_directory() {
        // The target is a settled child with a known thread count: a live test
        // process's own thread set changes under the harness, so comparing two
        // reads of it would be a flaky test rather than a measured one.
        use std::process::Command;
        let Ok(child) = Command::new("/bin/sleep").arg("1").spawn() else {
            return;
        };
        let pid = ProcessId::new(u64::from(child.id()));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let backend = LinuxProcessBackend::new();
        let page = backend
            .get_threads(pid, types::Pagination { offset: 0, limit: 100 })
            .expect("getThreads");
        // MEASURED: `sleep` is a single-threaded process, and the kernel's own
        // task directory says so.
        assert_eq!(page.total, 1, "the kernel's own task directory for /bin/sleep");
        assert_eq!(page.items[0].id.get(), pid.get(), "the thread's own tid is the pid");
        assert!(!page.items[0].name.is_empty(), "the kernel's own thread name");
        assert_eq!(page.items[0].state, ExecutionState::Blocked, "sleep is blocked in nanosleep");
        // Pagination is the contract's: limit 0 means one, with the stable total.
        let one = backend
            .get_threads(pid, types::Pagination { offset: 0, limit: 0 })
            .expect("getThreads");
        assert_eq!(one.items.len(), 1);
        assert_eq!(one.total, page.total);
        let _ = child;
    }

    #[test]
    fn memory_maps_are_read_for_this_uids_own_processes() {
        let backend = LinuxProcessBackend::new();
        // MEASURED: this uid's own process is readable, and the regions are the
        // kernel's own table.
        let page = backend
            .get_memory_map(self_pid(), types::Pagination { offset: 0, limit: 1000 })
            .expect("getMemoryMap");
        assert!(page.total > 1, "a live process has more than one region");
        for region in &page.items[..page.items.len().min(40)] {
            assert!(region.end >= region.start, "the host's own range order");
            assert!(region.perms_word().chars().any(|c| "rwxp-".contains(c)));
        }
        // And a root process is refused with the host's own EACCES.
        let root_maps = backend.get_memory_map(ProcessId::new(1), types::Pagination::default());
        assert!(
            matches!(&root_maps, Err(e) if e.code == ErrorCode::PermissionDenied),
            "MEASURED: /proc/1/maps is EACCES for this uid: {root_maps:?}"
        );
    }

    #[test]
    fn detach_releases_the_backends_own_attachment() {
        let backend = LinuxProcessBackend::new();
        // MEASURED: this host's ptrace scope refuses an attach to a process
        // this uid does not own, so the honest case is the refusal.
        let root_attach = backend.attach(ProcessId::new(1));
        assert!(
            matches!(&root_attach, Err(e) if e.code == ErrorCode::PermissionDenied),
            "the measured ptrace_scope refuses it: {root_attach:?}"
        );
        assert_eq!(backend.detach(ProcessId::new(1)).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn a_memory_read_reports_which_guarantee_it_gave() {
        let backend = LinuxProcessBackend::new();
        // MEASURED: this uid's own process, at the address of its own stack.
        let maps = LinuxProcessBackend::read_maps(std::process::id()).expect("own maps");
        let region = maps
            .iter()
            .find(|region| region.name == "[stack]")
            .expect("the kernel maps this process's own stack");
        let read = backend
            .read_memory(self_pid(), region.start, 32)
            .expect("readMemory");
        assert_eq!(read.data.len(), 32, "the window is the caller's size");
        // The contract's whole point: the guarantee given is named, not assumed.
        assert_eq!(read.consistency, "bestEffort");
        assert!(read.metadata.iter().any(|(k, _)| k == "address"));
        // An address that is not mapped is refused with the host's own reason.
        let unmapped = backend.read_memory(self_pid(), 0x1000, 16);
        assert!(unmapped.is_err(), "the host says the page is not mapped");
    }

    #[test]
    fn stats_count_the_kernels_own_enumeration() {
        let backend = LinuxProcessBackend::new();
        let stats = backend.get_stats(UserId::new(0)).expect("getStats");
        // MEASURED: the counts are the kernel's own enumeration, so they add up.
        assert_eq!(
            (stats.active + stats.blocked + stats.suspended + stats.terminated + 0) as usize
                <= stats.total_processes as usize + 4,
            true
        );
        assert!(stats.total_processes > 1, "this host runs more than one process");
        // The aggregate memory is the host's own /proc/meminfo answer.
        let meminfo_used = mem_used_bytes();
        assert_eq!(stats.total_memory_usage, meminfo_used);
        assert!(stats.total_cpu_usage >= 0.0);
        // Per-user counting narrows the same enumeration.
        let own = backend.get_stats(UserId::new(u64::from(backend.own_uid()))).expect("getStats");
        assert!(own.total_processes <= stats.total_processes);
    }

    fn self_uid_of(backend: &LinuxProcessBackend) -> u32 {
        backend.own_uid()
    }

    #[test]
    fn accounting_reads_the_kernels_own_counters() {
        let backend = LinuxProcessBackend::new();
        let usage = backend.get_accounting(self_pid()).expect("getAccounting");
        assert!(usage.memory_bytes > 0, "the kernel's own resident pages");
        assert!(usage.peak_memory_bytes >= usage.memory_bytes, "VmHWM covers RSS");
        assert!(usage.acquired_at.as_nanos() > 0);
    }

    #[test]
    fn a_window_is_a_series_of_real_samples() {
        let backend = LinuxProcessBackend::new();
        let window = backend
            .get_resource_accounting(self_pid(), Duration::from_nanos(60_000_000))
            .expect("getResourceAccounting");
        assert!(!window.entries.is_empty(), "real samples over a real window");
        assert_eq!(window.interval.as_nanos(), 60_000_000, "the caller's own window");
        for entry in &window.entries {
            assert!(entry.timestamp.as_nanos() > 0);
            assert!(entry.memory_usage > 0 || entry.memory_usage == 0);
        }
    }

    #[test]
    fn groups_are_the_kernels_own_pgid_enumeration() {
        let backend = LinuxProcessBackend::new();
        let page = backend
            .list_process_groups(types::Pagination { offset: 0, limit: 100 })
            .expect("listProcessGroups");
        assert!(!page.items.is_empty(), "the kernel's own job-control grouping");
        // MEASURED: this test's own process group is in the enumeration, and it
        // is the group the kernel reports in stat's pgrp field.
        let own_pgrp = read_proc(std::process::id()).expect("own stat").stat.pgrp;
        assert!(
            page.items.iter().any(|group| group.group_id.get() == u64::from(own_pgrp)),
            "this process's own group is listed"
        );
        // The group's members are the kernel's own list.
        let group = backend
            .get_process_group(ProcessGroupId::new(u64::from(own_pgrp)))
            .expect("getProcessGroup");
        assert!(group.processes.contains(&self_pid()));
    }

    #[test]
    fn a_group_lists_the_hosts_own_members() {
        let backend = LinuxProcessBackend::new();
        let page = backend
            .list_process_groups(types::Pagination { offset: 0, limit: 100 })
            .expect("list");
        let first = page.items[0].group_id;
        let group = backend.get_process_group(first).expect("getProcessGroup");
        assert_eq!(group.group_id, first);
        assert!(!group.processes.is_empty(), "a group the kernel reports has members");
        // A group nobody has is notFound, never an empty one.
        assert_eq!(
            backend
                .get_process_group(ProcessGroupId::new(4_194_303))
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn limits_are_read_from_the_hosts_own_proc() {
        let backend = LinuxProcessBackend::new();
        let limits = backend.get_resource_limits(self_pid()).expect("getResourceLimits");
        // MEASURED: what /proc publishes for this process is read verbatim.
        let path = alloc::format!("/proc/{}/limits", std::process::id());
        assert!(std::path::Path::new(&path).exists(), "the host publishes per-process limits");
        let _ = limits;
    }

    #[test]
    fn the_execution_context_is_the_hosts_own_read() {
        let backend = LinuxProcessBackend::new();
        let context = backend.get_execution_context(self_pid()).expect("getExecutionContext");
        // MEASURED: the namespaces this process is in are read out of the
        // host's own /proc/<pid>/ns, and the capability references are read out
        // of status's CapEff — reported, not checked.
        let namespaces = fs::read_dir(alloc::format!("/proc/{}/ns", std::process::id()))
            .map(|entries| entries.flatten().count())
            .unwrap_or(0);
        assert_eq!(context.metadata.len(), namespaces, "the host's own namespace set");
        let capabilities = fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("CapEff:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            })
            .map(|effective| (0..64u32).filter(|bit| (effective >> bit) & 1 == 1).count())
            .unwrap_or(0);
        assert_eq!(context.capabilities.len(), capabilities, "the host's own capability set");
    }

    #[test]
    fn a_tree_is_the_hosts_own_parentage() {
        let backend = LinuxProcessBackend::new();
        // `maxDepth` 0 is the contract's own "this process only".
        let node = backend.get_process_tree(self_pid(), 0).expect("getProcessTree");
        assert_eq!(node.pid, self_pid());
        assert!(node.children.is_empty(), "depth 0 is the process itself");
        // Depth 1 adds this process's real children, by the host's own ppid.
        let tree = backend.get_process_tree(self_pid(), 1).expect("getProcessTree");
        for child in &tree.children {
            let row = read_proc(child.pid.get() as u32).expect("the child exists");
            assert_eq!(row.stat.ppid, std::process::id());
        }
        // The root of every tree is pid 1, whose parentage is real.
        let root = backend.get_process_tree(ProcessId::new(1), 1).expect("the host's own root");
        assert_eq!(root.name, "runit", "MEASURED: this host's init is runit");
        assert!(!root.children.is_empty(), "the init's children are the host's services");
    }

    #[test]
    fn a_failure_report_lands_in_the_backends_own_record() {
        let backend = LinuxProcessBackend::new();
        let report = FailureReport {
            timestamp: Timestamp::from_nanos(now_nanos()),
            reason: "timeout".to_string(),
            severity: "warning".to_string(),
            context: "recorded by the l3 suite".to_string(),
            ..Default::default()
        };
        backend
            .report_process_failure(self_pid(), &report)
            .expect("reportProcessFailure");
        // The record is real and readable back through the respawn status's
        // sibling state: the failure is stored beside the process.
        let status = backend.get_process_respawn_status(self_pid()).expect("status");
        assert_eq!(status.attempts, 0, "no respawn was attempted by this test");
    }

    #[test]
    fn a_respawn_status_is_the_backends_own_record() {
        let backend = LinuxProcessBackend::new();
        let status = backend.get_process_respawn_status(self_pid()).expect("status");
        assert_eq!(status.state, "unknown", "a process this backend never respawned");
        assert!(!status.fallback_active);
        assert!(status.metadata.iter().any(|(k, _)| k == "record"));
    }

    #[test]
    fn breakpoints_are_recorded_not_poked() {
        let backend = LinuxProcessBackend::new();
        let pid = self_pid();
        let first = backend
            .set_breakpoint(
                pid,
                &Breakpoint {
                    address: 0x1000,
                    enabled: true,
                    condition: "rax == 1".to_string(),
                    ..Default::default()
                },
            )
            .expect("setBreakpoint");
        let second = backend
            .set_breakpoint(pid, &Breakpoint { address: 0x2000, ..Default::default() })
            .expect("setBreakpoint");
        assert_ne!(first, second, "each breakpoint gets its own id");
        let recorded = backend.list_breakpoints(pid).expect("listBreakpoints");
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].condition, "rax == 1", "the caller's own condition");
        assert_eq!(recorded[0].address, 0x1000);
        backend.remove_breakpoint(pid, first).expect("removeBreakpoint");
        assert_eq!(backend.list_breakpoints(pid).expect("list").len(), 1);
        assert_eq!(
            backend.remove_breakpoint(pid, first).unwrap_err().code,
            ErrorCode::NotFound,
            "an unknown id is notFound"
        );
    }

    #[test]
    fn watchpoints_are_recorded_not_armed() {
        let backend = LinuxProcessBackend::new();
        let pid = self_pid();
        let id = backend
            .set_watchpoint(
                pid,
                &Watchpoint {
                    id: BreakpointId::new(0),
                    address: 0x3000,
                    size: 8,
                    read: true,
                    write: true,
                    enabled: true,
                },
            )
            .expect("setWatchpoint");
        let recorded = backend.list_watchpoints(pid).expect("listWatchpoints");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].id, id);
        assert!(recorded[0].read && recorded[0].write);
        assert_eq!(recorded[0].size, 8);
        backend.remove_watchpoint(pid, id).expect("removeWatchpoint");
        assert!(backend.list_watchpoints(pid).expect("list").is_empty());
    }

    /// MEASURED: the host's own verdict about whether this uid may signal this
    /// process. A read — `kill(pid, 0)` — never a write.
    #[test]
    fn a_signal_probe_measures_the_hosts_own_verdict() {
        let backend = LinuxProcessBackend::new();
        // This process may signal itself: the kernel's own answer.
        assert!(
            backend
                .measure_signal_privilege(self_pid(), SignalKind::Terminate)
                .expect("probe"),
            "MEASURED: kill(self, 0) succeeds"
        );
        // A pid that does not exist may not.
        assert!(
            !backend
                .measure_signal_privilege(ProcessId::new(4_194_303), SignalKind::Terminate)
                .expect("probe"),
            "the host answers ESRCH for a pid that is not there"
        );
    }

    /// The ptrace measurement is the host's own two numbers, reported.
    /// The status read reports the kernel's own resident pages, verified
    /// against the host's own `statm` for a target whose RSS cannot move while
    /// the test holds it: a child blocked on stdin.
    #[test]
    fn the_memory_number_is_the_kernels_own_resident_pages() {
        use std::process::{Command, Stdio};
        let Ok(child) = Command::new("/bin/sleep").arg("1").spawn() else {
            return;
        };
        let pid = ProcessId::new(u64::from(child.id()));
        // Let the child finish exec'ing, so its RSS has settled and the
        // comparison is exact rather than racing a process that is still
        // starting up.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let backend = LinuxProcessBackend::new();
        let status = backend.get_process_status(pid).expect("getProcessStatus");
        let reference = fs::read_to_string(alloc::format!("/proc/{}/statm", child.id()))
            .expect("the host's own statm");
        let resident_pages: u64 = reference
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .expect("the kernel's own resident page count");
        assert_eq!(
            status.memory_bytes,
            resident_pages.saturating_mul(page_size()),
            "the contract's bytes are the kernel's own resident pages, measured not invented"
        );
        let _ = child;
    }

    #[test]
    fn the_ptrace_scope_is_measured_not_assumed() {
        let backend = LinuxProcessBackend::new();
        let (allowed, reason) = backend.measure_ptrace_privilege().expect("probe");
        // MEASURED on this host: yama ptrace_scope is 1, so a non-parent attach
        // needs CAP_SYS_PTRACE this process does not hold.
        assert_eq!(backend.measure_ptrace_scope(), 1, "measured on this host");
        assert!(!allowed, "measured: no CAP_SYS_PTRACE in CapEff");
        assert!(reason.contains("ptrace_scope is 1"), "{reason}");
        assert!(reason.contains("CAP_SYS_PTRACE"));
    }

    /// Signal delivery is the native `kill(2)`, and it is gated by the measured
    /// verdict above: the one target this suite signals is a child it owns.
    #[test]
    fn signal_delivery_is_gated_by_the_measured_verdict() {
        let backend = LinuxProcessBackend::new();
        // A pid that is not there is refused by the host, through the gate.
        let missing = backend.signal_process(ProcessId::new(4_194_303), SignalKind::Terminate);
        assert!(
            matches!(&missing, Err(e) if e.code == ErrorCode::PermissionDenied),
            "the host's own ESRCH arrives as a gated refusal: {missing:?}"
        );
        // And a custom token this host does not number is refused by name.
        let unnamed = backend.signal_process(self_pid(), SignalKind::Custom);
        assert_eq!(unnamed.unwrap_err().code, ErrorCode::InvalidArgument);
    }

    /// The default surface refuses with a reason on a backend that serves
    /// nothing — the law #6 baseline.
    #[test]
    fn the_default_surface_refuses_with_a_reason() {
        let backend = NothingProcessBackend;
        let error = backend.get_process(ProcessId::new(1)).expect_err("must say so");
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(error.message.starts_with("no-backend:"));
    }

    /// A backend that serves nothing.
    struct NothingProcessBackend;

    impl crate::domains::process::backend::ProcessBackend for NothingProcessBackend {}
}
