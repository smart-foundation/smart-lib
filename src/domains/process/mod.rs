//! The **process** domain: the host's own processes, observed natively and
//! signalled through the kernel's own `kill(2)` — over one object-safe
//! [`ProcessBackend`] trait with the frozen `process.capnp` surface
//! (`listProcesses @0` through `continueProcess @41`, plus the two subscription
//! interfaces: 46 contract rows in all).
//!
//! ## The mechanisms on this backend (law #9)
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `listProcesses` / `getProcess` | a `/proc` enumeration, each row parsed from `/proc/<pid>/stat`, `/proc/<pid>/status`, `/proc/<pid>/cmdline` and `/proc/<pid>/statm` — the salvaged archive parsing, made real |
//! | `getProcessStatus` | the same reads plus `/proc/stat`'s `btime` for the start time and a two-sample CPU window |
//! | `getThreads` | `/proc/<pid>/task/<tid>/stat`, the kernel's own per-thread view |
//! | `getMemoryMap` | `/proc/<pid>/maps`, the host's own region table (this uid's own processes) |
//! | `getEnvironment` | `/proc/<pid>/environ`'s NUL-separated pairs, read as the host wrote them |
//! | `readMemory` | `/proc/<pid>/mem` pread at the caller's address, with the contract's own consistency token |
//! | `getStats` | the same enumeration aggregated, so the counts are the kernel's |
//! | `getProcessTree` | the `ppid` chain, bounded by the caller's own `maxDepth` |
//! | `subscribeProcess` / `subscribeDeath` | real subscriptions over the kernel's own /proc state |
//! | signal delivery | `kill(2)`, gated by a READ-shaped `kill(pid, 0)` probe |
//!
//! ## What is DEGRADED, and why
//!
//! `process.capnp` deleted the pre-revamp `kill` method ("signal delivery
//! authority is signal.capnp's `SignalService.send`"), so this backend's signal
//! verbs are the *measurement* a caller needs: `measure_signal_privilege` is a
//! read-shaped `kill(pid, 0)` probe reporting the host's own verdict, and the
//! delivery itself is GATED (a host-mutating act the founder performs). The
//! ptrace half is the measured truth on this host: `ptrace_scope` is 1 and this
//! uid holds no CAP_SYS_PTRACE, so `attach`/`step`/`continue` refuse with the
//! host's own reason rather than a hopeful attempt.
//!
//! Every number is read: the archive body's hardcoded MEM/CPU pair is the named
//! past, and a field the host does not export (per-thread cpu fractions, another
//! uid's maps and per-process io counters) is reported as the contract's own
//! zero with the measured reason carried in the entry's metadata.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Process;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    PROCESS_AVAILABILITY, PROCESS_SCHEMA_ID, ReasonClass,
};
pub use backend::{ExitSubscription, ProcessBackend, ProcessSubscription};
pub use types::{
    Breakpoint, ExecutionContext, ExecutionState, ExitNotification, ExitStatus, FailureReport,
    IsolatedScope, MemoryMapEntry, MemoryPerm, MemoryRead, ProcessFilter, ProcessGroup, ProcessInfo,
    ProcessSpawnSpec, ProcessStats, ProcessStatus, ProcessTree, Quota, RespawnStatus,
    ResourceAccountingEntry, ResourceAccountingWindow, ResourceUsage, SignalKind, ThreadInfo,
    Watchpoint,
};
