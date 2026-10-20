//! The contract served over capnp RPC — the spine's own tests.
//!
//! Every test here drives the service the way a *contract-only* client
//! would: through the generated `Client` of the contract's own schema, over
//! a transport's byte stream, with no engine type in sight. If a test can
//! pass only by knowing something the contract does not say, it is a bug in
//! the harness, not a strength of the test.

#![cfg(all(
    feature = "rpc",
    feature = "std",
    feature = "domain-network",
    feature = "domain-discovery"
))]

extern crate std;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use smart_lib::core::ids::NetworkInterfaceId;
use smart_lib::core::values::Duration;
use smart_lib::domains::network::backend::NetworkBackend;
use smart_lib::domains::network::types::{
    ConnStatus, HostState, HostStateSource, InterfaceCounters, InterfaceSnapshot, KnownWifiNetwork,
    Page, Pagination, ProxyConfigInfo, RadioState, RouteInfo, SubscriptionId,
};
use smart_lib::schema::abi_capnp::abi_service;
use smart_lib::schema::capability_capnp::capability_service;
use smart_lib::schema::discovery_capnp::discovery;
use smart_lib::schema::network_capnp::network as net;
use smart_lib::serve::abi::SupportedAbi;
use smart_lib::serve::discovery::PublishedEndpoint;
use smart_lib::serve::network::NetworkDomain;
use smart_lib::serve::service::{serve_connection, ClientSession, Service};
use smart_lib::serve::CapabilityClass;
use smart_lib::transport::channel::{Channel, ChannelListener, TransportError};
use smart_lib::transport::registry::{TransportRegistry, TransportRequest};
use smart_lib::transport::unix::UnixListener;

/// The `matched` arm of the handshake's union.
fn matched_arm(
    handshake: smart_lib::schema::abi_capnp::abi_handshake_response::Reader<'_>,
) -> smart_lib::schema::abi_capnp::abi::Reader<'_> {
    match handshake.which().expect("the union arm") {
        smart_lib::schema::abi_capnp::abi_handshake_response::Which::Matched(arm) => {
            arm.expect("the matched arm")
        }
        _ => panic!("the handshake must be matched"),
    }
}

/// The `incompatible` arm of the handshake's union.
fn incompatible_arm(
    handshake: smart_lib::schema::abi_capnp::abi_handshake_response::Reader<'_>,
) -> smart_lib::schema::types_capnp::error::Reader<'_> {
    match handshake.which().expect("the union arm") {
        smart_lib::schema::abi_capnp::abi_handshake_response::Which::Incompatible(arm) => {
            arm.expect("the incompatible arm")
        }
        _ => panic!("the handshake must be incompatible"),
    }
}

/// A contract `Text` field, read losslessly.
fn text(reader: capnp::text::Reader<'_>) -> String {
    String::from_utf8_lossy(reader.as_bytes()).into_owned()
}

/// A counter for unique socket paths.
static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A scripted backend: it answers the served methods with known values, and
/// records that a mutating verb was reached (so the authority gate can be
/// proven from the *backend's* side, not only from the answer).
#[derive(Default)]
struct MockBackend {
    reached: std::sync::Mutex<Vec<String>>,
}

impl MockBackend {
    fn reached(&self) -> Vec<String> {
        self.reached.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    fn record(&self, verb: &str) {
        if let Ok(mut guard) = self.reached.lock() {
            guard.push(verb.to_string());
        }
    }
}

impl NetworkBackend for MockBackend {
    fn get_interfaces(&self, _page: Pagination) -> smart_lib::core::Result<Page<InterfaceSnapshot>> {
        self.record("getInterfaces");
        Ok(Page {
            items: vec![InterfaceSnapshot {
                id: NetworkInterfaceId::new(7),
                name: "mock0".to_string(),
                kind: "ethernet".to_string(),
                hardware_address: smart_lib::domains::network::types::MacAddress("00:11:22:33:44:55".to_string()),
                up: true,
                connected: true,
                mtu: 1500,
                managed: true,
            }],
            total: 1,
        })
    }

    fn get_status(&self, interface: NetworkInterfaceId) -> smart_lib::core::Result<ConnStatus> {
        self.record("getStatus");
        Ok(ConnStatus {
            connected: true,
            interface,
            kind: "ethernet".to_string(),
            strength_dbm: 0,
            speed_bps: 1_000_000,
            quality: 100,
            metered: false,
            behind_captive: false,
            host_state: HostState::Managed,
            host_state_source: HostStateSource::of(HostState::Managed),
            ssid: String::new(),
        })
    }

    fn subscribe_status(
        &self,
        _interface: NetworkInterfaceId,
    ) -> smart_lib::core::Result<SubscriptionId> {
        self.record("subscribeStatus");
        Ok(SubscriptionId(11))
    }

    fn pull_status(&self, _subscription: SubscriptionId) -> smart_lib::core::Result<ConnStatus> {
        self.record("pullStatus");
        Ok(ConnStatus {
            connected: true,
            interface: NetworkInterfaceId::new(7),
            kind: "ethernet".to_string(),
            strength_dbm: 0,
            speed_bps: 1_000_000,
            quality: 100,
            metered: false,
            behind_captive: false,
            host_state: HostState::Managed,
            host_state_source: HostStateSource::of(HostState::Managed),
            ssid: String::new(),
        })
    }

    fn cancel_subscription(&self, _subscription: SubscriptionId) -> smart_lib::core::Result<()> {
        self.record("cancelSubscription");
        Ok(())
    }

    fn get_counters(
        &self,
        _interface: NetworkInterfaceId,
        _window: Duration,
    ) -> smart_lib::core::Result<InterfaceCounters> {
        self.record("getCounters");
        Ok(InterfaceCounters {
            bytes_in: 42,
            bytes_out: 43,
            packets_in: 1,
            packets_out: 2,
            errors_in: 0,
            errors_out: 0,
            drops_in: 0,
            drops_out: 0,
            sampled_at: 99,
        })
    }

    fn subscribe_counters(
        &self,
        _interface: NetworkInterfaceId,
    ) -> smart_lib::core::Result<SubscriptionId> {
        self.record("subscribeCounters");
        Ok(SubscriptionId(12))
    }

    fn pull_counters(&self, _subscription: SubscriptionId) -> smart_lib::core::Result<InterfaceCounters> {
        self.record("pullCounters");
        Ok(InterfaceCounters {
            bytes_in: 1,
            bytes_out: 2,
            packets_in: 3,
            packets_out: 4,
            errors_in: 0,
            errors_out: 0,
            drops_in: 0,
            drops_out: 0,
            sampled_at: 7,
        })
    }

    fn is_networking_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isNetworkingEnabled");
        Ok(true)
    }

    fn set_networking_enabled(&self, _enabled: bool) -> smart_lib::core::Result<()> {
        self.record("setNetworking");
        Ok(())
    }

    fn set_radio_enabled(&self, _radio: &str, _enabled: bool) -> smart_lib::core::Result<()> {
        self.record("setRadioEnabled");
        Ok(())
    }

    fn list_radio_states(&self, _page: Pagination) -> smart_lib::core::Result<Page<RadioState>> {
        self.record("listRadioStates");
        Ok(Page {
            items: vec![RadioState {
                name: "wifi".to_string(),
                enabled: true,
                hard_blocked: false,
                soft_blocked: false,
            }],
            total: 1,
        })
    }

    fn is_mobile_data_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isMobileDataEnabled");
        Ok(false)
    }

    fn set_mobile_data_enabled(&self, _enabled: bool) -> smart_lib::core::Result<()> {
        self.record("setMobileData");
        Ok(())
    }

    fn is_data_roaming_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isDataRoamingEnabled");
        Ok(false)
    }

    fn set_data_roaming_enabled(&self, _enabled: bool) -> smart_lib::core::Result<()> {
        self.record("setDataRoaming");
        Ok(())
    }

    fn list_known_wifi_networks(
        &self,
        _page: Pagination,
    ) -> smart_lib::core::Result<Page<KnownWifiNetwork>> {
        self.record("listKnownWifiNetworks");
        Ok(Page {
            items: vec![KnownWifiNetwork {
                ssid: "home".to_string(),
                auto_connect: true,
                last_connected: 1_700_000_000_000_000_000,
                security: "wpa2".to_string(),
            }],
            total: 1,
        })
    }

    fn get_routes(&self, _page: Pagination) -> smart_lib::core::Result<Page<RouteInfo>> {
        self.record("getRoutes");
        Ok(Page {
            items: vec![RouteInfo {
                destination_cidr: "0.0.0.0/0".to_string(),
                gateway: "10.0.0.1".to_string(),
                metric: 100,
                interface_id: NetworkInterfaceId::new(7),
            }],
            total: 1,
        })
    }

    fn get_proxy_config(&self) -> smart_lib::core::Result<ProxyConfigInfo> {
        self.record("getProxyConfig");
        Ok(ProxyConfigInfo {
            enabled: true,
            url: "http://proxy.invalid:3128".to_string(),
            bypass: vec![("localhost".to_string(), "127.0.0.1".to_string())],
        })
    }
}

/// The service under test: the network domain plus the foundation
/// interfaces.
fn build_service(backend: Arc<MockBackend>) -> Arc<Service> {
    let service = Service::new();
    service.abi().publish(SupportedAbi {
        schema_id: smart_lib::domains::network::availability::NETWORK_SCHEMA_ID,
        name: "smart-protocol/network".to_string(),
        major: 0,
        minor: 1,
    });
    let service = service.with_domain(Arc::new(NetworkDomain::new(backend)));
    service.set_endpoints(vec![PublishedEndpoint {
        transport: "unix".to_string(),
        address: "/tmp/smart-lib-rpc-test.sock".to_string(),
    }]);
    Arc::new(service)
}

/// A unique socket path for one test.
fn socket_path() -> String {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "/tmp/smart-lib-rpc-{}-{counter}.sock",
        std::process::id()
    )
}

/// Run a contract-only client against the service.
///
/// The service is served **on this thread** (the backend is the host's own
/// state and is not shared across threads); the client runs on its own
/// thread, exactly as a separate process would.
fn run_client<T, F>(service: Arc<Service>, path: &str, class: CapabilityClass, body: F) -> T
where
    F: FnOnce(&mut ClientSession) -> T + Send + 'static,
    T: Send + 'static,
{
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).expect("bind");
    let (sender, receiver) = std::sync::mpsc::channel();
    let client_path = path.to_string();
    std::thread::spawn(move || {
        let mut session = connect_unix(&client_path);
        let _ = sender.send(body(&mut session));
    });
    let channel = listener.accept().expect("accept");
    let _ = serve_connection(service, channel, class);
    receiver.recv().expect("the client's answer")
}

/// A client session over a unix socket, connected to the service.
fn connect_unix(path: &str) -> ClientSession {
    let channel = smart_lib::transport::unix::UnixChannel::connect(path).expect("connect");
    let stream = Box::new(channel)
        .into_byte_stream()
        .expect("byte stream");
    ClientSession::connect(stream).expect("session")
}

// ═══════════════════════════════════════════════════════════════════════
// The transport registry
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_registry_answers_an_unknown_scheme_honestly() {
    let registry = TransportRegistry::with_builtins();
    let error = match registry.connect(&TransportRequest::new("carrier-pigeon", "nowhere")) {
        Ok(_) => panic!("an unregistered scheme must not connect"),
        Err(error) => error,
    };
    let code = match &error {
        TransportError::UnknownScheme { scheme, known } => {
            assert_eq!(scheme, "carrier-pigeon");
            assert!(
                known.contains(&"unix".to_string()),
                "the answer must name the schemes that do exist: {known:?}"
            );
            error.error_code()
        }
        other => panic!("expected an unknown-scheme answer, got {other:?}"),
    };
    // The answer is *unavailable with a reason*, not a rejection: the error
    // code says so.
    assert_eq!(code, smart_lib::schema::types_capnp::ErrorCode::Unavailable);
}

#[test]
fn the_registry_holds_the_four_transports_and_nothing_more() {
    let registry = TransportRegistry::with_builtins();
    let schemes = registry.schemes();
    assert!(schemes.contains(&"unix".to_string()));
    assert!(schemes.contains(&"tcp".to_string()));
    assert!(schemes.contains(&"shmem".to_string()));
    assert!(schemes.contains(&"inprocess".to_string()));
}

#[test]
fn a_fifth_transport_registers_without_touching_the_protocol_crates() {
    use smart_lib::transport::registry::{TransportDescription, TransportImpl};

    struct Pigeon;
    impl TransportImpl for Pigeon {
        fn scheme(&self) -> &'static str {
            "pigeon"
        }
        fn description(&self) -> TransportDescription {
            TransportDescription {
                scheme: "pigeon",
                framing: "coo",
                capability_transfer: smart_lib::transport::channel::CapabilityTransfer::InBandToken,
                byte_stream: false,
                default_address: None,
                note: "a test double",
            }
        }
        fn connect(
            &self,
            _request: &TransportRequest,
        ) -> Result<Box<dyn Channel>, TransportError> {
            Err(TransportError::Connect {
                detail: "no pigeons today".to_string(),
            })
        }
        fn listen(
            &self,
            _request: &TransportRequest,
        ) -> Result<Box<dyn ChannelListener>, TransportError> {
            Err(TransportError::Connect {
                detail: "no pigeons today".to_string(),
            })
        }
    }

    let mut registry = TransportRegistry::with_builtins();
    registry.register(Arc::new(Pigeon));
    assert!(registry.schemes().contains(&"pigeon".to_string()));
    // And the engine's own scheme tokens are untouched by the addition.
    assert!(registry.lookup("unix").is_some());
}

// ═══════════════════════════════════════════════════════════════════════
// The service over a transport
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_network_domain_answers_the_contract_over_the_unix_transport() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let answered = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = network.get_interfaces_request();
                {
                    let params = request.get();
                    let mut pagination = params.init_pagination();
                    pagination.set_offset(0);
                    pagination.set_limit(16);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let interfaces = results.get_interfaces().expect("the interface list");
        let first = interfaces.get(0);
        (
            interfaces.len(),
            first.get_id().expect("the id").get_id(),
            text(first.get_name().expect("the name")),
            text(first.get_type().expect("the type")),
            first.get_up(),
            first.get_connected(),
            first.get_mt_u(),
        )
    });
    assert_eq!(answered.0, 1);
    assert_eq!(answered.1, 7);
    assert_eq!(answered.2, "mock0");
    assert_eq!(answered.3, "ethernet");
    assert!(answered.4);
    assert!(answered.5);
    assert_eq!(answered.6, 1500);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_counters_and_status_methods_answer_the_contract() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let answered = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = network.get_counters_request();
                {
                    let mut params = request.get();
                    let mut interface = params.reborrow().init_interface_id();
                    interface.set_id(7);
                    let mut window = params.init_window();
                    window.set_nanoseconds(1_000);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let counters = response.get().expect("the results").get_counters().expect("counters");
        (
            counters.get_bytes_in(),
            counters.get_bytes_out(),
            counters.get_sampled_at().expect("the timestamp").get_nanoseconds(),
        )
    });
    assert_eq!(answered.0, 42);
    assert_eq!(answered.1, 43);
    assert_eq!(answered.2, 99);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_service_answers_over_the_tcp_transport_too() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let listener = smart_lib::transport::tcp::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    let client_address = address.clone();
    std::thread::spawn(move || {
        let channel = smart_lib::transport::tcp::TcpChannel::connect(&client_address).expect("connect");
        let stream = Box::new(channel)
        .into_byte_stream()
        .expect("byte stream");
        let mut session = ClientSession::connect(stream).expect("session");
        let network: net::Client = session.interface();
        let outcome = session.run(async move {
            let mut request = network.get_status_request();
            {
                let params = request.get();
                let mut interface = params.init_interface_id();
                interface.set_id(7);
            }
            request.send().promise.await
        });
        let connected = outcome
            .expect("the call answers")
            .get()
            .expect("the results")
            .get_status()
            .expect("the status")
            .get_connected();
        let _ = sender.send(connected);
    });
    let channel = listener.accept().expect("accept");
    let _ = serve_connection(service, channel, CapabilityClass::Control);
    let connected = receiver.recv().expect("the client's answer");
    assert!(connected);
}

#[test]
fn the_subscription_is_the_contract_own_capability() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let connected = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = network.subscribe_status_request();
                {
                    let params = request.get();
                    let mut interface = params.init_interface_id();
                    interface.set_id(7);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let subscription = response
            .get()
            .expect("the results")
            .get_subscription()
            .expect("a subscription capability");

        // The subscription is a capability the client holds: it can pull
        // through it, with no engine type involved.
        let pulled = session
            .run(async move {
                let request = subscription.pull_request();
                request.send().promise.await
            })
            .expect("the pull answers");
        let status = pulled.get().expect("the results").get_status().expect("status");
        status.get_connected()
    });
    assert!(connected);

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Discovery
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn discovery_publishes_the_service_and_a_client_can_look_it_up() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    // A lookup by the domain's name finds the descriptor the service
    // registered.
    let found = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.lookup_request();
                {
                    let params = request.get();
                    let mut filter = params.init_filter();
                    filter.set_name("network");
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let services = results.get_services().expect("the service list");
        let descriptor = services.get(0);
        let name = text(descriptor.get_name().expect("the name"));
        let category = text(descriptor.get_category().expect("the category"));
        let id = descriptor.get_id().expect("the id").get_id();
        let endpoints = descriptor.get_endpoints().expect("the endpoints");
        let count = endpoints.len();
        let transport = text(endpoints.get(0).get_transport().expect("the transport"));
        (name, category, id, count, transport)
    });
    assert_eq!(found.0, "network");
    assert_eq!(found.1, "network");
    assert_eq!(
        found.2,
        smart_lib::domains::network::availability::NETWORK_SCHEMA_ID
    );
    // The endpoint carries the open transport token, not a closed enum.
    assert_eq!(found.3, 1);
    assert_eq!(found.4, "unix");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn discovery_resolves_a_name_without_failing_when_it_is_unknown() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let present = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.resolve_request();
                {
                    let mut params = request.get();
                    params.set_name("no-such-service");
                }
                request.send().promise.await
            })
            .expect("the call answers");
        response.get().expect("the results").get_present()
    });
    assert!(!present);

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The ABI handshake
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_abi_handshake_answers_the_schema_intersection() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let answered = run_client(service, &path, CapabilityClass::Control, |session| {
        let abi_client: abi_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = abi_client.handshake_request();
                {
                    let params = request.get();
                    let request = params.init_request();
                    let mut caller = request.init_caller_abi();
                    caller.set_namespace("smart-protocol");
                    let mut version = caller.init_version();
                    version.set_major(0);
                    version.set_minor(1);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let handshake = response
            .get()
            .expect("the results")
            .get_response()
            .expect("the response");
        let matched = matched_arm(handshake);
        let schemas = matched.get_schema_versions().expect("the schemas");
        (
            text(matched.get_namespace().expect("the namespace")),
            schemas.len(),
            schemas.get(0).get_id(),
        )
    });
    assert_eq!(answered.0, "smart-protocol");
    assert_eq!(answered.1, 1);
    assert_eq!(
        answered.2,
        smart_lib::domains::network::availability::NETWORK_SCHEMA_ID
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_abi_handshake_refuses_a_foreign_contract_family_honestly() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let code = run_client(service, &path, CapabilityClass::Control, |session| {
        let abi_client: abi_service::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = abi_client.handshake_request();
                {
                    let params = request.get();
                    let request = params.init_request();
                    let mut caller = request.init_caller_abi();
                    caller.set_namespace("some-other-family");
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let handshake = response
            .get()
            .expect("the results")
            .get_response()
            .expect("the response");
        let incompatible = incompatible_arm(handshake);
        incompatible.get_code().expect("the code")
    });
    assert_eq!(
        code,
        smart_lib::schema::types_capnp::ErrorCode::ProtocolViolation
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The capability publication
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_capability_service_publishes_the_class_the_peer_earned() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let published = run_client(service, &path, CapabilityClass::ReadOnly, |session| {
        let capability_client: capability_service::Client = session.interface();
        let response = session
            .run(async move {
                let request = capability_client.snapshot_request();
                request.send().promise.await
            })
            .expect("the call answers");
        let capabilities = response
            .get()
            .expect("the results")
            .get_capabilities()
            .expect("the capabilities");
        let entry = capabilities.get(0);
        let operations = entry.get_operations().expect("the operations");
        (
            text(entry.get_target().expect("the target")),
            operations.len(),
            operations.get(0).expect("the first operation"),
            operations.get(1).expect("the second operation"),
        )
    });
    assert_eq!(published.0, "service");
    // The class is published, not hidden: a read-only peer's capability
    // grants the read operation and not the write one.
    assert_eq!(published.1, 2);
    assert_eq!(
        published.2,
        smart_lib::schema::capability_capnp::Operation::Read
    );
    assert_eq!(
        published.3,
        smart_lib::schema::capability_capnp::Operation::Subscribe
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The authority gate and the availability matrix
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn a_read_only_connection_never_reaches_a_control_verb() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let refusal = run_client(service, &path, CapabilityClass::ReadOnly, |session| {
        let network: net::Client = session.interface();
        let outcome = session.run(async move {
            let mut request = network.set_networking_request();
            {
                let mut params = request.get();
                params.set_enabled(true);
            }
            request.send().promise.await
        });
        match outcome {
            Ok(_) => (false, String::new()),
            Err(error) => (true, error.extra),
        }
    });
    // The refusal rides the contract's own error channel, with the contract's
    // own code token.
    assert!(refusal.0, "a read-only connection must be refused");
    assert!(
        refusal.1.contains("permissionDenied:"),
        "the refusal must carry the contract's code token: {}",
        refusal.1
    );
    // And the backend was never reached: the gate is before the backend.
    assert!(
        !backend.reached().iter().any(|verb| verb == "setNetworking"),
        "the backend must not be reached: {:?}",
        backend.reached()
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_control_connection_reaches_a_control_verb() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let answered = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let outcome = session.run(async move {
            let mut request = network.set_networking_request();
            {
                let mut params = request.get();
                params.set_enabled(true);
            }
            request.send().promise.await
        });
        outcome.is_ok()
    });
    assert!(answered, "a control connection may reach the verb");
    assert!(backend.reached().iter().any(|verb| verb == "setNetworking"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_unavailable_method_answers_the_matrix_reason() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let refusal = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let outcome = session.run(async move {
            // `getDnsServers` is UNAVAILABLE in the matrix: no-backend.
            let request = network.get_dns_servers_request();
            request.send().promise.await
        });
        match outcome {
            Ok(_) => (false, String::new()),
            Err(error) => (true, error.extra),
        }
    });
    assert!(refusal.0, "an unavailable method must not answer");
    assert!(
        refusal.1.contains("notSupported: no-backend:"),
        "the matrix's reasonClass and reason must ride the error: {}",
        refusal.1
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The wire: the bytes are the contract's own capnp messages
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_bytes_on_the_wire_are_capnp_messages() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    let (sender, receiver) = std::sync::mpsc::channel();
    let client_path = path.clone();
    std::thread::spawn(move || {
        // A raw socket, speaking nothing but capnp: the reader is capnp's
        // own deserializer and capnp-rpc's own message schema.
        use std::io::Write;
        let mut socket = std::os::unix::net::UnixStream::connect(&client_path).expect("connect");
        // The client's first message is the RPC protocol's own bootstrap
        // request, in capnp's own encoding.
        let mut request = capnp::message::Builder::new_default();
        {
            let message = request.init_root::<capnp_rpc::rpc_capnp::message::Builder<'_>>();
            message.init_bootstrap();
        }
        let mut bytes = Vec::new();
        capnp::serialize::write_message(&mut bytes, &request).expect("serialize");
        socket.write_all(&bytes).expect("write");
        // And the server's answer is a capnp message too. The read is
        // capnp's own deserializer over the socket: it reads exactly one
        // message, however the stream chops it up.
        let answer = capnp::serialize::read_message(
            &mut socket,
            capnp::message::ReaderOptions::new(),
        )
        .expect("the server's bytes are a capnp message");
        let _ = sender.send((bytes, answer));
    });
    let channel = listener.accept().expect("accept");
    let _ = serve_connection(service, channel, CapabilityClass::Control);
    let (bytes, answer) = receiver.recv().expect("the wire bytes");

    // The bytes this client wrote are a capnp message, and the message is
    // the RPC protocol's own `bootstrap` request.
    let mut cursor = &bytes[..];
    let message = capnp::serialize::read_message(
        &mut cursor,
        capnp::message::ReaderOptions::new(),
    )
    .expect("the wire bytes are a capnp message");
    let rpc = message
        .get_root::<capnp_rpc::rpc_capnp::message::Reader<'_>>()
        .expect("the message is an rpc-capnp message");
    assert!(
        rpc.has_bootstrap(),
        "the first message on the wire is the rpc bootstrap"
    );

    // And the server's answer is a capnp message as well.
    let rpc = answer
        .get_root::<capnp_rpc::rpc_capnp::message::Reader<'_>>()
        .expect("the message is an rpc-capnp message");
    assert!(
        rpc.has_return(),
        "the server's answer is the rpc return"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_contract_only_client_needs_no_engine_type() {
    // The proof that the service is a contract consumer's peer: the client
    // side of this test names *only* the contract's generated code.
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let answered = run_client(service, &path, CapabilityClass::Control, |session| {
        let network: net::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = network.list_radio_states_request();
                {
                    let params = request.get();
                    let mut pagination = params.init_pagination();
                    pagination.set_offset(0);
                    pagination.set_limit(8);
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let results = response.get().expect("the results");
        let radios = results.get_radios().expect("the radios");
        (
            radios.len(),
            text(radios.get(0).get_name().expect("the radio")),
            results.get_total(),
        )
    });
    assert_eq!(answered.0, 1);
    assert_eq!(answered.1, "wifi");
    assert_eq!(answered.2, 1);

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// The capability class policy
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn an_unidentified_peer_is_served_read_only() {
    // No peer identity means no authority claim.
    let class = smart_lib::serve::classify(None);
    assert_eq!(class, CapabilityClass::ReadOnly);
    assert!(!class.is_control());
}

#[test]
fn the_root_peer_is_served_at_the_control_class() {
    let peer = smart_lib::transport::channel::PeerCredentials {
        pid: 1,
        uid: 0,
        gid: 0,
    };
    assert_eq!(smart_lib::serve::classify(Some(peer)), CapabilityClass::Control);
}

// ═══════════════════════════════════════════════════════════════════════
// The transports that carry no byte stream, and the in-process path
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn a_transport_without_a_byte_stream_says_so_honestly() {
    // The in-process transport dispatches messages; it does not move bytes.
    let (client, server) = smart_lib::transport::inprocess::InProcessChannel::pair();
    let error = match Box::new(client).into_byte_stream() {
        Ok(_) => panic!("the in-process transport has no byte stream"),
        Err(error) => error,
    };
    match error {
        TransportError::UnsupportedCapability { kind, detail } => {
            assert_eq!(kind, "byte stream");
            assert!(
                detail.contains("capnp RPC"),
                "the reason must name the path that does work: {detail}"
            );
        }
        other => panic!("expected an honest refusal, got {other:?}"),
    }
    let _ = server;
}

#[test]
fn the_contract_answers_in_process_without_a_wire_at_all() {
    // The in-process path of the contract's own call model: the generated
    // server, called through capnp's own local client. No socket, no
    // framing, no bytes.
    let backend = Arc::new(MockBackend::default());
    let server = smart_lib::serve::network::NetworkService::new(backend, CapabilityClass::Control);
    let network: net::Client = capnp_rpc::new_client(server);
    let response = futures::executor::block_on(async move {
        let mut request = network.get_interfaces_request();
        {
            let params = request.get();
            let mut pagination = params.init_pagination();
            pagination.set_offset(0);
            pagination.set_limit(4);
        }
        request.send().promise.await
    })
    .expect("the call answers");
    let results = response.get().expect("the results");
    let interfaces = results.get_interfaces().expect("the interfaces");
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces.get(0).get_id().expect("the id").get_id(), 7);
}

#[test]
fn the_capability_mask_is_read_from_the_kernels_own_line() {
    // `CapEff` is the kernel's own line; the parse is total or it is
    // `None`, never a guess.
    let status = "Name:\tdaemon\nCapEff:\t0000000000001000\nCapBnd:\t0000000000001000\n";
    let mask = smart_lib::serve::cap_mask_from_status(status).expect("the mask");
    assert_eq!(mask, 0x1000);
    // Bit 12 is CAP_NET_ADMIN: the one the control verbs need.
    assert_eq!(mask & (1u64 << 12), 1u64 << 12);
    assert!(smart_lib::serve::cap_mask_from_status("Name:\tdaemon\n").is_none());
    assert!(smart_lib::serve::cap_mask_from_status("CapEff:\tnothex\n").is_none());
}

// ═══════════════════════════════════════════════════════════════════════
// The rest of the foundation surfaces
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn the_runtime_health_of_a_registered_service_is_answered() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let health = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.get_runtime_health_request();
                {
                    let params = request.get();
                    let mut service_id = params.init_service_id();
                    service_id.set_id(
                        smart_lib::domains::network::availability::NETWORK_SCHEMA_ID,
                    );
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let health = response
            .get()
            .expect("the results")
            .get_health()
            .expect("the health");
        (
            health.get_state().expect("the state"),
            health.get_last_check_passed(),
            health.get_fault_count(),
            health.get_since_valid(),
        )
    });
    assert_eq!(
        health.0,
        smart_lib::schema::discovery_capnp::ServiceState::Online
    );
    assert!(health.1, "the last check passed: the service is serving");
    assert_eq!(health.2, 0);
    // No transition has been observed: `sinceValid` false, never a
    // fabricated "it changed zero microseconds ago".
    assert!(!health.3);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_registry_refuses_a_field_it_does_not_model() {
    let backend = Arc::new(MockBackend::default());
    let service = build_service(Arc::clone(&backend));
    let path = socket_path();

    let refusal = run_client(service, &path, CapabilityClass::Control, |session| {
        let discovery_client: discovery::Client = session.interface();
        let response = session
            .run(async move {
                let mut request = discovery_client.registry_request();
                {
                    let params = request.get();
                    // The authority capability: the contract's own.
                    let _ = params.init_authority();
                }
                request.send().promise.await
            })
            .expect("the call answers");
        let registry = response
            .get()
            .expect("the results")
            .get_service()
            .expect("the registry capability");
        // The registry is the gated surface, and it is a capability the
        // caller holds.
        let outcome = session.run(async move {
            let mut request = registry.deregister_request();
            {
                let params = request.get();
                let mut service_id = params.init_service_id();
                service_id.set_id(0xdead_beef);
            }
            request.send().promise.await
        });
        match outcome {
            Ok(response) => {
                let result = response.get().expect("the results");
                match result.which().expect("the void-result arm") {
                    smart_lib::schema::types_capnp::void_result::Which::Error(error) => {
                        let error = error.expect("the error");
                        (
                            true,
                            String::from_utf8_lossy(
                                error.get_message().expect("the message").as_bytes(),
                            )
                            .into_owned(),
                        )
                    }
                    _ => (false, String::new()),
                }
            }
            Err(error) => (true, error.extra),
        }
    });
    // Deregistering an unknown service is a notFound, not a crash and not
    // a silent success.
    assert!(refusal.0, "the registry must answer");
    assert!(
        refusal.1.contains("notFound"),
        "the answer must be the contract's own code: {}",
        refusal.1
    );

    let _ = std::fs::remove_file(&path);
}
