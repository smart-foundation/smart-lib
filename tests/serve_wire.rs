//! The serve harness's over-the-wire tests — the integration target that
//! drives the harness through its **public** library API
//! (`smart_lib::serve`), the same path Wave 3's CLI links.
//!
//! Until `build.rs` wired the `serve` wave module, this file carried a
//! `#[path]` shim that compiled `src/serve/**` into this target directly
//! (the module was not part of the crate). With the wiring in place the
//! harness is a normal library module and these tests are ordinary
//! integration tests: every symbol they touch is public, so what they prove
//! is exactly what a consumer gets.
//!
//! Every test is hermetic and mock-based: the mock records every call (which
//! is what makes the capability gate provable — a read-only connection's
//! mutating verb must never reach the backend), the transports are the real
//! ones from `src/transport/**`, and no host state is touched beyond
//! `/proc/self/status` reads (the capability policy's own input) and a
//! temp-dir socket (law #8).

#![cfg(feature = "domain-network")]

extern crate std;
// The domain types this file builds (`Page`, `Pagination`, ...) and the
// mock's scripted data are `alloc`-spelled, matching the library's own
// `no_std` + `alloc` vocabulary; this target is a std crate, so the `alloc`
// crate is named explicitly.
extern crate alloc;

use smart_lib::serve;
use smart_lib::serve::{classify, frame, serve as serve_backend, CapabilityClass};

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use smart_lib::core::ids::NetworkInterfaceId;
use smart_lib::core::values::Duration;
use smart_lib::core::{Error, ErrorCode};
use smart_lib::domains::network::availability::{AvailStatus, NETWORK_SCHEMA_ID};
use smart_lib::domains::network::backend::NetworkBackend;
use smart_lib::domains::network::types::{
    ConnStatus, HostState, HostStateSource, InterfaceCounters, InterfaceSnapshot,
    KnownWifiNetwork, Page, Pagination, ProxyConfigInfo, RadioState, RouteInfo, SubscriptionId,
};
/// The `Network` interface's generated method modules: `<method>_params`
/// and `<method>_results`, one pair per contract method.
use smart_lib::schema::network_capnp::network as net;
use smart_lib::schema::{abi_capnp, types_capnp};
use capnp::message::ReaderSegments;

use smart_lib::transport::channel::{
    connect, listen, ChannelListener, PeerCredentials, Transport,
};
use smart_lib::transport::inprocess::InProcessChannel;
use smart_lib::transport::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// The mock backend: scripted answers, every call recorded
// ═══════════════════════════════════════════════════════════════════════

/// A scripted network backend whose call log is the capability gate's
/// witness.
struct Mock {
    /// Every method call, in order (the gate asserts this stays empty).
    calls: Mutex<Vec<String>>,
    interfaces: Vec<InterfaceSnapshot>,
    status: ConnStatus,
    /// `Some` ⇒ `get_status` fails with this error (error totality).
    status_error: Option<Error>,
    counters: InterfaceCounters,
    radios: Vec<RadioState>,
    known: Vec<KnownWifiNetwork>,
    routes: Vec<RouteInfo>,
    proxy: ProxyConfigInfo,
    networking_enabled: bool,
    mobile_data_enabled: bool,
    roaming_enabled: bool,
    /// Scripted status pulls, oldest first. An `Err(Temporary)` entry is the
    /// backend's "nothing yet"; an empty queue answers the stable status.
    status_pulls: Mutex<VecDeque<smart_lib::core::Result<ConnStatus>>>,
    counter_pulls: Mutex<VecDeque<smart_lib::core::Result<InterfaceCounters>>>,
    next_subscription: core::sync::atomic::AtomicU64,
    /// Minted and not yet cancelled.
    live: Mutex<Vec<u64>>,
}

impl Mock {
    fn new() -> Self {
        Mock {
            calls: Mutex::new(Vec::new()),
            interfaces: alloc::vec![
                InterfaceSnapshot {
                    id: NetworkInterfaceId::new(1),
                    name: "lo".to_string(),
                    kind: "loopback".to_string(),
                    hardware_address: Default::default(),
                    up: true,
                    connected: true,
                    mtu: 65536,
                    managed: true,
                },
                InterfaceSnapshot {
                    id: NetworkInterfaceId::new(2),
                    name: "enp0s31f6".to_string(),
                    kind: "ethernet".to_string(),
                    hardware_address: Default::default(),
                    up: true,
                    connected: false,
                    mtu: 1500,
                    managed: true,
                },
            ],
            status: ConnStatus {
                connected: true,
                interface: NetworkInterfaceId::new(1),
                kind: "loopback".to_string(),
                strength_dbm: 0,
                speed_bps: 0,
                quality: 100,
                metered: false,
                behind_captive: false,
                host_state: HostState::Managed,
                host_state_source: HostStateSource::of(HostState::Managed),
                ssid: String::new(),
            },
            status_error: None,
            counters: InterfaceCounters {
                bytes_in: 1_024,
                bytes_out: 2_048,
                packets_in: 12,
                packets_out: 24,
                errors_in: 0,
                errors_out: 1,
                drops_in: 0,
                drops_out: 0,
                sampled_at: 1_700_000_000_000_000_000,
            },
            radios: alloc::vec![RadioState {
                name: "wifi".to_string(),
                enabled: true,
                hard_blocked: false,
                soft_blocked: false,
            }],
            known: alloc::vec![KnownWifiNetwork {
                ssid: "hem".to_string(),
                auto_connect: true,
                last_connected: 1_700_000_000_000_000_000,
                security: "wpa3".to_string(),
            }],
            routes: alloc::vec![RouteInfo {
                destination_cidr: "0.0.0.0/0".to_string(),
                gateway: "192.168.18.1".to_string(),
                metric: 600,
                interface_id: NetworkInterfaceId::new(2),
            }],
            proxy: ProxyConfigInfo {
                enabled: true,
                url: "http://proxy.corp:3128".to_string(),
                bypass: alloc::vec![("localhost".to_string(), String::new())],
            },
            networking_enabled: true,
            mobile_data_enabled: false,
            roaming_enabled: false,
            status_pulls: Mutex::new(VecDeque::new()),
            counter_pulls: Mutex::new(VecDeque::new()),
            next_subscription: core::sync::atomic::AtomicU64::new(1),
            live: Mutex::new(Vec::new()),
        }
    }

    /// The recorded calls, in order.
    fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }

    /// Fail `get_status` with this error (the error-totality seam).
    fn failing_get_status(mut self, error: Error) -> Self {
        self.status_error = Some(error);
        self
    }

    fn record(&self, call: String) {
        self.calls.lock().push(call);
    }
}

impl NetworkBackend for Mock {
    fn get_interfaces(&self, page: Pagination) -> smart_lib::core::Result<Page<InterfaceSnapshot>> {
        self.record(format!("getInterfaces({}, {})", page.offset, page.limit));
        let items: Vec<InterfaceSnapshot> = self
            .interfaces
            .iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .cloned()
            .collect();
        Ok(Page {
            total: self.interfaces.len() as u64,
            items,
        })
    }

    fn get_status(&self, interface: NetworkInterfaceId) -> smart_lib::core::Result<ConnStatus> {
        self.record(format!("getStatus({:?})", interface.get()));
        if let Some(error) = &self.status_error {
            return Err(error.clone());
        }
        Ok(self.status.clone())
    }

    fn subscribe_status(&self, interface: NetworkInterfaceId) -> smart_lib::core::Result<SubscriptionId> {
        self.record(format!("subscribeStatus({:?})", interface.get()));
        let id = SubscriptionId(
            self.next_subscription
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        );
        self.live.lock().push(id.0);
        Ok(id)
    }

    fn pull_status(&self, subscription: SubscriptionId) -> smart_lib::core::Result<ConnStatus> {
        self.record(format!("pullStatus({})", subscription.0));
        match self.status_pulls.lock().pop_front() {
            Some(event) => event,
            None => Ok(self.status.clone()),
        }
    }

    fn cancel_subscription(&self, subscription: SubscriptionId) -> smart_lib::core::Result<()> {
        self.record(format!("cancelSubscription({})", subscription.0));
        let mut live = self.live.lock();
        match live.iter().position(|id| *id == subscription.0) {
            Some(at) => {
                live.remove(at);
                Ok(())
            }
            None => Err(Error::not_found().with_context("subscription not live or already cancelled")),
        }
    }

    fn get_counters(
        &self,
        _interface: NetworkInterfaceId,
        _window: Duration,
    ) -> smart_lib::core::Result<InterfaceCounters> {
        self.record(format!(
            "getCounters({:?}, {}ns)",
            _interface.get(),
            _window.nanoseconds
        ));
        Ok(self.counters)
    }

    fn subscribe_counters(
        &self,
        _interface: NetworkInterfaceId,
    ) -> smart_lib::core::Result<SubscriptionId> {
        self.record(format!("subscribeCounters({:?})", _interface.get()));
        let id = SubscriptionId(
            self.next_subscription
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        );
        self.live.lock().push(id.0);
        Ok(id)
    }

    fn pull_counters(&self, subscription: SubscriptionId) -> smart_lib::core::Result<InterfaceCounters> {
        self.record(format!("pullCounters({})", subscription.0));
        match self.counter_pulls.lock().pop_front() {
            Some(event) => event,
            None => Ok(self.counters),
        }
    }

    fn is_networking_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isNetworkingEnabled".to_string());
        Ok(self.networking_enabled)
    }

    fn set_networking_enabled(&self, enabled: bool) -> smart_lib::core::Result<()> {
        self.record(format!("setNetworking({enabled})"));
        Ok(())
    }

    fn set_radio_enabled(&self, radio: &str, enabled: bool) -> smart_lib::core::Result<()> {
        self.record(format!("setRadioEnabled({radio}, {enabled})"));
        Ok(())
    }

    fn list_radio_states(&self, _page: Pagination) -> smart_lib::core::Result<Page<RadioState>> {
        self.record("listRadioStates".to_string());
        Ok(Page {
            total: self.radios.len() as u64,
            items: self.radios.clone(),
        })
    }

    fn is_mobile_data_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isMobileDataEnabled".to_string());
        Ok(self.mobile_data_enabled)
    }

    fn set_mobile_data_enabled(&self, enabled: bool) -> smart_lib::core::Result<()> {
        self.record(format!("setMobileData({enabled})"));
        Ok(())
    }

    fn is_data_roaming_enabled(&self) -> smart_lib::core::Result<bool> {
        self.record("isDataRoamingEnabled".to_string());
        Ok(self.roaming_enabled)
    }

    fn set_data_roaming_enabled(&self, _enabled: bool) -> smart_lib::core::Result<()> {
        self.record(format!("setDataRoaming({_enabled})"));
        Ok(())
    }

    fn set_interface_mtu(&self, interface: NetworkInterfaceId, mtu: u32) -> smart_lib::core::Result<()> {
        self.record(format!("setInterfaceMtu({:?}, {mtu})", interface.get()));
        Ok(())
    }

    fn forget_wifi_network(&self, ssid: &str) -> smart_lib::core::Result<()> {
        self.record(format!("forgetWifiNetwork({ssid})"));
        Ok(())
    }

    fn list_known_wifi_networks(&self, _page: Pagination) -> smart_lib::core::Result<Page<KnownWifiNetwork>> {
        self.record("listKnownWifiNetworks".to_string());
        Ok(Page {
            total: self.known.len() as u64,
            items: self.known.clone(),
        })
    }

    fn get_routes(&self, _page: Pagination) -> smart_lib::core::Result<Page<RouteInfo>> {
        self.record("getRoutes".to_string());
        Ok(Page {
            total: self.routes.len() as u64,
            items: self.routes.clone(),
        })
    }

    fn get_proxy_config(&self) -> smart_lib::core::Result<ProxyConfigInfo> {
        self.record("getProxyConfig".to_string());
        Ok(self.proxy.clone())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The test client: one served connection, driven synchronously
// ═══════════════════════════════════════════════════════════════════════

/// A served connection's client side: the ABI handshake plus request/response
/// calls, over whichever channel the test built.
struct Client {
    channel: Box<dyn smart_lib::transport::channel::Channel>,
    scratch: Vec<u8>,
}

impl Client {
    /// Connect and run the ABI handshake, asserting the service matched.
    fn connect(channel: Box<dyn smart_lib::transport::channel::Channel>) -> Self {
        let mut client = Client {
            channel,
            scratch: Vec::new(),
        };
        client
            .send(handshake_message(Some(ABI_NAMESPACE)))
            .expect("handshake request sends");
        let batch = client.recv_batch().expect("handshake response arrives");
        let message = batch
            .message
            .root::<abi_capnp::abi_handshake_response::Owned>()
            .expect("the handshake response parses");
        match message.which().expect("a handshake answer") {
            abi_capnp::abi_handshake_response::WhichReader::Matched(Ok(abi)) => {
                assert_eq!(abi.get_namespace().expect("namespace"), ABI_NAMESPACE);
                let schemas = abi.get_schema_versions().expect("schema versions");
                assert_eq!(schemas.len(), 1, "the served schema is named");
                assert_eq!(
                    schemas.get(0).get_id(),
                    NETWORK_SCHEMA_ID,
                    "the handshake names the network contract it reached"
                );
            }
            _ => panic!("expected a matched handshake"),
        }
        client
    }

    /// Connect with a declared ABI namespace and summarize the handshake
    /// answer: the arm the service chose plus its detail text (the
    /// matched/incompatible witness). The channel is closed on the way out,
    /// which ends the served session.
    fn handshake_summary(
        channel: Box<dyn smart_lib::transport::channel::Channel>,
        namespace: Option<&str>,
    ) -> (String, String) {
        let mut client = Client {
            channel,
            scratch: Vec::new(),
        };
        client
            .send(handshake_message(namespace))
            .expect("handshake request sends");
        let batch = client.recv_batch().expect("handshake response arrives");
        let response = batch
            .message
            .root::<abi_capnp::abi_handshake_response::Owned>()
            .expect("the handshake response parses");
        let summary = match response.which().expect("a handshake answer") {
            abi_capnp::abi_handshake_response::WhichReader::Matched(Ok(abi)) => (
                "matched".to_string(),
                abi.get_namespace()
                    .expect("namespace")
                    .to_str()
                    .unwrap_or("")
                    .to_string(),
            ),
            abi_capnp::abi_handshake_response::WhichReader::Incompatible(Ok(error)) => (
                "incompatible".to_string(),
                error
                    .get_message()
                    .map(|m| m.to_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            ),
            _ => ("unexpected-arm".to_string(), String::new()),
        };
        client.close();
        summary
    }

    fn send(
        &mut self,
        message: capnp::message::Builder<capnp::message::HeapAllocator>,
    ) -> Result<(), smart_lib::transport::channel::TransportError> {
        self.channel
            .send(smart_lib::transport::channel::MessageOut::Owned(
                smart_lib::transport::channel::OwnedMessage::new(message),
            ))
    }

    /// Send one serve frame and read its response, waiting on a
    /// non-blocking transport rather than treating "nothing yet" as an
    /// answer.
    fn call(&mut self, ordinal: u16, params: &[u8]) -> frame::Response {
        self.call_frame(&frame::encode_request(NETWORK_SCHEMA_ID, ordinal, params, &[]))
    }

    /// A serve frame that names a foreign interface (the unknown-interface
    /// path).
    fn call_foreign_interface(&mut self, ordinal: u16, params: &[u8]) -> frame::Response {
        self.call_frame(&frame::encode_request(0xdead_beef, ordinal, params, &[]))
    }

    /// A raw frame (for the malformed-frame paths).
    fn call_raw(&mut self, bytes: &[u8]) -> frame::Response {
        self.call_frame(bytes)
    }

    fn call_frame(&mut self, frame_bytes: &[u8]) -> frame::Response {
        self.channel
            .send(smart_lib::transport::channel::MessageOut::Borrowed(&[&frame_bytes]))
            .expect("request sends");
        let batch = self.recv_batch().expect("response arrives");
        let bytes = envelope(&batch);
        let view = frame::parse_response(bytes).expect("the response frame parses");
        frame::Response {
            status: view.status,
            message: view.message_text(),
            payload: view.payload.to_vec(),
        }
    }

    fn recv_batch(&mut self) -> Option<smart_lib::transport::channel::RecvBatch> {
        // A bounded wait: a serve loop that never answers must fail the test
        // with a name, not hang it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match self.channel.recv(&mut self.scratch) {
                Ok(batch) => return Some(batch),
                Err(smart_lib::transport::channel::TransportError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the served connection never answered"
                    );
                    std::thread::yield_now();
                }
                Err(_) => return None,
            }
        }
    }

    fn close(self) {
        let _ = self.channel.close(serve::orderly_close());
    }
}

/// Where the received frame's bytes live (the segment the channel parsed).
fn envelope(batch: &smart_lib::transport::channel::RecvBatch) -> &[u8] {
    use smart_lib::transport::channel::RecvMessage;
    match &batch.message {
        RecvMessage::Direct(reader) => reader.get_segments().get_segment(0).unwrap_or(&[]),
        RecvMessage::Owned(reader) => reader.get_segments().get_segment(0).unwrap_or(&[]),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Params builders — the contract's own param structs, encoded by the client
// ═══════════════════════════════════════════════════════════════════════

/// The ABI namespace this service speaks (mirrors `serve`'s declaration).
const ABI_NAMESPACE: &str = "smart-protocol";

/// Serialize one filled params message. Every caller names the method's own
/// params struct, so the wire the test client writes is the contract's own
/// bytes — the same bytes the service decodes.
fn serialize(message: capnp::message::Builder<capnp::message::HeapAllocator>) -> Vec<u8> {
    let mut out = Vec::new();
    capnp::serialize::write_message(&mut out, &message).expect("serialize params");
    out
}

/// The `AbiHandshakeRequest` the client sends first.
fn handshake_message(namespace: Option<&str>) -> capnp::message::Builder<capnp::message::HeapAllocator> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut request = message.init_root::<abi_capnp::abi_handshake_request::Builder<'_>>();
        let mut abi = request.reborrow().init_caller_abi();
        if let Some(namespace) = namespace {
            abi.set_namespace(namespace);
        }
        let mut version = abi.reborrow().init_version();
        version.set_major(0);
        version.set_minor(1);
        version.set_patch(0);
        request.set_accept_downgrade(false);
        request.reborrow().init_session_id().set_id(1);
    }
    message
}

fn params_pagination(offset: u32, limit: u32) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::get_interfaces_params::Builder<'_>>();
        let mut page = params.reborrow().init_pagination();
        page.set_offset(offset);
        page.set_limit(limit);
    }
    serialize(message)
}

fn params_interface_id(id: u64) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::get_status_params::Builder<'_>>();
        let mut interface = params.reborrow().init_interface_id();
        interface.set_id(id);
    }
    serialize(message)
}

fn params_subscribe_interface(id: u64) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::subscribe_status_params::Builder<'_>>();
        let mut interface = params.reborrow().init_interface_id();
        interface.set_id(id);
    }
    serialize(message)
}

fn params_subscribe_counters_interface(id: u64) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::subscribe_counters_params::Builder<'_>>();
        let mut interface = params.reborrow().init_interface_id();
        interface.set_id(id);
    }
    serialize(message)
}

fn params_counters_window(id: u64, nanoseconds: u64) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::get_counters_params::Builder<'_>>();
        let mut interface = params.reborrow().init_interface_id();
        interface.set_id(id);
        let mut window = params.reborrow().init_window();
        window.set_nanoseconds(nanoseconds);
    }
    serialize(message)
}

fn params_enabled(enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::set_mobile_data_params::Builder<'_>>();
        params.set_enabled(enabled);
    }
    serialize(message)
}

fn params_networking(enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::set_networking_params::Builder<'_>>();
        params.set_enabled(enabled);
    }
    serialize(message)
}

fn params_radio(radio: &str, enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::set_radio_enabled_params::Builder<'_>>();
        params.set_radio(radio);
        params.set_enabled(enabled);
    }
    serialize(message)
}

fn params_mtu(id: u64, mtu: u32) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::set_interface_mtu_params::Builder<'_>>();
        let mut interface = params.reborrow().init_interface_id();
        interface.set_id(id);
        params.set_mtu(mtu);
    }
    serialize(message)
}

fn params_ssid(ssid: &str) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut params = message.init_root::<net::forget_wifi_network_params::Builder<'_>>();
        params.set_ssid(ssid);
    }
    serialize(message)
}

fn params_empty_is_mobile_data_enabled() -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let _params = message.init_root::<net::is_mobile_data_enabled_params::Builder<'_>>();
    }
    serialize(message)
}

fn params_empty_is_data_roaming_enabled() -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let _params = message.init_root::<net::is_data_roaming_enabled_params::Builder<'_>>();
    }
    serialize(message)
}

fn params_empty_is_networking_enabled() -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let _params = message.init_root::<net::is_networking_enabled_params::Builder<'_>>();
    }
    serialize(message)
}

fn params_empty_get_proxy_config() -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let _params = message.init_root::<net::get_proxy_config_params::Builder<'_>>();
    }
    serialize(message)
}


fn params_pull(sub_id: u64, limit: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&sub_id.to_le_bytes());
    out.extend_from_slice(&limit.to_le_bytes());
    out
}

fn params_cancel(sub_id: u64) -> Vec<u8> {
    sub_id.to_le_bytes().to_vec()
}

/// A frame whose declared params length does not match its body (the
/// malformed path), word-padded like every frame the transport carries.
fn raw_frame(interface_id: u64, ordinal: u16, declared_params_len: u16, flags: u32, body: &[u8]) -> Vec<u8> {
    let len = frame::REQUEST_HEADER_LEN + body.len();
    let mut out = Vec::with_capacity(frame::padded_len(len));
    out.extend_from_slice(&interface_id.to_le_bytes());
    out.extend_from_slice(&ordinal.to_le_bytes());
    out.extend_from_slice(&declared_params_len.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(body);
    out.resize(frame::padded_len(len), 0);
    out
}

// ═══════════════════════════════════════════════════════════════════════
// Result decoders — the contract's own result structs, read by the client
// ═══════════════════════════════════════════════════════════════════════

fn read_message(bytes: &[u8]) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
    capnp::serialize::read_message(&mut &bytes[..], capnp::message::ReaderOptions::new())
        .expect("the payload is a capnp message")
}

fn interfaces_of(payload: &[u8]) -> Vec<InterfaceSnapshot> {
    let message = read_message(payload);
    let results = message
        .get_root::<net::get_interfaces_results::Reader<'_>>()
        .expect("getInterfaces results");
    let list = results.get_interfaces().expect("interfaces list");
    let mut items = Vec::new();
    for item in list.iter() {
        items.push(InterfaceSnapshot {
            id: NetworkInterfaceId::new(item.get_id().map(|i| i.get_id()).unwrap_or(0)),
            name: item.get_name().map(|t| t.to_str().unwrap_or("").to_string()).unwrap_or_default(),
            kind: item.get_type().map(|t| t.to_str().unwrap_or("").to_string()).unwrap_or_default(),
            hardware_address: smart_lib::domains::network::types::MacAddress(
                item.get_hw_address()
                    .and_then(|hw| hw.get_address())
                    .map(|a| a.to_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            ),
            up: item.get_up(),
            connected: item.get_connected(),
            mtu: item.get_mt_u(),
            // The contract's NetworkInterface has no `managed` field, so the
            // domain's bit has no wire home here — it is not invented.
            managed: false,
        });
    }
    items
}

fn status_of(payload: &[u8]) -> ConnStatus {
    let message = read_message(payload);
    let results = message
        .get_root::<net::get_status_results::Reader<'_>>()
        .expect("getStatus results");
    ConnStatus::from_reader(&results.get_status().expect("status"))
}

fn counters_of(payload: &[u8]) -> InterfaceCounters {
    let message = read_message(payload);
    let results = message
        .get_root::<net::get_counters_results::Reader<'_>>()
        .expect("getCounters results");
    let counters = results.get_counters().expect("counters");
    InterfaceCounters {
        bytes_in: counters.get_bytes_in(),
        bytes_out: counters.get_bytes_out(),
        packets_in: counters.get_packets_in(),
        packets_out: counters.get_packets_out(),
        errors_in: counters.get_errors_in(),
        errors_out: counters.get_errors_out(),
        drops_in: counters.get_drops_in(),
        drops_out: counters.get_drops_out(),
        sampled_at: counters.get_sampled_at().map(|t| t.get_nanoseconds()).unwrap_or(0),
    }
}

fn radios_of(payload: &[u8]) -> (Vec<RadioState>, u64) {
    let message = read_message(payload);
    let results = message
        .get_root::<net::list_radio_states_results::Reader<'_>>()
        .expect("listRadioStates results");
    let list = results.get_radios().expect("radios list");
    let mut items = Vec::new();
    for item in list.iter() {
        items.push(RadioState {
            name: item.get_name().map(|t| t.to_str().unwrap_or("").to_string()).unwrap_or_default(),
            enabled: item.get_enabled(),
            hard_blocked: item.get_hard_blocked(),
            soft_blocked: item.get_soft_blocked(),
        });
    }
    (items, results.get_total())
}

fn known_of(payload: &[u8]) -> (Vec<KnownWifiNetwork>, u64) {
    let message = read_message(payload);
    let results = message
        .get_root::<net::list_known_wifi_networks_results::Reader<'_>>()
        .expect("listKnownWifiNetworks results");
    let list = results.get_networks().expect("networks list");
    let mut items = Vec::new();
    for item in list.iter() {
        items.push(KnownWifiNetwork::from_reader(&item));
    }
    (items, results.get_total())
}

fn routes_of(payload: &[u8]) -> (Vec<RouteInfo>, u64) {
    let message = read_message(payload);
    let results = message
        .get_root::<net::get_routes_results::Reader<'_>>()
        .expect("getRoutes results");
    let list = results.get_routes().expect("routes list");
    let mut items = Vec::new();
    for item in list.iter() {
        items.push(RouteInfo::from_reader(&item));
    }
    (items, results.get_total())
}

fn proxy_of(payload: &[u8]) -> ProxyConfigInfo {
    let message = read_message(payload);
    let results = message
        .get_root::<net::get_proxy_config_results::Reader<'_>>()
        .expect("getProxyConfig results");
    ProxyConfigInfo::from_reader(&results.get_config().expect("config"))
}

fn enabled_of(payload: &[u8]) -> bool {
    let message = read_message(payload);
    let results = message
        .get_root::<net::is_networking_enabled_results::Reader<'_>>()
        .expect("isNetworkingEnabled results");
    results.get_enabled()
}

fn error_of(payload: &[u8]) -> Error {
    let message = read_message(payload);
    let results = message
        .get_root::<types_capnp::error::Reader<'_>>()
        .expect("the error payload is the contract's T.Error");
    Error::from_reader(&results)
}

/// The success arm of a mutating verb's `T.VoidResult` payload.
fn void_success_of(payload: &[u8]) -> bool {
    let message = read_message(payload);
    let results = message
        .get_root::<types_capnp::void_result::Reader<'_>>()
        .expect("the payload is the contract's T.VoidResult");
    matches!(
        results.which().expect("a void result arm"),
        types_capnp::void_result::WhichReader::Success(())
    )
}

/// Assert a response succeeded and return its payload.
fn payload_of(response: &frame::Response) -> &[u8] {
    assert_eq!(
        response.status,
        frame::STATUS_OK,
        "expected ok, got error: {}",
        response.message
    );
    &response.payload
}

// ═══════════════════════════════════════════════════════════════════════
// The tests
// ═══════════════════════════════════════════════════════════════════════

/// Spawn the serve loop over one channel. The caller owns the client side;
/// the returned handle joins when the peer closes.
fn spawn_server(
    mock: &std::sync::Arc<Mock>,
    server_side: InProcessChannel,
    class: CapabilityClass,
) -> std::thread::JoinHandle<()> {
    let mock = mock.clone();
    std::thread::spawn(move || {
        serve_backend(&*mock, &server_side, class).expect("the served session ends cleanly");
    })
}

/// The pure frame codecs: a request round-trips field for field, a response
/// round-trips with its message and payload, the padding is word-aligned,
/// and every malformed-spelling is refused with a typed reason.
#[test]
fn frame_codecs_round_trip_and_refuse_malformed_spellings() {
    let params = b"params-bytes".to_vec();
    let wire = frame::encode_request(NETWORK_SCHEMA_ID, 1, &params, b"cap-bytes");
    assert_eq!(wire.len() % frame::WORD_BYTES, 0, "frames are word-padded");
    let request = frame::parse_request(&wire).expect("a well-formed frame parses");
    assert_eq!(request.interface_id, NETWORK_SCHEMA_ID);
    assert_eq!(request.ordinal, 1);
    assert_eq!(request.flags, 0);
    assert_eq!(request.params, &params[..]);
    // The trailing region after the declared params is the attached
    // capability bytes plus the frame's word padding (zero-filled, the
    // transport's alignment artifact — the transport's own attach path is
    // the length-authoritative capability mechanism).
    assert!(request.caps.starts_with(b"cap-bytes"), "{:?}", request.caps);
    assert!(request.caps[11..].iter().all(|byte| *byte == 0), "the padding is zero");

    // A reserved-flag bit this build does not know fails loudly.
    let bad_flags = raw_frame(NETWORK_SCHEMA_ID, 1, params.len() as u16, 1, &params);
    assert!(frame::parse_request(&bad_flags).is_err(), "flags must be 0");
    // A declared params length longer than the frame fails loudly.
    let long = raw_frame(NETWORK_SCHEMA_ID, 1, u16::MAX, 0, &params);
    assert!(frame::parse_request(&long).is_err(), "params length must fit");
    // A frame shorter than the header fails loudly.
    assert!(frame::parse_request(&wire[..8]).is_err());

    // The subscription pseudo-ordinal param spellings.
    let pull = frame::parse_pull_params(&params_pull(7, 9)).expect("pull params");
    assert_eq!(pull.sub_id, 7);
    assert_eq!(pull.limit, 9);
    assert_eq!(frame::parse_cancel_params(&params_cancel(7)).expect("cancel params"), 7);
    assert!(frame::parse_pull_params(&[0u8; 4]).is_err(), "a short pull is refused");

    // The response codec, including the subscription payload's sub-id.
    let response = frame::Response::error_with_payload("permissionDenied: nope", vec![1, 2, 3]);
    let encoded = response.encode();
    assert_eq!(encoded.len() % frame::WORD_BYTES, 0, "responses are word-padded");
    let view = frame::parse_response(&encoded).expect("a well-formed response parses");
    assert_eq!(view.status, frame::STATUS_ERROR);
    assert!(!view.is_ok());
    assert_eq!(view.message_text(), "permissionDenied: nope");
    assert_eq!(view.payload, &[1, 2, 3]);

    // An ok response with an empty payload (a subscription pull with nothing
    // yet) round-trips as ok-and-empty.
    let encoded = frame::Response::ok(Vec::new()).encode();
    let view = frame::parse_response(&encoded).expect("a well-formed response parses");
    assert!(view.is_ok());
    assert!(view.payload.is_empty());
    assert!(view.message.is_empty());
    let payload = frame::payload_with_sub_id(42, b"event");
    assert_eq!(frame::sub_id_of(&payload), Some(42));
}

/// Every servable method, round-tripped over an InProcess channel pair
/// against the mock backend, asserting the payload decodes to the expected
/// contract result. This is the direct-call path (05 §1) with the transport
/// as the only difference.
#[test]
fn every_servable_read_round_trips_over_an_in_process_channel() {
    let mock = std::sync::Arc::new(Mock::new());
    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    // @0 getInterfaces — the kernel link table.
    // The contract's pagination law, repeated here: limit 0 means "one", so
    // the two-interface page is asked for with a real bound.
    let response = client.call(0, &params_pagination(0, 10));
    let items = interfaces_of(payload_of(&response));
    assert_eq!(items.len(), 2, "the contract's getInterfaces result carries no total");
    assert_eq!(items[0].name, "lo");
    assert_eq!(items[0].kind, "loopback");
    assert_eq!(items[0].mtu, 65536);
    assert!(items[0].up && items[0].connected);
    assert_eq!(items[1].name, "enp0s31f6");
    assert_eq!(items[1].kind, "ethernet");
    assert_eq!(items[1].mtu, 1500);
    assert!(items[1].up && !items[1].connected);

    // @1 getStatus — hostState travels with the status.
    let response = client.call(1, &params_interface_id(1));
    let status = status_of(payload_of(&response));
    assert_eq!(status.interface.get(), 1);
    assert_eq!(status.host_state, HostState::Managed);
    assert_eq!(status.host_state.as_token(), "managed");
    assert_eq!(status.kind, "loopback");

    // @12 / @14 / @73 — the boolean reads.
    let response = client.call(12, &params_empty_is_mobile_data_enabled());
    assert!(!enabled_of(payload_of(&response)));
    let response = client.call(14, &params_empty_is_data_roaming_enabled());
    assert!(!enabled_of(payload_of(&response)));
    let response = client.call(73, &params_empty_is_networking_enabled());
    assert!(enabled_of(payload_of(&response)));

    // @35 getCounters — the window travels and the counters come back.
    let response = client.call(35, &params_counters_window(1, 5_000_000_000));
    let counters = counters_of(payload_of(&response));
    assert_eq!(counters.bytes_in, 1_024);
    assert_eq!(counters.bytes_out, 2_048);
    assert_eq!(counters.errors_out, 1);
    assert_eq!(counters.sampled_at, 1_700_000_000_000_000_000);

    // @75 listRadioStates — hard/soft blocks are their own fields.
    let response = client.call(75, &params_pagination(0, 0));
    let (radios, total) = radios_of(payload_of(&response));
    assert_eq!(total, 1);
    assert_eq!(radios[0].name, "wifi");
    assert!(radios[0].enabled);
    assert!(!radios[0].hard_blocked && !radios[0].soft_blocked);

    // @78 listKnownWifiNetworks — and the proof it carries no credential:
    // the decoded struct's field set is the contract's own.
    let response = client.call(78, &params_pagination(0, 0));
    let (known, total) = known_of(payload_of(&response));
    assert_eq!(total, 1);
    assert_eq!(known[0].ssid, "hem");
    assert_eq!(known[0].security, "wpa3");
    assert!(known[0].auto_connect);
    assert_eq!(known[0].last_connected, 1_700_000_000_000_000_000);

    // @79 getRoutes — the kernel's table, default route included.
    let response = client.call(79, &params_pagination(0, 0));
    let (routes, total) = routes_of(payload_of(&response));
    assert_eq!(total, 1);
    assert_eq!(routes[0].destination_cidr, "0.0.0.0/0");
    assert_eq!(routes[0].gateway, "192.168.18.1");
    assert_eq!(routes[0].metric, 600);
    assert_eq!(routes[0].interface_id.get(), 2);

    // @81 getProxyConfig — the bypass list survives.
    let response = client.call(81, &params_empty_get_proxy_config());
    let proxy = proxy_of(payload_of(&response));
    assert!(proxy.enabled);
    assert_eq!(proxy.url, "http://proxy.corp:3128");
    assert_eq!(proxy.bypass, alloc::vec![("localhost".to_string(), String::new())]);

    client.close();
    server.join().expect("the server thread finishes");
}

/// The mutating verbs on a control connection, and the mock's record that
/// each one actually reached the backend.
#[test]
fn every_servable_mutating_verb_round_trips_on_a_control_connection() {
    let mock = std::sync::Arc::new(Mock::new());
    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    // @72 setNetworking.
    let response = client.call(72, &params_networking(true));
    assert!(void_success_of(payload_of(&response)));
    // @74 setRadioEnabled — the open radio token travels.
    let response = client.call(74, &params_radio("wifi", false));
    assert!(void_success_of(payload_of(&response)));
    // @11 setMobileData.
    let response = client.call(11, &params_enabled(true));
    assert!(void_success_of(payload_of(&response)));
    // @76 setInterfaceMtu.
    let response = client.call(76, &params_mtu(2, 9000));
    assert!(void_success_of(payload_of(&response)));
    // @77 forgetWifiNetwork.
    let response = client.call(77, &params_ssid("hem"));
    assert!(void_success_of(payload_of(&response)));

    client.close();
    server.join().expect("the server thread finishes");
    let calls = mock.calls();
    for expected in [
        "setNetworking(true)",
        "setRadioEnabled(wifi, false)",
        "setMobileData(true)",
        "setInterfaceMtu(2, 9000)",
        "forgetWifiNetwork(hem)",
    ] {
        assert!(calls.iter().any(|call| call == expected), "{expected} reached the backend: {calls:?}");
    }
}

/// The capability policy: a read-only connection's mutating verbs are
/// refused with `permissionDenied` **before the backend is reached** — the
/// mock's call log is the witness — while its reads still work.
#[test]
fn read_only_class_refuses_the_mutating_verbs_before_the_backend() {
    let mock = std::sync::Arc::new(Mock::new());
    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::ReadOnly);
    let mut client = Client::connect(Box::new(client_side));

    for (verb, ordinal, params) in [
        ("setMobileData", 11, params_enabled(true)),
        ("setNetworking", 72, params_networking(true)),
        ("setRadioEnabled", 74, params_radio("wifi", false)),
        ("setInterfaceMtu", 76, params_mtu(2, 9000)),
        ("forgetWifiNetwork", 77, params_ssid("hem")),
    ] {
        let response = client.call(ordinal, &params);
        assert_eq!(response.status, frame::STATUS_ERROR, "{verb} must be refused");
        assert!(
            response.message.starts_with("permissionDenied:"),
            "{verb} answers with the contract's denial vocabulary: {}",
            response.message
        );
        assert!(response.message.contains(verb), "{verb} names itself: {}", response.message);
        let error = error_of(&response.payload);
        assert_eq!(
            error.code,
            ErrorCode::PermissionDenied,
            "{verb} carries the contract's kind in its T.Error"
        );
        assert!(!error.retryable, "a denial is not retryable");
    }

    // A read-only connection is not a broken one: the reads still answer.
    let response = client.call(73, &params_empty_is_networking_enabled());
    assert!(enabled_of(payload_of(&response)));

    client.close();
    server.join().expect("the server thread finishes");
    // The read above is the only call the read-only capability earned; no
    // mutating verb ever reached the backend.
    let calls = mock.calls();
    for verb in ["setNetworking", "setRadioEnabled", "setMobileData", "setInterfaceMtu", "forgetWifiNetwork"] {
        assert!(
            !calls.iter().any(|call| call.starts_with(verb)),
            "{verb} must never reach the backend through a ReadOnly capability: {calls:?}"
        );
    }
    assert_eq!(calls, alloc::vec!["isNetworkingEnabled".to_string()], "{calls:?}");
}

/// Availability is data, not code: an unavailable method answers the matrix
/// row's own reasonClass + reason, and a capability pull ordinal names the
/// wire's own path.
#[test]
fn unavailable_methods_answer_the_matrix_row_over_the_wire() {
    let mock = std::sync::Arc::new(Mock::new());
    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    // One row per reasonClass the table uses on the wire path.
    for (ordinal, class) in [
        (4u16, "no-privilege"),   // connect
        (6, "no-backend"),        // wifiScan
        (80, "no-privilege"),     // setRoutes
        (82, "no-privilege"),     // setProxyConfig
        (59, "not-applicable"),   // getSatelliteConfig
        (13, "not-applicable"),   // setDataRoaming
    ] {
        let response = client.call(ordinal, &[]);
        assert_eq!(response.status, frame::STATUS_ERROR, "@{ordinal} must not be served");
        assert!(
            response.message.starts_with(&format!("{class}:")),
            "@{ordinal} answers with the row's reasonClass: {}",
            response.message
        );
        let row = smart_lib::domains::network::availability::NETWORK_AVAILABILITY
            .iter()
            .find(|row| row.ordinal == ordinal)
            .expect("the row exists");
        assert_eq!(row.status, AvailStatus::Unavailable);
        assert!(
            response.message.contains(row.reason),
            "@{ordinal} repeats the row's own reason: {}",
            response.message
        );
        let error = error_of(&response.payload);
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(!error.retryable, "notSupported is never retryable");
    }

    // The contract's pull ordinals take a capability param: the wire answers
    // by naming the supported path instead of pretending.
    for ordinal in [3u16, 38] {
        let response = client.call(ordinal, &[]);
        let error = error_of(&response.payload);
        assert_eq!(error.code, ErrorCode::InvalidArgument, "@{ordinal}");
        assert!(
            response.message.contains("SUB_PULL"),
            "@{ordinal} names the wire's pull path: {}",
            response.message
        );
    }

    // An ordinal no row covers is a caller error, answered as one.
    let response = client.call(200, &[]);
    let error = error_of(&response.payload);
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(response.message.contains("200"), "{}", response.message);

    client.close();
    server.join().expect("the server thread finishes");
}

/// Frame-level faults are typed answers, and the loop survives them.
#[test]
fn unknown_interface_and_malformed_frames_are_typed_answers() {
    let mock = std::sync::Arc::new(Mock::new());
    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    // A foreign interface id.
    let response = client.call_foreign_interface(0, &params_pagination(0, 0));
    assert!(response.message.starts_with("unknown-interface:"), "{}", response.message);
    assert!(response.message.contains("deadbeef"), "{}", response.message);

    // A frame whose declared params length does not fit its body.
    let response = client.call_raw(&raw_frame(NETWORK_SCHEMA_ID, 0, 100, 0, b"short"));
    assert!(response.message.starts_with("bad-frame:"), "{}", response.message);

    // A frame with the reserved flags set.
    let response = client.call_raw(&raw_frame(NETWORK_SCHEMA_ID, 0, 0, 1, &[]));
    assert!(response.message.starts_with("bad-frame:"), "{}", response.message);

    // The loop is still alive after every fault.
    let response = client.call(73, &params_empty_is_networking_enabled());
    assert!(enabled_of(payload_of(&response)));

    client.close();
    server.join().expect("the server thread finishes");
}

/// Subscriptions over the wire: subscribe mints the id and carries the first
/// event, SUB_PULL reads the backend's next answer under the caller's bound,
/// "nothing yet" is an empty payload (never a fabricated event), SUB_CANCEL
/// retires the token, and every bounded pull relays exactly what the backend
/// answered — the drop accounting stays the backend surface's own.
#[test]
fn subscriptions_round_trip_over_the_wire() {
    let mock = std::sync::Arc::new(Mock::new());
    // The scripted pull sequence: the subscribe's first event, one more
    // event, then the backend's "nothing yet".
    let first_event = ConnStatus {
        kind: "first".to_string(),
        ..mock.status.clone()
    };
    let second_event = ConnStatus {
        kind: "second".to_string(),
        ..mock.status.clone()
    };
    mock.status_pulls.lock().push_back(Ok(first_event.clone()));
    mock.status_pulls.lock().push_back(Ok(second_event.clone()));
    mock.status_pulls
        .lock()
        .push_back(Err(Error::temporary("no status event yet")));

    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    // @2 subscribeStatus — the first event rides the answer.
    let response = client.call(2, &params_subscribe_interface(1));
    let payload = payload_of(&response);
    let sub_id = frame::sub_id_of(payload).expect("the subscribe answer carries the minted id");
    assert_eq!(sub_id, 1);
    assert_eq!(status_of(&payload[frame::SUB_ID_LEN..]).kind, "first");

    // SUB_PULL — the next backend answer, bounded by the caller.
    let response = client.call(frame::SUB_PULL, &params_pull(sub_id, 1));
    let payload = payload_of(&response);
    assert_eq!(frame::sub_id_of(payload), Some(sub_id));
    assert_eq!(status_of(&payload[frame::SUB_ID_LEN..]).kind, "second");

    // SUB_PULL when the backend has nothing yet: ok, empty payload.
    let response = client.call(frame::SUB_PULL, &params_pull(sub_id, 4));
    assert!(response.payload.is_empty(), "nothing yet is empty, never fabricated");
    assert_eq!(response.status, frame::STATUS_OK);

    // The scripted queue is drained: the mock's stable status answers, and
    // the harness relays it unchanged (one backend pull, one event).
    let response = client.call(frame::SUB_PULL, &params_pull(sub_id, 0));
    let payload = payload_of(&response);
    assert_eq!(status_of(&payload[frame::SUB_ID_LEN..]).kind, "loopback");

    // SUB_CANCEL retires the token; the second cancel is an honest notFound.
    let response = client.call(frame::SUB_CANCEL, &params_cancel(sub_id));
    assert!(void_success_of(payload_of(&response)));
    let response = client.call(frame::SUB_CANCEL, &params_cancel(sub_id));
    let error = error_of(&response.payload);
    assert_eq!(error.code, ErrorCode::NotFound);

    // A pull naming a token this connection never minted is a notFound too.
    let response = client.call(frame::SUB_PULL, &params_pull(9_999, 1));
    assert_eq!(error_of(&response.payload).code, ErrorCode::NotFound);

    // @37 subscribeCounters — the counters subscription's first event.
    let response = client.call(37, &params_subscribe_counters_interface(1));
    let payload = payload_of(&response);
    let counter_sub = frame::sub_id_of(payload).expect("the counters subscribe mints an id");
    assert_ne!(counter_sub, sub_id, "each subscription mints its own token");
    assert_eq!(counters_of(&payload[frame::SUB_ID_LEN..]).bytes_in, 1_024);

    // The counters pull reads the counters verb, not the status verb.
    let response = client.call(frame::SUB_PULL, &params_pull(counter_sub, 1));
    let payload = payload_of(&response);
    assert_eq!(counters_of(&payload[frame::SUB_ID_LEN..]).bytes_out, 2_048);

    client.close();
    server.join().expect("the server thread finishes");
    let calls = mock.calls();
    assert_eq!(
        calls.iter().filter(|call| call.starts_with("subscribeStatus")).count(),
        1,
        "{calls:?}"
    );
    // One backend pull per SUB_PULL plus the subscribe's first event — the
    // harness relays exactly what the backend answered, never more.
    assert_eq!(
        calls.iter().filter(|call| call.starts_with("pullStatus")).count(),
        4,
        "{calls:?}"
    );
    // The second cancel is refused by the service's own bookkeeping, so the
    // backend sees exactly one cancel.
    assert_eq!(
        calls.iter().filter(|call| call.starts_with("cancelSubscription")).count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|call| call.starts_with("pullCounters")),
        "the counters pull reads the counters verb: {calls:?}"
    );
}

/// Error totality over the wire: every backend error kind the contract
/// declares survives the seam with its code, its message and its honest
/// `retryable` — the frame's error channel is the backend's own vocabulary,
/// not a flattened generic failure.
#[test]
fn backend_errors_propagate_over_the_wire_with_their_kind() {
    // `temporary` on a *read* is an error frame (the "nothing yet" empty
    // payload is a pull's answer, not a read's).
    let mock = std::sync::Arc::new(
        Mock::new().failing_get_status(Error::temporary("the kernel store is cold").with_retryable(true)),
    );

    let (client_side, server_side) = InProcessChannel::pair();
    let server = spawn_server(&mock, server_side, CapabilityClass::Control);
    let mut client = Client::connect(Box::new(client_side));

    let response = client.call(1, &params_interface_id(1));
    assert_eq!(response.status, frame::STATUS_ERROR);
    let error = error_of(&response.payload);
    assert_eq!(error.code, ErrorCode::Temporary, "the code survives the seam unchanged");
    assert!(error.retryable, "a temporary failure stays retryable");
    assert!(error.message.contains("the kernel store is cold"), "{}", error.message);
    assert!(response.message.contains("temporary:"), "{}", response.message);

    client.close();
    server.join().expect("the server thread finishes");
}

/// The UnixSocket end-to-end path — the daemon-shaped session: a listener,
/// an accept in a thread, one client session carrying a read, a subscription
/// and a denial over a real socket. The class is pinned to ReadOnly so the
/// denial assertion is deterministic; the classified class is printed as the
/// diagnostic it is (the policy itself is unit-tested against synthetic
/// credentials).
#[test]
#[cfg(all(unix, feature = "std"))]
fn unix_socket_end_to_end_serves_a_client_session() {
    let dir = std::env::temp_dir().join("smart-lib-serve-unix-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("service.sock");
    let _ = std::fs::remove_file(&path);
    let path_text = path.display().to_string();

    let listener = listen(&Transport::UnixSocket { path: path_text.clone() })
        .expect("the listener binds");
    let mock = std::sync::Arc::new(Mock::new());
    let server_mock = mock.clone();
    let server = std::thread::spawn(move || {
        let channel = listener.accept().expect("the client connects");
        // The daemon-style classification: the class comes from the peer's
        // host identity. The test pins the session to ReadOnly so the
        // denial below is deterministic on any host.
        let classified = classify(channel.peer_credentials());
        std::eprintln!("unix session: peer classified as {classified}");
        serve_backend(&*server_mock, &*channel, CapabilityClass::ReadOnly)
            .expect("the served session ends cleanly");
    });

    let channel = connect(Transport::UnixSocket { path: path_text })
        .expect("the client connects");
    let mut client = Client::connect(channel);

    // A read.
    let response = client.call(73, &params_empty_is_networking_enabled());
    assert!(enabled_of(payload_of(&response)));

    // A subscription: subscribe, pull, cancel.
    let response = client.call(2, &params_subscribe_interface(1));
    let payload = payload_of(&response);
    let sub_id = frame::sub_id_of(payload).expect("the minted id");
    assert_eq!(status_of(&payload[frame::SUB_ID_LEN..]).host_state, HostState::Managed);
    let response = client.call(frame::SUB_PULL, &params_pull(sub_id, 1));
    let payload = payload_of(&response);
    assert_eq!(frame::sub_id_of(payload), Some(sub_id));
    assert_eq!(status_of(&payload[frame::SUB_ID_LEN..]).kind, "loopback");
    let response = client.call(frame::SUB_CANCEL, &params_cancel(sub_id));
    assert!(void_success_of(payload_of(&response)));

    // A denial on the read-only capability — the honest answer over a real
    // socket, with the backend never reached.
    let response = client.call(72, &params_networking(false));
    assert_eq!(error_of(&response.payload).code, ErrorCode::PermissionDenied);

    client.close();
    server.join().expect("the server thread finishes");
    let _ = std::fs::remove_file(&path);
    assert!(
        mock.calls().iter().all(|call| !call.starts_with("setNetworking")),
        "the read-only capability never reached the mutating verb: {:?}",
        mock.calls()
    );
}

/// The ABI handshake: a caller speaking the contract family is matched and
/// told which schema it reached; a caller speaking anything else is told,
/// in the contract's own error vocabulary, that it cannot be served.
#[test]
fn the_handshake_matches_the_contract_and_refuses_a_foreign_abi() {
    // A foreign namespace is answered `incompatible`, naming the mismatch.
    let (client_side, server_side) = InProcessChannel::pair();
    let server = std::thread::spawn(move || {
        let mock = Mock::new();
        serve_backend(&mock, &server_side, CapabilityClass::Control).expect("the session ends cleanly");
    });
    let (arm, detail) =
        Client::handshake_summary(Box::new(client_side), Some("not-a-contract"));
    assert_eq!(arm, "incompatible", "{detail}");
    assert!(detail.contains("not-a-contract"), "{detail}");
    server.join().expect("the server thread finishes");

    // A matched handshake names the served schema (the client can see
    // exactly which contract it reached).
    let (client_side, server_side) = InProcessChannel::pair();
    let server = std::thread::spawn(move || {
        let mock = Mock::new();
        serve_backend(&mock, &server_side, CapabilityClass::Control).expect("the session ends cleanly");
    });
    let (arm, detail) =
        Client::handshake_summary(Box::new(client_side), Some(ABI_NAMESPACE));
    assert_eq!(arm, "matched", "{detail}");
    assert_eq!(detail, ABI_NAMESPACE);
    server.join().expect("the server thread finishes");
}

/// The uid-peer capability policy, against synthetic credentials and the
/// pure capability-mask parser (the `/proc` read is the only host input, and
/// it is read-only — law #8).
#[test]
fn classify_maps_peer_identity_into_the_capability_class() {
    // No peer identity at all (Shmem, Tcp, InProcess): ReadOnly, never a
    // fabricated grant.
    assert_eq!(classify(None), CapabilityClass::ReadOnly);

    // uid 0 is Control; a peer whose /proc entry cannot be read at all is
    // ReadOnly (absent evidence is not authority).
    assert_eq!(
        classify(Some(PeerCredentials { pid: 1, uid: 0, gid: 0 })),
        CapabilityClass::Control
    );
    assert_eq!(
        classify(Some(PeerCredentials { pid: i32::MAX, uid: 1_000, gid: 1_000 })),
        CapabilityClass::ReadOnly
    );

    // The capability rule itself: bit 12 is CAP_NET_ADMIN, and a mask
    // without it is a read-only class.
    #[cfg(all(target_os = "linux", feature = "std"))]
    {
        assert!(serve::cap_mask_from_status("Name:\tinit\nCapEff:\t00000000a80425fb\n").is_some());
        let with_cap = serve::cap_mask_from_status(
            "Name:\troot\nCapEff:\t000001ffffffffff\nCapPrm:\t0000000000000000\n",
        )
        .expect("CapEff parses");
        assert_eq!(with_cap & (1 << 12), 1 << 12, "bit 12 is CAP_NET_ADMIN");
        let without_cap = serve::cap_mask_from_status("CapEff:\t0000000000000000\n").expect("parses");
        assert_eq!(without_cap & (1 << 12), 0);
        assert!(serve::cap_mask_from_status("Name:\tnocaps\n").is_none());
        assert!(serve::cap_mask_from_status("CapEff:\tnot-hex\n").is_none());
    }
}

/// The policy reads this process's own capabilities and agrees with a
/// direct read of the same `/proc/<pid>/status` — the whole `classify` path
/// exercised on the live host, with reading as the only host interaction.
#[test]
#[cfg(all(target_os = "linux", feature = "std"))]
fn classify_reads_this_process_own_capabilities() {
    let pid = std::process::id() as i32;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("own status");
    let mask = serve::cap_mask_from_status(&status).expect("own CapEff line");
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|uid| uid.parse::<u32>().ok())
        .expect("own Uid line");
    let expected = if uid == 0 || mask & (1 << 12) != 0 {
        CapabilityClass::Control
    } else {
        CapabilityClass::ReadOnly
    };
    assert_eq!(
        classify(Some(PeerCredentials { pid, uid, gid: 0 })),
        expected,
        "classify must agree with the process's own kernel-reported capabilities"
    );
}

/// The TCP end-to-end path: a loopback session over the transport that
/// declares **no peer identity** — so the capability policy's honest answer
/// (ReadOnly, never a fabricated grant) is exactly what the wire must show.
/// The session itself is the same one every other transport serves: a read
/// succeeds, a mutating verb is refused before the backend.
#[test]
#[cfg(feature = "std")]
fn tcp_end_to_end_serves_a_client_session_at_the_honest_class() {
    let listener = smart_lib::transport::tcp::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("bound address");
    let mock = std::sync::Arc::new(Mock::new());
    let server_mock = mock.clone();
    let server = std::thread::spawn(move || {
        let channel = listener.accept().expect("the client connects");
        // Over TCP there is no peer identity to classify — the transport
        // declares it, and the policy's answer is the honest weaker class.
        assert_eq!(
            classify(channel.peer_credentials()),
            CapabilityClass::ReadOnly,
            "tcp carries no peer identity, so no authority claim exists"
        );
        serve_backend(&*server_mock, &*channel, CapabilityClass::ReadOnly)
            .expect("the served session ends cleanly");
    });

    let channel = smart_lib::transport::tcp::TcpChannel::connect(&addr).expect("connect");
    let mut client = Client::connect(Box::new(channel));

    let response = client.call(73, &params_empty_is_networking_enabled());
    assert!(enabled_of(payload_of(&response)));
    let response = client.call(72, &params_networking(false));
    assert_eq!(error_of(&response.payload).code, ErrorCode::PermissionDenied);

    client.close();
    server.join().expect("the server thread finishes");
    assert!(
        !mock.calls().iter().any(|call| call.starts_with("setNetworking")),
        "the read-only class never reached the mutating verb: {:?}",
        mock.calls()
    );
}

/// The Shmem end-to-end path: both ends of the arena-constructed pair serve
/// one session, because the dispatch envelope is transport machinery and
/// never names a transport (law #4: one API shape, and the harness only
/// speaks `Channel`).
#[test]
#[cfg(all(unix, feature = "std"))]
fn shmem_end_to_end_serves_a_client_session() {
    use smart_lib::transport::shmem::{Arena, Direction, ShmemChannel, ShmemHandle};
    use smart_lib::transport::bootstrap;

    let arena = Arena::create(smart_lib::transport::shmem::ArenaConfig {
        slot_bytes: 64 * 1024,
        slot_count: 8,
        ring_slots: 8,
        ring_bytes: 64 * 1024,
    })
    .expect("arena");
    arena.set_namespace_root(bootstrap::namespace_root_for(arena.epoch()));
    arena.establish();
    let client_side = ShmemChannel::from_handle(ShmemHandle::new(arena.clone(), Direction::A, None));
    let server_side = ShmemChannel::from_handle(ShmemHandle::new(arena, Direction::B, None));

    let mock = std::sync::Arc::new(Mock::new());
    let server_mock = mock.clone();
    let server = std::thread::spawn(move || {
        serve_backend(&*server_mock, &server_side, CapabilityClass::Control)
            .expect("the served session ends cleanly");
    });
    let mut client = Client::connect(Box::new(client_side));

    let response = client.call(1, &params_interface_id(1));
    assert_eq!(status_of(payload_of(&response)).host_state, HostState::Managed);
    let response = client.call(75, &params_pagination(0, 10));
    let (radios, total) = radios_of(payload_of(&response));
    assert_eq!(total, 1);
    assert_eq!(radios[0].name, "wifi");

    client.close();
    server.join().expect("the server thread finishes");
}
