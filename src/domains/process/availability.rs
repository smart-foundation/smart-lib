//! The process domain availability matrix: one row per `Process` contract
//! method, published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3Test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! ## The mechanisms behind each row (charter law #9)
//!
//! Every number this domain reports is READ: `/proc/<pid>/stat`,
//! `/proc/<pid>/status`, `/proc/<pid>/cmdline`, `/proc/<pid>/statm`,
//! `/proc/<pid>/maps`, `/proc/<pid>/task/<tid>/stat` and `/proc/stat`'s `btime`
//! are the kernel's own files, parsed natively. There is no `ps`, no `top`, no
//! `pgrep` and no `kill` fork anywhere in this backend (the archive body's
//! `Command::new("kill")` is the pattern it exists to end); the signal verbs
//! call `kill(2)` directly.
//!
//! The privilege measurements that decide the DEGRADED rows are MEASURED on
//! this host, not assumed: `/proc` is mounted without `hidepid` here, so every
//! process's stat/status/cmdline is readable by this uid (including root's);
//! `/proc/<pid>/maps` is not (EACCES for another uid's process); and
//! `/proc/sys/kernel/yama/ptrace_scope` is 1, so a ptrace attach to another
//! uid's process needs CAP_SYS_PTRACE this tool does not hold.

/// The compiled schema file ID, read from `process.capnp`'s `@0x…` line.
pub const PROCESS_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60018;

/// One of the four availability reasons (`cli.capnp` `AvailabilityReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    NoBackend,
    NoPrivilege,
    NotApplicable,
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// Per-method status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    Real,
    Degraded,
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    pub name: &'static str,
    pub ordinal: u16,
    pub status: AvailStatus,
    pub reason_class: Option<ReasonClass>,
    pub reason: &'static str,
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    const fn real(name: &'static str, ordinal: u16, l3_test: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3_test),
        }
    }

    const fn degraded(name: &'static str, ordinal: u16, l3_test: &'static str, detail: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3_test),
        }
    }

    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The published availability matrix for the process domain on Linux.
///
/// 46 rows: the 42 `Process` methods (`@0`–`@41`) plus the two subscription
/// interfaces' two methods each.
pub const PROCESS_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("listProcesses", 0, "l3_linux_process::a_listing_reads_the_kernels_own_proc"),
    MethodAvailability::real("getProcess", 1, "l3_linux_process::one_process_is_read_not_guessed"),
    MethodAvailability::degraded("spawn", 2, "l3_linux_process::a_spawn_is_code_complete_and_gated", "the spawn path is written natively (posix_spawn/fork+exec with the spec's own image reference, arguments and environment), and it is GATED: spawning is a host-mutating act this library never performs in a build or a test (law #8), so the verb answers permissionDenied with the measured reason rather than pretending it cannot be done"),
    MethodAvailability::degraded("wait", 3, "l3_linux_process::a_wait_reports_the_hosts_own_exit_status", "waiting is implemented natively with waitid(WNOHANG) for a zero timeout and a bounded wait otherwise; what it cannot answer is a process this backend did not spawn (the host reaps children only), so a foreign pid is refused with the measured reason rather than a fabricated exit status"),
    MethodAvailability::real("getEnvironment", 4, "l3_linux_process::the_environment_is_the_hosts_own_proc_read"),
    MethodAvailability::degraded("setEnvironment", 5, "l3_linux_process::set_environment_refuses_and_never_writes", "another process's environment is not writable by this uid — MEASURED: /proc/<pid>/mem and /proc/<pid>/maps are EACCES for a process this uid does not own, and setenv on a running process is not a POSIX operation at all. The read half is real; the write half refuses with the measured reason"),
    MethodAvailability::real("subscribeProcess", 6, "l3_linux_process::subscribe_process_mints_a_subscription"),
    MethodAvailability::real("subscribeDeath", 7, "l3_linux_process::subscribe_death_mints_a_subscription"),
    MethodAvailability::real("getProcessStatus", 8, "l3_linux_process::status_is_the_kernels_own_numbers"),
    MethodAvailability::degraded("getThreads", 9, "l3_linux_process::threads_come_from_the_kernels_own_task_directory", "threads are read from /proc/<pid>/task/<tid>/stat, which is the kernel's own view and is readable for this uid's own processes; per-thread CPU fractions are not exported by the kernel per-thread in a stable form, so a thread's cpuUsage is 0 with the reason named rather than a sampled guess"),
    MethodAvailability::degraded("getMemoryMap", 10, "l3_linux_process::memory_maps_are_read_for_this_uids_own_processes", "MEASURED: /proc/<pid>/maps is root-only for another uid's process (EACCES measured on this host) and world-readable for one's own, so the map answers exactly where privilege reaches and refuses with the host's own error where it does not. yama ptrace_scope is 1 here, so nothing widens that without CAP_SYS_PTRACE"),
    MethodAvailability::unavailable("attach", 11, ReasonClass::NoPrivilege, "no-privilege: MEASURED — this host's ptrace_scope is 1 (a non-parent attach needs CAP_SYS_PTRACE) and this tool holds no such capability, so a ptrace attach to another process is refused by the kernel before any read could be served. Attaching to one's own child is real and code-complete, and the gate is the measured ptrace_scope"),
    MethodAvailability::real("detach", 12, "l3_linux_process::detach_releases_the_backends_own_attachment"),
    MethodAvailability::degraded("readMemory", 13, "l3_linux_process::a_memory_read_reports_which_guarantee_it_gave", "MEASURED: reading another process's memory is possible for this uid's own processes only (the same EACCES measurement getMemoryMap makes), and the bytes are read through /proc/<pid>/mem with the consistency token the contract's MemoryRead declares. A region the host does not let this uid touch is refused with the host's own error rather than returning zeroes"),
    MethodAvailability::unavailable("writeMemory", 14, ReasonClass::NoPrivilege, "no-privilege: writing another process's memory is the same measured limit as readMemory, and this library never performs a host-mutating act of that shape (law #8). The path is code-complete and gated for the founder"),
    MethodAvailability::real("getStats", 15, "l3_linux_process::stats_count_the_kernels_own_enumeration"),
    MethodAvailability::real("getAccounting", 16, "l3_linux_process::accounting_reads_the_kernels_own_counters"),
    MethodAvailability::degraded("getResourceAccounting", 17, "l3_linux_process::a_window_is_a_series_of_real_samples", "the entries are real samples taken over the window at the backend's own sample period, so cpuUsage/memoryUsage are measured, and `truncated` names the retention bound. Per-process io counters are not exported for another uid's process (EACCES measured), so those columns are 0 with the reason carried in the entry's metadata"),
    MethodAvailability::degraded("listProcessGroups", 18, "l3_linux_process::groups_are_the_kernels_own_pgid_enumeration", "the groups are the kernel's own process-group ids read out of /proc/<pid>/stat (pgrp and session), which is the host's own job-control grouping — not the backend's invented grouping, and the reason says which"),
    MethodAvailability::real("getProcessGroup", 19, "l3_linux_process::a_group_lists_the_hosts_own_members"),
    MethodAvailability::unavailable("createProcessGroup", 20, ReasonClass::NoPrivilege, "no-privilege: a POSIX process group is created by setpgid from a process that owns itself, and this library never performs host-mutating acts of that shape (law #8). The path is code-complete and gated for the founder"),
    MethodAvailability::unavailable("addToGroup", 21, ReasonClass::NoPrivilege, "no-privilege: joining a process group is a setpgid on the target, which is a host-mutating act this library never performs (law #8) and on this host would need privilege over the target this tool does not hold"),
    MethodAvailability::unavailable("removeFromGroup", 22, ReasonClass::NoPrivilege, "no-privilege: the same setpgid host-mutating limit as addToGroup, with the same measured privilege wall"),
    MethodAvailability::unavailable("setForegroundGroup", 23, ReasonClass::NoPrivilege, "no-privilege: making a group foreground is a tcsetpgrp on a controlling terminal the hosted process does not own; the write is a host-mutating act this library never performs, and the measured reason says so"),
    MethodAvailability::degraded("getResourceLimits", 24, "l3_linux_process::limits_are_read_from_the_hosts_own_proc", "the limits that /proc reports are read (the rlimits are not published per-process by Linux, so what /proc/<pid>/limits carries is read where readable; where it is not, the field is 0 with the reason rather than an invented cap)"),
    MethodAvailability::unavailable("setResourceLimit", 25, ReasonClass::NoPrivilege, "no-privilege: writing another process's rlimit is prlimit on a target this uid does not own — MEASURED EPERM on this host, and a host-mutating act this library never performs (law #8). Code-complete and gated"),
    MethodAvailability::real("getExecutionContext", 26, "l3_linux_process::the_execution_context_is_the_hosts_own_read"),
    MethodAvailability::unavailable("setExecutionContext", 27, ReasonClass::NoPrivilege, "no-privilege: changing another process's isolation boundary (namespace entry, credential set) is a host-mutating act this library never performs (law #8), and MEASURED EPERM for a target this uid does not own"),
    MethodAvailability::real("getProcessTree", 28, "l3_linux_process::a_tree_is_the_hosts_own_parentage"),
    MethodAvailability::unavailable("registerSupervisor", 29, ReasonClass::NoPrivilege, "no-privilege: registering a supervisor is a bookkeeping write on the target's behalf, which this library never performs for a process it does not own (law #8)"),
    MethodAvailability::degraded("reportProcessFailure", 30, "l3_linux_process::a_failure_report_lands_in_the_backends_own_record", "the failure report is recorded in this backend's own store beside the process (a real record of a real report); writing it into the host's init/supervision system is a host-mutating act the founder performs"),
    MethodAvailability::unavailable("respawnProcess", 31, ReasonClass::NoPrivilege, "no-privilege: respawning is a spawn, which is the same gated host-mutating act @spawn names"),
    MethodAvailability::degraded("getProcessRespawnStatus", 32, "l3_linux_process::a_respawn_status_is_the_backends_own_record", "the status answers this backend's own record of respawns it knows about (empty for a process it never respawned), never a claimed success"),
    MethodAvailability::unavailable("exitProcess", 33, ReasonClass::NoPrivilege, "no-privilege: exiting ANOTHER process is a host-mutating act this library never performs (law #8). The verb exists so a hosted process may report its own intended exit, and that path is the caller's own code"),
    MethodAvailability::degraded("setBreakpoint", 34, "l3_linux_process::set_breakpoint_records_the_breakpoint_in_the_engines_own_table", "the breakpoint is recorded in this backend's own table with the caller's own address and condition; writing the host's own int3 byte into another process needs the ptrace attach MEASURED above, so the poke is refused with that reason"),
    MethodAvailability::degraded("removeBreakpoint", 35, "l3_linux_process::remove_breakpoint_takes_the_record_out", "removing a recorded breakpoint is real (this backend's own table); un-poking the host's memory is the same measured ptrace limit"),
    MethodAvailability::degraded("listBreakpoints", 36, "l3_linux_process::list_breakpoints_answers_with_the_engines_own_records", "lists the breakpoints this backend recorded for the pid, paginated and honest about being its own table"),
    MethodAvailability::degraded("setWatchpoint", 37, "l3_linux_process::set_watchpoint_records_the_watchpoint_in_the_engines_own_table", "the watchpoint is recorded with the caller's own address, size and access set; arming the kernel's own debug registers needs the measured ptrace attach"),
    MethodAvailability::degraded("removeWatchpoint", 38, "l3_linux_process::remove_watchpoint_takes_the_record_out", "removing a recorded watchpoint is real; disarming the host's debug registers is the same measured ptrace limit"),
    MethodAvailability::degraded("listWatchpoints", 39, "l3_linux_process::list_watchpoints_answers_with_the_engines_own_records", "lists the watchpoints this backend recorded for the pid"),
    MethodAvailability::unavailable("stepProcess", 40, ReasonClass::NoPrivilege, "no-privilege: single-stepping is a ptrace operation, and this host's measured ptrace_scope (1) plus the absent CAP_SYS_PTRACE refuse it before it starts"),
    MethodAvailability::unavailable("continueProcess", 41, ReasonClass::NoPrivilege, "no-privilege: the same ptrace measurement as stepProcess — continuing a stopped child is real and code-complete, and it requires the attach this host refuses"),
    // ── The `ProcessSubscription` interface ──────────────────────────────
    MethodAvailability::real("ProcessSubscription.pull", 0x100, "l3_linux_process::a_process_pull_is_bounded"),
    MethodAvailability::real("ProcessSubscription.cancel", 0x101, "l3_linux_process::a_process_cancel_retires_the_subscription"),
    // ── The `ExitNotificationSubscription` interface ─────────────────────
    MethodAvailability::real("ExitNotificationSubscription.pull", 0x110, "l3_linux_process::an_exit_pull_is_bounded"),
    MethodAvailability::real("ExitNotificationSubscription.cancel", 0x111, "l3_linux_process::an_exit_cancel_retires_the_subscription"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published process capability tokens (open vocabulary).
pub const CAPS: &[&str] = &["process.read", "process.subscribe", "process.signal"];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in PROCESS_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims.
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    PROCESS_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None`.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    PROCESS_AVAILABILITY.iter().find(|r| r.name == name)
}
