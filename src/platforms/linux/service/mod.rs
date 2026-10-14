//! The Linux **service** backend: the hub this platform exposes. One
//! [`LinuxServiceBackend`] speaks to whichever supervisor the host actually
//! runs, and the **detection is measured, never assumed** — the same four
//! backends the archive enumerated (`systemd`, `runit`, `openrc`, `sysvinit`),
//! each behind one native read path, and an honest absence when none is
//! reached.
//!
//! ## The per-supervisor native decision (the "innovation like netlink"
//! mandate — the archive body's `Command::new("sv" / "systemctl" /
//! "rc-service")` fork-per-query shellout is replaced on every path)
//!
//! | supervisor | native read path | control path | status here |
//! |---|---|---|---|
//! | **runit** | the supervise tree: `/etc/sv` (declared), `/var/service` (linked), `/run/runit/supervise.<name>/{stat,pid}` (state), the service's own scripts for its declared dependencies | one byte to `/run/runit/supervise.<name>/control` — the supervisor's control FIFO, exactly what `sv` writes | **LIVE-PROVEN on this host** (16 linked services, 58 declared, 26 supervised) |
//! | **systemd** | sd-bus/D-Bus: the Rust `dbus` crate's system-bus client against `org.freedesktop.systemd1` (`ListUnits` / `ListUnitFiles` / the unit's own properties) — the same native-D-Bus standard the network domain met with its NM marshaller | `StartUnit`/`StopUnit`/`RestartUnit` on the system bus | **built, mock-proven over a scripted bus peer, live-unproven** (this host runs no systemd) |
//! | **openrc** | openrc's own state files (`/run/openrc/state/{started,stopped,…}`), the init scripts in `/etc/init.d`, the runlevel links | **honest absence**: openrc's only control surface is the `rc-service` script invocation, which this domain does not shell out to — `Unavailable`-with-reason naming it | built, mock-proven over a scripted sysroot; not this host |
//! | **sysvinit** | **honest absence** (decided, not forgotten): sysvinit has no portable state — the pidfile convention is per-script, so a run state would be a guess | same | answered `Unavailable`-with-reason |
//!
//! ## Law #8 (the host is read-only)
//!
//! Every read here is `read_dir`/`read`/`metadata` (runit, openrc) or a D-Bus
//! method that queries (systemd). The only write anywhere in this module is
//! the supervisor's control byte, host-mutating, reachable only through
//! [`ServiceLifecycle`] and gated at the shell; its live bring-back proof is
//! the founder's running it.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::sync::Mutex;

use crate::core::ids::ServiceId;
use crate::core::{Error, ErrorCode, Result};
use crate::domains::service::availability::{runtime_gate, ReasonClass};
use crate::domains::service::backend::{
    LifecycleVerb, PresentedCapability, ServiceBackend, ServiceControl, ServiceLifecycle,
};
use crate::domains::service::types::{
    DependencyType, HealthState, Metadata, Page, Pagination, ServiceDependency, ServiceEvent,
    ServiceEventPayload, ServiceHealth, ServiceHealthCheck, ServiceInstanceInfo,
    ServiceInstanceState, ServiceMetrics, ServiceResourceAllocation, ServiceStatus, SubscriptionId,
    SupervisionCapabilities, SupervisorIdentity,
};

#[cfg(test)]
pub mod tests;
#[path = "openrc.rs"]
pub mod openrc;
#[path = "proc.rs"]
pub mod proc;
#[path = "runit.rs"]
pub mod runit;
#[path = "systemd.rs"]
pub mod systemd;

use self::openrc::OpenrcBackend;
use self::runit::RunitBackend;
use self::systemd::SystemdBackend;

// ═══════════════════════════════════════════════════════════════════════
// The internal native-supervision seam
// ═══════════════════════════════════════════════════════════════════════

/// A dependency's native reading before it is typed: the dependency's name and
/// whether the supervisor reports it active.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeDep {
    pub name: String,
    pub satisfied: bool,
}

/// One service's measured run state.
#[derive(Clone, Debug, Default)]
pub struct NativeState {
    pub status: ServiceStatus,
    pub main_pid: u64,
    pub since: u64,
    pub since_us: u64,
    pub since_valid: bool,
    pub reason: String,
}

/// One service's native observation, before the backend's honesty layer turns
/// it into a contract answer. A `state` of `None` is always paired with a
/// `state_reason`, so a caller can never confuse "not running" with "could not
/// read".
#[derive(Clone, Debug, Default)]
pub struct NativeObservation {
    pub name: String,
    pub description: String,
    /// The supervisor has a service directory for it.
    pub declared: bool,
    /// It is linked into the supervisor's scan dir (its "enabled").
    pub enabled: bool,
    /// A supervisor process watches it.
    pub supervised: bool,
    pub state: Option<NativeState>,
    pub state_reason: String,
    pub dependencies: Vec<NativeDep>,
    /// The measured facts the unprivileged runit path derives from `/proc`,
    /// published beside the state so a caller can see where it came from.
    /// All three are zero/empty on the cells that measure no such thing
    /// (systemd, openrc, sysvinit) rather than fabricated.
    pub proc_facts: NativeProcFacts,
}

/// The `/proc`-derived supervision truth for one service, measured on the
/// runit cell (the only cell whose supervisor truth is visible in `/proc`).
/// Zero values are the honest "not measured" — the cell that does not measure
/// them leaves them at zero and never fills them with a guess.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeProcFacts {
    /// The `runsv` pid supervising this service (zero = none is in `/proc`).
    pub runsv_pid: u64,
    /// How many live children that `runsv` has (the service plus its logger).
    pub live_children: usize,
    /// Where the reported state came from: `supervise.stat` (the privileged
    /// exact read), `proc` (the `/proc` derivation), or empty (none).
    pub state_source: &'static str,
    /// The measured pid-file fact, spelled out for the reason a caller reads.
    pub pidfile: String,
}

/// The internal native-supervision seam every cell implements: what the
/// supervisor's own state says, with no contract shape in the signature. The
/// hub backend turns these into the contract's owned types and adds the
/// honesty layer (reasons, the honest empties, the degradation marks).
pub trait NativeSupervision: Send + 'static {
    /// The supervisor's measured identity.
    fn supervisor(&self) -> SupervisorIdentity;

    /// Every service this supervisor knows, with its measured state or the
    /// measured reason it could not be read.
    fn enumerate(&self) -> Vec<NativeObservation>;

    /// The dependencies one service declares, in the supervisor's own terms.
    /// The default is the honest "none declared", which a cell overrides when
    /// the supervisor really declares a graph.
    fn dependencies(&self, _name: &str) -> Vec<NativeDep> {
        Vec::new()
    }

    /// Apply one lifecycle verb through the supervisor's native control path
    /// (host-mutating).
    fn control(&self, _name: &str, _verb: LifecycleVerb) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: this supervisor exposes no native control path this backend can reach",
        ))
    }

    /// The identity and capabilities this cell measures on this host. Every
    /// capability is a measurement (the supervisor reached, the state files
    /// actually read, the control path actually reached), never a compile-time
    /// claim (pitfall P11).
    fn measured(&self) -> (SupervisorIdentity, SupervisionCapabilities);
}

// ═══════════════════════════════════════════════════════════════════════
// Identity: a stable, opaque name → ServiceId mapping
// ═══════════════════════════════════════════════════════════════════════

/// The contract's `T.ServiceId` is opaque; this backend issues one stable
/// number per service name (FNV-1a over the UTF-8 bytes). Deriving an id from
/// the service's *name* — never from a host identity, a uid, or a pid — keeps
/// two identical enumerations answering with identical ids (the contract's
/// stable-`total` promise), and a name that disappears never renumbers the
/// services that remain.
fn service_id_for(name: &str) -> ServiceId {
    // FNV-1a, 64-bit: a name in, one opaque id out.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ServiceId::new(hash)
}

/// Nanoseconds since the epoch (the contract's `T.Timestamp` unit). A zero
/// means "the clock could not be read" — never fabricated.
pub(crate) fn now_nanos() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// The error a reason class becomes when the runtime gate refuses, so the
/// code a caller sees is the matrix's own class (never a defaulted code).
fn error_for_reason_class(class: ReasonClass, reason: &str) -> Error {
    match class {
        ReasonClass::NoPrivilege => Error::with_message(ErrorCode::PermissionDenied, reason),
        ReasonClass::NotApplicable | ReasonClass::NoBackend => Error::not_supported(reason),
        ReasonClass::DisabledByPolicy | ReasonClass::Unknown => Error::unavailable(reason),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The cells, chosen by measurement
// ═══════════════════════════════════════════════════════════════════════

/// Which supervisor the hub is speaking to. `pub` so a caller (and the tests)
/// can name a cell; the host reaches it only through
/// [`LinuxServiceBackend::native_connect`], which measures it.
pub enum Cell {
    Runit(RunitBackend),
    Systemd(SystemdBackend),
    Openrc(OpenrcBackend),
    /// The supervisor is present and enumerable but its state is not a native
    /// read (sysvinit): every answer carries the reason.
    Sysvinit,
    /// No supervisor reached: the honest absence.
    None,
}

/// How many pending events one subscription keeps before a pull must collapse
/// the oldest (a bound so a peer that never pulls cannot grow us without limit).
const SUB_QUEUE_CAP: usize = 256;

/// One subscription's fingerprint of the observation it last saw, per service,
/// so a pull answers "what changed since your last pull" from a fresh native
/// read rather than from an invented event stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    entries: Vec<(String, Option<(ServiceStatus, u64)>)>,
}

impl Snapshot {
    /// The fingerprint one observation set produces.
    fn of(observations: &[NativeObservation]) -> Snapshot {
        let mut entries: Vec<(String, Option<(ServiceStatus, u64)>)> = observations
            .iter()
            .map(|o| (o.name.clone(), o.state.as_ref().map(|s| (s.status, s.main_pid))))
            .collect();
        entries.sort();
        Snapshot { entries }
    }

    /// The service names whose status or pid changed between two snapshots,
    /// in name order. A name that vanished is not an event (the supervisor
    /// stopped serving it, which the next full enumeration reports).
    fn changed_since(&self, previous: &Snapshot) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(name, state)| {
                previous
                    .entries
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, prev)| prev != state)
                    .unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// One live subscription: the service it is scoped to (`None` = every
/// service), its last snapshot, and its bounded pending queue.
struct Subscription {
    service: Option<String>,
    snapshot: Option<Snapshot>,
    queue: Vec<ServiceEvent>,
    collapsed: u64,
    active: bool,
}

/// The mutable half, behind one mutex.
struct SubscriptionState {
    subscriptions: Vec<(u64, Subscription)>,
    next_sub: u64,
}

/// The gated control reference this backend mints. Every `ServiceControl`
/// verb answers with the contract's own honest `notSupported` (a host
/// supervisor owns no registry surface), and the granted scope is kept so the
/// refusal never claims an authority it was not given.
struct SupervisorControl {
    /// The scope the presented capability covered, spelled out.
    scope: String,
}

impl ServiceControl for SupervisorControl {
    fn set_dependencies(&self, _service_id: ServiceId, _dependencies: &[ServiceDependency]) -> Result<()> {
        Err(Error::not_supported(format!(
            "no-backend: setDependencies is a service-registry operation; the supervisor this \
             backend reached ({scope}) declares no dependency graph a client can write",
            scope = self.scope
        )))
    }

    fn set_resource_allocation(
        &self,
        _service_id: ServiceId,
        _allocation: ServiceResourceAllocation,
    ) -> Result<()> {
        Err(Error::not_supported(format!(
            "no-backend: no per-service quota store exists behind the supervisor this backend \
             reached ({scope}) to write into",
            scope = self.scope
        )))
    }

    fn set_health_check(&self, _service_id: ServiceId, _check: ServiceHealthCheck) -> Result<()> {
        Err(Error::not_supported(format!(
            "no-backend: rewriting a probe configuration would edit the host's service \
             directories under {scope}, which a read-only backend does not do",
            scope = self.scope
        )))
    }

    fn update_config(&self, _service_id: ServiceId, _config: &Metadata) -> Result<()> {
        Err(Error::not_supported(format!(
            "no-backend: no configuration store for a service exists behind {scope}",
            scope = self.scope
        )))
    }

    fn set_quarantine(&self, _service_id: ServiceId, _quarantined: bool, _reason: &str) -> Result<()> {
        Err(Error::not_supported(format!(
            "no-backend: {scope} has no quarantine concept; the nearest native authority is the \
             lifecycle verb, which is separately gate-routed",
            scope = self.scope
        )))
    }

    fn set_log_level(&self, _service_id: ServiceId, _level: crate::domains::service::types::LogLevel) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: a per-service log level is logging.capnp's surface; this backend keeps no \
             level store",
        ))
    }

    fn clear_metrics(&self, _service_id: ServiceId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: no service metrics store exists behind this backend, so there is nothing \
             to purge",
        ))
    }
}

/// The Linux service backend: one cell per supervisor the host runs, chosen by
/// measurement at construction, plus the subscriptions and the identity
/// mapping shared by every cell. Object-safe as `dyn ServiceBackend`.
pub struct LinuxServiceBackend {
    cell: Cell,
    identity: SupervisorIdentity,
    caps: SupervisionCapabilities,
    subscriptions: Mutex<SubscriptionState>,
}

impl LinuxServiceBackend {
    /// Detect the host's supervisor and build the backend for it. Detection is
    /// measured (the supervisor's own marker paths), never a compile-time
    /// claim (pitfall P11): a box running none of the four yields the honest
    /// `none` backend whose every read answers with a reason.
    pub fn native_connect() -> Self {
        // systemd's own marker, first: on a systemd host it is the one that
        // answers, and its native path (the system bus) is the one this domain
        // speaks (no `systemctl` subprocess).
        if std::path::Path::new("/run/systemd/system").is_dir() {
            return Self::of(Cell::Systemd(
                SystemdBackend::connect().unwrap_or_else(|_| SystemdBackend::host()),
            ));
        }
        // runit: its per-service state root, or its service scan dir.
        if std::path::Path::new("/run/runit").is_dir() || std::path::Path::new("/var/service").is_dir()
        {
            return Self::of(Cell::Runit(RunitBackend::host()));
        }
        // openrc: its own state directory.
        if std::path::Path::new("/run/openrc").is_dir() {
            return Self::of(Cell::Openrc(OpenrcBackend::host()));
        }
        // sysvinit: init scripts exist, and there is no portable native state
        // to read (the honest absence, decided and named).
        if std::path::Path::new("/etc/init.d").is_dir() {
            return Self::of(Cell::Sysvinit);
        }
        Self::of(Cell::None)
    }

    /// Build over an explicit cell, measuring its identity and capabilities.
    fn of(cell: Cell) -> Self {
        let (identity, caps) = match &cell {
            Cell::Runit(b) => b.measured(),
            Cell::Systemd(b) => b.measured(),
            Cell::Openrc(b) => b.measured(),
            Cell::Sysvinit => (
                SupervisorIdentity {
                    supervisor: "sysvinit".to_string(),
                    service_root: "/etc/init.d".to_string(),
                    declared_root: "/etc/init.d".to_string(),
                    state_root: String::new(),
                },
                SupervisionCapabilities {
                    supervisor: "sysvinit".to_string(),
                    can_enumerate: true,
                    // Measured: not one state file was readable, because
                    // sysvinit has no portable one to read.
                    can_read_state: false,
                    can_read_exact_state: false,
                    can_control: false,
                },
            ),
            Cell::None => (SupervisorIdentity::default(), SupervisionCapabilities::default()),
        };
        LinuxServiceBackend {
            cell,
            identity,
            caps,
            subscriptions: Mutex::new(SubscriptionState { subscriptions: Vec::new(), next_sub: 1 }),
        }
    }

    /// An explicit cell, for the tests (they build a scripted supervisor tree
    /// and never touch the host's).
    pub fn with_cell(cell: Cell) -> Self {
        Self::of(cell)
    }

    /// The measured cell, so a test can assert which supervisor was chosen.
    pub fn cell_name(&self) -> &'static str {
        match &self.cell {
            Cell::Runit(_) => "runit",
            Cell::Systemd(_) => "systemd",
            Cell::Openrc(_) => "openrc",
            Cell::Sysvinit => "sysvinit",
            Cell::None => "none",
        }
    }

    /// One native enumeration, or the honest absence when no supervisor was
    /// reached. The error names what was looked for and not found.
    fn observe(&self) -> Result<Vec<NativeObservation>> {
        match &self.cell {
            Cell::Runit(b) => Ok(b.enumerate()),
            Cell::Systemd(b) => Ok(b.enumerate()),
            Cell::Openrc(b) => Ok(b.enumerate()),
            Cell::Sysvinit => Err(Error::unavailable(
                "no-backend: a sysvinit host has no portable native state to read — the pidfile \
                 convention is per-script, so a run state read here would be a guess. The archive \
                 body shelled out to `service <name> status` and parsed its text; that shellout is \
                 the mechanism this domain replaces, and no native path replaces it on sysvinit.",
            )),
            Cell::None => Err(Error::unavailable(
                "no-backend: no supported supervisor was reached on this host — looked for \
                 /run/systemd/system (systemd), /run/runit and /var/service (runit), /run/openrc \
                 (openrc) and /etc/init.d (sysvinit), and found none of them.",
            )),
        }
    }

    /// The namespace of the observation, used by the reason strings so a
    /// refusal always names the supervisor it is about.
    fn scope_label(&self) -> String {
        if self.identity.supervisor.is_empty() {
            "no supervisor".to_string()
        } else {
            self.identity.supervisor.clone()
        }
    }

    /// One observation by name.
    fn find<'a>(&self, observations: &'a [NativeObservation], name: &str) -> Option<&'a NativeObservation> {
        observations.iter().find(|o| o.name == name)
    }

    /// The service name an id maps to, given the current enumeration (the
    /// mapping is stable per name, so this is a scan over the observed set).
    fn name_for_id(&self, observations: &[NativeObservation], id: ServiceId) -> Option<String> {
        observations
            .iter()
            .find(|o| service_id_for(&o.name) == id)
            .map(|o| o.name.clone())
    }

    /// The dependencies of one service, in the supervisor's own terms.
    fn dependencies_of(&self, name: &str) -> Result<Vec<NativeDep>> {
        let deps = match &self.cell {
            Cell::Runit(b) => Some(b.dependencies(name)),
            Cell::Systemd(b) => Some(b.dependencies(name)),
            Cell::Openrc(b) => Some(b.dependencies(name)),
            Cell::Sysvinit | Cell::None => None,
        };
        match deps {
            Some(deps) => Ok(deps),
            None => Err(Error::not_supported(format!(
                "not-applicable: the supervisor this backend reached ({}) declares no dependency \
                 graph, so there are no dependency facts to answer with",
                self.scope_label()
            ))),
        }
    }

    /// Turn one native observation into the contract's instance info.
    fn info_of(&self, observation: &NativeObservation) -> ServiceInstanceInfo {
        ServiceInstanceInfo {
            id: service_id_for(&observation.name),
            name: observation.name.clone(),
            description: observation.description.clone(),
            state: self.state_of(observation),
            enabled: observation.enabled,
            // A supervisor that starts a service when it is scanned has no
            // demand concept; `on_demand` is the false that measures rather
            // than a guess.
            on_demand: false,
            manifest: 0,
        }
    }

    /// One observation's state, honest about what could not be read: a state
    /// the supervisor reported, or `unknown` with the measured reason (the
    /// contract's own first-class answer for "the state is not known").
    fn state_of(&self, observation: &NativeObservation) -> ServiceInstanceState {
        match &observation.state {
            Some(state) => ServiceInstanceState {
                status: state.status,
                main_pid: state.main_pid,
                since: state.since,
                since_us: state.since_us,
                since_valid: state.since_valid,
                quarantined: false,
                reason: state.reason.clone(),
                metadata: self.metadata_of(observation),
            },
            None => ServiceInstanceState {
                status: ServiceStatus::Unknown,
                main_pid: 0,
                since: 0,
                since_us: 0,
                since_valid: false,
                quarantined: false,
                reason: if observation.state_reason.is_empty() {
                    format!(
                        "unavailable: the supervisor {} reports no run state for this service",
                        self.scope_label()
                    )
                } else {
                    observation.state_reason.clone()
                },
                metadata: self.metadata_of(observation),
            },
        }
    }

    /// The measured facts that travel beside a state: which supervisor read
    /// it, and whether the service is declared/linked/supervised. Ordered, so
    /// two renders of the same answer never differ. On the runit cell the
    /// `/proc`-derived supervision truth travels with it too (the `runsv` pid,
    /// the live child count, where the state came from, the pid-file reading),
    /// because that is what an unprivileged caller's state is actually made of.
    fn metadata_of(&self, observation: &NativeObservation) -> Metadata {
        let mut facts = alloc::vec![
            ("supervisor".to_string(), self.identity.supervisor.clone()),
            ("declared".to_string(), observation.declared.to_string()),
            ("linked".to_string(), observation.enabled.to_string()),
            ("supervised".to_string(), observation.supervised.to_string()),
        ];
        if observation.proc_facts.runsv_pid != 0 {
            facts.push(("runsv".to_string(), observation.proc_facts.runsv_pid.to_string()));
        }
        if !observation.proc_facts.state_source.is_empty() {
            facts.push(("state_source".to_string(), observation.proc_facts.state_source.to_string()));
        }
        if observation.proc_facts.live_children != 0 {
            facts.push(("live_children".to_string(), observation.proc_facts.live_children.to_string()));
        }
        if !observation.proc_facts.pidfile.is_empty() {
            facts.push(("pidfile".to_string(), observation.proc_facts.pidfile.clone()));
        }
        facts
    }

    /// The discovery-view health of one observation: the lifecycle is measured,
    /// the probe is declared unrun (a read-only backend never executes a
    /// service's probe script), and the fault counters read zero because a
    /// supervisor observes none — the availability matrix's `getHealth` row
    /// carries that declaration.
    fn health_of(&self, observation: &NativeObservation) -> ServiceHealth {
        let state = self.state_of(observation);
        let health_state = match state.status {
            ServiceStatus::Active => HealthState::Online,
            ServiceStatus::Activating | ServiceStatus::Reloading => HealthState::Starting,
            ServiceStatus::Deactivating => HealthState::Maintenance,
            ServiceStatus::Inactive | ServiceStatus::Failed => HealthState::Offline,
            ServiceStatus::Unknown | ServiceStatus::Custom => HealthState::Unknown,
        };
        ServiceHealth {
            service_id: service_id_for(&observation.name),
            state: health_state,
            reason: state.reason.clone(),
            quarantined: false,
            since_us: state.since_us,
            since_valid: state.since_valid,
            last_check_passed: false,
            last_check_at: 0,
            fault_count: 0,
            consecutive_faults: 0,
            restarts: 0,
        }
    }

    /// The name one subscription is scoped to, resolved through the current
    /// enumeration (`None` = every service, the subscription's own default).
    fn subscription_name(&self, id: ServiceId) -> Option<String> {
        if id.get() == 0 {
            return None;
        }
        match self.observe() {
            Ok(observations) => self.name_for_id(&observations, id),
            // A supervisor that cannot enumerate cannot scope a subscription
            // to one service; the subscription stays unscoped rather than
            // inventing a scope.
            Err(_) => None,
        }
    }

    /// Broadcast one event into every other active subscription's bounded
    /// queue.
    fn broadcast(subscriptions: &mut [(u64, Subscription)], event: &ServiceEvent, mine: Option<u64>) {
        for (id, sub) in subscriptions.iter_mut() {
            if !sub.active || Some(*id) == mine {
                continue;
            }
            if let Some(scope) = sub.service.as_deref() {
                if service_id_for(scope) != event.service_id {
                    continue;
                }
            }
            if sub.queue.len() >= SUB_QUEUE_CAP {
                sub.queue.remove(0);
                sub.collapsed += 1;
            }
            sub.queue.push(event.clone());
        }
    }

    /// The events this pull yields, from a fresh native enumeration diffed
    /// against the subscription's last snapshot. Shared by the state pull and
    /// the service-event pull: one observation, two projections of it.
    fn pull_events(
        &self,
        subscription: &mut Subscription,
        max_events: u32,
    ) -> Result<Vec<ServiceEvent>> {
        let observations = self.observe()?;
        let scoped: Vec<NativeObservation> = match subscription.service.as_deref() {
            Some(name) => observations.into_iter().filter(|o| &o.name == name).collect(),
            None => observations,
        };
        let fresh = Snapshot::of(&scoped);
        let changed = match subscription.snapshot.as_ref() {
            Some(previous) => fresh.changed_since(previous),
            // The first pull reports every service it can see: a subscription
            // that starts with nothing must answer with what exists, not with
            // an invented stream of nothing.
            None => scoped.iter().map(|o| o.name.clone()).collect(),
        };
        subscription.snapshot = Some(fresh);
        let bounded = max_events.max(1) as usize;
        let timestamp = now_nanos();
        Ok(changed
            .into_iter()
            .filter_map(|name| self.find(&scoped, &name))
            .take(bounded)
            .map(|observation| ServiceEvent {
                payload: ServiceEventPayload::StateChanged(self.state_of(&observation)),
                timestamp,
                service_id: service_id_for(&observation.name),
            })
            .collect())
    }
}

impl ServiceLifecycle for LinuxServiceBackend {
    fn apply_lifecycle(&self, service_id: ServiceId, service_name: &str, verb: LifecycleVerb) -> Result<()> {
        // The accessor is honest before the write: the caller must hold the
        // capability the shell's write gate carries, and the engine reports
        // the refusal rather than performing a write it was not asked for.
        if let Some((class, reason)) = runtime_gate(&self.caps, verb.as_token()) {
            return Err(error_for_reason_class(class, reason));
        }
        let _ = service_id;
        match &self.cell {
            Cell::Runit(b) => b.control(service_name, verb),
            Cell::Systemd(b) => b.control(service_name, verb),
            Cell::Openrc(_) | Cell::Sysvinit | Cell::None => Err(Error::not_supported(format!(
                "no-backend: the supervisor this backend reached ({}) exposes no native control \
                 path this domain speaks — openrc's only control surface is the rc-service script \
                 invocation and sysvinit has none portable; the archive body's `sv`/`systemctl` \
                 shellout is the mechanism this domain replaces, and it is not replaced here",
                self.scope_label()
            ))),
        }
    }
}

impl ServiceBackend for LinuxServiceBackend {
    fn supervision_caps(&self) -> SupervisionCapabilities {
        self.caps.clone()
    }

    fn supervisor(&self) -> SupervisorIdentity {
        self.identity.clone()
    }

    fn list_services(&self, page: Pagination) -> Result<Page<ServiceInstanceInfo>> {
        let observations = self.observe()?;
        let total = observations.len() as u64;
        let limit = page.effective_limit() as usize;
        let items = observations
            .iter()
            .skip(page.offset as usize)
            .take(limit)
            .map(|o| self.info_of(o))
            .collect();
        Ok(Page { items, total })
    }

    fn get_service(&self, service_id: ServiceId) -> Result<ServiceInstanceInfo> {
        let observations = self.observe()?;
        let found = self
            .name_for_id(&observations, service_id)
            .and_then(|name| self.find(&observations, &name));
        match found {
            Some(observation) => Ok(self.info_of(observation)),
            None => Err(Error::with_message(ErrorCode::NotFound, format!(
                "not-found: no service with id {} is served by the supervisor {} — the archive \
                 body would have answered `sv status`'s text here instead",
                service_id.get(),
                self.scope_label()
            ))),
        }
    }

    fn resolve(&self, name: &str) -> Result<(ServiceId, bool)> {
        let observations = self.observe()?;
        match self.find(&observations, name) {
            Some(observation) if observation.declared || observation.enabled || observation.supervised => {
                Ok((service_id_for(&observation.name), true))
            }
            // A name the supervisor does not serve is the answer, not a fault.
            _ => Ok((ServiceId::new(0), false)),
        }
    }

    fn get_state(&self, service_id: ServiceId) -> Result<ServiceInstanceState> {
        let observations = self.observe()?;
        let found = self
            .name_for_id(&observations, service_id)
            .and_then(|name| self.find(&observations, &name));
        match found {
            Some(observation) => Ok(self.state_of(observation)),
            None => Err(Error::with_message(ErrorCode::NotFound, format!(
                "not-found: no service with id {} is served by the supervisor {}",
                service_id.get(),
                self.scope_label()
            ))),
        }
    }

    fn subscribe_state(&self, service_id: ServiceId) -> Result<SubscriptionId> {
        let service = self.subscription_name(service_id);
        let mut state = self.subscriptions.lock().unwrap_or_else(|e| e.into_inner());
        let id = state.next_sub;
        state.next_sub += 1;
        state.subscriptions.push((
            id,
            Subscription { service, snapshot: None, queue: Vec::new(), collapsed: 0, active: true },
        ));
        Ok(SubscriptionId(id))
    }

    fn pull_state(&self, subscription: SubscriptionId, max_states: u32) -> Result<Vec<ServiceInstanceState>> {
        let mut state = self.subscriptions.lock().unwrap_or_else(|e| e.into_inner());
        let events = {
            let (_, sub) = state
                .subscriptions
                .iter_mut()
                .find(|(id, _)| *id == subscription.0)
                .ok_or_else(|| {
                    Error::with_message(ErrorCode::NotFound, format!(
                        "not-found: no subscription with id {} — mint one with subscribeState",
                        subscription.0
                    ))
                })?;
            self.pull_events(sub, max_states)?
        };
        // Every other active subscription hears about the same transitions.
        for event in &events {
            Self::broadcast(&mut state.subscriptions, event, Some(subscription.0));
        }
        Ok(events
            .into_iter()
            .map(|event| match event.payload {
                ServiceEventPayload::StateChanged(state) => state,
                // The state pull only ever yields state events.
                _ => ServiceInstanceState::default(),
            })
            .collect())
    }

    fn cancel_state(&self, subscription: SubscriptionId) -> Result<()> {
        let mut state = self.subscriptions.lock().unwrap_or_else(|e| e.into_inner());
        match state.subscriptions.iter_mut().find(|(id, _)| *id == subscription.0) {
            Some((_, sub)) => {
                sub.active = false;
                Ok(())
            }
            None => Err(Error::with_message(ErrorCode::NotFound, format!(
                "not-found: no subscription with id {} to retire",
                subscription.0
            ))),
        }
    }

    fn get_health(&self, service_id: ServiceId) -> Result<ServiceHealth> {
        let observations = self.observe()?;
        let found = self
            .name_for_id(&observations, service_id)
            .and_then(|name| self.find(&observations, &name));
        match found {
            Some(observation) => Ok(self.health_of(observation)),
            None => Err(Error::with_message(ErrorCode::NotFound, format!(
                "not-found: no service with id {} is served by the supervisor {}",
                service_id.get(),
                self.scope_label()
            ))),
        }
    }

    fn list_health(&self, page: Pagination) -> Result<Page<ServiceHealth>> {
        let observations = self.observe()?;
        let total = observations.len() as u64;
        let limit = page.effective_limit() as usize;
        let items = observations
            .iter()
            .skip(page.offset as usize)
            .take(limit)
            .map(|o| self.health_of(o))
            .collect();
        Ok(Page { items, total })
    }

    fn list_quarantined(&self, _page: Pagination) -> Result<Page<ServiceHealth>> {
        // Measured, not assumed: a host supervisor has no quarantine concept,
        // so the honest answer is the empty list with a zero total. That is a
        // fact about the supervisor, not a fabricated sample.
        Ok(Page::default())
    }

    fn get_dependencies(&self, service_id: ServiceId) -> Result<Vec<ServiceDependency>> {
        let observations = self.observe()?;
        let name = self.name_for_id(&observations, service_id).ok_or_else(|| {
            Error::with_message(ErrorCode::NotFound, format!(
                "not-found: no service with id {} is served by the supervisor {}",
                service_id.get(),
                self.scope_label()
            ))
        })?;
        self.dependencies_of(&name).map(|deps| {
            deps.into_iter()
                .map(|dep| ServiceDependency {
                    service_id: service_id_for(&dep.name),
                    dependency_type: DependencyType::Required,
                    version_constraint: String::new(),
                    satisfied: dep.satisfied,
                })
                .collect()
        })
    }

    fn get_resource_allocation(&self, service_id: ServiceId) -> Result<ServiceResourceAllocation> {
        Err(Error::not_supported(format!(
            "not-applicable: no quota is declared for a service by the supervisor {} — the \
             supervisor runs the process and scopes nothing, so a quota read would be a \
             fabrication (the request was for service id {})",
            self.scope_label(),
            service_id.get()
        )))
    }

    fn get_health_check(&self, service_id: ServiceId) -> Result<ServiceHealthCheck> {
        Err(Error::not_supported(format!(
            "not-applicable: no health-check configuration is declared for a service by the \
             supervisor {}; its probe is a script this read-only backend does not execute or \
             rewrite (the request was for service id {})",
            self.scope_label(),
            service_id.get()
        )))
    }

    fn get_metrics(&self, service_id: ServiceId, _window_ns: u64) -> Result<ServiceMetrics> {
        Err(Error::not_supported(format!(
            "not-applicable: request counts, error counts and latency quantiles are application \
             measurements; the supervisor {} observes none of them, so none are reported (the \
             request was for service id {})",
            self.scope_label(),
            service_id.get()
        )))
    }

    fn subscribe_service_events(&self, service_id: ServiceId) -> Result<SubscriptionId> {
        self.subscribe_state(service_id)
    }

    fn pull_service_events(&self, subscription: SubscriptionId, max_events: u32) -> Result<Vec<ServiceEvent>> {
        let mut state = self.subscriptions.lock().unwrap_or_else(|e| e.into_inner());
        let (_, sub) = state
            .subscriptions
            .iter_mut()
            .find(|(id, _)| *id == subscription.0)
            .ok_or_else(|| {
                Error::with_message(ErrorCode::NotFound, format!(
                    "not-found: no subscription with id {} — mint one with \
                     subscribeServiceEvents",
                    subscription.0
                ))
            })?;
        let events = self.pull_events(sub, max_events)?;
        // A pull's own events are its answer; a second pull returns the next
        // batch, never this one again (the snapshot already advanced).
        let _ = sub;
        for event in &events {
            Self::broadcast(&mut state.subscriptions, event, Some(subscription.0));
        }
        Ok(events)
    }

    fn cancel_service_events(&self, subscription: SubscriptionId) -> Result<()> {
        self.cancel_state(subscription)
    }

    fn control(&self, authority: PresentedCapability) -> Result<Box<dyn ServiceControl>> {
        if self.identity.supervisor.is_empty() {
            return Err(Error::with_message(
                ErrorCode::PermissionDenied,
                "permission-denied: no ServiceControl reference can be minted — no supervisor was \
                 reached on this host to bind it to",
            ));
        }
        // The scope rule: `*` covers every service this supervisor serves, a
        // service's name covers that service, and an empty or unresolvable
        // target covers nothing.
        if authority.target == "*" {
            return Ok(Box::new(SupervisorControl { scope: self.identity.supervisor.clone() }));
        }
        let observations = self.observe()?;
        let covered = self
            .find(&observations, &authority.target)
            .is_some_and(|o| o.declared || o.enabled || o.supervised);
        if covered {
            Ok(Box::new(SupervisorControl {
                scope: format!("{} service {}", self.identity.supervisor, authority.target),
            }))
        } else {
            Err(Error::with_message(ErrorCode::PermissionDenied, format!(
                "permission-denied: the presented capability's target {target:?} is not a service \
                 the supervisor {supervisor} serves, so no control reference is minted for it",
                target = authority.target,
                supervisor = self.identity.supervisor
            )))
        }
    }
}
