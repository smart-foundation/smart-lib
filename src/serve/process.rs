//! The process domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `process.capnp`'s `interface Process` block (`@0`–`@41`), its
//! `ProcessSubscription` interface (`@0`–`@1`) and its
//! `ExitNotificationSubscription` interface (`@0`–`@1`). The dispatch is the
//! generated `ServerDispatch`; there is no ordinal, no method table and no
//! envelope of this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5).
//!
//! # What the engine can do that the contract does not name
//!
//! The engine's trait carries three helpers the contract has no method for:
//! the measured signal-delivery verdict, the measured ptrace verdict, and
//! the signal delivery itself (the contract moved signal delivery to
//! `signal.capnp`'s `SignalService`, which this schema does not serve). They
//! stay in the engine; the contract surface publishes only what the contract
//! declares.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::ids::{BreakpointId, ProcessGroupId, ProcessId, UserId};
use crate::core::values::Duration;
use crate::core::ErrorCode;
use crate::domains::process::availability;
use crate::domains::process::backend::{ExitSubscription, ProcessBackend, ProcessSubscription};
use crate::domains::process::types::{
    Breakpoint, ExecutionContext, ExitNotification, ExitStatus, FailureReport, IsolatedScope,
    MemoryRead, Metadata, Page, Pagination, ProcessFilter, ProcessGroup,
    ProcessInfo, ProcessSpawnSpec, ProcessStats, ProcessStatus, ProcessTree, Quota,
    ResourceAccountingWindow, ResourceUsage, SignalKind, Watchpoint,
};
use crate::schema::process_capnp::process as proc;
use crate::schema::process_capnp::process_subscription as psub;
use crate::schema::process_capnp::exit_notification_subscription as esub;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The process service: the domain's backend, and the class this connection
/// earned.
pub struct ProcessService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for ProcessService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> ProcessService<B>
where
    B: ProcessBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }

    /// The class's answer for a mutating verb on a connection that did not
    /// earn the right to change this host's processes (law #5). The verb is
    /// refused *before* the backend is reached: a change that is refused
    /// afterwards has already happened.
    fn gate(&self, verb: &str) -> Option<capnp::Error> {
        if self.class.is_control() {
            return None;
        }
        Some(errors::decline(
            ErrorCode::PermissionDenied,
            &format!(
                "permissionDenied: the {verb} verb changes this host's processes, and this \
                 connection holds the read-only class"
            ),
        ))
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then
/// goes to the backend. The answer comes BEFORE the class's gate: what the
/// host cannot do, no class can do.
fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Process contract method"),
        )),
    }
}
/// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

/// The contract's pagination as the domain's.
fn pagination(reader: &types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// The contract's duration as the domain's.
fn duration(reader: &types_capnp::duration::Reader<'_>) -> Duration {
    Duration::from_nanos(reader.get_nanoseconds())
}

/// Read a `Text` field that may be absent, into an owned string.
fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => super::text_of(text),
        Err(_) => String::new(),
    }
}

/// Write the engine's metadata map into the contract's `T.StringMap`.
fn write_metadata(
    metadata: &Metadata,
    builder: &mut types_capnp::string_map::Builder<'_>,
) {
    let mut list = builder.reborrow().init_entries(metadata.len() as u32);
    for (index, (key, value)) in metadata.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.set_key(key);
        entry.set_value(value);
    }
}

/// Read the contract's `T.StringMap` into the engine's metadata map.
fn read_metadata(reader: &types_capnp::string_map::Reader<'_>) -> Metadata {
    let mut metadata = Vec::new();
    if let Ok(entries) = reader.get_entries() {
        for entry in entries.iter() {
            metadata.push((text_field(entry.get_key()), text_field(entry.get_value())));
        }
    }
    metadata
}

/// The contract's `T.ExecutionState` as the domain's state.
fn execution_state(state: types_capnp::ExecutionState) -> crate::domains::process::types::ExecutionState {
    crate::domains::process::types::ExecutionState::from_generated(state)
}

/// The domain's state as the contract's `T.ExecutionState`.
fn execution_state_enum(
    state: crate::domains::process::types::ExecutionState,
) -> types_capnp::ExecutionState {
    state.to_generated()
}

/// The domain's signal as the contract's `T.SignalKind`.
fn signal_kind_enum(signal: SignalKind) -> types_capnp::SignalKind {
    match signal {
        SignalKind::Terminate => types_capnp::SignalKind::Terminate,
        SignalKind::Interrupt => types_capnp::SignalKind::Interrupt,
        SignalKind::Kill => types_capnp::SignalKind::Kill,
        SignalKind::HangUp => types_capnp::SignalKind::HangUp,
        SignalKind::User1 => types_capnp::SignalKind::UserDefined1,
        SignalKind::User2 => types_capnp::SignalKind::UserDefined2,
        // The contract's `SignalKind` has no `quit` member; the engine's
        // token maps to the contract's own open member rather than to a
        // guessed signal.
        SignalKind::Quit => types_capnp::SignalKind::Custom,
        SignalKind::Child => types_capnp::SignalKind::Child,
        SignalKind::Stop => types_capnp::SignalKind::Stop,
        SignalKind::Continue => types_capnp::SignalKind::Continue,
        SignalKind::Custom => types_capnp::SignalKind::Custom,
    }
}

/// The domain's exit status as the contract's `T.ExitStatus` union.
fn write_exit_status(
    status: &ExitStatus,
    builder: &mut types_capnp::exit_status::Builder<'_>,
) {
    match status {
        ExitStatus::Exited(code) => {
            builder.set_exited(*code);
        }
        ExitStatus::Signaled(signal) => {
            builder.set_signaled(signal_kind_enum(*signal));
        }
        ExitStatus::TimedOut => {
            builder.set_timed_out(());
        }
        ExitStatus::Custom(bytes) => {
            builder.set_custom(bytes);
        }
    }
}

/// The domain's quota as the contract's `T.Quota`.
fn write_quota(quota: &Quota, builder: &mut types_capnp::quota::Builder<'_>) {
    builder.set_memory_bytes(quota.memory_bytes);
    builder.set_cpu_percent(quota.cpu_percent);
    builder.set_io_read_bytes_per_sec(quota.io_read_bytes_per_sec);
    builder.set_io_write_bytes_per_sec(quota.io_write_bytes_per_sec);
    builder.set_pids(quota.pids);
    builder
        .reborrow()
        .init_wall_clock_budget()
        .set_nanoseconds(quota.wall_clock_budget.as_nanos());
}

/// The contract's `T.Quota` as the domain's quota.
fn read_quota(reader: &types_capnp::quota::Reader<'_>) -> Quota {
    Quota {
        memory_bytes: reader.get_memory_bytes(),
        cpu_percent: reader.get_cpu_percent(),
        io_read_bytes_per_sec: reader.get_io_read_bytes_per_sec(),
        io_write_bytes_per_sec: reader.get_io_write_bytes_per_sec(),
        pids: reader.get_pids(),
        wall_clock_budget: Duration::from_nanos(
            reader
                .get_wall_clock_budget()
                .map(|d| d.get_nanoseconds())
                .unwrap_or(0),
        ),
    }
}

/// The contract's `T.IsolatedScope` as the domain's scope.
fn read_isolated_scope(reader: &types_capnp::isolated_scope::Reader<'_>) -> IsolatedScope {
    IsolatedScope {
        tier: match reader.get_tier() {
            Ok(types_capnp::TierClass::Kernel) => "kernel".to_string(),
            Ok(types_capnp::TierClass::Platform) => "platform".to_string(),
            Ok(types_capnp::TierClass::System) => "system".to_string(),
            Ok(types_capnp::TierClass::Application) => "application".to_string(),
            Ok(types_capnp::TierClass::Sandbox) => "sandbox".to_string(),
            Ok(types_capnp::TierClass::Custom) => "custom".to_string(),
            _ => "unknown".to_string(),
        },
        scope_id: reader.get_scope_id(),
    }
}

/// The domain's scope as the contract's `T.IsolatedScope`.
fn write_isolated_scope(
    scope: &IsolatedScope,
    builder: &mut types_capnp::isolated_scope::Builder<'_>,
) {
    builder.set_tier(match scope.tier.as_str() {
        "kernel" => types_capnp::TierClass::Kernel,
        "platform" => types_capnp::TierClass::Platform,
        "system" => types_capnp::TierClass::System,
        "application" => types_capnp::TierClass::Application,
        "sandbox" => types_capnp::TierClass::Sandbox,
        "custom" => types_capnp::TierClass::Custom,
        // An unnamed tier is the contract's own `custom` member — never a
        // guessed isolation level.
        _ => types_capnp::TierClass::Custom,
    });
    builder.set_scope_id(scope.scope_id);
}

/// The domain's resource usage as the contract's `T.ResourceUsage`.
fn write_resource_usage(
    usage: &ResourceUsage,
    builder: &mut types_capnp::resource_usage::Builder<'_>,
) {
    builder.reborrow().init_cpu_time().set_nanoseconds(usage.cpu_time.as_nanos());
    builder.set_memory_bytes(usage.memory_bytes);
    builder.set_peak_memory_bytes(usage.peak_memory_bytes);
    builder.set_io_read_bytes(usage.io_read_bytes);
    builder.set_io_write_bytes(usage.io_write_bytes);
    builder.set_network_rx_bytes(usage.network_rx_bytes);
    builder.set_network_tx_bytes(usage.network_tx_bytes);
    builder
        .reborrow()
        .init_acquired_at()
        .set_nanoseconds(usage.acquired_at.as_nanos());
}

/// The domain's failure report as the contract's `T.FailureReport`.
fn write_failure_report(
    report: &FailureReport,
    builder: &mut types_capnp::failure_report::Builder<'_>,
) {
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(report.timestamp.as_nanos());
    builder.set_reason(failure_reason(&report.reason));
    builder.set_severity(failure_severity(&report.severity));
    builder.set_context(&report.context);
    builder.set_diagnostic_data(&report.diagnostic_data);
    builder.set_suggested_action(recovery_action(&report.suggested_action));
    builder.set_last_known_good_state(&report.last_known_good_state);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&report.metadata, &mut metadata);
}

/// The contract's `T.FailureReport` as the domain's report.
fn read_failure_report(reader: &types_capnp::failure_report::Reader<'_>) -> FailureReport {
    FailureReport {
        timestamp: crate::core::values::Timestamp::from_nanos(
            reader
                .get_timestamp()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
        ),
        reason: failure_reason_token(
            reader.get_reason().unwrap_or(types_capnp::FailureReason::Custom),
        ),
        severity: failure_severity_token(
            reader.get_severity().unwrap_or(types_capnp::FailureSeverity::Critical),
        ),
        context: text_field(reader.get_context()),
        diagnostic_data: text_field(reader.get_diagnostic_data()),
        suggested_action: recovery_action_token(
            reader
                .get_suggested_action()
                .unwrap_or(types_capnp::RecoveryAction::RequireManual),
        ),
        last_known_good_state: text_field(reader.get_last_known_good_state()),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The contract's failure reason as the enum the contract declares.
fn failure_reason(token: &str) -> types_capnp::FailureReason {
    match token {
        "timeout" => types_capnp::FailureReason::Timeout,
        "hardwareFault" => types_capnp::FailureReason::HardwareFault,
        "driverError" => types_capnp::FailureReason::DriverError,
        "memoryExhaustion" => types_capnp::FailureReason::MemoryExhaustion,
        "ioError" => types_capnp::FailureReason::IoError,
        "protocolViolation" => types_capnp::FailureReason::ProtocolViolation,
        "powerStateFailure" => types_capnp::FailureReason::PowerStateFailure,
        "thermalThrottling" => types_capnp::FailureReason::ThermalThrottling,
        "busError" => types_capnp::FailureReason::BusError,
        "authenticationFailure" => types_capnp::FailureReason::AuthenticationFailure,
        "panic" => types_capnp::FailureReason::Panic,
        _ => types_capnp::FailureReason::Custom,
    }
}

/// The enum the contract declares as the engine's open token.
fn failure_reason_token(reason: types_capnp::FailureReason) -> String {
    match reason {
        types_capnp::FailureReason::Timeout => "timeout",
        types_capnp::FailureReason::HardwareFault => "hardwareFault",
        types_capnp::FailureReason::DriverError => "driverError",
        types_capnp::FailureReason::MemoryExhaustion => "memoryExhaustion",
        types_capnp::FailureReason::IoError => "ioError",
        types_capnp::FailureReason::ProtocolViolation => "protocolViolation",
        types_capnp::FailureReason::PowerStateFailure => "powerStateFailure",
        types_capnp::FailureReason::ThermalThrottling => "thermalThrottling",
        types_capnp::FailureReason::BusError => "busError",
        types_capnp::FailureReason::AuthenticationFailure => "authenticationFailure",
        types_capnp::FailureReason::Panic => "panic",
        _ => "custom",
    }
    .to_string()
}

/// The contract's severity as the enum it declares.
fn failure_severity(token: &str) -> types_capnp::FailureSeverity {
    match token {
        "recoverable" => types_capnp::FailureSeverity::Recoverable,
        "critical" => types_capnp::FailureSeverity::Critical,
        "fatal" => types_capnp::FailureSeverity::Fatal,
        _ => types_capnp::FailureSeverity::Critical,
    }
}

/// The enum the contract declares as the engine's open token.
fn failure_severity_token(severity: types_capnp::FailureSeverity) -> String {
    match severity {
        types_capnp::FailureSeverity::Recoverable => "recoverable",
        types_capnp::FailureSeverity::Critical => "critical",
        types_capnp::FailureSeverity::Fatal => "fatal",
    }
    .to_string()
}

/// The contract's recovery action as the enum it declares.
fn recovery_action(token: &str) -> types_capnp::RecoveryAction {
    match token {
        "retry" => types_capnp::RecoveryAction::Retry,
        "retryWithBackoff" => types_capnp::RecoveryAction::RetryWithBackoff,
        "resetDevice" => types_capnp::RecoveryAction::ResetDevice,
        "resetBus" => types_capnp::RecoveryAction::ResetBus,
        "switchToFallback" => types_capnp::RecoveryAction::SwitchToFallback,
        "requireManual" => types_capnp::RecoveryAction::RequireManual,
        "escalate" => types_capnp::RecoveryAction::Escalate,
        "shutdown" => types_capnp::RecoveryAction::Shutdown,
        _ => types_capnp::RecoveryAction::RequireManual,
    }
}

/// The enum the contract declares as the engine's open token.
fn recovery_action_token(action: types_capnp::RecoveryAction) -> String {
    match action {
        types_capnp::RecoveryAction::Retry => "retry",
        types_capnp::RecoveryAction::RetryWithBackoff => "retryWithBackoff",
        types_capnp::RecoveryAction::ResetDevice => "resetDevice",
        types_capnp::RecoveryAction::ResetBus => "resetBus",
        types_capnp::RecoveryAction::SwitchToFallback => "switchToFallback",
        types_capnp::RecoveryAction::RequireManual => "requireManual",
        types_capnp::RecoveryAction::Escalate => "escalate",
        types_capnp::RecoveryAction::Shutdown => "shutdown",
    }
    .to_string()
}

/// Write the domain's process into the contract's `ProcessInfo`.
fn write_process_info(
    info: &ProcessInfo,
    builder: &mut crate::schema::process_capnp::process_info::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(info.pid.get());
    builder.reborrow().init_ppid().set_id(info.ppid.get());
    builder.set_name(&info.name);
    builder.set_command(&info.command);
    let mut args = builder.reborrow().init_args(info.args.len() as u32);
    for (index, arg) in info.args.iter().enumerate() {
        args.reborrow().set(index as u32, arg);
    }
    builder.set_state(execution_state_enum(info.state));
    builder.set_cpu_usage(info.cpu_usage);
    builder.set_memory_usage(info.memory_usage);
    builder.set_threads(info.threads);
    builder
        .reborrow()
        .init_start_time()
        .set_nanoseconds(info.start_time.as_nanos());
    builder.reborrow().init_user().set_id(info.user.get());
    let mut groups = builder.reborrow().init_groups(info.groups.len() as u32);
    for (index, group) in info.groups.iter().enumerate() {
        groups.reborrow().get(index as u32).set_id(group.get());
    }
}

/// Write the domain's process status into the contract's `ProcessStatus`.
fn write_process_status(
    status: &ProcessStatus,
    builder: &mut crate::schema::process_capnp::process_status::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(status.pid.get());
    builder.set_name(&status.name);
    builder.set_state(execution_state_enum(status.state));
    write_exit_status(&status.exit_status, &mut builder.reborrow().init_exit_status());
    builder
        .reborrow()
        .init_uptime()
        .set_nanoseconds(status.uptime.as_nanos());
    builder.set_memory_bytes(status.memory_bytes);
    builder
        .reborrow()
        .init_cpu_time()
        .set_nanoseconds(status.cpu_time.as_nanos());
    if let Some(failure) = &status.last_failure {
        write_failure_report(failure, &mut builder.reborrow().init_last_failure());
    }
}

/// Write the domain's thread into the contract's `ThreadInfo`.
fn write_thread_info(
    thread: &crate::domains::process::types::ThreadInfo,
    builder: &mut crate::schema::process_capnp::thread_info::Builder<'_>,
) {
    builder.reborrow().init_id().set_id(thread.id.get());
    builder.set_name(&thread.name);
    builder.set_state(execution_state_enum(thread.state));
    builder.set_cpu_usage(thread.cpu_usage);
}

/// Write the domain's memory read into the contract's `MemoryRead`.
fn write_memory_read(
    read: &MemoryRead,
    builder: &mut crate::schema::process_capnp::memory_read::Builder<'_>,
) {
    builder.reborrow().init_data().set_inline(&read.data);
    builder.set_consistency(&read.consistency);
    builder.set_torn(read.torn);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&read.metadata, &mut metadata);
}

/// Write the domain's statistics into the contract's `ProcessStats`.
fn write_process_stats(
    stats: &ProcessStats,
    builder: &mut crate::schema::process_capnp::process_stats::Builder<'_>,
) {
    builder.set_total_processes(stats.total_processes);
    builder.set_active(stats.active);
    builder.set_blocked(stats.blocked);
    builder.set_suspended(stats.suspended);
    builder.set_terminated(stats.terminated);
    builder.set_total_cpu_usage(stats.total_cpu_usage);
    builder.set_total_memory_usage(stats.total_memory_usage);
}

/// Write the domain's group into the contract's `ProcessGroup`.
fn write_process_group(
    group: &ProcessGroup,
    builder: &mut crate::schema::process_capnp::process_group::Builder<'_>,
) {
    builder.reborrow().init_group_id().set_id(group.group_id.get());
    builder.set_name(&group.name);
    let mut processes = builder.reborrow().init_processes(group.processes.len() as u32);
    for (index, pid) in group.processes.iter().enumerate() {
        processes.reborrow().get(index as u32).set_id(pid.get());
    }
    builder.reborrow().init_session_id().set_id(group.session_id);
    builder.set_foreground(group.foreground);
}

/// Write the domain's tree into the contract's `ProcessTree`.
fn write_process_tree(
    tree: &ProcessTree,
    builder: &mut crate::schema::process_capnp::process_tree::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(tree.pid.get());
    builder.set_name(&tree.name);
    builder.set_state(execution_state_enum(tree.state));
    let mut children = builder.reborrow().init_children(tree.children.len() as u32);
    for (index, child) in tree.children.iter().enumerate() {
        let mut item = children.reborrow().get(index as u32);
        write_process_tree(child, &mut item);
    }
}

/// Write the domain's respawn status into the contract's `RespawnStatus`.
fn write_respawn_status(
    status: &crate::domains::process::types::RespawnStatus,
    builder: &mut crate::schema::process_capnp::respawn_status::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(status.pid.get());
    builder.set_state(entity_state(&status.state));
    builder.set_attempts(status.attempts);
    if let Some(failure) = &status.last_failure {
        write_failure_report(failure, &mut builder.reborrow().init_last_failure());
    }
    builder.set_fallback_active(status.fallback_active);
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&status.metadata, &mut metadata);
}

/// The engine's open state token as the contract's `T.EntityState`.
fn entity_state(token: &str) -> types_capnp::EntityState {
    match token {
        "unknown" => types_capnp::EntityState::Unknown,
        "initializing" => types_capnp::EntityState::Initializing,
        "running" => types_capnp::EntityState::Running,
        "degraded" => types_capnp::EntityState::Degraded,
        "respawning" => types_capnp::EntityState::Respawning,
        "fallbackActive" => types_capnp::EntityState::FallbackActive,
        "awaitingManual" => types_capnp::EntityState::AwaitingManual,
        "permanentlyFailed" => types_capnp::EntityState::PermanentlyFailed,
        _ => types_capnp::EntityState::Unknown,
    }
}

/// Write the domain's accounting window into the contract's struct.
fn write_accounting_window(
    window: &ResourceAccountingWindow,
    builder: &mut crate::schema::process_capnp::resource_accounting_window::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(window.pid.get());
    builder
        .reborrow()
        .init_interval()
        .set_nanoseconds(window.interval.as_nanos());
    let mut list = builder.reborrow().init_entries(window.entries.len() as u32);
    for (index, entry) in window.entries.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        item.reborrow()
            .init_timestamp()
            .set_nanoseconds(entry.timestamp.as_nanos());
        item.set_cpu_usage(entry.cpu_usage);
        item.set_memory_usage(entry.memory_usage);
        item.set_io_read(entry.io_read);
        item.set_io_write(entry.io_write);
    }
    builder.set_truncated(window.truncated);
}

/// Write the domain's breakpoint into the contract's `Breakpoint`.
fn write_breakpoint(
    breakpoint: &Breakpoint,
    builder: &mut crate::schema::process_capnp::breakpoint::Builder<'_>,
) {
    builder.reborrow().init_id().set_id(breakpoint.id.get());
    builder.set_address(breakpoint.address);
    builder.set_enabled(breakpoint.enabled);
    builder.set_condition(&breakpoint.condition);
}

/// The contract's `Breakpoint` as the domain's.
fn read_breakpoint(
    reader: &crate::schema::process_capnp::breakpoint::Reader<'_>,
) -> Breakpoint {
    Breakpoint {
        id: BreakpointId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
        address: reader.get_address(),
        enabled: reader.get_enabled(),
        condition: text_field(reader.get_condition()),
    }
}

/// Write the domain's watchpoint into the contract's `Watchpoint`.
fn write_watchpoint(
    watchpoint: &Watchpoint,
    builder: &mut crate::schema::process_capnp::watchpoint::Builder<'_>,
) {
    builder.reborrow().init_id().set_id(watchpoint.id.get());
    builder.set_address(watchpoint.address);
    builder.set_size(watchpoint.size);
    builder.set_read(watchpoint.read);
    builder.set_write(watchpoint.write);
    builder.set_enabled(watchpoint.enabled);
}

/// The contract's `Watchpoint` as the domain's.
fn read_watchpoint(
    reader: &crate::schema::process_capnp::watchpoint::Reader<'_>,
) -> Watchpoint {
    Watchpoint {
        id: BreakpointId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
        address: reader.get_address(),
        size: reader.get_size(),
        read: reader.get_read(),
        write: reader.get_write(),
        enabled: reader.get_enabled(),
    }
}

/// Write the domain's execution context into the contract's struct.
fn write_execution_context(
    context: &ExecutionContext,
    builder: &mut crate::schema::process_capnp::execution_context::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(context.pid.get());
    write_isolated_scope(&context.isolation, &mut builder.reborrow().init_isolation());
    let mut handles = builder.reborrow().init_capabilities(context.capabilities.len() as u32);
    for (index, handle) in context.capabilities.iter().enumerate() {
        handles.reborrow().get(index as u32).set_handle(*handle);
    }
    write_quota(&context.quota, &mut builder.reborrow().init_quota());
    let mut metadata = builder.reborrow().init_metadata();
    write_metadata(&context.metadata, &mut metadata);
}

/// The contract's `ExecutionContext` as the domain's.
fn read_execution_context(
    reader: &crate::schema::process_capnp::execution_context::Reader<'_>,
) -> ExecutionContext {
    let mut capabilities = Vec::new();
    if let Ok(list) = reader.get_capabilities() {
        for handle in list.iter() {
            capabilities.push(handle.get_handle());
        }
    }
    ExecutionContext {
        pid: ProcessId::new(reader.get_pid().map(|p| p.get_id()).unwrap_or(0)),
        isolation: reader
            .get_isolation()
            .map(|s| read_isolated_scope(&s))
            .unwrap_or_default(),
        capabilities,
        quota: reader
            .get_quota()
            .map(|q| read_quota(&q))
            .unwrap_or_default(),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// Write the domain's exit notification into the contract's struct.
fn write_exit_notification(
    notification: &ExitNotification,
    builder: &mut crate::schema::process_capnp::exit_notification::Builder<'_>,
) {
    builder.reborrow().init_pid().set_id(notification.pid.get());
    write_exit_status(
        &notification.exit_status,
        &mut builder.reborrow().init_exit_status(),
    );
    builder.set_signal(signal_kind_enum(notification.signal));
    builder.set_core_dump_flag(notification.core_dump_flag);
    builder
        .reborrow()
        .init_timestamp()
        .set_nanoseconds(notification.timestamp.as_nanos());
    builder
        .reborrow()
        .init_duration()
        .set_nanoseconds(notification.duration.as_nanos());
    if let Some(report) = &notification.report {
        write_failure_report(report, &mut builder.reborrow().init_report());
    }
}

/// The contract's `ProcessFilter` as the domain's filter.
fn read_process_filter(
    reader: &crate::schema::process_capnp::process_filter::Reader<'_>,
) -> ProcessFilter {
    ProcessFilter {
        name: text_field(reader.get_name()),
        state: execution_state(reader.get_state().unwrap_or(types_capnp::ExecutionState::Unknown)),
        user: UserId::new(reader.get_user().map(|u| u.get_id()).unwrap_or(0)),
        min_cpu_usage: reader.get_min_cpu_usage(),
        min_memory_usage: reader.get_min_memory_usage(),
    }
}

/// The contract's `ProcessSpawnSpec` as the domain's spec.
fn read_spawn_spec(
    reader: &crate::schema::process_capnp::process_spawn_spec::Reader<'_>,
) -> ProcessSpawnSpec {
    let mut args = Vec::new();
    if let Ok(list) = reader.get_args() {
        for arg in list.iter() {
            args.push(text_field(arg));
        }
    }
    ProcessSpawnSpec {
        image_ref: text_field(reader.get_image_ref()),
        args,
        env: reader.get_env().map(|e| read_metadata(&e)).unwrap_or_default(),
        working_dir: text_field(reader.get_working_dir()),
        priority_hint: reader.get_priority_hint(),
        isolation: reader
            .get_isolation()
            .map(|s| read_isolated_scope(&s))
            .unwrap_or_default(),
        quota: reader.get_quota().map(|q| read_quota(&q)).unwrap_or_default(),
        metadata: reader
            .get_metadata()
            .map(|m| read_metadata(&m))
            .unwrap_or_default(),
    }
}

/// The process subscription the service hands back for `subscribeProcess @6`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct ProcessSubscriptionService<B> {
    backend: Arc<B>,
    subscription: ProcessSubscription,
}

impl<B> Clone for ProcessSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> psub::Server for ProcessSubscriptionService<B>
where
    B: ProcessBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: psub::PullParams,
        mut results: psub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_infos();
            let infos = backend
                .pull_process(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            // The contract's pull carries ONE info; the engine's hands over a
            // bounded batch. The batch's first entry is the snapshot the
            // contract's shape carries.
            if let Some(info) = infos.first() {
                write_process_info(info, &mut builder.init_info());
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: psub::CancelParams,
        mut results: psub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_process(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The exit-notification subscription the service hands back for
/// `subscribeDeath @7`.
pub struct ExitSubscriptionService<B> {
    backend: Arc<B>,
    subscription: ExitSubscription,
}

impl<B> Clone for ExitSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> esub::Server for ExitSubscriptionService<B>
where
    B: ProcessBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: esub::PullParams,
        mut results: esub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_notifications();
            let notifications = backend
                .pull_exit(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            if let Some(notification) = notifications.first() {
                write_exit_notification(notification, &mut builder.init_notification());
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: esub::CancelParams,
        mut results: esub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_exit(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

impl<B> proc::Server for ProcessService<B>
where
    B: ProcessBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn list_processes(
        self: Rc<Self>,
        params: proc::ListProcessesParams,
        mut results: proc::ListProcessesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = read_process_filter(&params.get_filter()?);
            let page = pagination(&params.get_page()?);
            let processes: Page<ProcessInfo> = backend
                .list_processes(&filter, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_processes(processes.items.len() as u32);
            for (index, info) in processes.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_process_info(info, &mut item);
            }
            builder.set_total(processes.total);
            Ok(())
        }
    }

    fn get_process(
        self: Rc<Self>,
        params: proc::GetProcessParams,
        mut results: proc::GetProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let info = backend
                .get_process(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_process_info(&info, &mut builder.init_info());
            Ok(())
        }
    }

    fn get_process_status(
        self: Rc<Self>,
        params: proc::GetProcessStatusParams,
        mut results: proc::GetProcessStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let status = backend
                .get_process_status(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_process_status(&status, &mut builder.init_status());
            Ok(())
        }
    }

    fn get_threads(
        self: Rc<Self>,
        params: proc::GetThreadsParams,
        mut results: proc::GetThreadsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let page = pagination(&params.get_page()?);
            let threads: Page<crate::domains::process::types::ThreadInfo> = backend
                .get_threads(pid, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_threads(threads.items.len() as u32);
            for (index, thread) in threads.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_thread_info(thread, &mut item);
            }
            builder.set_total(threads.total);
            Ok(())
        }
    }

    fn get_memory_map(
        self: Rc<Self>,
        params: proc::GetMemoryMapParams,
        mut results: proc::GetMemoryMapResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let page = pagination(&params.get_page()?);
            let regions: Page<crate::domains::process::types::MemoryMapEntry> = backend
                .get_memory_map(pid, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_regions(regions.items.len() as u32);
            for (index, region) in regions.items.iter().enumerate() {
                let item = list.reborrow().get(index as u32);
                region.write_to(item);
            }
            builder.set_total(regions.total);
            Ok(())
        }
    }

    fn get_environment(
        self: Rc<Self>,
        params: proc::GetEnvironmentParams,
        mut results: proc::GetEnvironmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let env = backend
                .get_environment(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut map = builder.init_env();
            let mut list = map.reborrow().init_entries(env.len() as u32);
            for (index, (key, value)) in env.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                entry.set_key(key);
                entry.set_value(value);
            }
            Ok(())
        }
    }

    fn get_stats(
        self: Rc<Self>,
        params: proc::GetStatsParams,
        mut results: proc::GetStatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let user = UserId::new(params.get_user()?.get_id());
            let stats = backend
                .get_stats(user)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_process_stats(&stats, &mut builder.init_stats());
            Ok(())
        }
    }

    fn get_accounting(
        self: Rc<Self>,
        params: proc::GetAccountingParams,
        mut results: proc::GetAccountingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let usage = backend
                .get_accounting(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_resource_usage(&usage, &mut builder.init_usage());
            Ok(())
        }
    }

    fn get_resource_accounting(
        self: Rc<Self>,
        params: proc::GetResourceAccountingParams,
        mut results: proc::GetResourceAccountingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let window = duration(&params.get_duration()?);
            let accounting: ResourceAccountingWindow = backend
                .get_resource_accounting(pid, window)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_accounting_window(&accounting, &mut builder.init_window());
            Ok(())
        }
    }

    fn list_process_groups(
        self: Rc<Self>,
        params: proc::ListProcessGroupsParams,
        mut results: proc::ListProcessGroupsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_page()?);
            let groups: Page<ProcessGroup> = backend
                .list_process_groups(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_groups(groups.items.len() as u32);
            for (index, group) in groups.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_process_group(group, &mut item);
            }
            builder.set_total(groups.total);
            Ok(())
        }
    }

    fn get_process_group(
        self: Rc<Self>,
        params: proc::GetProcessGroupParams,
        mut results: proc::GetProcessGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let group_id = ProcessGroupId::new(params.get_group_id()?.get_id());
            let group = backend
                .get_process_group(group_id)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_process_group(&group, &mut builder.init_group());
            Ok(())
        }
    }

    fn get_resource_limits(
        self: Rc<Self>,
        params: proc::GetResourceLimitsParams,
        mut results: proc::GetResourceLimitsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let quota = backend
                .get_resource_limits(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_quota(&quota, &mut builder.init_limits());
            Ok(())
        }
    }

    fn get_execution_context(
        self: Rc<Self>,
        params: proc::GetExecutionContextParams,
        mut results: proc::GetExecutionContextResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let context = backend
                .get_execution_context(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_execution_context(&context, &mut builder.init_context());
            Ok(())
        }
    }

    fn get_process_tree(
        self: Rc<Self>,
        params: proc::GetProcessTreeParams,
        mut results: proc::GetProcessTreeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let depth = params.get_max_depth();
            let tree = backend
                .get_process_tree(pid, depth)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_process_tree(&tree, &mut builder.init_tree());
            Ok(())
        }
    }

    fn get_process_respawn_status(
        self: Rc<Self>,
        params: proc::GetProcessRespawnStatusParams,
        mut results: proc::GetProcessRespawnStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let status = backend
                .get_process_respawn_status(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_respawn_status(&status, &mut builder.init_status());
            Ok(())
        }
    }

    fn list_breakpoints(
        self: Rc<Self>,
        params: proc::ListBreakpointsParams,
        mut results: proc::ListBreakpointsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let breakpoints = backend
                .list_breakpoints(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_breakpoints(breakpoints.len() as u32);
            for (index, breakpoint) in breakpoints.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_breakpoint(breakpoint, &mut item);
            }
            Ok(())
        }
    }

    fn list_watchpoints(
        self: Rc<Self>,
        params: proc::ListWatchpointsParams,
        mut results: proc::ListWatchpointsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let watchpoints = backend
                .list_watchpoints(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_watchpoints(watchpoints.len() as u32);
            for (index, watchpoint) in watchpoints.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_watchpoint(watchpoint, &mut item);
            }
            Ok(())
        }
    }

    fn read_memory(
        self: Rc<Self>,
        params: proc::ReadMemoryParams,
        mut results: proc::ReadMemoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let address = params.get_address();
            let size = params.get_size();
            let read = backend
                .read_memory(pid, address, size)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_memory_read(&read, &mut builder.init_read());
            Ok(())
        }
    }

    fn wait(
        self: Rc<Self>,
        params: proc::WaitParams,
        mut results: proc::WaitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let timeout = duration(&params.get_timeout()?);
            let status = backend
                .wait(pid, timeout)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_exit_status(&status, &mut builder.init_exit_status());
            Ok(())
        }
    }

    // ── The subscriptions ─────────────────────────────────────────────────

    fn subscribe_process(
        self: Rc<Self>,
        params: proc::SubscribeProcessParams,
        mut results: proc::SubscribeProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let subscription = backend
                .subscribe_process(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = ProcessSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: psub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn subscribe_death(
        self: Rc<Self>,
        params: proc::SubscribeDeathParams,
        mut results: proc::SubscribeDeathResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let subscription = backend
                .subscribe_death(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = ExitSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: esub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    // ── The writes ───────────────────────────────────────────────────────

    fn spawn(
        self: Rc<Self>,
        params: proc::SpawnParams,
        mut results: proc::SpawnResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("spawn") {
                return Err(refusal);
            }
            let params = params.get()?;
            let spec = read_spawn_spec(&params.get_spec()?);
            let pid = backend
                .spawn(&spec)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_pid().set_id(pid.get());
            Ok(())
        }
    }

    fn set_environment(
        self: Rc<Self>,
        params: proc::SetEnvironmentParams,
        mut results: proc::SetEnvironmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setEnvironment") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let env = read_metadata(&params.get_env()?);
            let outcome = backend.set_environment(pid, &env);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn attach(
        self: Rc<Self>,
        params: proc::AttachParams,
        mut results: proc::AttachResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("attach") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("attach") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.attach(pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn detach(
        self: Rc<Self>,
        params: proc::DetachParams,
        mut results: proc::DetachResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("detach") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.detach(pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn write_memory(
        self: Rc<Self>,
        params: proc::WriteMemoryParams,
        mut results: proc::WriteMemoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("writeMemory") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("writeMemory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let address = params.get_address();
            let data = params.get_data()?;
            let bytes = read_data_handle(&data);
            let outcome = backend.write_memory(pid, address, &bytes);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn create_process_group(
        self: Rc<Self>,
        params: proc::CreateProcessGroupParams,
        mut results: proc::CreateProcessGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("createProcessGroup") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("createProcessGroup") {
                return Err(refusal);
            }
            let params = params.get()?;
            let name = text_field(params.get_name());
            let group = backend
                .create_process_group(&name)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_group_id().set_id(group.get());
            Ok(())
        }
    }

    fn add_to_group(
        self: Rc<Self>,
        params: proc::AddToGroupParams,
        mut results: proc::AddToGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("addToGroup") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("addToGroup") {
                return Err(refusal);
            }
            let params = params.get()?;
            let group = ProcessGroupId::new(params.get_group_id()?.get_id());
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.add_to_group(group, pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn remove_from_group(
        self: Rc<Self>,
        params: proc::RemoveFromGroupParams,
        mut results: proc::RemoveFromGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("removeFromGroup") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("removeFromGroup") {
                return Err(refusal);
            }
            let params = params.get()?;
            let group = ProcessGroupId::new(params.get_group_id()?.get_id());
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.remove_from_group(group, pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_foreground_group(
        self: Rc<Self>,
        params: proc::SetForegroundGroupParams,
        mut results: proc::SetForegroundGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("setForegroundGroup") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("setForegroundGroup") {
                return Err(refusal);
            }
            let params = params.get()?;
            let group = ProcessGroupId::new(params.get_group_id()?.get_id());
            let outcome = backend.set_foreground_group(group);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_resource_limit(
        self: Rc<Self>,
        params: proc::SetResourceLimitParams,
        mut results: proc::SetResourceLimitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("setResourceLimit") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("setResourceLimit") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let quota = read_quota(&params.get_limit()?);
            let outcome = backend.set_resource_limit(pid, &quota);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_execution_context(
        self: Rc<Self>,
        params: proc::SetExecutionContextParams,
        mut results: proc::SetExecutionContextResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("setExecutionContext") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("setExecutionContext") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let context = read_execution_context(&params.get_context()?);
            let outcome = backend.set_execution_context(pid, &context);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn register_supervisor(
        self: Rc<Self>,
        params: proc::RegisterSupervisorParams,
        mut results: proc::RegisterSupervisorResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("registerSupervisor") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("registerSupervisor") {
                return Err(refusal);
            }
            let params = params.get()?;
            let supervisor = ProcessId::new(params.get_supervisor()?.get_id());
            let target = ProcessId::new(params.get_target()?.get_id());
            let handle = backend
                .register_supervisor(supervisor, target)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_handle().set_id(handle.get());
            Ok(())
        }
    }

    fn report_process_failure(
        self: Rc<Self>,
        params: proc::ReportProcessFailureParams,
        mut results: proc::ReportProcessFailureResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("reportProcessFailure") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let report = read_failure_report(&params.get_report()?);
            let outcome = backend.report_process_failure(pid, &report);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn respawn_process(
        self: Rc<Self>,
        params: proc::RespawnProcessParams,
        mut results: proc::RespawnProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("respawnProcess") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("respawnProcess") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let respawned = backend
                .respawn_process(pid)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_respawned().set_id(respawned.get());
            Ok(())
        }
    }

    fn exit_process(
        self: Rc<Self>,
        params: proc::ExitProcessParams,
        mut results: proc::ExitProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("exitProcess") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("exitProcess") {
                return Err(refusal);
            }
            let params = params.get()?;
            let reason = read_failure_report(&params.get_reason()?);
            let outcome = backend.exit_process(&reason);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_breakpoint(
        self: Rc<Self>,
        params: proc::SetBreakpointParams,
        mut results: proc::SetBreakpointResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setBreakpoint") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let breakpoint = read_breakpoint(&params.get_breakpoint()?);
            let id = backend
                .set_breakpoint(pid, &breakpoint)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_id().set_id(id.get());
            Ok(())
        }
    }

    fn remove_breakpoint(
        self: Rc<Self>,
        params: proc::RemoveBreakpointParams,
        mut results: proc::RemoveBreakpointResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("removeBreakpoint") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let id = BreakpointId::new(params.get_id()?.get_id());
            let outcome = backend.remove_breakpoint(pid, id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_watchpoint(
        self: Rc<Self>,
        params: proc::SetWatchpointParams,
        mut results: proc::SetWatchpointResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setWatchpoint") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let watchpoint = read_watchpoint(&params.get_watchpoint()?);
            let id = backend
                .set_watchpoint(pid, &watchpoint)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            builder.init_id().set_id(id.get());
            Ok(())
        }
    }

    fn remove_watchpoint(
        self: Rc<Self>,
        params: proc::RemoveWatchpointParams,
        mut results: proc::RemoveWatchpointResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("removeWatchpoint") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let id = BreakpointId::new(params.get_id()?.get_id());
            let outcome = backend.remove_watchpoint(pid, id);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn step_process(
        self: Rc<Self>,
        params: proc::StepProcessParams,
        mut results: proc::StepProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("stepProcess") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("stepProcess") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.step_process(pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn continue_process(
        self: Rc<Self>,
        params: proc::ContinueProcessParams,
        mut results: proc::ContinueProcessResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("continueProcess") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("continueProcess") {
                return Err(refusal);
            }
            let params = params.get()?;
            let pid = ProcessId::new(params.get_pid()?.get_id());
            let outcome = backend.continue_process(pid);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// Read the contract's `T.DataHandle` inline arm.
fn read_data_handle(handle: &types_capnp::data_handle::Reader<'_>) -> Vec<u8> {
    use types_capnp::data_handle::WhichReader;
    match handle.which() {
        Ok(WhichReader::Inline(bytes)) => bytes.map(|b| b.to_vec()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The process domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct ProcessDomain<B> {
    backend: Arc<B>,
}

impl<B> ProcessDomain<B>
where
    B: ProcessBackend + 'static,
{
    /// The process domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for ProcessDomain<B>
where
    B: ProcessBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        proc::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::PROCESS_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "process"
    }

    fn category(&self) -> &'static str {
        "process"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the process domain cannot answer twelve
    /// methods of its contract on this host (the ptrace and setpgid walls,
    /// measured), so the service is `degraded` and the missing methods are
    /// named in the registry.
    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    /// The commands this domain publishes through the registry (cli.capnp).
    ///
    /// The paths are the PRODUCT's own words — what `smart process …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "process list",
                "Process",
                "listProcesses",
                "the processes this host is running",
                0,
                true,
            )
            .with_row(matrix_row("listProcesses")),
            DomainCommand::composing(
                "process inspect",
                "Process",
                "getProcess",
                "one process's detail",
                1,
                false,
                "getProcessStatus,getAccounting",
            )
            .with_row(matrix_row("getProcess")),
            DomainCommand::new(
                "process status",
                "Process",
                "getProcessStatus",
                "one process's status",
                2,
                false,
            )
            .with_row(matrix_row("getProcessStatus")),
            DomainCommand::new(
                "process tree",
                "Process",
                "getProcessTree",
                "the process tree one process belongs to",
                3,
                false,
            )
            .with_row(matrix_row("getProcessTree")),
            DomainCommand::new(
                "process threads",
                "Process",
                "getThreads",
                "the threads of one process",
                4,
                false,
            )
            .with_row(matrix_row("getThreads")),
            DomainCommand::new(
                "process env",
                "Process",
                "getEnvironment",
                "one process's environment",
                5,
                false,
            )
            .with_row(matrix_row("getEnvironment")),
            DomainCommand::new(
                "process maps",
                "Process",
                "getMemoryMap",
                "one process's memory map",
                6,
                false,
            )
            .with_row(matrix_row("getMemoryMap")),
            DomainCommand::new(
                "process limits",
                "Process",
                "getResourceLimits",
                "one process's declared limits",
                7,
                false,
            )
            .with_row(matrix_row("getResourceLimits")),
            DomainCommand::new(
                "process accounting",
                "Process",
                "getAccounting",
                "one process's resource accounting",
                8,
                false,
            )
            .with_row(matrix_row("getAccounting")),
            DomainCommand::new(
                "process resource-accounting",
                "Process",
                "getResourceAccounting",
                "one process's accounting over a window",
                9,
                false,
            )
            .with_row(matrix_row("getResourceAccounting")),
            DomainCommand::new(
                "process groups",
                "Process",
                "listProcessGroups",
                "this host's process groups",
                10,
                true,
            )
            .with_row(matrix_row("listProcessGroups")),
            DomainCommand::new(
                "process group",
                "Process",
                "getProcessGroup",
                "one group's members",
                11,
                false,
            )
            .with_row(matrix_row("getProcessGroup")),
            DomainCommand::new(
                "process group create",
                "Process",
                "createProcessGroup",
                "create a process group",
                12,
                false,
            )
            .with_row(matrix_row("createProcessGroup")),
            DomainCommand::new(
                "process group add",
                "Process",
                "addToGroup",
                "add a process to a group",
                13,
                false,
            )
            .with_row(matrix_row("addToGroup")),
            DomainCommand::new(
                "process group remove",
                "Process",
                "removeFromGroup",
                "remove a process from a group",
                14,
                false,
            )
            .with_row(matrix_row("removeFromGroup")),
            DomainCommand::new(
                "process group foreground",
                "Process",
                "setForegroundGroup",
                "make a group the foreground group",
                15,
                false,
            )
            .with_row(matrix_row("setForegroundGroup")),
            DomainCommand::new(
                "process stats",
                "Process",
                "getStats",
                "the aggregate statistics over every process",
                16,
                true,
            )
            .with_row(matrix_row("getStats")),
            DomainCommand::new(
                "process context",
                "Process",
                "getExecutionContext",
                "one process's execution context",
                17,
                false,
            )
            .with_row(matrix_row("getExecutionContext")),
            DomainCommand::new(
                "process set-context",
                "Process",
                "setExecutionContext",
                "set one process's execution context",
                18,
                false,
            )
            .with_row(matrix_row("setExecutionContext")),
            DomainCommand::new(
                "process set-limit",
                "Process",
                "setResourceLimit",
                "set one process's declared limit",
                19,
                false,
            )
            .with_row(matrix_row("setResourceLimit")),
            DomainCommand::new(
                "process spawn",
                "Process",
                "spawn",
                "create a process from an opaque image reference",
                20,
                false,
            )
            .with_row(matrix_row("spawn")),
            DomainCommand::new(
                "process wait",
                "Process",
                "wait",
                "wait for one process to exit",
                21,
                false,
            )
            .with_row(matrix_row("wait")),
            DomainCommand::new(
                "process set-env",
                "Process",
                "setEnvironment",
                "set one process's environment",
                22,
                false,
            )
            .with_row(matrix_row("setEnvironment")),
            DomainCommand::new(
                "process attach",
                "Process",
                "attach",
                "attach to one process for inspection",
                23,
                false,
            )
            .with_row(matrix_row("attach")),
            DomainCommand::new(
                "process detach",
                "Process",
                "detach",
                "release an inspection attachment",
                24,
                false,
            )
            .with_row(matrix_row("detach")),
            DomainCommand::new(
                "process read-memory",
                "Process",
                "readMemory",
                "read one process's memory, with the guarantee named",
                25,
                false,
            )
            .with_row(matrix_row("readMemory")),
            DomainCommand::new(
                "process write-memory",
                "Process",
                "writeMemory",
                "write one process's memory",
                26,
                false,
            )
            .with_row(matrix_row("writeMemory")),
            DomainCommand::composing(
                "process watch",
                "Process",
                "subscribeProcess",
                "a live process snapshot stream",
                27,
                false,
                "pull,cancel",
            )
            .with_row(matrix_row("subscribeProcess")),
            DomainCommand::composing(
                "process watch-exit",
                "Process",
                "subscribeDeath",
                "the exit notification for one process",
                28,
                false,
                "pull,cancel",
            )
            .with_row(matrix_row("subscribeDeath")),
            DomainCommand::new(
                "process supervise",
                "Process",
                "registerSupervisor",
                "register a supervisor for one process",
                29,
                false,
            )
            .with_row(matrix_row("registerSupervisor")),
            DomainCommand::new(
                "process report-failure",
                "Process",
                "reportProcessFailure",
                "record a failure report for one process",
                30,
                false,
            )
            .with_row(matrix_row("reportProcessFailure")),
            DomainCommand::new(
                "process respawn",
                "Process",
                "respawnProcess",
                "respawn one process",
                31,
                false,
            )
            .with_row(matrix_row("respawnProcess")),
            DomainCommand::new(
                "process respawn-status",
                "Process",
                "getProcessRespawnStatus",
                "one process's respawn status",
                32,
                false,
            )
            .with_row(matrix_row("getProcessRespawnStatus")),
            DomainCommand::new(
                "process exit",
                "Process",
                "exitProcess",
                "report this process's own intended exit",
                33,
                false,
            )
            .with_row(matrix_row("exitProcess")),
            DomainCommand::new(
                "process breakpoint set",
                "Process",
                "setBreakpoint",
                "record a breakpoint for one process",
                34,
                false,
            )
            .with_row(matrix_row("setBreakpoint")),
            DomainCommand::new(
                "process breakpoint remove",
                "Process",
                "removeBreakpoint",
                "remove a recorded breakpoint",
                35,
                false,
            )
            .with_row(matrix_row("removeBreakpoint")),
            DomainCommand::new(
                "process breakpoint list",
                "Process",
                "listBreakpoints",
                "the breakpoints recorded for one process",
                36,
                false,
            )
            .with_row(matrix_row("listBreakpoints")),
            DomainCommand::new(
                "process watchpoint set",
                "Process",
                "setWatchpoint",
                "record a watchpoint for one process",
                37,
                false,
            )
            .with_row(matrix_row("setWatchpoint")),
            DomainCommand::new(
                "process watchpoint remove",
                "Process",
                "removeWatchpoint",
                "remove a recorded watchpoint",
                38,
                false,
            )
            .with_row(matrix_row("removeWatchpoint")),
            DomainCommand::new(
                "process watchpoint list",
                "Process",
                "listWatchpoints",
                "the watchpoints recorded for one process",
                39,
                false,
            )
            .with_row(matrix_row("listWatchpoints")),
            DomainCommand::new(
                "process step",
                "Process",
                "stepProcess",
                "single-step one process",
                40,
                false,
            )
            .with_row(matrix_row("stepProcess")),
            DomainCommand::new(
                "process continue",
                "Process",
                "continueProcess",
                "continue one stopped process",
                41,
                false,
            )
            .with_row(matrix_row("continueProcess")),
            DomainCommand::new(
                "process pull",
                "ProcessSubscription",
                "pull",
                "take the next snapshot a process subscription has",
                42,
                true,
            )
            .with_row(matrix_row("ProcessSubscription.pull")),
            DomainCommand::new(
                "process subscription cancel",
                "ProcessSubscription",
                "cancel",
                "retire a process subscription",
                43,
                false,
            )
            .with_row(matrix_row("ProcessSubscription.cancel")),
            DomainCommand::new(
                "process exit pull",
                "ExitNotificationSubscription",
                "pull",
                "take the exit notification a subscription has",
                44,
                true,
            )
            .with_row(matrix_row("ExitNotificationSubscription.pull")),
            DomainCommand::new(
                "process exit cancel",
                "ExitNotificationSubscription",
                "cancel",
                "retire an exit-notification subscription",
                45,
                false,
            )
            .with_row(matrix_row("ExitNotificationSubscription.cancel")),
        ]
    }

    /// The process schema's node tree: what a client that never compiled
    /// `process.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/process.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = ProcessService::new(Arc::clone(&self.backend), class);
        let client: proc::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
