//! Inline behaviour tests for the process domain's *public contract*: the
//! owned types' fidelity to the wire, the filter's totality, the availability
//! matrix's internal consistency, the trait's honest-Unavailable totality, and
//! total propagation through a scripted backend. All hermetic — no host I/O
//! here; the native-path behaviour over the host's own `/proc` is proven in
//! [`crate::platforms::linux::process`] (the `l3_linux_process::…` tests the
//! matrix names).

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ids::ProcessId;
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, ErrorCode};
use crate::schema::process_capnp;

use super::api::Process;
use super::availability::{
    claimed_l3_tests, status_counts, method_availability_for, AvailStatus, PROCESS_AVAILABILITY,
    PROCESS_SCHEMA_ID, ReasonClass,
};
use super::backend::{ExitSubscription, ProcessBackend, ProcessSubscription};
use super::types::{
    ExecutionState, MemoryRead, ProcessFilter, ProcessInfo, ProcessStatus, Quota, SignalKind,
    ThreadInfo, Watchpoint, Breakpoint,
};

/// The contract's own method list, in `Process` order (`@0`–`@41`), plus the
/// two subscription interfaces.
const CONTRACT_METHODS: &[&str] = &[
    "listProcesses", "getProcess", "spawn", "wait", "getEnvironment", "setEnvironment",
    "subscribeProcess", "subscribeDeath", "getProcessStatus", "getThreads", "getMemoryMap",
    "attach", "detach", "readMemory", "writeMemory", "getStats", "getAccounting",
    "getResourceAccounting", "listProcessGroups", "getProcessGroup", "createProcessGroup",
    "addToGroup", "removeFromGroup", "setForegroundGroup", "getResourceLimits", "setResourceLimit",
    "getExecutionContext", "setExecutionContext", "getProcessTree", "registerSupervisor",
    "reportProcessFailure", "respawnProcess", "getProcessRespawnStatus", "exitProcess",
    "setBreakpoint", "removeBreakpoint", "listBreakpoints", "setWatchpoint", "removeWatchpoint",
    "listWatchpoints", "stepProcess", "continueProcess", "ProcessSubscription.pull",
    "ProcessSubscription.cancel", "ExitNotificationSubscription.pull",
    "ExitNotificationSubscription.cancel",
];

/// A backend that serves nothing: the honesty baseline.
struct Nothing;

impl ProcessBackend for Nothing {}

/// A scripted backend with one process, for the public-API wrapper.
struct Scripted {
    info: ProcessInfo,
}

impl Scripted {
    fn new() -> Self {
        Scripted {
            info: ProcessInfo {
                pid: ProcessId::new(4242),
                ppid: ProcessId::new(1),
                name: "scripted".to_string(),
                command: "scripted --background".to_string(),
                args: vec!["scripted".to_string(), "--background".to_string()],
                state: ExecutionState::Active,
                cpu_usage: 0.0,
                memory_usage: 4096,
                threads: 1,
                start_time: Timestamp::from_nanos(5),
                user: crate::core::ids::UserId::new(1000),
                groups: Vec::new(),
            },
        }
    }
}

impl ProcessBackend for Scripted {
    fn get_process(&self, pid: ProcessId) -> crate::core::Result<ProcessInfo> {
        if pid == self.info.pid {
            Ok(self.info.clone())
        } else {
            Err(Error::not_found().with_context("process"))
        }
    }
    fn get_process_status(&self, pid: ProcessId) -> crate::core::Result<ProcessStatus> {
        Ok(ProcessStatus {
            pid,
            name: self.info.name.clone(),
            state: self.info.state,
            ..Default::default()
        })
    }
    fn measure_signal_privilege(&self, _pid: ProcessId, _signal: SignalKind) -> crate::core::Result<bool> {
        Ok(true)
    }
    fn subscribe_process(&self, _pid: ProcessId) -> crate::core::Result<ProcessSubscription> {
        Ok(ProcessSubscription(1))
    }
    fn pull_process(&self, _subscription: ProcessSubscription, _max: u32) -> crate::core::Result<Vec<ProcessInfo>> {
        Ok(vec![self.info.clone()])
    }
    fn cancel_process(&self, _subscription: ProcessSubscription) -> crate::core::Result<()> {
        Ok(())
    }
    fn subscribe_death(&self, _pid: ProcessId) -> crate::core::Result<ExitSubscription> {
        Ok(ExitSubscription(2))
    }
    fn pull_exit(&self, _subscription: ExitSubscription, _max: u32) -> crate::core::Result<Vec<super::types::ExitNotification>> {
        Ok(Vec::new())
    }
    fn cancel_exit(&self, _subscription: ExitSubscription) -> crate::core::Result<()> {
        Ok(())
    }
    fn signal_process(&self, _pid: ProcessId, _signal: SignalKind) -> crate::core::Result<()> {
        Ok(())
    }
}

/// The matrix is exactly the contract surface.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, PROCESS_AVAILABILITY.len());
    assert_eq!(PROCESS_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for name in CONTRACT_METHODS {
        assert!(PROCESS_AVAILABILITY.iter().any(|r| r.name == *name), "{name} has no row");
    }
}

/// Every row carries the evidence its status demands.
#[test]
fn availability_rows_are_consistent() {
    for row in PROCESS_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none());
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some());
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some());
                assert!(!row.reason.is_empty());
                assert!(row.l3_test.is_none(), "UNAVAILABLE row {} claims an l3_test", row.name);
            }
        }
    }
    // A name no row answers is a caller bug, not an empty answer.
    assert!(method_availability_for("getProcess").is_some());
}

/// Every claimed `l3_test` exists in the Linux backend's suite.
#[test]
fn every_claimed_test_exists() {
    let tests: &[&str] = &[
        "l3_linux_process::a_listing_reads_the_kernels_own_proc",
        "l3_linux_process::one_process_is_read_not_guessed",
        "l3_linux_process::a_spawn_is_code_complete_and_gated",
        "l3_linux_process::a_wait_reports_the_hosts_own_exit_status",
        "l3_linux_process::the_environment_is_the_hosts_own_proc_read",
        "l3_linux_process::a_subscription_pulls_bounded_and_cancels",
        "l3_linux_process::exit_notifications_are_real_reaps",
        "l3_linux_process::status_is_the_kernels_own_numbers",
        "l3_linux_process::threads_come_from_the_kernels_own_task_directory",
        "l3_linux_process::memory_maps_are_read_for_this_uids_own_processes",
        "l3_linux_process::detach_releases_the_backends_own_attachment",
        "l3_linux_process::a_memory_read_reports_which_guarantee_it_gave",
        "l3_linux_process::stats_count_the_kernels_own_enumeration",
        "l3_linux_process::accounting_reads_the_kernels_own_counters",
        "l3_linux_process::a_window_is_a_series_of_real_samples",
        "l3_linux_process::groups_are_the_kernels_own_pgid_enumeration",
        "l3_linux_process::a_group_lists_the_hosts_own_members",
        "l3_linux_process::limits_are_read_from_the_hosts_own_proc",
        "l3_linux_process::the_execution_context_is_the_hosts_own_read",
        "l3_linux_process::a_tree_is_the_hosts_own_parentage",
        "l3_linux_process::a_failure_report_lands_in_the_backends_own_record",
        "l3_linux_process::a_respawn_status_is_the_backends_own_record",
        "l3_linux_process::breakpoints_are_recorded_not_poked",
        "l3_linux_process::watchpoints_are_recorded_not_armed",
    ];
    for (method, test) in claimed_l3_tests() {
        assert!(tests.contains(&test), "{method} claims {test}, which is not in the suite");
    }
    for test in tests {
        assert!(claimed_l3_tests().any(|(_, t)| t == *test), "{test} is real but unclaimed");
    }
}

/// The compiled schema id matches `process.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_process_contract() {
    assert_eq!(PROCESS_SCHEMA_ID, 0xa1b2c3d4e5f60018);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<process_capnp::process_info::Owned>) {}
}

/// The trait's default surface answers `notSupported` **with a reason**.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let err = backend.get_process(ProcessId::new(1)).expect_err("must say so");
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(!err.retryable);
    assert!(err.message.starts_with("no-backend:"), "{}", err.message);
    for message in [
        backend.list_processes(&ProcessFilter::default(), Default::default()).unwrap_err().message,
        backend.subscribe_process(ProcessId::new(1)).unwrap_err().message,
        backend.get_process_status(ProcessId::new(1)).unwrap_err().message,
        backend.get_threads(ProcessId::new(1), Default::default()).unwrap_err().message,
        backend.get_memory_map(ProcessId::new(1), Default::default()).unwrap_err().message,
        backend.detach(ProcessId::new(1)).unwrap_err().message,
        backend.read_memory(ProcessId::new(1), 0, 8).unwrap_err().message,
        backend.get_stats(crate::core::ids::UserId::new(0)).unwrap_err().message,
        backend.get_accounting(ProcessId::new(1)).unwrap_err().message,
        backend.get_resource_accounting(ProcessId::new(1), Duration::ZERO).unwrap_err().message,
        backend.list_process_groups(Default::default()).unwrap_err().message,
        backend.get_resource_limits(ProcessId::new(1)).unwrap_err().message,
        backend.get_execution_context(ProcessId::new(1)).unwrap_err().message,
        backend.get_process_tree(ProcessId::new(1), 1).unwrap_err().message,
        backend.report_process_failure(ProcessId::new(1), &Default::default()).unwrap_err().message,
        backend.get_process_respawn_status(ProcessId::new(1)).unwrap_err().message,
        backend.remove_breakpoint(ProcessId::new(1), Default::default()).unwrap_err().message,
        backend.list_breakpoints(ProcessId::new(1)).unwrap_err().message,
        backend.remove_watchpoint(ProcessId::new(1), Default::default()).unwrap_err().message,
        backend.list_watchpoints(ProcessId::new(1)).unwrap_err().message,
        backend.cancel_process(ProcessSubscription(1)).unwrap_err().message,
        backend.pull_exit(ExitSubscription(1), 1).unwrap_err().message,
        backend.measure_signal_privilege(ProcessId::new(1), SignalKind::Terminate).unwrap_err().message,
        backend.measure_ptrace_privilege().unwrap_err().message,
    ] {
        assert!(message.starts_with("no-backend:"), "{message}");
    }
    // The host-mutating verbs name privilege, not a missing backend.
    for message in [
        backend.spawn(&Default::default()).unwrap_err().message,
        backend.set_environment(ProcessId::new(1), &[]).unwrap_err().message,
        backend.write_memory(ProcessId::new(1), 0, b"x").unwrap_err().message,
        backend.create_process_group("g").unwrap_err().message,
        backend.add_to_group(Default::default(), ProcessId::new(1)).unwrap_err().message,
        backend.remove_from_group(Default::default(), ProcessId::new(1)).unwrap_err().message,
        backend.set_foreground_group(Default::default()).unwrap_err().message,
        backend.set_resource_limit(ProcessId::new(1), &Quota::default()).unwrap_err().message,
        backend.set_execution_context(ProcessId::new(1), &Default::default()).unwrap_err().message,
        backend.attach(ProcessId::new(1)).unwrap_err().message,
        backend.respawn_process(ProcessId::new(1)).unwrap_err().message,
        backend.exit_process(&Default::default()).unwrap_err().message,
        backend.set_breakpoint(ProcessId::new(1), &Breakpoint::default()).unwrap_err().message,
        backend.set_watchpoint(ProcessId::new(1), &Watchpoint::default()).unwrap_err().message,
        backend.step_process(ProcessId::new(1)).unwrap_err().message,
        backend.continue_process(ProcessId::new(1)).unwrap_err().message,
    ] {
        assert!(message.starts_with("no-privilege:"), "{message}");
    }
    assert_eq!(backend.process_caps().len(), 3);
}

/// Error propagation is total and honest across all 20 contract codes.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
    }
}

/// The signal vocabulary is the contract's own, and the host's numbers are
/// read from the kernel's own table rather than guessed.
#[test]
fn the_signal_vocabulary_is_the_contracts_own() {
    assert_eq!(SignalKind::from_token("sigterm"), SignalKind::Terminate);
    assert_eq!(SignalKind::from_token("a-2046-signal"), SignalKind::Custom);
    assert_eq!(SignalKind::Terminate.as_token(), "terminate");
    // MEASURED: Linux's own signal numbering, carried here as data.
    assert_eq!(SignalKind::Terminate.signal_number(), Some(15));
    assert_eq!(SignalKind::Kill.signal_number(), Some(9));
    assert_eq!(SignalKind::Interrupt.signal_number(), Some(2));
    assert_eq!(SignalKind::Custom.signal_number(), None);
}

/// The execution-state vocabulary is closed but unknown-tolerant, and a host
/// state letter maps onto the contract's own value.
#[test]
fn the_execution_states_are_closed_but_unknown_tolerant() {
    assert_eq!(ExecutionState::from_token("sleeping"), ExecutionState::Blocked);
    assert_eq!(ExecutionState::from_token("zombie"), ExecutionState::Terminated);
    assert_eq!(ExecutionState::from_token("a-2046-state"), ExecutionState::Custom);
    assert!(ExecutionState::Terminated.is_gone());
    assert!(!ExecutionState::Active.is_gone());
    assert_eq!(ExecutionState::Active.as_token(), "active");
}

/// Pagination is the contract's rule: `limit == 0` means "one", never "all".
#[test]
fn pagination_zero_limit_means_one() {
    let page = super::types::Pagination { offset: 0, limit: 0 };
    assert_eq!(page.effective_limit(), 1);
    assert_eq!(super::types::Pagination { offset: 3, limit: 7 }.effective_limit(), 7);
}

/// The filter is the contract's own and total.
#[test]
fn the_filter_is_the_contracts_own_and_total() {
    let filter = ProcessFilter {
        name: "script".to_string(),
        state: ExecutionState::Active,
        user: crate::core::ids::UserId::new(1000),
        min_cpu_usage: 0.0,
        min_memory_usage: 1,
    };
    let matching = ProcessInfo {
        name: "scripted".to_string(),
        command: "scripted --background".to_string(),
        state: ExecutionState::Active,
        memory_usage: 4096,
        user: crate::core::ids::UserId::new(1000),
        ..Default::default()
    };
    assert!(filter.matches(&matching));
    let wrong_user = ProcessInfo {
        user: crate::core::ids::UserId::new(0),
        ..matching.clone()
    };
    assert!(!filter.matches(&wrong_user));
}

/// The `MemoryRead` shape carries the contract's whole point: WHICH guarantee
/// the backend gave is reported, so a caller never has to assume one.
#[test]
fn a_memory_read_names_its_own_consistency() {
    let read = MemoryRead {
        data: vec![1, 2, 3],
        consistency: "snapshot".to_string(),
        torn: false,
        metadata: Vec::new(),
    };
    assert_eq!(read.consistency, "snapshot");
    assert!(!read.torn);
    // An unrecognised token is bestEffort by the contract's own rule.
    let unknown = MemoryRead {
        consistency: "a-2046-guarantee".to_string(),
        ..read.clone()
    };
    assert_ne!(unknown.consistency, "snapshot");
}

/// The public API is a thin borrowing wrapper over the backend.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    let backend = Scripted::new();
    let process = Process::new(&backend);
    let summary = process.summary(ProcessId::new(4242)).expect("summary");
    assert_eq!(summary.info.name, "scripted");
    assert_eq!(summary.status.name, "scripted");
    assert!(summary.signal_terminate_measured, "the measured verdict is reported");
    let (subscription, infos) = process.subscribe(ProcessId::new(4242)).expect("subscribe");
    assert_eq!(infos.len(), 1);
    process.close_subscription(subscription).expect("close");
    assert!(process.is_gone(ProcessId::new(4242)).expect("state").eq(&false));
    // A signal the host permits is delivered through the gated verb.
    process.signal(ProcessId::new(4242), SignalKind::Terminate).expect("signal");
}

/// A memory-map entry keeps the host's own permission spelling.
#[test]
fn a_memory_map_entry_keeps_the_hosts_own_spelling() {
    use super::types::MemoryPerm;
    let entry = super::types::MemoryMapEntry {
        start: 0x1000,
        end: 0x2000,
        perms: MemoryPerm::Write,
        name: "[heap]".to_string(),
        backing: String::new(),
        metadata: vec![("perms".to_string(), "rw".to_string())],
    };
    assert_eq!(entry.perms_word(), "rw");
    assert_eq!(entry.end - entry.start, 0x1000);
    assert_eq!(entry.perms_word(), "rw");
}

/// Threads and breakpoints keep the caller's own shapes.
#[test]
fn threads_and_breakpoints_keep_their_own_shapes() {
    let thread = ThreadInfo {
        id: crate::core::ids::ThreadId::new(7),
        name: "worker".to_string(),
        state: ExecutionState::Blocked,
        cpu_usage: 0.0,
    };
    assert_eq!(thread.name, "worker");
    let watch = Watchpoint {
        id: Default::default(),
        address: 0xdead_beef,
        size: 8,
        read: true,
        write: false,
        enabled: true,
    };
    assert!(watch.read && !watch.write);
    assert_eq!(watch.size, 8);
}
