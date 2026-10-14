//! Owned value types the **service** domain speaks (the types a caller
//! actually touches), plus their conversions to and from the frozen contract.
//!
//! Law #7: the capnp state structs are generated (`crate::schema::service_capnp`
//! and `crate::schema::discovery_capnp`); this module is the *hand-written*
//! owned mirror used at the domain's public/backend seam. It is ergonomic
//! (owned) for the methods this milestone implements and a faithful echo of
//! the ones that are honestly `DEGRADED`/`UNAVAILABLE` on a supervisor host,
//! so the trait has one honest, well-typed method per contract method.
//!
//! ## What the contract's observation surface actually is
//!
//! `service.capnp` **observes**: what is running, in what state, with what
//! health, using what. It deliberately deleted its own `start`/`stop`/
//! `restart`/`enable`/`disable` (they duplicated `init.capnp`'s control
//! authority) and gates every mutation that remains behind one accessor
//! (`control @13` → `ServiceControl`, whose methods take no token — the
//! reference IS the authority). This domain mirrors that split exactly: the
//! hub trait is the ungated observation surface, and the mutating half is
//! [`crate::domains::service::backend::ServiceControl`].
//!
//! ## What is NOT mirrored, and why that is honest
//!
//! `T.StringMap` is mirrored as an **ordered** `Vec<(String, String)>` (the
//! contract's `StringMapEntry` list preserves insertion order), because the
//! supervisor's measured facts (the supervise path, the enablement marker) are
//! reported in a stable order rather than through a hash map. The
//! `T.DataHandle` payload surface (a shared-memory byte handle) appears
//! nowhere in this domain's observation surface, so nothing here fabricates a
//! `BufferId`: a `ServiceMetrics`/`ServiceEvent` answer that carried one would
//! name shared memory this in-process backend never allocated.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::ServiceId;
use crate::schema::{discovery_capnp, service_capnp, types_capnp};

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`), under the
/// contract-wide conformance rule (`knowledge/03-CONFORMANCE.md` §2.3):
/// `limit == 0` means "one", never "all".
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

/// A one-page window over an unbounded list, with the stable `total` the
/// contract promises across two identical calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

/// An opaque subscription handle minted by `subscribe_state` /
/// `subscribe_service_events`. The backend is a pull model: subscribing mints
/// a token, `pull` reads the buffered events under the caller's stated bound,
/// and the token is retired on `cancel`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// An ordered metadata map: the contract's `T.StringMap` is a list of entries
/// that preserves insertion order, so the mirror is a list too (a `HashMap`
/// would reorder it and make two renders of the same answer differ).
pub type Metadata = Vec<(String, String)>;

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// The contract's `ServiceStatus` enum (`service.capnp:203`) — the lifecycle
/// of one running instance, as its supervisor observed it.
///
/// `Default` is `Unknown` (the enum's own `@0`): a defaulted state is the
/// contract's own "not known", never a fabricated `Active`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceStatus {
    #[default]
    Unknown,
    Active,
    Reloading,
    Inactive,
    Failed,
    Activating,
    Deactivating,
    Custom,
}

impl ServiceStatus {
    /// The contract token, so the mapping is auditable.
    pub const fn as_token(self) -> &'static str {
        match self {
            ServiceStatus::Unknown => "unknown",
            ServiceStatus::Active => "active",
            ServiceStatus::Reloading => "reloading",
            ServiceStatus::Inactive => "inactive",
            ServiceStatus::Failed => "failed",
            ServiceStatus::Activating => "activating",
            ServiceStatus::Deactivating => "deactivating",
            ServiceStatus::Custom => "custom",
        }
    }

    /// Read the status off a generated `ServiceStatus`.
    pub fn from_generated(g: service_capnp::ServiceStatus) -> Self {
        match g {
            service_capnp::ServiceStatus::Unknown => ServiceStatus::Unknown,
            service_capnp::ServiceStatus::Active => ServiceStatus::Active,
            service_capnp::ServiceStatus::Reloading => ServiceStatus::Reloading,
            service_capnp::ServiceStatus::Inactive => ServiceStatus::Inactive,
            service_capnp::ServiceStatus::Failed => ServiceStatus::Failed,
            service_capnp::ServiceStatus::Activating => ServiceStatus::Activating,
            service_capnp::ServiceStatus::Deactivating => ServiceStatus::Deactivating,
            service_capnp::ServiceStatus::Custom => ServiceStatus::Custom,
        }
    }

    /// The generated enum value for this status.
    pub fn to_generated(self) -> service_capnp::ServiceStatus {
        match self {
            ServiceStatus::Unknown => service_capnp::ServiceStatus::Unknown,
            ServiceStatus::Active => service_capnp::ServiceStatus::Active,
            ServiceStatus::Reloading => service_capnp::ServiceStatus::Reloading,
            ServiceStatus::Inactive => service_capnp::ServiceStatus::Inactive,
            ServiceStatus::Failed => service_capnp::ServiceStatus::Failed,
            ServiceStatus::Activating => service_capnp::ServiceStatus::Activating,
            ServiceStatus::Deactivating => service_capnp::ServiceStatus::Deactivating,
            ServiceStatus::Custom => service_capnp::ServiceStatus::Custom,
        }
    }
}

/// The state of one RUNNING service instance (`ServiceInstanceState`).
///
/// The schema's own honesty rule is carried in the types: `sinceUs` is zero
/// **with `sinceValid` false** when no transition has been observed (never
/// "it changed zero microseconds ago"), `mainPid` is zero when the supervisor
/// names no process, and `reason` is the measured explanation whenever
/// `status` is anything other than `active`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceInstanceState {
    pub status: ServiceStatus,
    /// Which process currently holds the service's handler (zero = none).
    pub main_pid: u64,
    /// When the service entered its current status (nanoseconds since the
    /// epoch; zero when the transition was not observed).
    pub since: u64,
    /// How long it has been in this status, as measured by whoever observed
    /// the transition.
    pub since_us: u64,
    /// Whether `since`/`sinceUs` carry a real observation.
    pub since_valid: bool,
    /// Running but not permitted to serve.
    pub quarantined: bool,
    /// Why, whenever status is anything other than `active`. Never a
    /// placeholder.
    pub reason: String,
    /// Measured facts this backend carries alongside the status: the
    /// supervisor's own paths and markers. Ordered.
    pub metadata: Metadata,
}

/// One entry in the bounded service listing (`ServiceInstanceInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceInstanceInfo {
    pub id: ServiceId,
    pub name: String,
    pub description: String,
    pub state: ServiceInstanceState,
    pub enabled: bool,
    /// Whether this service was started on demand (not at boot).
    pub on_demand: bool,
    /// The manifest it was declared from, if any (zero = none).
    pub manifest: u64,
}

// ═══════════════════════════════════════════════════════════════════════
// Health (the canonical discovery type, imported by service.capnp)
// ═══════════════════════════════════════════════════════════════════════

/// discovery.capnp's registry-visible lifecycle (`ServiceState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HealthState {
    #[default]
    Unknown,
    Online,
    Offline,
    Degraded,
    Starting,
    Maintenance,
    Custom,
}

impl HealthState {
    pub const fn as_token(self) -> &'static str {
        match self {
            HealthState::Unknown => "unknown",
            HealthState::Online => "online",
            HealthState::Offline => "offline",
            HealthState::Degraded => "degraded",
            HealthState::Starting => "starting",
            HealthState::Maintenance => "maintenance",
            HealthState::Custom => "custom",
        }
    }

    /// Read the state off a generated `ServiceState`.
    pub fn from_generated(g: discovery_capnp::ServiceState) -> Self {
        match g {
            discovery_capnp::ServiceState::Unknown => HealthState::Unknown,
            discovery_capnp::ServiceState::Online => HealthState::Online,
            discovery_capnp::ServiceState::Offline => HealthState::Offline,
            discovery_capnp::ServiceState::Degraded => HealthState::Degraded,
            discovery_capnp::ServiceState::Starting => HealthState::Starting,
            discovery_capnp::ServiceState::Maintenance => HealthState::Maintenance,
            discovery_capnp::ServiceState::Custom => HealthState::Custom,
        }
    }

    /// The generated enum value for this state.
    pub fn to_generated(self) -> discovery_capnp::ServiceState {
        match self {
            HealthState::Unknown => discovery_capnp::ServiceState::Unknown,
            HealthState::Online => discovery_capnp::ServiceState::Online,
            HealthState::Offline => discovery_capnp::ServiceState::Offline,
            HealthState::Degraded => discovery_capnp::ServiceState::Degraded,
            HealthState::Starting => discovery_capnp::ServiceState::Starting,
            HealthState::Maintenance => {
                discovery_capnp::ServiceState::Maintenance
            }
            HealthState::Custom => discovery_capnp::ServiceState::Custom,
        }
    }
}

/// `getHealth`'s answer: lifecycle AND probe, one honest answer
/// (`D.ServiceHealth`). A service can pass every probe and still be
/// quarantined after a fault storm; the type carries both halves so a broken
/// system cannot look fine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceHealth {
    pub service_id: ServiceId,
    pub state: HealthState,
    /// Why, whenever `state` is not `online`. Never a placeholder.
    pub reason: String,
    pub quarantined: bool,
    pub since_us: u64,
    pub since_valid: bool,
    /// The last probe result. Distinct from `state`.
    pub last_check_passed: bool,
    pub last_check_at: u64,
    /// Faults taken since the service was last started (never reset by a
    /// passing probe).
    pub fault_count: u64,
    pub consecutive_faults: u32,
    pub restarts: u32,
}

// ═══════════════════════════════════════════════════════════════════════
// Dependencies, allocation, health checks, metrics
// ═══════════════════════════════════════════════════════════════════════

/// discovery.capnp's canonical dependency edge (`D.ServiceDependency`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceDependency {
    pub service_id: ServiceId,
    pub dependency_type: DependencyType,
    /// Version constraint as an open token ("" = none declared).
    pub version_constraint: String,
    pub satisfied: bool,
}

/// discovery.capnp's `DependencyType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DependencyType {
    #[default]
    Unknown,
    Required,
    Optional,
    Custom,
}

impl DependencyType {
    pub const fn as_token(self) -> &'static str {
        match self {
            DependencyType::Unknown => "unknown",
            DependencyType::Required => "required",
            DependencyType::Optional => "optional",
            DependencyType::Custom => "custom",
        }
    }

    /// Read the type off a generated `DependencyType`.
    pub fn from_generated(g: discovery_capnp::DependencyType) -> Self {
        match g {
            discovery_capnp::DependencyType::Unknown => DependencyType::Unknown,
            discovery_capnp::DependencyType::Required => {
                DependencyType::Required
            }
            discovery_capnp::DependencyType::Optional => {
                DependencyType::Optional
            }
            discovery_capnp::DependencyType::Custom => DependencyType::Custom,
        }
    }

    /// The generated enum value for this type.
    pub fn to_generated(self) -> discovery_capnp::DependencyType {
        match self {
            DependencyType::Unknown => discovery_capnp::DependencyType::Unknown,
            DependencyType::Required => {
                discovery_capnp::DependencyType::Required
            }
            DependencyType::Optional => {
                discovery_capnp::DependencyType::Optional
            }
            DependencyType::Custom => discovery_capnp::DependencyType::Custom,
        }
    }
}

/// The canonical `T.Quota`, mirrored owned (declared limits, platform-neutral).
/// Each zero means "no declared bound", so a zero is never a fabricated value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quota {
    pub memory_bytes: u64,
    pub cpu_percent: f32,
    pub io_read_bytes_per_sec: u64,
    pub io_write_bytes_per_sec: u64,
    pub pids: u32,
    /// Wall-clock budget as nanoseconds (zero = none declared).
    pub wall_clock_budget_ns: u64,
}

/// A declared resource allocation (`ServiceResourceAllocation`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ServiceResourceAllocation {
    pub quota: Quota,
}

/// A health-check configuration (`ServiceHealthCheck`). The endpoint is an
/// open token: the protocol never names a probe mechanism.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceHealthCheck {
    pub endpoint: String,
    pub interval_ns: u64,
    pub timeout_ns: u64,
    pub unhealthy_threshold: u32,
    pub healthy_threshold: u32,
}

/// Runtime measurements with a declared window, so two backends cannot answer
/// "since when?" differently (`ServiceMetrics`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ServiceMetrics {
    pub service_id: ServiceId,
    pub window_ns: u64,
    pub request_count: u64,
    pub error_count: u64,
    /// Latency quantiles over the window in nanoseconds (zero = not measured).
    pub latency_p50_ns: u64,
    pub latency_p95_ns: u64,
    pub latency_p99_ns: u64,
    pub active_requests: u32,
    /// True when the backend could not measure some field in the window.
    pub partial: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// Events
// ═══════════════════════════════════════════════════════════════════════

/// The contract's event vocabulary (`ServiceEventType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceEventType {
    Unknown,
    Started,
    Stopped,
    Failed,
    HealthChanged,
    ConfigChanged,
    Custom,
}

impl ServiceEventType {
    pub const fn as_token(self) -> &'static str {
        match self {
            ServiceEventType::Unknown => "unknown",
            ServiceEventType::Started => "started",
            ServiceEventType::Stopped => "stopped",
            ServiceEventType::Failed => "failed",
            ServiceEventType::HealthChanged => "healthChanged",
            ServiceEventType::ConfigChanged => "configChanged",
            ServiceEventType::Custom => "custom",
        }
    }
}

/// One service event (`ServiceEvent`): the union the contract declares plus
/// the timestamp and service id every arm carries.
#[derive(Clone, Debug, PartialEq)]
pub enum ServiceEventPayload {
    StateChanged(ServiceInstanceState),
    HealthChanged(ServiceHealth),
    ConfigChanged(Metadata),
    /// `T.UnknownEvent` — a peer's event this schema version does not name.
    Unknown,
}

/// One service event with its envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceEvent {
    pub payload: ServiceEventPayload,
    pub timestamp: u64,
    pub service_id: ServiceId,
}

impl ServiceEvent {
    /// The event's vocabulary entry, derived from its own payload (never
    /// asserted twice: a `StateChanged` arm is a `healthChanged`/`started`
    /// event exactly when its state says so).
    pub fn event_type(&self) -> ServiceEventType {
        match &self.payload {
            ServiceEventPayload::StateChanged(state) => match state.status {
                ServiceStatus::Active => ServiceEventType::Started,
                ServiceStatus::Failed => ServiceEventType::Failed,
                ServiceStatus::Inactive => ServiceEventType::Stopped,
                // A state this backend could not read is not a transition to
                // anything: the event's own `unknown` is the honest answer.
                ServiceStatus::Unknown => ServiceEventType::Unknown,
                _ => ServiceEventType::Custom,
            },
            ServiceEventPayload::HealthChanged(_) => ServiceEventType::HealthChanged,
            ServiceEventPayload::ConfigChanged(_) => ServiceEventType::ConfigChanged,
            ServiceEventPayload::Unknown => ServiceEventType::Unknown,
        }
    }
}

/// The contract's `T.LogLevel`, mirrored owned (the declared log level a
/// `ServiceControl.setLogLevel` call states). Levels the supervisor has no
/// mapping for are carried as the contract's own `custom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Silent,
    Crash,
    Security,
    Audit,
    Custom,
}

impl LogLevel {
    pub const fn as_token(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
            LogLevel::Fatal => "fatal",
            LogLevel::Silent => "silent",
            LogLevel::Crash => "crash",
            LogLevel::Security => "security",
            LogLevel::Audit => "audit",
            LogLevel::Custom => "custom",
        }
    }

    /// The generated enum value for this level.
    pub fn to_generated(self) -> types_capnp::LogLevel {
        match self {
            LogLevel::Trace => types_capnp::LogLevel::Trace,
            LogLevel::Debug => types_capnp::LogLevel::Debug,
            LogLevel::Info => types_capnp::LogLevel::Info,
            LogLevel::Warn => types_capnp::LogLevel::Warn,
            LogLevel::Error => types_capnp::LogLevel::Error,
            LogLevel::Fatal => types_capnp::LogLevel::Fatal,
            LogLevel::Silent => types_capnp::LogLevel::Silent,
            LogLevel::Crash => types_capnp::LogLevel::Crash,
            LogLevel::Security => types_capnp::LogLevel::Security,
            LogLevel::Audit => types_capnp::LogLevel::Audit,
            LogLevel::Custom => types_capnp::LogLevel::Custom,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The supervisor's own identity (what this backend observed it with)
// ═══════════════════════════════════════════════════════════════════════

/// The supervisor this backend is speaking to, published so a caller can tell
/// a native supervision read from a decided absence before any method is
/// called. `None` means no supervisor was reached — every read answers
/// honestly with a reason rather than fabricating services.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupervisorIdentity {
    /// The supervisor token ("runit", "systemd", "openrc", "sysvinit", or
    /// "none"). Mirrors the archive's `ServiceBackend::name()` vocabulary —
    /// the four backends it enumerated are the same four named here.
    pub supervisor: String,
    /// The directory the supervisor scans for services, measured
    /// (e.g. `/var/service` on a runit host).
    pub service_root: String,
    /// The directory the declared service dirs live in (e.g. `/etc/sv`).
    pub declared_root: String,
    /// The per-service state directory the supervisor keeps
    /// (e.g. `/run/runit`).
    pub state_root: String,
}

/// The capability surface this service backend advertises (law #6): open
/// tokens naming what is real, so a consumer reading the handle knows what it
/// may do without a permission string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupervisionCapabilities {
    /// The supervisor actually reached (see [`SupervisorIdentity`]).
    pub supervisor: String,
    /// Whether service enumeration is served natively.
    pub can_enumerate: bool,
    /// Whether the per-service run state is readable by THIS caller — either
    /// through the supervisor's own state files (privileged on many hosts) or
    /// through the supervised process truth every uid can read.
    pub can_read_state: bool,
    /// Whether the supervisor's own EXACT state read is readable by THIS caller
    /// (runit's `supervise.<name>/stat`, its own transition clock and want
    /// up/down flags). On a runit host with a root-only supervise tree this is
    /// false for uid 1000 while `can_read_state` stays true: the /proc
    /// derivation answers for every uid, and the exact half is the founder's
    /// privileged command.
    pub can_read_exact_state: bool,
    /// Whether lifecycle control (start/stop/restart) is reachable by THIS
    /// caller through the supervisor's own control path.
    pub can_control: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// Wire conversions for the state structs
// ═══════════════════════════════════════════════════════════════════════

/// Read a `Text` field into an owned string. Absent or non-UTF-8 is the empty
/// string — the same equivalence the writers emit, so the round-trip is total.
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Read a `T.StringMap` into the owned ordered pairs.
pub(crate) fn read_string_map(reader: &types_capnp::string_map::Reader<'_>) -> Metadata {
    let mut out = Vec::new();
    if let Ok(entries) = reader.get_entries() {
        for entry in entries.iter() {
            out.push((text_field(entry.get_key()), text_field(entry.get_value())));
        }
    }
    out
}

/// Write a `T.StringMap` from the owned ordered pairs.
pub(crate) fn write_string_map(builder: &mut types_capnp::string_map::Builder<'_>, map: &Metadata) {
    let mut entries = builder.reborrow().init_entries(map.len() as u32);
    for (i, (k, v)) in map.iter().enumerate() {
        let mut entry = entries.reborrow().get(i as u32);
        entry.set_key(k.as_str());
        entry.set_value(v.as_str());
    }
}

/// Write a `T.Duration` from nanoseconds.
fn set_duration_ns(builder: &mut types_capnp::duration::Builder<'_>, nanos: u64) {
    builder.set_nanoseconds(nanos);
}

/// Read a `T.Duration` into nanoseconds.
fn duration_ns(reader: &types_capnp::duration::Reader<'_>) -> u64 {
    reader.get_nanoseconds()
}

/// Write a `T.Quota`.
fn write_quota(builder: &mut types_capnp::quota::Builder<'_>, quota: &Quota) {
    builder.set_memory_bytes(quota.memory_bytes);
    builder.set_cpu_percent(quota.cpu_percent);
    builder.set_io_read_bytes_per_sec(quota.io_read_bytes_per_sec);
    builder.set_io_write_bytes_per_sec(quota.io_write_bytes_per_sec);
    builder.set_pids(quota.pids);
    set_duration_ns(
        &mut builder.reborrow().init_wall_clock_budget(),
        quota.wall_clock_budget_ns,
    );
}

/// Read a `T.Quota`.
fn read_quota(reader: &types_capnp::quota::Reader<'_>) -> Quota {
    Quota {
        memory_bytes: reader.get_memory_bytes(),
        cpu_percent: reader.get_cpu_percent(),
        io_read_bytes_per_sec: reader.get_io_read_bytes_per_sec(),
        io_write_bytes_per_sec: reader.get_io_write_bytes_per_sec(),
        pids: reader.get_pids(),
        wall_clock_budget_ns: match reader.get_wall_clock_budget() {
            Ok(d) => duration_ns(&d),
            Err(_) => 0,
        },
    }
}

impl ServiceInstanceState {
    /// Write into a generated `ServiceInstanceState` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_instance_state::Builder<'_>) {
        builder.set_status(self.status.to_generated());
        let mut pid = builder.reborrow().init_main_pid();
        pid.set_id(self.main_pid);
        let mut since = builder.reborrow().init_since();
        since.set_nanoseconds(self.since);
        builder.set_since_us(self.since_us);
        builder.set_since_valid(self.since_valid);
        builder.set_quarantined(self.quarantined);
        builder.set_reason(self.reason.as_str());
        write_string_map(&mut builder.reborrow().init_metadata(), &self.metadata);
    }

    /// Read from a generated `ServiceInstanceState` reader.
    pub fn from_reader(reader: &service_capnp::service_instance_state::Reader<'_>) -> Self {
        ServiceInstanceState {
            status: match reader.get_status() {
                Ok(s) => ServiceStatus::from_generated(s),
                Err(_) => ServiceStatus::Unknown,
            },
            main_pid: match reader.get_main_pid() {
                Ok(p) => p.get_id(),
                Err(_) => 0,
            },
            since: match reader.get_since() {
                Ok(t) => t.get_nanoseconds(),
                Err(_) => 0,
            },
            since_us: reader.get_since_us(),
            since_valid: reader.get_since_valid(),
            quarantined: reader.get_quarantined(),
            reason: text_field(reader.get_reason()),
            metadata: match reader.get_metadata() {
                Ok(m) => read_string_map(&m),
                Err(_) => Vec::new(),
            },
        }
    }
}

impl ServiceInstanceInfo {
    /// Write into a generated `ServiceInstanceInfo` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_instance_info::Builder<'_>) {
        let mut id = builder.reborrow().init_id();
        id.set_id(self.id.get());
        builder.reborrow().set_name(self.name.as_str());
        builder.reborrow().set_description(self.description.as_str());
        self.state
            .write_to(&mut builder.reborrow().init_state());
        builder.reborrow().set_enabled(self.enabled);
        builder.reborrow().set_on_demand(self.on_demand);
        let mut manifest = builder.reborrow().init_manifest();
        manifest.set_id(self.manifest);
    }

    /// Read from a generated `ServiceInstanceInfo` reader.
    pub fn from_reader(reader: &service_capnp::service_instance_info::Reader<'_>) -> Self {
        ServiceInstanceInfo {
            id: ServiceId::new(match reader.get_id() {
                Ok(i) => i.get_id(),
                Err(_) => 0,
            }),
            name: text_field(reader.get_name()),
            description: text_field(reader.get_description()),
            state: match reader.get_state() {
                Ok(s) => ServiceInstanceState::from_reader(&s),
                Err(_) => ServiceInstanceState::default(),
            },
            enabled: reader.get_enabled(),
            on_demand: reader.get_on_demand(),
            manifest: match reader.get_manifest() {
                Ok(m) => m.get_id(),
                Err(_) => 0,
            },
        }
    }
}

impl ServiceHealth {
    /// Write into a generated `D.ServiceHealth` builder.
    pub fn write_to(&self, builder: &mut discovery_capnp::service_health::Builder<'_>) {
        let mut id = builder.reborrow().init_service_id();
        id.set_id(self.service_id.get());
        builder.reborrow().set_state(self.state.to_generated());
        builder.reborrow().set_reason(self.reason.as_str());
        builder.reborrow().set_quarantined(self.quarantined);
        builder.reborrow().set_since_us(self.since_us);
        builder.reborrow().set_since_valid(self.since_valid);
        builder.reborrow().set_last_check_passed(self.last_check_passed);
        let mut at = builder.reborrow().init_last_check_at();
        at.set_nanoseconds(self.last_check_at);
        builder.reborrow().set_fault_count(self.fault_count);
        builder.reborrow().set_consecutive_faults(self.consecutive_faults);
        builder.reborrow().set_restarts(self.restarts);
    }

    /// Read from a generated `D.ServiceHealth` reader.
    pub fn from_reader(reader: &discovery_capnp::service_health::Reader<'_>) -> Self {
        ServiceHealth {
            service_id: ServiceId::new(match reader.get_service_id() {
                Ok(i) => i.get_id(),
                Err(_) => 0,
            }),
            state: match reader.get_state() {
                Ok(s) => HealthState::from_generated(s),
                Err(_) => HealthState::Unknown,
            },
            reason: text_field(reader.get_reason()),
            quarantined: reader.get_quarantined(),
            since_us: reader.get_since_us(),
            since_valid: reader.get_since_valid(),
            last_check_passed: reader.get_last_check_passed(),
            last_check_at: match reader.get_last_check_at() {
                Ok(t) => t.get_nanoseconds(),
                Err(_) => 0,
            },
            fault_count: reader.get_fault_count(),
            consecutive_faults: reader.get_consecutive_faults(),
            restarts: reader.get_restarts(),
        }
    }
}

impl ServiceDependency {
    /// Write into a generated `D.ServiceDependency` builder.
    pub fn write_to(&self, builder: &mut discovery_capnp::service_dependency::Builder<'_>) {
        let mut id = builder.reborrow().init_service_id();
        id.set_id(self.service_id.get());
        builder
            .reborrow()
            .set_dependency_type(self.dependency_type.to_generated());
        builder
            .reborrow()
            .set_version_constraint(self.version_constraint.as_str());
        builder.reborrow().set_satisfied(self.satisfied);
    }

    /// Read from a generated `D.ServiceDependency` reader.
    pub fn from_reader(reader: &discovery_capnp::service_dependency::Reader<'_>) -> Self {
        ServiceDependency {
            service_id: ServiceId::new(match reader.get_service_id() {
                Ok(i) => i.get_id(),
                Err(_) => 0,
            }),
            dependency_type: match reader.get_dependency_type() {
                Ok(t) => DependencyType::from_generated(t),
                Err(_) => DependencyType::Unknown,
            },
            version_constraint: text_field(reader.get_version_constraint()),
            satisfied: reader.get_satisfied(),
        }
    }
}

impl ServiceResourceAllocation {
    /// Write into a generated `ServiceResourceAllocation` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_resource_allocation::Builder<'_>) {
        write_quota(&mut builder.reborrow().init_quota(), &self.quota);
    }

    /// Read from a generated `ServiceResourceAllocation` reader.
    pub fn from_reader(reader: &service_capnp::service_resource_allocation::Reader<'_>) -> Self {
        ServiceResourceAllocation {
            quota: match reader.get_quota() {
                Ok(q) => read_quota(&q),
                Err(_) => Quota::default(),
            },
        }
    }
}

impl ServiceHealthCheck {
    /// Write into a generated `ServiceHealthCheck` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_health_check::Builder<'_>) {
        builder.reborrow().set_endpoint(self.endpoint.as_str());
        set_duration_ns(&mut builder.reborrow().init_interval(), self.interval_ns);
        set_duration_ns(&mut builder.reborrow().init_timeout(), self.timeout_ns);
        builder.reborrow().set_unhealthy_threshold(self.unhealthy_threshold);
        builder.reborrow().set_healthy_threshold(self.healthy_threshold);
    }

    /// Read from a generated `ServiceHealthCheck` reader.
    pub fn from_reader(reader: &service_capnp::service_health_check::Reader<'_>) -> Self {
        ServiceHealthCheck {
            endpoint: text_field(reader.get_endpoint()),
            interval_ns: match reader.get_interval() {
                Ok(d) => duration_ns(&d),
                Err(_) => 0,
            },
            timeout_ns: match reader.get_timeout() {
                Ok(d) => duration_ns(&d),
                Err(_) => 0,
            },
            unhealthy_threshold: reader.get_unhealthy_threshold(),
            healthy_threshold: reader.get_healthy_threshold(),
        }
    }
}

impl ServiceMetrics {
    /// Write into a generated `ServiceMetrics` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_metrics::Builder<'_>) {
        let mut id = builder.reborrow().init_service_id();
        id.set_id(self.service_id.get());
        set_duration_ns(&mut builder.reborrow().init_window(), self.window_ns);
        builder.reborrow().set_request_count(self.request_count);
        builder.reborrow().set_error_count(self.error_count);
        set_duration_ns(
            &mut builder.reborrow().init_latency_p50(),
            self.latency_p50_ns,
        );
        set_duration_ns(
            &mut builder.reborrow().init_latency_p95(),
            self.latency_p95_ns,
        );
        set_duration_ns(
            &mut builder.reborrow().init_latency_p99(),
            self.latency_p99_ns,
        );
        builder.reborrow().set_active_requests(self.active_requests);
        builder.reborrow().set_partial(self.partial);
    }

    /// Read from a generated `ServiceMetrics` reader.
    pub fn from_reader(reader: &service_capnp::service_metrics::Reader<'_>) -> Self {
        let ns = |r: capnp::Result<types_capnp::duration::Reader<'_>>| match r {
            Ok(d) => duration_ns(&d),
            Err(_) => 0,
        };
        ServiceMetrics {
            service_id: ServiceId::new(match reader.get_service_id() {
                Ok(i) => i.get_id(),
                Err(_) => 0,
            }),
            window_ns: ns(reader.get_window()),
            request_count: reader.get_request_count(),
            error_count: reader.get_error_count(),
            latency_p50_ns: ns(reader.get_latency_p50()),
            latency_p95_ns: ns(reader.get_latency_p95()),
            latency_p99_ns: ns(reader.get_latency_p99()),
            active_requests: reader.get_active_requests(),
            partial: reader.get_partial(),
        }
    }
}

impl ServiceEvent {
    /// Write into a generated `ServiceEvent` builder.
    pub fn write_to(&self, builder: &mut service_capnp::service_event::Builder<'_>) {
        {
            let mut event = builder.reborrow();
            match &self.payload {
                ServiceEventPayload::StateChanged(state) => {
                    state.write_to(&mut event.reborrow().init_state_changed());
                }
                ServiceEventPayload::HealthChanged(health) => {
                    health.write_to(&mut event.reborrow().init_health_changed());
                }
                ServiceEventPayload::ConfigChanged(map) => {
                    write_string_map(&mut event.reborrow().init_config_changed(), map);
                }
                // `T.UnknownEvent` carries no fields this backend fills: an
                // unnameable event stays unnameable rather than guessed.
                ServiceEventPayload::Unknown => {
                    let _ = event.reborrow().init_unknown();
                }
            }
        }
        let mut ts = builder.reborrow().init_timestamp();
        ts.set_nanoseconds(self.timestamp);
        let mut id = builder.reborrow().init_service_id();
        id.set_id(self.service_id.get());
    }

    /// Read from a generated `ServiceEvent` reader.
    pub fn from_reader(reader: &service_capnp::service_event::Reader<'_>) -> Self {
        use service_capnp::service_event::WhichReader;
        let payload = match reader.which() {
            Ok(WhichReader::StateChanged(state)) => {
                match state {
                    Ok(s) => ServiceEventPayload::StateChanged(ServiceInstanceState::from_reader(&s)),
                    Err(_) => ServiceEventPayload::Unknown,
                }
            }
            Ok(WhichReader::HealthChanged(health)) => {
                match health {
                    Ok(h) => ServiceEventPayload::HealthChanged(ServiceHealth::from_reader(&h)),
                    Err(_) => ServiceEventPayload::Unknown,
                }
            }
            Ok(WhichReader::ConfigChanged(map)) => {
                match map {
                    Ok(m) => ServiceEventPayload::ConfigChanged(read_string_map(&m)),
                    Err(_) => ServiceEventPayload::ConfigChanged(Vec::new()),
                }
            }
            Ok(WhichReader::Unknown(_)) | Err(_) => ServiceEventPayload::Unknown,
        };
        ServiceEvent {
            payload,
            timestamp: match reader.get_timestamp() {
                Ok(t) => t.get_nanoseconds(),
                Err(_) => 0,
            },
            service_id: ServiceId::new(match reader.get_service_id() {
                Ok(i) => i.get_id(),
                Err(_) => 0,
            }),
        }
    }
}

/// Write a `T.VoidResult` from a plain success. The mutating half of this
/// domain's wire shape rides this union: `success` or a `T.Error`.
pub fn write_void_success(builder: &mut types_capnp::void_result::Builder<'_>) {
    builder.reborrow().set_success(());
}

/// Write a `T.VoidResult` carrying a core error (the mapping is total: the
/// error's own code and message are preserved verbatim).
pub fn write_void_error(
    builder: &mut types_capnp::void_result::Builder<'_>,
    error: &crate::core::Error,
) {
    error.write_to(&mut builder.reborrow().init_error());
}

