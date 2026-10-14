//! The public service API: a thin, borrowing wrapper over [`ServiceBackend`]
//! exposing the calls a client actually touches. Borrowed and zero-cost — it
//! adds no state over `&B`.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::core::ids::ServiceId;
use crate::core::Result;

use super::backend::{PresentedCapability, ServiceBackend, ServiceControl};
use super::types::{
    Page, Pagination, ServiceDependency, ServiceEvent, ServiceHealth, ServiceHealthCheck,
    ServiceInstanceInfo, ServiceInstanceState, ServiceMetrics, ServiceResourceAllocation,
    SubscriptionId, SupervisionCapabilities, SupervisorIdentity,
};

/// A borrowing handle to any [`ServiceBackend`]. The caller has already
/// acquired authority to hold it (law #5: the handle is the authority), and
/// observation stays open — a caller may learn what exists and in what state.
pub struct Service<'a, B: ServiceBackend + ?Sized>(&'a B);

impl<'a, B: ServiceBackend + ?Sized> Service<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Service(backend)
    }

    /// The supervisor's measured identity.
    #[must_use]
    pub fn supervisor(&self) -> SupervisorIdentity {
        self.0.supervisor()
    }

    /// This backend's published capability tokens.
    #[must_use]
    pub fn caps(&self) -> SupervisionCapabilities {
        self.0.supervision_caps()
    }

    /// The bounded service listing.
    #[must_use]
    pub fn list(&self, page: Pagination) -> Result<Page<ServiceInstanceInfo>> {
        self.0.list_services(page)
    }

    /// One service by identity.
    #[must_use]
    pub fn get(&self, service_id: ServiceId) -> Result<ServiceInstanceInfo> {
        self.0.get_service(service_id)
    }

    /// Resolve a name to an identity; `present == false` is a first-class
    /// answer (a service that has not started yet is the answer, not a fault).
    #[must_use]
    pub fn resolve(&self, name: &str) -> Result<(ServiceId, bool)> {
        self.0.resolve(name)
    }

    /// The current state of one service.
    #[must_use]
    pub fn state(&self, service_id: ServiceId) -> Result<ServiceInstanceState> {
        self.0.get_state(service_id)
    }

    /// Subscribe to state transitions and pull the first bounded batch.
    #[must_use]
    pub fn watch_state(
        &self,
        service_id: ServiceId,
        max_states: u32,
    ) -> Result<(SubscriptionId, Vec<ServiceInstanceState>)> {
        let sub = self.0.subscribe_state(service_id)?;
        let first = self.0.pull_state(sub, max_states)?;
        Ok((sub, first))
    }

    /// Retire a state subscription obtained from [`Service::watch_state`].
    pub fn close_state(&self, sub: SubscriptionId) -> Result<()> {
        self.0.cancel_state(sub)
    }

    /// Runtime health: lifecycle AND probe, one honest answer.
    #[must_use]
    pub fn health(&self, service_id: ServiceId) -> Result<ServiceHealth> {
        self.0.get_health(service_id)
    }

    /// The bounded health listing.
    #[must_use]
    pub fn health_list(&self, page: Pagination) -> Result<Page<ServiceHealth>> {
        self.0.list_health(page)
    }

    /// Services currently quarantined (the honest empty where the supervisor
    /// has no quarantine concept).
    #[must_use]
    pub fn quarantined(&self, page: Pagination) -> Result<Page<ServiceHealth>> {
        self.0.list_quarantined(page)
    }

    /// The dependencies of one service.
    #[must_use]
    pub fn dependencies(&self, service_id: ServiceId) -> Result<Vec<ServiceDependency>> {
        self.0.get_dependencies(service_id)
    }

    /// The declared resource allocation of one service.
    #[must_use]
    pub fn allocation(&self, service_id: ServiceId) -> Result<ServiceResourceAllocation> {
        self.0.get_resource_allocation(service_id)
    }

    /// The health-check configuration of one service.
    #[must_use]
    pub fn health_check(&self, service_id: ServiceId) -> Result<ServiceHealthCheck> {
        self.0.get_health_check(service_id)
    }

    /// Runtime measurements over a declared window.
    #[must_use]
    pub fn metrics(&self, service_id: ServiceId, window_ns: u64) -> Result<ServiceMetrics> {
        self.0.get_metrics(service_id, window_ns)
    }

    /// Subscribe to service events and pull the first bounded batch.
    #[must_use]
    pub fn watch_events(
        &self,
        service_id: ServiceId,
        max_events: u32,
    ) -> Result<(SubscriptionId, Vec<ServiceEvent>)> {
        let sub = self.0.subscribe_service_events(service_id)?;
        let first = self.0.pull_service_events(sub, max_events)?;
        Ok((sub, first))
    }

    /// Retire an event subscription obtained from [`Service::watch_events`].
    pub fn close_events(&self, sub: SubscriptionId) -> Result<()> {
        self.0.cancel_service_events(sub)
    }

    /// The gated control surface: evaluate the presented capability, then hold
    /// the reference that IS the authority.
    pub fn control(&self, authority: PresentedCapability) -> Result<Box<dyn ServiceControl>> {
        self.0.control(authority)
    }
}
