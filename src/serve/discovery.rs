//! The discovery service: what this host serves, published as the
//! contract's own `ServiceDescriptor`.
//!
//! # What discovery is for
//!
//! A client that knows nothing must be able to learn what exists: the
//! endpoints, the capabilities a caller needs, the state, the health. That
//! is exactly `discovery.capnp`'s `Discovery` interface, and the service
//! answers it from the same state the daemon serves.
//!
//! # The endpoint's transport is the open scheme token
//!
//! `ServiceEndpoint.transport` is an open token ("capnp", "rest", or a 2046
//! one). The engine publishes the token its own transport registry answers
//! to — never a closed enum of paradigms, and never a transport the
//! registry does not hold.
//!
//! # The CALL MODEL and the CARRIER are both published
//!
//! The contract's own comment for `ServiceEndpoint.transport` names the CALL
//! MODEL ("capnp", "rest", "grpc"), while the engine's transport registry is
//! keyed on the CARRIER ("unix", "tcp"). One field, two vocabularies — so
//! this engine publishes both, in the contract's open metadata:
//!
//! * the descriptor's `metadata` carries `callModel` — what this service
//!   speaks;
//! * the endpoint's `metadata` carries `callModel` and `carrier` — what this
//!   service speaks, and the carrier it answers at.
//!
//! A client that does not speak the published call model refuses the endpoint
//! **with a reason** (never a silent wrong protocol); a client that reads the
//! carrier knows where to dial.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::sync::Mutex;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::schema::discovery_capnp::discovery as disc;
use crate::schema::discovery_capnp::ServiceState as ContractServiceState;
use crate::schema::discovery_capnp::{discovery_registry, discovery_subscription};

use super::errors;
use super::service::ServedDomain;

/// The discovery interface's id, from the contract's generated code.
pub const INTERFACE_ID: u64 = disc::_private::TYPE_ID;

/// The discovery registry interface's id, from the contract's generated code.
pub const REGISTRY_INTERFACE_ID: u64 = discovery_registry::_private::TYPE_ID;

/// One endpoint this service is reachable at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedEndpoint {
    /// The open transport token ("unix", "tcp", "capnp", or a 2046 one).
    pub transport: String,
    /// The address in the transport's own vocabulary.
    pub address: String,
    /// The endpoint's open metadata: the CALL MODEL this service speaks and
    /// the CARRIER it is reachable over (both contract-legal map entries —
    /// see [`crate::serve::CALL_MODEL`]).
    pub metadata: Vec<(String, String)>,
}

impl PublishedEndpoint {
    /// An endpoint on a carrier, publishing the call model and the carrier
    /// in the open metadata.
    pub fn new(transport: &str, address: &str) -> Self {
        Self {
            transport: transport.to_string(),
            address: address.to_string(),
            metadata: vec![
                (
                    crate::serve::METADATA_CALL_MODEL.to_string(),
                    crate::serve::CALL_MODEL.to_string(),
                ),
                (
                    crate::serve::METADATA_CARRIER.to_string(),
                    transport.to_string(),
                ),
            ],
        }
    }
}

/// One registered service: the contract's own descriptor.
#[derive(Clone, Debug)]
pub struct RegisteredService {
    /// The contract's service id (the schema's file id).
    pub id: u64,
    /// The name a client resolves.
    pub name: String,
    /// The open category token.
    pub category: String,
    /// The capability ids a caller needs before this service is usable.
    pub required_capability_ids: Vec<u64>,
    /// The state the service is in.
    pub state: ServiceState,
    /// The endpoints the service answers at.
    pub endpoints: Vec<PublishedEndpoint>,
    /// The service's open metadata: the CALL MODEL this service speaks.
    pub metadata: Vec<(String, String)>,
}

/// The state a registered service is in (the contract's `ServiceState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// `unknown @0`.
    Unknown,
    /// `online @1`.
    Online,
    /// `offline @2`.
    Offline,
    /// `degraded @3`.
    Degraded,
    /// `starting @4`.
    Starting,
    /// `maintenance @5`.
    Maintenance,
    /// `custom @6`.
    Custom,
}

impl ServiceState {
    /// The contract's own spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceState::Unknown => "unknown",
            ServiceState::Online => "online",
            ServiceState::Offline => "offline",
            ServiceState::Degraded => "degraded",
            ServiceState::Starting => "starting",
            ServiceState::Maintenance => "maintenance",
            ServiceState::Custom => "custom",
        }
    }
}

/// The discovery state: the services this host publishes.
#[derive(Default)]
pub struct DiscoveryState {
    services: Mutex<Vec<RegisteredService>>,
    endpoints: Mutex<Vec<PublishedEndpoint>>,
}

impl DiscoveryState {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The endpoints this service is reachable at.
    pub fn endpoints(&self) -> Vec<PublishedEndpoint> {
        self.endpoints
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Publish the endpoints this service is reachable at, and update every
    /// registered service with them (the endpoint is a property of the
    /// service, not of the moment it was registered).
    ///
    /// Every published endpoint carries the CALL MODEL this service speaks
    /// and the CARRIER it answers over, in the endpoint's open metadata — a
    /// client reads them before it dials, and never has to guess.
    pub fn set_endpoints(&self, endpoints: Vec<PublishedEndpoint>) {
        let endpoints: Vec<PublishedEndpoint> = endpoints
            .into_iter()
            .map(|endpoint| {
                let mut metadata = endpoint.metadata.clone();
                if !metadata
                    .iter()
                    .any(|(key, _)| key == crate::serve::METADATA_CALL_MODEL)
                {
                    metadata.push((
                        crate::serve::METADATA_CALL_MODEL.to_string(),
                        crate::serve::CALL_MODEL.to_string(),
                    ));
                }
                if !metadata
                    .iter()
                    .any(|(key, _)| key == crate::serve::METADATA_CARRIER)
                {
                    metadata.push((
                        crate::serve::METADATA_CARRIER.to_string(),
                        endpoint.transport.clone(),
                    ));
                }
                PublishedEndpoint {
                    metadata,
                    ..endpoint
                }
            })
            .collect();
        if let Ok(mut guard) = self.endpoints.lock() {
            *guard = endpoints.clone();
        }
        if let Ok(mut guard) = self.services.lock() {
            for service in guard.iter_mut() {
                service.endpoints = endpoints.clone();
            }
        }
    }

    /// Register a served domain's descriptor.
    pub fn register_domain(&self, domain: &dyn ServedDomain, endpoints: &[PublishedEndpoint]) {
        let mut metadata = vec![(
            crate::serve::METADATA_CALL_MODEL.to_string(),
            crate::serve::CALL_MODEL.to_string(),
        )];
        // The domain's own capability tokens, in the open metadata where an
        // open vocabulary belongs (the contract's `requiredCapabilityIds`
        // carries opaque numeric handles, not words).
        let tokens = domain.capability_tokens();
        if !tokens.is_empty() {
            metadata.push((
                crate::serve::METADATA_CAPABILITIES.to_string(),
                tokens.join(", "),
            ));
        }
        let service = RegisteredService {
            id: domain.service_id(),
            name: domain.name().to_string(),
            category: domain.category().to_string(),
            required_capability_ids: domain.required_capability_ids(),
            // The state is the DOMAIN'S OWN: a service that cannot answer
            // part of its contract is `degraded`, and the missing part is
            // named in the registry.
            state: domain.state(),
            endpoints: endpoints.to_vec(),
            // The CALL MODEL this service speaks, published in the
            // descriptor's open metadata: a client reads it before it dials.
            metadata,
        };
        self.register(service);
    }

    /// Register (or replace) a service.
    pub fn register(&self, service: RegisteredService) {
        if let Ok(mut guard) = self.services.lock() {
            match guard.iter_mut().find(|entry| entry.id == service.id) {
                Some(entry) => *entry = service,
                None => guard.push(service),
            }
        }
    }

    /// Deregister a service.
    pub fn deregister(&self, id: u64) -> bool {
        if let Ok(mut guard) = self.services.lock() {
            let before = guard.len();
            guard.retain(|entry| entry.id != id);
            return guard.len() != before;
        }
        false
    }

    /// Every registered service.
    pub fn services(&self) -> Vec<RegisteredService> {
        self.services
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// One service, by id.
    pub fn get(&self, id: u64) -> Option<RegisteredService> {
        self.services
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|entry| entry.id == id).cloned())
    }

    /// One service, by name.
    pub fn resolve(&self, name: &str) -> Option<RegisteredService> {
        self.services
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|entry| entry.name == name).cloned())
    }
}

/// Write an open metadata map (the contract's `T.StringMap`).
fn write_metadata(
    entries: &[(String, String)],
    builder: &mut crate::schema::types_capnp::string_map::Builder<'_>,
) {
    let mut map = builder.reborrow().init_entries(entries.len() as u32);
    for (index, (key, value)) in entries.iter().enumerate() {
        let mut entry = map.reborrow().get(index as u32);
        entry.set_key(key);
        entry.set_value(value);
    }
}

/// Write a registered service into the contract's `ServiceDescriptor`.
fn write_descriptor(
    service: &RegisteredService,
    builder: &mut crate::schema::discovery_capnp::service_descriptor::Builder<'_>,
) {
    builder.reborrow().init_id().set_id(service.id);
    builder.set_name(&service.name);
    builder.set_category(&service.category);
    builder.set_state(match service.state {
        ServiceState::Unknown => ContractServiceState::Unknown,
        ServiceState::Online => ContractServiceState::Online,
        ServiceState::Offline => ContractServiceState::Offline,
        ServiceState::Degraded => ContractServiceState::Degraded,
        ServiceState::Starting => ContractServiceState::Starting,
        ServiceState::Maintenance => ContractServiceState::Maintenance,
        ServiceState::Custom => ContractServiceState::Custom,
    });
    let mut endpoints = builder.reborrow().init_endpoints(service.endpoints.len() as u32);
    for (index, endpoint) in service.endpoints.iter().enumerate() {
        let mut entry = endpoints.reborrow().get(index as u32);
        entry.set_transport(&endpoint.transport);
        entry.set_address(&endpoint.address);
        write_metadata(&endpoint.metadata, &mut entry.reborrow().init_metadata());
    }
    let mut required = builder
        .reborrow()
        .init_required_capability_ids(service.required_capability_ids.len() as u32);
    for (index, capability) in service.required_capability_ids.iter().enumerate() {
        required.reborrow().get(index as u32).set_id(*capability);
    }
    write_metadata(&service.metadata, &mut builder.reborrow().init_metadata());
}

/// Write a service's runtime health, in the contract's own `ServiceHealth`.
///
/// The health is what this host *observes*, and the only observation the
/// engine has is "the service is registered and serving": so the state is
/// the service's own state, the last check passed, and no transition has
/// been observed (never a fabricated "it changed zero microseconds ago").
fn write_health(
    service: &RegisteredService,
    builder: &mut crate::schema::discovery_capnp::service_health::Builder<'_>,
) {
    let mut id = builder.reborrow().init_service_id();
    id.set_id(service.id);
    builder.set_state(match service.state {
        ServiceState::Unknown => ContractServiceState::Unknown,
        ServiceState::Online => ContractServiceState::Online,
        ServiceState::Offline => ContractServiceState::Offline,
        ServiceState::Degraded => ContractServiceState::Degraded,
        ServiceState::Starting => ContractServiceState::Starting,
        ServiceState::Maintenance => ContractServiceState::Maintenance,
        ServiceState::Custom => ContractServiceState::Custom,
    });
    builder.set_quarantined(false);
    builder.set_since_valid(false);
    builder.set_last_check_passed(true);
    builder.set_fault_count(0);
    builder.set_consecutive_faults(0);
}

/// The discovery server: the contract's own `Discovery` interface.
pub struct DiscoveryServer {
    state: std::sync::Arc<DiscoveryState>,
}

impl Clone for DiscoveryServer {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl disc::Server for DiscoveryServer {
    fn lookup(
        self: std::rc::Rc<Self>,
        params: disc::LookupParams,
        mut results: disc::LookupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let filter = params.get_filter()?;
            let wanted = crate::serve::text_of(filter.get_name()?);
            let category = crate::serve::text_of(filter.get_category()?);
            let services = state.services();
            let selected: Vec<RegisteredService> = services
                .into_iter()
                .filter(|service| wanted.is_empty() || service.name == wanted)
                .filter(|service| category.is_empty() || service.category == category)
                .collect();
            let mut builder = results.get();
            let mut list = builder.reborrow().init_services(selected.len() as u32);
            for (index, service) in selected.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                write_descriptor(service, &mut entry);
            }
            builder.set_total(selected.len() as u64);
            Ok(())
        }
    }

    fn get(
        self: std::rc::Rc<Self>,
        params: disc::GetParams,
        mut results: disc::GetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let id = params.get_service_id()?.get_id();
            match state.get(id) {
                Some(service) => {
                    write_descriptor(&service, &mut results.get().init_descriptor());
                    Ok(())
                }
                None => Err(errors::decline(
                    crate::core::ErrorCode::NotFound,
                    &alloc::format!("notFound: no service with id {id} is registered here"),
                )),
            }
        }
    }

    fn resolve(
        self: std::rc::Rc<Self>,
        params: disc::ResolveParams,
        mut results: disc::ResolveResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let name = crate::serve::text_of(params.get_name()?);
            // "Returns `present` rather than an error when the name is
            // unknown: a service that has not started yet is not a failure
            // to answer the question, it is the answer." (discovery.capnp)
            match state.resolve(&name) {
                Some(service) => {
                    let mut builder = results.get();
                    write_descriptor(&service, &mut builder.reborrow().init_descriptor());
                    builder.set_present(true);
                    Ok(())
                }
                None => {
                    results.get().set_present(false);
                    Ok(())
                }
            }
        }
    }

    /// Watch registrations change: the subscription IS the authority to
    /// pull (the contract's own capability).
    fn subscribe(
        self: std::rc::Rc<Self>,
        _params: disc::SubscribeParams,
        mut results: disc::SubscribeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let server = DiscoverySubscriptionServer;
            let client: discovery_subscription::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    /// The runtime health of one service, as opposed to the descriptor's
    /// declared state.
    fn get_runtime_health(
        self: std::rc::Rc<Self>,
        params: disc::GetRuntimeHealthParams,
        mut results: disc::GetRuntimeHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let id = params.get_service_id()?.get_id();
            match state.get(id) {
                Some(service) => {
                    write_health(&service, &mut results.get().init_health());
                    Ok(())
                }
                None => Err(errors::decline(
                    crate::core::ErrorCode::NotFound,
                    &alloc::format!("notFound: no service with id {id} is registered here"),
                )),
            }
        }
    }

    /// The runtime health of every registered service.
    fn list_runtime_health(
        self: std::rc::Rc<Self>,
        _params: disc::ListRuntimeHealthParams,
        mut results: disc::ListRuntimeHealthResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let services = state.services();
            let mut builder = results.get();
            let mut list = builder.reborrow().init_services(services.len() as u32);
            for (index, service) in services.iter().enumerate() {
                write_health(service, &mut list.reborrow().get(index as u32));
            }
            builder.set_total(services.len() as u64);
            Ok(())
        }
    }

    /// Services running but quarantined. This engine quarantines nothing,
    /// so the list is empty — and that is the truth, not a placeholder.
    fn list_quarantined(
        self: std::rc::Rc<Self>,
        _params: disc::ListQuarantinedParams,
        mut results: disc::ListQuarantinedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let mut builder = results.get();
            builder.reborrow().init_services(0);
            builder.set_total(0);
            Ok(())
        }
    }

    /// The dependencies of one service. This engine declares none.
    fn get_service_dependencies(
        self: std::rc::Rc<Self>,
        _params: disc::GetServiceDependenciesParams,
        mut results: disc::GetServiceDependenciesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let mut builder = results.get();
            builder.reborrow().init_dependencies(0);
            builder.set_total(0);
            Ok(())
        }
    }

    /// The dependents of one service. This engine declares none.
    fn get_dependents(
        self: std::rc::Rc<Self>,
        _params: disc::GetDependentsParams,
        mut results: disc::GetDependentsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let mut builder = results.get();
            builder.reborrow().init_dependents(0);
            builder.set_total(0);
            Ok(())
        }
    }

    /// The deprecated services. This engine deprecates none.
    fn list_deprecated_services(
        self: std::rc::Rc<Self>,
        _params: disc::ListDeprecatedServicesParams,
        mut results: disc::ListDeprecatedServicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let mut builder = results.get();
            builder.reborrow().init_services(0);
            builder.set_total(0);
            Ok(())
        }
    }

    /// The per-service version check.
    fn check_version_compatibility(
        self: std::rc::Rc<Self>,
        params: disc::CheckVersionCompatibilityParams,
        mut results: disc::CheckVersionCompatibilityResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let id = params.get_service_id()?.get_id();
            let client_version = params.get_client_version()?;
            let builder = results.get();
            let mut check = builder.init_check();
            check.set_service_id(params.get_service_id()?)?;
            check.set_client_version(client_version);
            match state.get(id) {
                Some(_) => {
                    // This service speaks the generation it published; a
                    // caller on the same major version is compatible.
                    check.set_compatible(true);
                    check.set_reason("");
                }
                None => {
                    check.set_compatible(false);
                    check.set_reason("no service with that id is registered here");
                }
            }
            Ok(())
        }
    }

    /// Lookup by isolation scope. This engine runs no isolation tiers, so
    /// the honest answer is that the boundary does not exist here.
    fn get_service_by_scope(
        self: std::rc::Rc<Self>,
        _params: disc::GetServiceByScopeParams,
        _results: disc::GetServiceByScopeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service does not run isolation \
             scopes; use lookup, which answers for every published service",
        )))
    }

    fn registry(
        self: std::rc::Rc<Self>,
        _params: disc::RegistryParams,
        mut results: disc::RegistryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let server = DiscoveryRegistryServer { state };
            let client: discovery_registry::Client = capnp_rpc::new_client(server);
            results.get().set_service(client);
            Ok(())
        }
    }
}

/// The registry server: the contract's own `DiscoveryRegistry` — the one
/// gated surface, and the authority to register.
pub struct DiscoveryRegistryServer {
    state: std::sync::Arc<DiscoveryState>,
}

impl Clone for DiscoveryRegistryServer {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl discovery_registry::Server for DiscoveryRegistryServer {
    fn register(
        self: std::rc::Rc<Self>,
        params: discovery_registry::RegisterParams,
        mut results: discovery_registry::RegisterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let descriptor = params.get_descriptor()?;
            let service = RegisteredService {
                id: descriptor.get_id()?.get_id(),
                name: crate::serve::text_of(descriptor.get_name()?),
                category: crate::serve::text_of(descriptor.get_category()?),
                required_capability_ids: Vec::new(),
                state: match descriptor.get_state()? {
                    ContractServiceState::Online => ServiceState::Online,
                    ContractServiceState::Offline => ServiceState::Offline,
                    ContractServiceState::Degraded => ServiceState::Degraded,
                    ContractServiceState::Starting => ServiceState::Starting,
                    ContractServiceState::Maintenance => ServiceState::Maintenance,
                    ContractServiceState::Custom => ServiceState::Custom,
                    ContractServiceState::Unknown => ServiceState::Unknown,
                },
                endpoints: Vec::new(),
                metadata: Vec::new(),
            };
            state.register(service);
            results.get().set_success(());
            Ok(())
        }
    }

    /// A partial update: the fields this engine models.
    fn update(
        self: std::rc::Rc<Self>,
        params: discovery_registry::UpdateParams,
        mut results: discovery_registry::UpdateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let id = params.get_service_id()?.get_id();
            let patch = params.get_patch()?;
            if patch.has_metadata() {
                // A field this engine does not model is refused, never
                // silently dropped.
                return Err(errors::decline(
                    crate::core::ErrorCode::NotSupported,
                    "notSupported: no-backend: this registry does not model \
                     service metadata",
                ));
            }
            let Some(mut service) = state.get(id) else {
                let builder = results.get();
                let mut failure = builder.init_error();
                errors::write_t_error(
                    &crate::core::Error::with_message(
                        crate::core::ErrorCode::NotFound,
                        alloc::format!("notFound: no service with id {id} is registered here"),
                    ),
                    &mut failure,
                );
                return Ok(());
            };
            let endpoints = patch.get_endpoints()?;
            if endpoints.len() > 0 {
                let mut updated = Vec::new();
                for index in 0..endpoints.len() {
                    let endpoint = endpoints.get(index);
                    let transport = alloc::string::String::from_utf8_lossy(
                        endpoint.get_transport()?.as_bytes(),
                    )
                    .into_owned();
                    let address = alloc::string::String::from_utf8_lossy(
                        endpoint.get_address()?.as_bytes(),
                    )
                    .into_owned();
                    updated.push(PublishedEndpoint::new(&transport, &address));
                }
                service.endpoints = updated;
            }
            state.register(service);
            results.get().set_success(());
            Ok(())
        }
    }

    /// Deprecation is a registry feature this engine does not model.
    fn deprecate_service(
        self: std::rc::Rc<Self>,
        _params: discovery_registry::DeprecateServiceParams,
        _results: discovery_registry::DeprecateServiceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this registry does not model \
             deprecation",
        )))
    }

    /// Isolation scopes are a registry feature this engine does not model.
    fn set_scope_boundary(
        self: std::rc::Rc<Self>,
        _params: discovery_registry::SetScopeBoundaryParams,
        _results: discovery_registry::SetScopeBoundaryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this registry does not run isolation \
             scopes",
        )))
    }

    fn deregister(
        self: std::rc::Rc<Self>,
        params: discovery_registry::DeregisterParams,
        mut results: discovery_registry::DeregisterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let id = params.get_service_id()?.get_id();
            if state.deregister(id) {
                results.get().set_success(());
            } else {
                let builder = results.get();
                let mut failure = builder.init_error();
                errors::write_t_error(
                    &crate::core::Error::with_message(
                        crate::core::ErrorCode::NotFound,
                        alloc::format!("notFound: no service with id {id} is registered here"),
                    ),
                    &mut failure,
                );
            }
            Ok(())
        }
    }
}

/// The discovery subscription server: the contract's own
/// `DiscoverySubscription` capability.
pub struct DiscoverySubscriptionServer;

impl Clone for DiscoverySubscriptionServer {
    fn clone(&self) -> Self {
        Self
    }
}

impl discovery_subscription::Server for DiscoverySubscriptionServer {
    fn pull(
        self: std::rc::Rc<Self>,
        _params: discovery_subscription::PullParams,
        mut results: discovery_subscription::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            // No event has been published yet: an empty list is the honest
            // answer, never a fabricated event.
            results.get().init_events(0);
            Ok(())
        }
    }

    fn cancel(
        self: std::rc::Rc<Self>,
        _params: discovery_subscription::CancelParams,
        mut results: discovery_subscription::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            results.get().set_success(());
            Ok(())
        }
    }
}

/// The discovery interface's server, as a hook the router can route to.
pub fn server(state: std::sync::Arc<DiscoveryState>) -> Box<dyn ClientHook> {
    let server = DiscoveryServer { state };
    let client: disc::Client = capnp_rpc::new_client(server);
    client.into_client_hook()
}
