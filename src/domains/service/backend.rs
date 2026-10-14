//! The `ServiceBackend` trait: the **ungated observation** half of
//! `service.capnp` as one object-safe hub, plus the gated
//! [`ServiceControl`] accessor and the two subscriptions' `pull`/`cancel`.
//!
//! ## Authority story (law #5)
//!
//! There is no permission string on any method in this file. The contract's
//! split is mirrored: **observation stays open** (a caller that holds the
//! `Service` reference may learn what exists and in what state), and every
//! mutation lives behind `control`, whose returned [`ServiceControl`]
//! reference **is** the authority — no method on it takes a token. The
//! lifecycle verbs (start/stop/restart) are the contract's delegated
//! authority: `service.capnp` deleted its own copies and pointed at
//! `init.capnp`'s `InitService`; until that domain lands, the supervisor's
//! native control path is exposed through [`crate::domains::service::
//! backend::ServiceLifecycle`], host-mutating and gate-routed at the shell.
//!
//! ## What is real vs honestly absent (law #6)
//!
//! A supervisor host can serve the observation surface natively — the runit
//! path reads the supervise tree, the systemd path speaks the system bus, the
//! openrc path reads openrc's own state files. What a supervisor genuinely
//! does not own is answered with a `reasonClass`-prefixed reason, never a
//! fabricated value: no supervisor declares a per-service quota, a health-check
//! configuration, or application metrics, and none of the 7 `ServiceControl`
//! verbs exist on a host supervisor. See [`super::availability`] for the
//! matrix and `crate::platforms::linux::service` for the per-supervisor native
//! decisions.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::core::ids::ServiceId;
use crate::core::Result;

use super::types::{
    Metadata, Page, Pagination, ServiceDependency, ServiceEvent, ServiceHealth,
    ServiceHealthCheck, ServiceInstanceInfo, ServiceInstanceState, ServiceMetrics,
    ServiceResourceAllocation, SubscriptionId, SupervisionCapabilities, SupervisorIdentity,
};

/// The capability reference a caller presents to `control @13`.
///
/// Mirrored owned from `capability.capnp`'s `Capability`: the opaque token, the
/// target it covers, the operations it grants, and its expiry. The backend
/// evaluates it at acquisition — the contract's own rule: a constraint it
/// cannot evaluate is an error, never a silent grant. `target` is the
/// backend-defined scope: the string `"*"` names every service this backend
/// supervises, and any other value is resolved against the enumerated service
/// names (a supervisor's names ARE its scope).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresentedCapability {
    /// The opaque capability token (typically 32–64 bytes).
    pub id: Vec<u8>,
    /// The target scope: `"*"` for all services, or one service's name.
    pub target: String,
    /// The operations granted (the contract's open `Operation` vocabulary).
    pub operations: Vec<String>,
    /// Expiry in nanoseconds since the epoch (zero = never).
    pub expiry_ns: u64,
}

impl PresentedCapability {
    /// Whether the presented capability covers `service_name`, by the scope
    /// rule above. A malformed or empty target covers nothing — an
    /// over-broad read of an authority is the defect the authority design
    /// exists to stop.
    pub fn covers(&self, service_name: &str) -> bool {
        self.target == "*" || self.target == service_name
    }

    /// Whether it grants an operation (the vocabulary is open, so the match
    /// is a plain token match — a backend never invents an operation name).
    pub fn grants(&self, operation: &str) -> bool {
        self.operations.iter().any(|o| o == operation)
    }
}

/// How the caller asks the lifecycle verbs to be applied. Every verb is
/// host-mutating; the engine method is honest about being a write by its very
/// existence, and at the shell every call routes through the host-write gate
/// (charter law #8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleVerb {
    Start,
    Stop,
    Restart,
}

impl LifecycleVerb {
    /// The verb's own token (the archive's `ServiceBackend` vocabulary:
    /// `start`/`stop`/`restart`).
    pub const fn as_token(self) -> &'static str {
        match self {
            LifecycleVerb::Start => "start",
            LifecycleVerb::Stop => "stop",
            LifecycleVerb::Restart => "restart",
        }
    }
}

/// The gated mutating half: `ServiceControl`, the interface `control @13`
/// returns. Every method takes no token — the reference itself is the
/// authority (the contract's own rule). No method here exists on a host
/// supervisor, so the honest default for each is a `notSupported` with the
/// reason that names what a supervisor does and does not own.
pub trait ServiceControl: Send + 'static {
    /// `setDependencies @0` — a registry graph operation, not a supervisor's.
    fn set_dependencies(&self, _service_id: ServiceId, _dependencies: &[ServiceDependency]) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setDependencies is a service-registry operation; a host supervisor \
             (runit/systemd/openrc) declares no dependency graph a client can write — the archive \
             body's enable/disable symlinks were the closest thing and they are not this verb",
        ))
    }

    /// `setResourceAllocation @1` — a declared quota. A host supervisor owns
    /// no per-service quota a client may set through this interface.
    fn set_resource_allocation(
        &self,
        _service_id: ServiceId,
        _allocation: ServiceResourceAllocation,
    ) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setResourceAllocation writes a declared quota; on this backend the quota \
             is not a supervisor fact (the supervisor runs the process and nothing scopes it), so \
             nothing is written and nothing is fabricated",
        ))
    }

    /// `setHealthCheck @2` — health-check *configuration*. The supervisor's own
    /// probe (runit's `check` script, systemd's `WatchdogSec`) is a file the
    /// backend reads but never rewrites: writing one changes host state.
    fn set_health_check(&self, _service_id: ServiceId, _check: ServiceHealthCheck) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setHealthCheck writes the probe configuration; a supervisor's probe lives \
             in its own service directory (runit's ./check, systemd's unit file), which this \
             read-only backend does not rewrite",
        ))
    }

    /// `updateConfig @3` — an open configuration map the backend owns the
    /// validation of.
    fn update_config(&self, _service_id: ServiceId, _config: &Metadata) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: updateConfig is the engine's own configuration surface for a service; a \
             host supervisor has no such store, and none is invented here",
        ))
    }

    /// `setQuarantine @4` — running but not permitted to serve.
    fn set_quarantine(&self, _service_id: ServiceId, _quarantined: bool, _reason: &str) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setQuarantine is a serving-policy operation; a host supervisor has no \
             quarantine concept (the closest native mechanism is the lifecycle verb, which is \
             ServiceLifecycle's own gate-routed authority)",
        ))
    }

    /// `setLogLevel @5` — the declared log level over the canonical logging
    /// contract.
    fn set_log_level(&self, _service_id: ServiceId, _level: super::types::LogLevel) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: setLogLevel writes a service's declared log level; a supervisor has no \
             per-service level of its own, and the log store is logging.capnp's surface",
        ))
    }

    /// `clearMetrics @6` — purge this service's retained metrics.
    fn clear_metrics(&self, _service_id: ServiceId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: clearMetrics purges the measurement store; this backend keeps no service \
             metrics store (a supervisor observes none), so there is nothing to purge",
        ))
    }

    /// A nameable reason a method is unavailable — used by tooling that
    /// mirrors the availability matrix. Not a permission string; a diagnostic
    /// label.
    fn last_unavailable_reason(&self, _method: &str) -> Option<String> {
        None
    }
}

/// The lifecycle authority this engine exposes while the contract's own
/// controller (`init.capnp`'s `InitService.start/stop/restart`) has not
/// landed: the supervisor's **native control path**, spoken directly.
///
/// The archive body ran `sv start <name>` / `systemctl start <name>` as a
/// subprocess per verb — the condemned fork-per-op shellout (P5). This trait's
/// implementations write the supervisor's own control protocol instead: on
/// runit a single byte to the supervise control FIFO, on systemd a method call
/// on the system bus. Every method is host-mutating, so the live bring-back
/// proof is the founder's (charter law #8) and every call at the shell routes
/// through the host-write gate.
pub trait ServiceLifecycle: Send + 'static {
    /// Apply one lifecycle verb to a service.
    fn apply_lifecycle(&self, _service_id: ServiceId, _service_name: &str, _verb: LifecycleVerb) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no native control path is reachable from this host — the archive body's \
             `sv`/`systemctl` subprocess shellout is the mechanism this domain replaces, and no \
             supervisor control protocol is available here",
        ))
    }
}

/// A protocol-compliant service backend: one method per `Service` contract
/// method, with honest `Unavailable`/`notSupported`-with-reason for everything
/// the host cannot serve.
pub trait ServiceBackend: ServiceLifecycle {
    /// Publish this backend's honest supervision capability tokens (law #6):
    /// which supervisor is reached and what this caller can do through it. A
    /// consumer can tell a native read from a decided absence without a
    /// permission string.
    fn supervision_caps(&self) -> SupervisionCapabilities {
        SupervisionCapabilities::default()
    }

    /// The supervisor's measured identity: the supervisor token and the three
    /// directories it was observed with. A `none` supervisor is an honest
    /// absence, never a guess.
    fn supervisor(&self) -> SupervisorIdentity {
        SupervisorIdentity::default()
    }

    // ── Bounded listing and lookup ──────────────────────────────────────────

    /// The bounded service listing (`listServices @0`). Every page read is
    /// bounded by the caller's `limit`; `total` is stable across two
    /// identical calls.
    fn list_services(&self, _page: Pagination) -> Result<Page<ServiceInstanceInfo>> {
        Err(crate::core::Error::unavailable(
            "no-backend: no supervisor was reached on this host, so no service can be enumerated \
             (the archive body shelled out to `sv`/`systemctl list-units`; that shellout is the \
             mechanism this domain replaces)",
        ))
    }

    /// One service by identity (`getService @1`).
    fn get_service(&self, _service_id: ServiceId) -> Result<ServiceInstanceInfo> {
        Err(crate::core::Error::unavailable(
            "no-backend: no supervisor was reached on this host, so no service can be looked up",
        ))
    }

    /// Resolve a name to an identity (`resolve @7`). `present == false` is a
    /// first-class answer — a service that has not started yet is not a
    /// failure to answer the question, it IS the answer — so the default is
    /// the honest empty rather than an error.
    fn resolve(&self, _name: &str) -> Result<(ServiceId, bool)> {
        Ok((ServiceId::new(0), false))
    }

    // ── State and its subscription ──────────────────────────────────────────

    /// The current state of one service (`getState @2`).
    fn get_state(&self, _service_id: ServiceId) -> Result<ServiceInstanceState> {
        Err(crate::core::Error::unavailable(
            "no-backend: no supervisor was reached on this host, so no service state can be read",
        ))
    }

    /// Mint a state-subscription handle (`subscribeState @3`).
    fn subscribe_state(&self, _service_id: ServiceId) -> Result<SubscriptionId> {
        Err(crate::core::Error::not_supported(
            "no-backend: a state subscription needs a supervisor to observe; none was reached on \
             this host",
        ))
    }

    /// Pull the next bounded batch of states
    /// (`ServiceStateSubscription.pull @0`), collapsed to `max_states`.
    fn pull_state(&self, _subscription: SubscriptionId, _max_states: u32) -> Result<Vec<ServiceInstanceState>> {
        Err(crate::core::Error::not_supported(
            "no-backend: no state subscription exists on this host to pull from",
        ))
    }

    /// Retire a state subscription (`ServiceStateSubscription.cancel @1`).
    fn cancel_state(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no state subscription exists on this host to retire",
        ))
    }

    // ── Health ──────────────────────────────────────────────────────────────

    /// Runtime health of one service: lifecycle AND probe, one honest answer
    /// (`getHealth @4`, the canonical `D.ServiceHealth` type).
    fn get_health(&self, _service_id: ServiceId) -> Result<ServiceHealth> {
        Err(crate::core::Error::unavailable(
            "no-backend: no supervisor was reached on this host, so no service health can be read",
        ))
    }

    /// The bounded health listing (`listHealth @5`).
    fn list_health(&self, _page: Pagination) -> Result<Page<ServiceHealth>> {
        Err(crate::core::Error::unavailable(
            "no-backend: no supervisor was reached on this host, so no health listing can be read",
        ))
    }

    /// Services currently quarantined (`listQuarantined @6`) — running but not
    /// permitted to serve. On a host supervisor with no quarantine concept
    /// this is the honest empty (`total == 0`), never a fabricated entry.
    fn list_quarantined(&self, _page: Pagination) -> Result<Page<ServiceHealth>> {
        Ok(Page::default())
    }

    // ── Dependencies and configuration ──────────────────────────────────────

    /// The dependencies of one service (`getDependencies @8`), from the
    /// canonical discovery contract. A supervisor that declares no dependency
    /// graph answers with a reason rather than an empty (an empty would claim
    /// "it has no dependencies", which is a different statement).
    fn get_dependencies(&self, _service_id: ServiceId) -> Result<Vec<ServiceDependency>> {
        Err(crate::core::Error::not_supported(
            "not-applicable: this host's supervisor declares no per-service dependency graph, so \
             there are no dependency facts to answer with (the archive body inferred none either)",
        ))
    }

    /// The declared resource allocation (`getResourceAllocation @9`).
    fn get_resource_allocation(&self, _service_id: ServiceId) -> Result<ServiceResourceAllocation> {
        Err(crate::core::Error::not_supported(
            "not-applicable: no quota is declared for a service by this host's supervisor — the \
             supervisor runs the process and scopes nothing, so a quota read would be fabricated",
        ))
    }

    /// The health-check configuration (`getHealthCheck @10`).
    fn get_health_check(&self, _service_id: ServiceId) -> Result<ServiceHealthCheck> {
        Err(crate::core::Error::not_supported(
            "not-applicable: no health-check configuration is declared for a service by this \
             host's supervisor; its probe is a script this read-only backend does not execute or \
             rewrite",
        ))
    }

    // ── Measurements ────────────────────────────────────────────────────────

    /// Runtime measurements with a declared window (`getMetrics @11`).
    fn get_metrics(&self, _service_id: ServiceId, _window_ns: u64) -> Result<ServiceMetrics> {
        Err(crate::core::Error::not_supported(
            "not-applicable: request counts, error counts and latency quantiles are application \
             measurements; a host supervisor observes none of them, so none are reported",
        ))
    }

    // ── Service events and their subscription ───────────────────────────────

    /// Mint a service-event subscription (`subscribeServiceEvents @12`).
    fn subscribe_service_events(&self, _service_id: ServiceId) -> Result<SubscriptionId> {
        Err(crate::core::Error::not_supported(
            "no-backend: an event subscription needs a supervisor to observe; none was reached on \
             this host",
        ))
    }

    /// Pull the next bounded batch of service events
    /// (`ServiceEventSubscription.pull @0`), collapsed to `max_events`.
    fn pull_service_events(&self, _subscription: SubscriptionId, _max_events: u32) -> Result<Vec<ServiceEvent>> {
        Err(crate::core::Error::not_supported(
            "no-backend: no event subscription exists on this host to pull from",
        ))
    }

    /// Retire an event subscription (`ServiceEventSubscription.cancel @1`).
    fn cancel_service_events(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(crate::core::Error::not_supported(
            "no-backend: no event subscription exists on this host to retire",
        ))
    }

    // ── The one gated surface ───────────────────────────────────────────────

    /// `control @13`: evaluate the presented capability and mint the
    /// [`ServiceControl`] reference that IS the authority. A backend that
    /// cannot evaluate the authority's scope refuses (`permissionDenied` with
    /// a reason) rather than minting a handle the host did not grant.
    fn control(&self, _authority: PresentedCapability) -> Result<Box<dyn ServiceControl>> {
        Err(crate::core::Error::with_message(
            crate::core::ErrorCode::PermissionDenied,
            "permission-denied: no ServiceControl reference can be minted on this backend — no \
             supervisor control surface exists here to bind it to",
        ))
    }

    /// A nameable reason a method is unavailable — used by tooling that mirrors
    /// the availability matrix. Not a permission string; a diagnostic label.
    fn last_unavailable_reason(&self, _method: &str) -> Option<String> {
        None
    }
}
