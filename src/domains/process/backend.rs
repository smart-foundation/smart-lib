//! The `ProcessBackend` trait: one object-safe method per `Process` contract
//! method (`listProcesses @0` through `continueProcess @41`), plus the
//! `ProcessSubscription` and `ExitNotificationSubscription` interfaces.
//!
//! ## Capability story (law #5)
//!
//! Authority is the interface reference: holding the `Process` reference — or,
//! for inspection, an `attach`-derived one — is the authority. There is no
//! per-method token (R3) and no permission name on any verb.
//!
//! ## The honest default surface
//!
//! Each method has a default body answering `notSupported` **with a
//! `reasonClass`-prefixed reason** — the law #6 baseline a platform backend
//! overrides exactly where it serves something. The pre-revamp body answered
//! several of these with a hardcoded `MEM`/`CPU` pair and a zeroed pid; that is
//! the named past this default surface exists to end.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::ids::{BreakpointId, ProcessGroupId, ProcessId, UserId};
use crate::core::values::Duration;
use crate::core::{Error, Result};

use super::types::{
    Breakpoint, ExecutionContext, ExecutionState, ExitNotification, MemoryMapEntry, MemoryRead,
    ProcessFilter, ProcessGroup, ProcessInfo, ProcessSpawnSpec, ProcessStats, ProcessStatus,
    ProcessTree, Quota, RespawnStatus, ResourceAccountingWindow, ResourceUsage, SignalKind,
    ThreadInfo, Watchpoint,
};

/// A subscription over one process's observations. Holding the returned id IS
/// the authority to pull.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProcessSubscription(pub u64);

/// A subscription over one process's exit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExitSubscription(pub u64);

/// A protocol-compliant process backend.
pub trait ProcessBackend {
    fn list_processes(&self, _filter: &ProcessFilter, _page: super::types::Pagination) -> Result<super::types::Page<ProcessInfo>> {
        Err(Error::not_supported(
            "no-backend: the process listing is not served on this backend",
        ))
    }

    fn get_process(&self, _pid: ProcessId) -> Result<ProcessInfo> {
        Err(Error::not_supported(
            "no-backend: the process read is not served on this backend",
        ))
    }

    fn spawn(&self, _spec: &ProcessSpawnSpec) -> Result<ProcessId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: spawning is a host-mutating act this backend never performs",
        ))
    }

    fn wait(&self, _pid: ProcessId, _timeout: Duration) -> Result<super::types::ExitStatus> {
        Err(Error::not_supported(
            "no-backend: the wait path is not served on this backend",
        ))
    }

    fn get_environment(&self, _pid: ProcessId) -> Result<Vec<(String, String)>> {
        Err(Error::not_supported(
            "no-backend: the environment read is not served on this backend",
        ))
    }

    fn set_environment(&self, _pid: ProcessId, _env: &[(String, String)]) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: another process's environment is not writable by this backend",
        ))
    }

    fn subscribe_process(&self, _pid: ProcessId) -> Result<ProcessSubscription> {
        Err(Error::not_supported(
            "no-backend: process subscriptions are not served on this backend",
        ))
    }

    fn subscribe_death(&self, _pid: ProcessId) -> Result<ExitSubscription> {
        Err(Error::not_supported(
            "no-backend: exit notifications are not served on this backend",
        ))
    }

    fn get_process_status(&self, _pid: ProcessId) -> Result<ProcessStatus> {
        Err(Error::not_supported(
            "no-backend: the status read is not served on this backend",
        ))
    }

    fn get_threads(&self, _pid: ProcessId, _page: super::types::Pagination) -> Result<super::types::Page<ThreadInfo>> {
        Err(Error::not_supported(
            "no-backend: the thread read is not served on this backend",
        ))
    }

    fn get_memory_map(&self, _pid: ProcessId, _page: super::types::Pagination) -> Result<super::types::Page<MemoryMapEntry>> {
        Err(Error::not_supported(
            "no-backend: the memory-map read is not served on this backend",
        ))
    }

    fn attach(&self, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: a ptrace attach is refused by this host's ptrace_scope",
        ))
    }

    fn detach(&self, _pid: ProcessId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the detach path is not served on this backend",
        ))
    }

    fn read_memory(&self, _pid: ProcessId, _address: u64, _size: u64) -> Result<MemoryRead> {
        Err(Error::not_supported(
            "no-backend: the memory read is not served on this backend",
        ))
    }

    fn write_memory(&self, _pid: ProcessId, _address: u64, _data: &[u8]) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing another process's memory is a host-mutating act this backend never performs",
        ))
    }

    fn get_stats(&self, _user: UserId) -> Result<ProcessStats> {
        Err(Error::not_supported(
            "no-backend: the aggregate statistics are not served on this backend",
        ))
    }

    fn get_accounting(&self, _pid: ProcessId) -> Result<ResourceUsage> {
        Err(Error::not_supported(
            "no-backend: the accounting read is not served on this backend",
        ))
    }

    fn get_resource_accounting(
        &self,
        _pid: ProcessId,
        _duration: Duration,
    ) -> Result<ResourceAccountingWindow> {
        Err(Error::not_supported(
            "no-backend: the accounting window is not served on this backend",
        ))
    }

    fn list_process_groups(&self, _page: super::types::Pagination) -> Result<super::types::Page<ProcessGroup>> {
        Err(Error::not_supported(
            "no-backend: the process-group listing is not served on this backend",
        ))
    }

    fn get_process_group(&self, _group_id: ProcessGroupId) -> Result<ProcessGroup> {
        Err(Error::not_supported(
            "no-backend: the process-group read is not served on this backend",
        ))
    }

    fn create_process_group(&self, _name: &str) -> Result<ProcessGroupId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: creating a process group is a host-mutating act this backend never performs",
        ))
    }

    fn add_to_group(&self, _group_id: ProcessGroupId, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: joining a process group is a host-mutating act this backend never performs",
        ))
    }

    fn remove_from_group(&self, _group_id: ProcessGroupId, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: leaving a process group is a host-mutating act this backend never performs",
        ))
    }

    fn set_foreground_group(&self, _group_id: ProcessGroupId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: making a group foreground is a tcsetpgrp this backend never performs",
        ))
    }

    fn get_resource_limits(&self, _pid: ProcessId) -> Result<Quota> {
        Err(Error::not_supported(
            "no-backend: the resource-limit read is not served on this backend",
        ))
    }

    fn set_resource_limit(&self, _pid: ProcessId, _limit: &Quota) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing another process's rlimit is a host-mutating act this backend never performs",
        ))
    }

    fn get_execution_context(&self, _pid: ProcessId) -> Result<ExecutionContext> {
        Err(Error::not_supported(
            "no-backend: the execution-context read is not served on this backend",
        ))
    }

    fn set_execution_context(&self, _pid: ProcessId, _context: &ExecutionContext) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: changing a process's isolation boundary is a host-mutating act this backend never performs",
        ))
    }

    fn get_process_tree(&self, _pid: ProcessId, _max_depth: u32) -> Result<ProcessTree> {
        Err(Error::not_supported(
            "no-backend: the process tree is not served on this backend",
        ))
    }

    fn register_supervisor(&self, _supervisor: ProcessId, _target: ProcessId) -> Result<ProcessGroupId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: registering a supervisor is a host-mutating act this backend never performs",
        ))
    }

    fn report_process_failure(&self, _pid: ProcessId, _report: &super::types::FailureReport) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the failure report is not served on this backend",
        ))
    }

    fn respawn_process(&self, _pid: ProcessId) -> Result<ProcessId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: respawning is a spawn, which is the gated host-mutating act",
        ))
    }

    fn get_process_respawn_status(&self, _pid: ProcessId) -> Result<RespawnStatus> {
        Err(Error::not_supported(
            "no-backend: the respawn status is not served on this backend",
        ))
    }

    fn exit_process(&self, _reason: &super::types::FailureReport) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: exiting another process is a host-mutating act this backend never performs",
        ))
    }

    fn set_breakpoint(&self, _pid: ProcessId, _breakpoint: &Breakpoint) -> Result<BreakpointId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: arming a breakpoint is the same ptrace-limited act as attach",
        ))
    }

    fn remove_breakpoint(&self, _pid: ProcessId, _id: BreakpointId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the breakpoint table is not served on this backend",
        ))
    }

    fn list_breakpoints(&self, _pid: ProcessId) -> Result<Vec<Breakpoint>> {
        Err(Error::not_supported(
            "no-backend: the breakpoint table is not served on this backend",
        ))
    }

    fn set_watchpoint(&self, _pid: ProcessId, _watchpoint: &Watchpoint) -> Result<BreakpointId> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: arming a watchpoint is the same ptrace-limited act as attach",
        ))
    }

    fn remove_watchpoint(&self, _pid: ProcessId, _id: BreakpointId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: the watchpoint table is not served on this backend",
        ))
    }

    fn list_watchpoints(&self, _pid: ProcessId) -> Result<Vec<Watchpoint>> {
        Err(Error::not_supported(
            "no-backend: the watchpoint table is not served on this backend",
        ))
    }

    fn step_process(&self, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: single-stepping is a ptrace operation this host refuses",
        ))
    }

    fn continue_process(&self, _pid: ProcessId) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: continuing a stopped process is a ptrace operation this host refuses",
        ))
    }

    /// Pull one process observation (`ProcessSubscription.pull @0`).
    fn pull_process(&self, _subscription: ProcessSubscription, _max_infos: u32) -> Result<Vec<ProcessInfo>> {
        Err(Error::not_supported(
            "no-backend: process subscriptions are not served on this backend",
        ))
    }

    /// Retire a process subscription (`ProcessSubscription.cancel @1`).
    fn cancel_process(&self, _subscription: ProcessSubscription) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: process subscriptions are not served on this backend",
        ))
    }

    /// Pull one exit notification (`ExitNotificationSubscription.pull @0`).
    fn pull_exit(&self, _subscription: ExitSubscription, _max_notifications: u32) -> Result<Vec<ExitNotification>> {
        Err(Error::not_supported(
            "no-backend: exit notifications are not served on this backend",
        ))
    }

    /// Retire an exit subscription (`ExitNotificationSubscription.cancel @1`).
    fn cancel_exit(&self, _subscription: ExitSubscription) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: exit notifications are not served on this backend",
        ))
    }

    /// The measured signal-delivery verdict: may this uid deliver `signal` to
    /// `pid`? A READ-shaped probe (the host's own answer to `kill(pid, 0)`),
    /// never a write and never an assumption.
    fn measure_signal_privilege(&self, _pid: ProcessId, _signal: SignalKind) -> Result<bool> {
        Err(Error::not_supported(
            "no-backend: the signal-privilege probe is not served on this backend",
        ))
    }

    /// The measured ptrace verdict on this host, with the host's own words.
    fn measure_ptrace_privilege(&self) -> Result<(bool, String)> {
        Err(Error::not_supported(
            "no-backend: the ptrace-privilege probe is not served on this backend",
        ))
    }

    /// Deliver a signal to a process: the native `kill(2)`, DEGRADED-gated by
    /// the measured privilege above.
    fn signal_process(&self, _pid: ProcessId, _signal: SignalKind) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: signal delivery is not served on this backend",
        ))
    }

    /// Publish this backend's honest process capability tokens (law #6).
    fn process_caps(&self) -> Vec<&'static str> {
        alloc::vec!["process.read", "process.subscribe", "process.signal"]
    }
}

impl ProcessStatus {
    /// The state token a CLI renders, in the host's own vocabulary.
    pub fn state_token(&self) -> &'static str {
        self.state.as_token()
    }
}

impl ExecutionState {
    /// Whether this state means "the process is gone".
    pub fn is_gone(&self) -> bool {
        matches!(self, ExecutionState::Terminated)
    }
}
