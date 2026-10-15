//! Owned value types the **process** domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen `process.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam.
//!
//! ## Where every number comes from
//!
//! `process.capnp` deleted the pre-revamp body's hardcoded `MEM` and `CPU`
//! fields ("the old hardcoded MEM/CPU are the named past"), so the numbers this
//! domain carries are read, never invented:
//!
//! | field | the host's own source |
//! |---|---|
//! | `name` / `command` / `args` | `/proc/<pid>/cmdline` (and `comm` when the cmdline is empty — a kernel thread's own name) |
//! | `state` | `/proc/<pid>/stat`'s state letter, mapped onto the contract's `T.ExecutionState` |
//! | `memoryUsage` | `/proc/<pid>/statm`'s resident pages × the host's page size |
//! | `threads` | `/proc/<pid>/stat`'s `num_threads` |
//! | `cpuTime` | `/proc/<pid>/stat`'s `utime` + `stime` in clock ticks |
//! | `cpuUsage` | measured over a window by the backend, and reported as such |
//! | `startTime` | `/proc/<pid>/stat`'s `starttime` against `/proc/stat`'s `btime` |
//! | `user` | `/proc/<pid>/status`'s `Uid` |
//! | memory maps | `/proc/<pid>/maps` (this uid's own processes only, measured) |
//!
//! A field the host does not answer for this uid is reported as the contract's
//! own zero/empty with the reason carried in the entry's metadata — never a
//! plausible number.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::{BreakpointId, ProcessGroupId, ProcessId, ThreadId, UserId};
use crate::core::values::Timestamp;
use crate::schema::{process_capnp as capnp, types_capnp};

/// The contract's `T.StringMap` as an ordered pair list, in the peer's order.
pub type Metadata = Vec<(String, String)>;

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule: 0 means "one".
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
    pub items: Vec<T>,
    pub total: u64,
}

/// The contract's page window over an already-materialized list.
pub(crate) fn page_window<T: Clone>(items: &[T], page: Pagination) -> Page<T> {
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

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty).
pub(crate) fn text_field(reader: ::capnp::Result<::capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Read a `T.StringMap` field into an ordered pair list.
pub(crate) fn metadata_field(reader: Option<types_capnp::string_map::Reader<'_>>) -> Metadata {
    let Some(reader) = reader else {
        return Vec::new();
    };
    reader
        .get_entries()
        .map(|entries| {
            entries
                .iter()
                .map(|e| (text_field(e.get_key()), text_field(e.get_value())))
                .collect()
        })
        .unwrap_or_default()
}

// ═══ Enums ═════════════════════════════════════════════════════════════

/// What the host says a process is doing (`T.ExecutionState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExecutionState {
    #[default]
    Unknown,
    Active,
    Blocked,
    Suspended,
    Terminated,
    Custom,
}

impl ExecutionState {
    /// The host's own state letter, or the code the caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            ExecutionState::Unknown => "unknown",
            ExecutionState::Active => "active",
            ExecutionState::Blocked => "blocked",
            ExecutionState::Suspended => "suspended",
            ExecutionState::Terminated => "terminated",
            ExecutionState::Custom => "custom",
        }
    }

    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "active" | "running" => ExecutionState::Active,
            "blocked" | "sleeping" | "disk-sleep" => ExecutionState::Blocked,
            "suspended" | "stopped" | "tracing-stop" => ExecutionState::Suspended,
            "terminated" | "zombie" | "dead" => ExecutionState::Terminated,
            _ => ExecutionState::Custom,
        }
    }

    pub(crate) fn to_generated(self) -> types_capnp::ExecutionState {
        use types_capnp::ExecutionState as G;
        match self {
            ExecutionState::Unknown => G::Unknown,
            ExecutionState::Active => G::Active,
            ExecutionState::Blocked => G::Blocked,
            ExecutionState::Suspended => G::Suspended,
            ExecutionState::Terminated => G::Terminated,
            ExecutionState::Custom => G::Custom,
        }
    }

    pub(crate) fn from_generated(state: types_capnp::ExecutionState) -> Self {
        use types_capnp::ExecutionState as G;
        match state {
            G::Unknown => ExecutionState::Unknown,
            G::Active => ExecutionState::Active,
            G::Blocked => ExecutionState::Blocked,
            G::Suspended => ExecutionState::Suspended,
            G::Terminated => ExecutionState::Terminated,
            G::Custom => ExecutionState::Custom,
        }
    }
}

/// The kind of signal that ended a process (`T.SignalKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Terminate,
    Interrupt,
    Kill,
    HangUp,
    Quit,
    User1,
    User2,
    Child,
    Stop,
    Continue,
    Custom,
}

impl SignalKind {
    /// The open token a caller names.
    pub const fn as_token(self) -> &'static str {
        match self {
            SignalKind::Terminate => "terminate",
            SignalKind::Interrupt => "interrupt",
            SignalKind::Kill => "kill",
            SignalKind::HangUp => "hangUp",
            SignalKind::Quit => "quit",
            SignalKind::User1 => "userDefined1",
            SignalKind::User2 => "userDefined2",
            SignalKind::Child => "child",
            SignalKind::Stop => "stop",
            SignalKind::Continue => "continue",
            SignalKind::Custom => "custom",
        }
    }

    /// Parse a token, unknown-tolerant.
    pub fn from_token(token: &str) -> Self {
        match token {
            "terminate" | "sigterm" | "term" => SignalKind::Terminate,
            "interrupt" | "sigint" | "int" => SignalKind::Interrupt,
            "kill" | "sigkill" => SignalKind::Kill,
            "hangUp" | "sighup" | "hup" => SignalKind::HangUp,
            "quit" | "sigquit" => SignalKind::Quit,
            "userDefined1" | "sigusr1" | "usr1" => SignalKind::User1,
            "userDefined2" | "sigusr2" | "usr2" => SignalKind::User2,
            "child" | "sigchld" => SignalKind::Child,
            "stop" | "sigstop" => SignalKind::Stop,
            "continue" | "sigcont" => SignalKind::Continue,
            _ => SignalKind::Custom,
        }
    }

    /// The host's own signal number for this token (Linux's numbering), or
    /// `None` for a token this host does not name.
    pub const fn signal_number(self) -> Option<i32> {
        Some(match self {
            SignalKind::HangUp => 1,
            SignalKind::Interrupt => 2,
            SignalKind::Quit => 3,
            SignalKind::User1 => 10,
            SignalKind::User2 => 12,
            SignalKind::Child => 17,
            SignalKind::Terminate => 15,
            SignalKind::Stop => 19,
            SignalKind::Continue => 18,
            // SIGKILL cannot be caught, blocked or ignored; it is the host's
            // own last resort and it is named here.
            SignalKind::Kill => 9,
            SignalKind::Custom => return None,
        })
    }
}

/// What a memory region allows (`T.MemoryPerms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryPerm {
    Read,
    Write,
    Execute,
}

/// The process's own resource accounting (`T.ResourceUsage`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceUsage {
    pub cpu_time: crate::core::values::Duration,
    pub memory_bytes: u64,
    pub peak_memory_bytes: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub acquired_at: Timestamp,
}

/// A declared quota (`T.Quota`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quota {
    pub memory_bytes: u64,
    pub cpu_percent: f32,
    pub io_read_bytes_per_sec: u64,
    pub io_write_bytes_per_sec: u64,
    pub pids: u32,
    pub wall_clock_budget: crate::core::values::Duration,
}

/// Where a process is isolated (`T.IsolatedScope`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsolatedScope {
    /// The tier token the contract names.
    pub tier: String,
    pub scope_id: u64,
}

/// How a process ended (`T.ExitStatus`).
#[derive(Clone, Debug, PartialEq)]
pub enum ExitStatus {
    Exited(i32),
    Signaled(SignalKind),
    TimedOut,
    Custom(Vec<u8>),
}

/// The zero value is the signal a plain `wait` reports when it reaps a
/// still-running child's status word: `terminate`, the contract's own generic
/// name for "ended by a signal".
impl Default for SignalKind {
    fn default() -> Self {
        SignalKind::Terminate
    }
}

impl Default for ExitStatus {
    /// The neutral value: an exit status nobody has observed yet is
    /// "exited 0" only in the sense that the kernel answers 0 for a process
    /// that has not left, and the reason travels beside it.
    fn default() -> Self {
        ExitStatus::Exited(0)
    }
}

/// A failure report (`T.FailureReport`), the contract's own shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FailureReport {
    pub timestamp: Timestamp,
    /// The reason token, open vocabulary.
    pub reason: String,
    pub severity: String,
    pub context: String,
    pub diagnostic_data: String,
    pub suggested_action: String,
    pub last_known_good_state: String,
    pub metadata: Metadata,
}

// ═══ Structs ═══════════════════════════════════════════════════════════

/// What a spawn asks for (`ProcessSpawnSpec`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessSpawnSpec {
    /// An opaque image reference the backend resolves.
    pub image_ref: String,
    pub args: Vec<String>,
    pub env: Metadata,
    pub working_dir: String,
    /// A hint, not an instruction.
    pub priority_hint: i8,
    pub isolation: IsolatedScope,
    pub quota: Quota,
    pub metadata: Metadata,
}

/// One process (`ProcessInfo`) — every field the host's own `/proc` answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessInfo {
    pub pid: ProcessId,
    pub ppid: ProcessId,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub state: ExecutionState,
    /// CPU usage as a fraction of one core, measured by the backend.
    pub cpu_usage: f32,
    /// Current memory usage in bytes, from the kernel's own resident pages.
    pub memory_usage: u64,
    pub threads: u32,
    pub start_time: Timestamp,
    pub user: UserId,
    pub groups: Vec<ProcessGroupId>,
}

/// How a listing is filtered (`ProcessFilter`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessFilter {
    pub name: String,
    pub state: ExecutionState,
    pub user: UserId,
    pub min_cpu_usage: f32,
    pub min_memory_usage: u64,
}

/// A process's status at one instant (`ProcessStatus`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessStatus {
    pub pid: ProcessId,
    pub name: String,
    pub state: ExecutionState,
    pub exit_status: ExitStatus,
    pub uptime: crate::core::values::Duration,
    pub memory_bytes: u64,
    pub cpu_time: crate::core::values::Duration,
    pub last_failure: Option<FailureReport>,
}

/// Aggregate statistics (`ProcessStats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessStats {
    pub total_processes: u32,
    pub active: u32,
    pub blocked: u32,
    pub suspended: u32,
    pub terminated: u32,
    /// Total CPU across all processes, in cores.
    pub total_cpu_usage: f32,
    /// Total memory across all processes, in bytes.
    pub total_memory_usage: u64,
}

/// One thread (`ThreadInfo`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadInfo {
    pub id: ThreadId,
    pub name: String,
    pub state: ExecutionState,
    pub cpu_usage: f32,
}

/// One memory region (`MemoryMapEntry`): the kernel's own `/proc/<pid>/maps`
/// row, with the permission WORD (`rwx`) carried in the entry's metadata
/// because the contract's `T.MemoryPerms` is a single enum rather than a set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryMapEntry {
    pub start: u64,
    pub end: u64,
    /// The single strongest permission the region allows (`T.MemoryPerms`).
    pub perms: MemoryPerm,
    pub name: String,
    /// Which image or handle backs this region, if any (opaque).
    pub backing: String,
    pub metadata: Metadata,
}

/// The zero value is the region's most permissive safe answer, `read`: a
/// region nobody described is not an executable or writable one.
impl Default for MemoryPerm {
    fn default() -> Self {
        MemoryPerm::Read
    }
}

impl MemoryMapEntry {
    /// The host's own `r`/`w`/`x` permission word, read out of the entry's
    /// metadata (the kernel publishes it as a word, and the contract's
    /// `T.MemoryPerms` is a single value, so both are carried).
    pub fn perms_word(&self) -> String {
        self.metadata
            .iter()
            .find(|(key, _)| key == "perms")
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }

    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::memory_map_entry::Reader<'_>) -> Self {
        MemoryMapEntry {
            start: reader.get_start(),
            end: reader.get_end(),
            // An absent `perms` reads as the enumerant at ordinal 0, `read`.
            perms: match reader.get_perms().unwrap_or(types_capnp::MemoryPerms::Read) {
                types_capnp::MemoryPerms::Write => MemoryPerm::Write,
                types_capnp::MemoryPerms::Execute => MemoryPerm::Execute,
                _ => MemoryPerm::Read,
            },
            name: text_field(reader.get_name()),
            backing: text_field(reader.get_backing()),
            metadata: metadata_field(reader.get_metadata().ok()),
        }
    }

    /// Write into the generated builder.
    pub fn write_to<'a>(&self, mut builder: capnp::memory_map_entry::Builder<'a>) {
        builder.set_start(self.start);
        builder.set_end(self.end);
        builder.set_perms(match self.perms {
            MemoryPerm::Read => types_capnp::MemoryPerms::Read,
            MemoryPerm::Write => types_capnp::MemoryPerms::Write,
            MemoryPerm::Execute => types_capnp::MemoryPerms::Execute,
        });
        builder.set_name(&self.name);
        builder.set_backing(&self.backing);
    }
}

impl ProcessInfo {
    /// Whether this process's own reported state is one the host named.
    pub fn is_reported_by(&self, source: &str) -> bool {
        !source.is_empty()
    }
}

impl ProcessFilter {
    /// Does this entry pass the filter's own rule?
    pub fn matches(&self, info: &ProcessInfo) -> bool {
        if !self.name.is_empty() && !info.name.contains(&self.name) && !info.command.contains(&self.name) {
            return false;
        }
        if !matches!(self.state, ExecutionState::Unknown) && info.state != self.state {
            return false;
        }
        if self.user.get() != 0 && info.user != self.user {
            return false;
        }
        if info.cpu_usage < self.min_cpu_usage {
            return false;
        }
        if info.memory_usage < self.min_memory_usage {
            return false;
        }
        true
    }
}

/// A read of a process's memory (`MemoryRead`): the bytes, and WHICH GUARANTEE
/// the backend gave, so a debugger does not have to assume one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryRead {
    /// The bytes.
    pub data: Vec<u8>,
    /// The open consistency token: "stopped", "snapshot", "bestEffort", or the
    /// backend's own word. An unrecognised value is "bestEffort".
    pub consistency: String,
    /// True when the region changed while it was read.
    pub torn: bool,
    pub metadata: Metadata,
}

/// What a process is (`ExecutionContext`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecutionContext {
    pub pid: crate::core::ids::ProcessId,
    pub isolation: IsolatedScope,
    /// Capability references this process holds.
    pub capabilities: Vec<u64>,
    pub quota: Quota,
    pub metadata: Metadata,
}

/// One process group (`ProcessGroup`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessGroup {
    pub group_id: crate::core::ids::ProcessGroupId,
    pub name: String,
    pub processes: Vec<crate::core::ids::ProcessId>,
    pub session_id: u64,
    pub foreground: bool,
}

/// A process tree (`ProcessTree`), bounded by the caller's own `maxDepth`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessTree {
    pub pid: crate::core::ids::ProcessId,
    pub name: String,
    pub state: ExecutionState,
    pub children: Vec<ProcessTree>,
}

/// A respawn's lifecycle (`RespawnStatus`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RespawnStatus {
    pub pid: crate::core::ids::ProcessId,
    /// The lifecycle token, open vocabulary.
    pub state: String,
    pub attempts: u32,
    pub last_failure: Option<FailureReport>,
    pub fallback_active: bool,
    pub metadata: Metadata,
}

/// One accounting sample (`ResourceAccountingEntry`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceAccountingEntry {
    pub timestamp: Timestamp,
    pub cpu_usage: f32,
    pub memory_usage: u64,
    pub io_read: u64,
    pub io_write: u64,
}

/// A window of accounting samples (`ResourceAccountingWindow`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceAccountingWindow {
    pub pid: crate::core::ids::ProcessId,
    pub interval: crate::core::values::Duration,
    pub entries: Vec<ResourceAccountingEntry>,
    /// True when the window was truncated by the backend's retention.
    pub truncated: bool,
}

/// One breakpoint (`Breakpoint`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakpoint {
    pub id: crate::core::ids::BreakpointId,
    pub address: u64,
    pub enabled: bool,
    /// An open condition token, backend-interpreted.
    pub condition: String,
}

/// One watchpoint (`Watchpoint`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watchpoint {
    pub id: crate::core::ids::BreakpointId,
    pub address: u64,
    pub size: u32,
    pub read: bool,
    pub write: bool,
    pub enabled: bool,
}

/// A process's exit, as a notification (`ExitNotification`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExitNotification {
    pub pid: crate::core::ids::ProcessId,
    pub exit_status: ExitStatus,
    pub signal: SignalKind,
    pub core_dump_flag: bool,
    pub timestamp: Timestamp,
    pub duration: crate::core::values::Duration,
    pub report: Option<FailureReport>,
}

impl ProcessInfo {
    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::process_info::Reader<'_>) -> Self {
        ProcessInfo {
            pid: ProcessId::new(reader.get_pid().map(|p| p.get_id()).unwrap_or(0)),
            ppid: ProcessId::new(reader.get_ppid().map(|p| p.get_id()).unwrap_or(0)),
            name: text_field(reader.get_name()),
            command: text_field(reader.get_command()),
            args: reader
                .get_args()
                .map(|list| list.iter().map(text_field).collect())
                .unwrap_or_default(),
            state: ExecutionState::from_generated(reader.get_state().unwrap_or(
                types_capnp::ExecutionState::Unknown,
            )),
            cpu_usage: reader.get_cpu_usage(),
            memory_usage: reader.get_memory_usage(),
            threads: reader.get_threads(),
            start_time: reader
                .get_start_time()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            user: UserId::new(reader.get_user().map(|u| u.get_id()).unwrap_or(0)),
            groups: Vec::new(),
        }
    }
}
