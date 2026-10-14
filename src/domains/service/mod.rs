//! The **service** domain: the runtime observation of the services a host
//! supervisor runs — what exists, in what state, with what health, using what
//! — over one object-safe [`ServiceBackend`] trait that mirrors the frozen
//! `service.capnp` surface (`listServices @0` through `control @13`, plus the
//! two subscriptions and the gated [`ServiceControl`] half).
//!
//! What this domain is: the contract's observation surface as an honest
//! backend, reading supervision **natively** — runit's supervise tree, systemd's
//! system bus, openrc's own state files. The archive body's per-call `sv` /
//! `systemctl` / `rc-service` subprocess shellout (fork per op, locale text
//! parsing, lost exit codes — pitfall P5) is the mechanism this domain
//! replaces; there is no fork on any path here.
//!
//! What a host supervisor cannot serve is answered honestly with a reason:
//! no supervisor declares a per-service quota, a health-check configuration or
//! application metrics, and none of the seven [`ServiceControl`] registry verbs
//! exist on a host. Those rows are published as `UNAVAILABLE`-with-reason in
//! the availability matrix (see [`super::availability`]), and the runtime
//! repeats them one level down (a root-only supervise tree answers
//! `status = unknown` with the measured reason, never a fabricated value).
//!
//! ## The contract split this domain mirrors
//!
//! ```text
//!   Service (ungated)          observation: list, get, state, health,
//!                              dependencies, metrics, subscriptions
//!   control @13 → ServiceControl   the ONE gated surface; the returned
//!                              reference IS the authority (no method takes
//!                              a token)
//!   init.capnp (not here)      start / stop / restart — the contract moved the
//!                              controller there; until it lands, the
//!                              supervisor's native control path is exposed
//!                              through ServiceLifecycle (host-mutating,
//!                              gate-routed)
//! ```
//!
//! See `crate::platforms::linux::service` for the per-supervisor native-path
//! decisions with their evidence, and `smart-lib-ng/knowledge/19-service.md`
//! for the wave report.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Service;
pub use availability::{
    claimed_l3_tests, method_availability_for, runtime_gate, status_counts, AvailStatus,
    MethodAvailability, ReasonClass, SERVICE_AVAILABILITY, SERVICE_SCHEMA_ID,
};
pub use backend::{LifecycleVerb, PresentedCapability, ServiceBackend, ServiceControl, ServiceLifecycle};
pub use types::{
    DependencyType, HealthState, LogLevel, Metadata, Page, Pagination, Quota, ServiceDependency,
    ServiceEvent, ServiceEventPayload, ServiceEventType, ServiceHealth, ServiceHealthCheck,
    ServiceInstanceInfo, ServiceInstanceState, ServiceMetrics, ServiceResourceAllocation,
    ServiceStatus, SubscriptionId, SupervisionCapabilities, SupervisorIdentity,
};
