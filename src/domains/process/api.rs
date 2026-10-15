//! The public process API: a thin, borrowing wrapper over [`ProcessBackend`]
//! exposing the handful of calls a client actually touches.

use alloc::vec::Vec;

use crate::core::ids::ProcessId;
use crate::core::values::Duration;
use crate::core::Result;

use super::backend::{ExitSubscription, ProcessBackend, ProcessSubscription};
use super::types::{
    ExecutionState, ExitNotification, MemoryRead, Page, Pagination, ProcessFilter, ProcessInfo,
    ProcessStatus, ProcessStats, SignalKind, ThreadInfo,
};

/// The combined view the CLI's `process` renders for one process: the info,
/// the status at the same instant, and the measured signal verdict — three
/// reads, one answer.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessSummary {
    pub info: ProcessInfo,
    pub status: ProcessStatus,
    /// Whether this uid may deliver a SIGTERM to this process (MEASURED).
    pub signal_terminate_measured: bool,
}

/// A borrowing handle to any [`ProcessBackend`].
pub struct Process<'a, B: ProcessBackend + ?Sized>(&'a B);

impl<'a, B: ProcessBackend + ?Sized> Process<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Process(backend)
    }

    /// The combined view for one process.
    #[must_use = "the reading is the answer; ignoring it is never useful"]
    pub fn summary(&self, pid: ProcessId) -> Result<ProcessSummary> {
        let info = self.0.get_process(pid)?;
        let status = self.0.get_process_status(pid)?;
        // MEASURED, not assumed: the kill(pid, 0) probe is the host's own
        // verdict about delivering a signal to this process.
        let signal_terminate_measured =
            self.0.measure_signal_privilege(pid, SignalKind::Terminate).unwrap_or(false);
        Ok(ProcessSummary {
            info,
            status,
            signal_terminate_measured,
        })
    }

    /// The first page of the process listing under the caller's filter.
    #[must_use]
    pub fn list(&self, filter: &ProcessFilter, page: Pagination) -> Result<Page<ProcessInfo>> {
        self.0.list_processes(filter, page)
    }

    /// Subscribe to one process, pull the first batch, and return both.
    #[must_use]
    pub fn subscribe(&self, pid: ProcessId) -> Result<(ProcessSubscription, Vec<ProcessInfo>)> {
        let subscription = self.0.subscribe_process(pid)?;
        let first = self.0.pull_process(subscription, 8)?;
        Ok((subscription, first))
    }

    /// Retire a subscription obtained from [`Process::subscribe`].
    pub fn close_subscription(&self, subscription: ProcessSubscription) -> Result<()> {
        self.0.cancel_process(subscription)
    }

    /// Subscribe to one process's exit and pull the first batch.
    #[must_use]
    pub fn subscribe_death(&self, pid: ProcessId) -> Result<(ExitSubscription, Vec<ExitNotification>)> {
        let subscription = self.0.subscribe_death(pid)?;
        let first = self.0.pull_exit(subscription, 8)?;
        Ok((subscription, first))
    }

    /// Retire an exit subscription.
    pub fn close_death_subscription(&self, subscription: ExitSubscription) -> Result<()> {
        self.0.cancel_exit(subscription)
    }

    /// One read of a process's memory, with the consistency the backend gave.
    #[must_use]
    pub fn read_memory(&self, pid: ProcessId, address: u64, size: u64) -> Result<MemoryRead> {
        self.0.read_memory(pid, address, size)
    }

    /// The aggregate statistics for one user (or everyone when `user` is 0).
    #[must_use]
    pub fn stats(&self, user: crate::core::ids::UserId) -> Result<ProcessStats> {
        self.0.get_stats(user)
    }

    /// The threads of one process.
    #[must_use]
    pub fn threads(&self, pid: ProcessId, page: Pagination) -> Result<Page<ThreadInfo>> {
        self.0.get_threads(pid, page)
    }

    /// Whether the host says this process is gone.
    #[must_use]
    pub fn is_gone(&self, pid: ProcessId) -> Result<bool> {
        let info = self.0.get_process(pid)?;
        Ok(info.state.is_gone())
    }

    /// Deliver a signal, DEGRADED-gated by the measured privilege verdict.
    pub fn signal(&self, pid: ProcessId, signal: SignalKind) -> Result<()> {
        let allowed = self.0.measure_signal_privilege(pid, signal)?;
        if !allowed {
            return Err(crate::core::Error::with_message(
                crate::core::ErrorCode::PermissionDenied,
                "no-privilege: the host's own kill(pid, 0) probe says this uid may not signal this process",
            ));
        }
        self.0.signal_process(pid, signal)
    }

    /// A bounded wait for one process to exit.
    pub fn wait(&self, pid: ProcessId, timeout: Duration) -> Result<super::types::ExitStatus> {
        self.0.wait(pid, timeout)
    }
}

impl ExecutionState {
    /// The open token the CLI renders.
    pub const fn token(&self) -> &'static str {
        self.as_token()
    }
}
