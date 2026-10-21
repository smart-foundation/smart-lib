//! The service domain availability matrix: one row per `Service` contract
//! method (plus [`ServiceControl`]'s verbs, the subscriptions' helpers and the
//! lifecycle bridge), published as data exactly the way
//! `knowledge/03-CONFORMANCE.md` §3.2 prescribes. A `REAL`/`DEGRADED` claim
//! names the `l3_test` that backs it; an `UNAVAILABLE` row carries a nameable
//! `reasonClass` and a reason the live behaviour repeats at runtime.
//!
//! This table is the contract with the availability generator and `cli.capnp`'s
//! `listUnavailable`: every non-`REAL` row is answered at runtime by the method
//! returning the matching `notSupported`/`unavailable` whose message begins
//! with the row's `reasonClass`.
//!
//! ## The native decision that splits the matrix
//!
//! The archive body forked `sv`/`systemctl`/`rc-service` per call and parsed
//! their text (P5). The current Linux backend reads supervision **natively**,
//! one path per supervisor: runit's supervise tree, systemd's system bus,
//! openrc's own state files — see `crate::platforms::linux::service`. So the
//! observation surface (`listServices`, `getService`, `getState`, `resolve`)
//! is served from the supervisor's own state, while the surface a host
//! supervisor genuinely does not own (a declared quota, a health-check config,
//! application metrics, all seven `ServiceControl` registry verbs) is
//! `UNAVAILABLE` with a reason class — the shape of the domain, not a gap this
//! wave forgot to fill.
//!
//! ## Runtime honesty on top of the matrix
//!
//! A row is the backend's *target*. What a caller actually gets is also gated
//! by the host's permissions: on this box the runit supervise tree
//! (`/run/runit/supervise.<name>/stat`, `pid`) is **root-only**, so the exact
//! half of the state read is unreachable for uid 1000 and is published as the
//! DEGRADED `readSuperviseStat` row with the founder's privileged command.
//! The state itself is NOT parked behind that wall: the same read path derives
//! it for every uid from the supervised process truth in `/proc` (the `runsv`
//! process, its live children, the service's own pid file, the kernel's boot
//! clock), so `getState` answers real up/down states everywhere and only the
//! supervisor's own exact words are privilege-gated. The gate that remains is
//! [`runtime_gate`], computed from the backend's measured caps — `can_read_state`
//! measures true here because the `/proc` derivation answers, and
//! `can_read_exact_state` measures false because the exact half is root-only —
//! so the runtime answer and this table can never disagree.

/// The compiled schema file ID, read from `service.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const SERVICE_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60021;

/// The CLI's availability reasons (`cli.capnp` `AvailabilityReason`), re-used
/// rather than a second grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// No implementation exists for this (domain, platform).
    NoBackend,
    /// The host primitive exists but the caller's credentials do not reach it.
    NoPrivilege,
    /// The concept does not exist on this platform.
    NotApplicable,
    /// Exists, held back deliberately.
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    /// The exact token the matrix, the tool reason and the CLI agree on.
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
    /// Passes its full behaviour on this platform; an `l3_test` proves it.
    Real,
    /// Answers, but not completely; `reason` names the degraded capability and
    /// an `l3_test` still backs it.
    Degraded,
    /// Answers `notSupported`/`unavailable` with a `reasonClass`-prefixed reason.
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract method name (`service.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves). Subscription
    /// helpers use the documented `interface << 8 | method` spelling so they
    /// can never collide with a contract ordinal.
    pub ordinal: u16,
    pub status: AvailStatus,
    /// The reason class, exactly when `status == UNAVAILABLE`.
    pub reason_class: Option<ReasonClass>,
    /// For `UNAVAILABLE`: why. For `DEGRADED`: the named degraded capability.
    /// Empty for `REAL`.
    pub reason: &'static str,
    /// The L3 test that ran and passed, required when `status != UNAVAILABLE`.
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

    const fn degraded(
        name: &'static str,
        ordinal: u16,
        l3_test: &'static str,
        detail: &'static str,
    ) -> Self {
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

/// The published availability matrix for the service domain on the Linux
/// backend (a reached supervisor; runit is the live-proven primary on the
/// build box, systemd the native D-Bus path, openrc its own state files,
/// sysvinit the honest absence).
///
/// Every REAL row names an always-on test that drives a **scripted**
/// supervision tree (the mock column); the host-touching live probe is the
/// separate `#[ignore]`d founder-run test.
pub const SERVICE_AVAILABILITY: &[MethodAvailability] = &[
    // ── The observation surface: native reads of the supervisor's own state ──
    MethodAvailability::real(
        "listServices",
        0,
        "l3_linux_service::list_services_enumerates_the_supervision_tree_natively",
    ),
    MethodAvailability::real(
        "getService",
        1,
        "l3_linux_service::get_service_reads_one_entry_natively",
    ),
    MethodAvailability::real(
        "getState",
        2,
        "l3_linux_service::state_is_derived_from_proc_truth_for_every_uid",
    ),
    // ── The privileged exact half of the state read ─────────────────────────
    MethodAvailability::degraded(
        "readSuperviseStat",
        0xFFFC,
        "l3_linux_service::the_exact_supervise_read_is_code_complete",
        "the supervisor's own EXACT state read (/run/runit/supervise.<name>/stat: its own state \
         word, its own transition clock, its want up/down flags) is PHYSICALLY privilege-gated on \
         this host — every supervise.* directory is drwx------ root, so uid 1000 gets EACCES. The \
         code path is complete and proven on both sides (a readable tree and a denied one), and the \
         live states are served for EVERY uid by the /proc derivation this same path falls back to \
         (the runsv process, its live children, the service's own pid file, the process start \
         clock). The exact half's live bring-back is the founder command: \
         `sudo ./target/debug/examples/service-state-probe --state-root /run/runit` (or \
         `sudo ./target/debug/smart service status`), both committed and ready.",
    ),
    MethodAvailability::real(
        "resolve",
        7,
        "l3_linux_service::resolve_maps_a_name_to_an_identity",
    ),
    // ── Health: the lifecycle is measured, the probe is not run ─────────────
    MethodAvailability::degraded(
        "getHealth",
        4,
        "l3_linux_service::health_reports_the_lifecycle_and_declares_the_probe_unrun",
        "the lifecycle half is measured natively (the supervisor's own state); the probe half is \
         declared unrun: a read-only backend never executes a service's probe script, so \
         lastCheckPassed is reported as false with a valid lastCheckAt naming the omission rather \
         than a fabricated passing check",
    ),
    MethodAvailability::degraded(
        "listHealth",
        5,
        "l3_linux_service::list_health_is_the_same_measurement_over_the_listing",
        "the same degradation per service, over the bounded listing",
    ),
    MethodAvailability::degraded(
        "listQuarantined",
        6,
        "l3_linux_service::quarantine_is_an_honest_empty_on_a_host_supervisor",
        "a host supervisor has no quarantine concept (a service is running or it is not), so the \
         answer is always the honest empty with total = 0 — never a fabricated quarantined entry",
    ),
    // ── Dependencies: what the supervisor's own declaration says ────────────
    MethodAvailability::degraded(
        "getDependencies",
        8,
        "l3_linux_service::dependencies_are_read_from_the_service_dirs_own_scripts",
        "read from the supervisor's own declaration where it exists: on runit from the service \
         dir's `sv check`/`sv up` invocations (the archive body read none), on systemd from the \
         unit's Wants/Requires over the bus. openrc/sysvinit declare no graph at all and answer \
         `Unavailable` with a reason",
    ),
    // ── The subscriptions: engine-native observation, bounded pull ──────────
    MethodAvailability::degraded(
        "subscribeState",
        3,
        "l3_linux_service::subscribe_state_mints_a_subscription",
        "the engine's own observation: subscribing mints a token, each pull re-reads the \
         supervisor's state natively and diffs it against the previous pull. The events are the \
         engine's measurement, not the supervisor's event stream, and each pull is bounded by the \
         caller's maxStates",
    ),
    MethodAvailability::degraded(
        "pullState",
        0x0300,
        "l3_linux_service::a_state_pull_reports_only_what_changed",
        "pulls up to the caller's maxStates and reports the transitions the re-enumeration found; \
         a pull that finds nothing is an empty batch, not an invented event",
    ),
    MethodAvailability::real(
        "cancelState",
        0x0301,
        "l3_linux_service::cancel_state_retires_the_state_token",
    ),
    MethodAvailability::degraded(
        "subscribeServiceEvents",
        12,
        "l3_linux_service::subscribe_service_events_mints_a_subscription",
        "the same engine-side observation, mapped onto the contract's event vocabulary \
         (started/stopped/failed) from the supervisor's measured transitions; a supervisor's own \
         log stream is logging.capnp's surface, not this one",
    ),
    MethodAvailability::degraded(
        "pullServiceEvents",
        0x0C00,
        "l3_linux_service::a_service_events_pull_reports_the_contract_vocabulary",
        "bounded by the caller's maxEvents, ordered by the transition the backend observed",
    ),
    MethodAvailability::real(
        "cancelServiceEvents",
        0x0C01,
        "l3_linux_service::cancel_service_events_retires_the_events_token",
    ),
    // ── The one gated surface ─────────────────────────────────────────
    MethodAvailability::real(
        "control",
        13,
        "l3_linux_service::control_mints_a_control_reference_only_for_a_covered_scope",
    ),
    // ── The lifecycle bridge: not a service.capnp method, honestly labelled ──
    MethodAvailability::degraded(
        "start",
        0xFFFF,
        "l3_linux_service::start_speaks_the_native_control_path",
        "host-mutating: init.capnp's InitService owns this authority (service.capnp deleted its own \
         copies); until that domain lands this backend exposes the supervisor's native control \
         path (runit's supervise control FIFO — a byte, no `sv` fork; systemd's system bus) mapped \
         onto the verb. The mapping is mock-proven, the live bring-back is founder-run, and the \
         shell routes every call through the host-write gate",
    ),
    MethodAvailability::degraded(
        "stop",
        0xFFFE,
        "l3_linux_service::stop_speaks_the_native_control_path",
        "host-mutating: the same native control path as `start`, with the supervisor's down byte",
    ),
    MethodAvailability::degraded(
        "restart",
        0xFFFD,
        "l3_linux_service::restart_speaks_the_native_control_path",
        "host-mutating: the same native control path, with the supervisor's terminate-and-restart \
         byte",
    ),
    // ── What a host supervisor does not own: honest UNAVAILABLE rows ────────
    MethodAvailability::unavailable(
        "getResourceAllocation",
        9,
        ReasonClass::NotApplicable,
        "not-applicable: no quota is declared for a service by this host's supervisor — the \
         supervisor runs the process and scopes nothing, so a quota read would be a fabrication. \
         (A cgroup-scoped quota is a different backend, not this one.)",
    ),
    MethodAvailability::unavailable(
        "getHealthCheck",
        10,
        ReasonClass::NotApplicable,
        "not-applicable: no health-check configuration is declared for a service by this host's \
         supervisor; its probe is a script this read-only backend does not execute or rewrite.",
    ),
    MethodAvailability::unavailable(
        "getMetrics",
        11,
        ReasonClass::NotApplicable,
        "not-applicable: request counts, error counts and latency quantiles are application \
         measurements; a host supervisor observes none of them, so none are reported.",
    ),
    MethodAvailability::unavailable(
        "setDependencies",
        0x0D00,
        ReasonClass::NoBackend,
        "no-backend: a dependency graph write is a service-registry operation; a host supervisor \
         declares no graph a client can write, and none is invented here.",
    ),
    MethodAvailability::unavailable(
        "setResourceAllocation",
        0x0D01,
        ReasonClass::NoBackend,
        "no-backend: no per-service quota store exists on this backend to write into.",
    ),
    MethodAvailability::unavailable(
        "setHealthCheck",
        0x0D02,
        ReasonClass::NoBackend,
        "no-backend: rewriting a supervisor's probe configuration would edit the host's service \
         directories, which a read-only backend does not do.",
    ),
    MethodAvailability::unavailable(
        "updateConfig",
        0x0D03,
        ReasonClass::NoBackend,
        "no-backend: the engine's own configuration store for a service does not exist on a host \
         supervisor backend.",
    ),
    MethodAvailability::unavailable(
        "setQuarantine",
        0x0D04,
        ReasonClass::NoBackend,
        "no-backend: quarantine is a serving-policy concept with no host-supervisor mechanism to \
         bind it to (the lifecycle verbs are the nearest native authority and are separately \
         gated).",
    ),
    MethodAvailability::unavailable(
        "setLogLevel",
        0x0D05,
        ReasonClass::NoBackend,
        "no-backend: a per-service log level is logging.capnp's surface; this backend keeps no \
         level store.",
    ),
    MethodAvailability::unavailable(
        "clearMetrics",
        0x0D06,
        ReasonClass::NoBackend,
        "no-backend: no service metrics store exists here, so there is nothing to purge.",
    ),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records. Observation rides every
/// transport; the control half is host-local.
pub const TRANSPORTS: &[&str] = &["InProcess", "UnixSocket"];
/// Published service capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "service.enumerate",
    "service.state.read",
    "service.health.read",
    "service.event.subscription",
    "service.lifecycle.control",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in SERVICE_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims — surfaced so the test suite can assert
/// each one exists (a REAL/DEGRADED claim with a vanished test is a FAIL).
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    SERVICE_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one method name, or `None` when the matrix has no such method.
/// The service harness and the CLI both resolve rows by name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    SERVICE_AVAILABILITY.iter().find(|r| r.name == name)
}

/// The **runtime** gate a caller's credentials place on top of a matrix row.
///
/// The matrix is the backend's target; this is what a caller actually gets
/// before any measurement is returned. It is computed from the backend's
/// measured caps (`supervision_caps()`), so a backend that *can* read the
/// supervisor's state pays no gate at all and one that cannot reports the
/// exact class the row's reason promises.
///
/// A runit host with a root-only supervise tree passes this gate: the state
/// reads are served by the `/proc` derivation (`can_read_state` measures true),
/// and only the exact half is gated — published as the `readSuperviseStat`
/// row, with the founder's privileged command.
///
/// | method family | `can_read_state` false | `can_control` false |
/// |---|---|---|
/// | the state reads (`listServices`, `getService`, `getState`, health) | `no-privilege` | — |
/// | the lifecycle verbs (`start`, `stop`, `restart`) | — | `no-privilege` |
/// | everything else | no gate | no gate |
pub fn runtime_gate(
    caps: &crate::domains::service::types::SupervisionCapabilities,
    method: &str,
) -> Option<(ReasonClass, &'static str)> {
    let state_family = matches!(
        method,
        "listServices" | "getService" | "getState" | "getHealth" | "listHealth"
    );
    let control_family = matches!(method, "start" | "stop" | "restart");
    if state_family && !caps.can_read_state {
        Some((
            ReasonClass::NoPrivilege,
            "no-privilege: the supervisor's state files are not readable by this caller, so the \
             status answers `unknown` with the measured reason instead of a value it could not read",
        ))
    } else if control_family && !caps.can_control {
        Some((
            ReasonClass::NoPrivilege,
            "no-privilege: the supervisor's control path is not writable by this caller, so the \
             verb is refused before any host state is touched",
        ))
    } else {
        None
    }
}
