//! The network domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `network.capnp`'s `interface Network` block. The dispatch itself is
//! the generated `ServerDispatch` — there is no ordinal, no method table and
//! no envelope of this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason,
//!   read as data (`availability.rs`), never as code;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached, so a backend can never be
//!   reached through authority the peer does not hold (law #5).
//!
//! The subscription methods are the contract's own capability model: the
//! service hands back a `StatusSubscription` / `CounterSubscription`
//! capability, and the pull rides it. The old wire's pseudo-ordinals
//! (`SUB_PULL`) are gone with the wire that invented them.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::core::ErrorCode;
use crate::domains::network::availability::{self, AvailStatus, ReasonClass};
use crate::domains::network::backend::NetworkBackend;
use crate::core::ids::NetworkInterfaceId;
use crate::domains::network::types::{
    InterfaceCounters, InterfaceSnapshot, Pagination, SubscriptionId, WifiNetwork,
};
use crate::schema::network_capnp::network as net;
use crate::schema::network_capnp::{counter_subscription, status_subscription};
use crate::schema::types_capnp;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

/// The network service: the domain's backend, and the class this
/// connection earned.
///
/// The backend is shared behind an `Arc`: the *same* host state answers
/// every connection, and a per-connection server is a view of it, not a
/// copy.
pub struct NetworkService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for NetworkService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> NetworkService<B>
where
    B: NetworkBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

/// The mutating verbs: the ones a read-only connection may not reach.
///
/// This is the authority gate's data (law #5), and it is *data* on purpose:
/// the class is decided once per connection, and this list says which verbs
/// that class decides about. The matrix marks these verbs `DEGRADED` — "a
/// host-mutating verb no kernel path can own" — and the read-shaped verbs it
/// marks `DEGRADED` too (a privilege *measurement*) are not on this list,
/// because a measurement is not a mutation.
const CONTROL_VERBS: &[&str] = &[
    "wifiConnect",
    "wifiDisconnect",
    "setMobileData",
    "setNetworking",
    "setRadioEnabled",
    "setInterfaceMtu",
    "forgetWifiNetwork",
];

/// Run `then` only on a control connection; a read-only connection is
/// refused **before** the backend is reached.
fn gate<T>(
    class: CapabilityClass,
    verb: &str,
    then: impl FnOnce() -> T,
) -> Result<T, capnp::Error> {
    // Fail closed: a verb that is not on the control list is refused even on
    // a control connection, so a new mutating verb cannot slip through the
    // gate by forgetting to list it.
    if class.is_control() && CONTROL_VERBS.contains(&verb) {
        Ok(then())
    } else {
        Err(denied(verb))
    }
}

/// The refusal answer for a mutating verb on a read-only connection.
fn denied(verb: &str) -> capnp::Error {
    errors::decline(
        ErrorCode::PermissionDenied,
        &format!(
            "{verb} is a control verb; this connection's capability class is \
             ReadOnly (uid 0 or CAP_NET_ADMIN grants Control)"
        ),
    )
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
fn unavailable(name: &str) -> capnp::Error {
    match availability::method_availability_for(name) {
        Some(row) if matches!(row.status, AvailStatus::Unavailable) => {
            let class = row.reason_class.unwrap_or(ReasonClass::Unknown).as_str();
            errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {}", row.reason),
            )
        }
        Some(row) => errors::decline(
            ErrorCode::InvalidArgument,
            &format!(
                "invalidArgument: {} (@{}) is not served by this backend",
                row.name, row.ordinal
            ),
        ),
        None => errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Network contract method"),
        ),
    }
}

/// The contract's `T.Pagination` as the domain's pagination.
fn pagination(reader: types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// Fill a `T.VoidResult` from a backend answer: the contract's own error
/// arm carries the failure, so the code is not lost in a string.
fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

/// The status subscription the service hands back for `subscribeStatus`.
///
/// This is the contract's own capability: the client holds it, and the pull
/// rides it. The backend's subscription id is the handle behind it.
pub struct StatusSubscription<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for StatusSubscription<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> status_subscription::Server for StatusSubscription<B>
where
    B: NetworkBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        _params: status_subscription::PullParams,
        mut results: status_subscription::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let status = backend
                .pull_status(subscription)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            status.write_to(&mut builder.init_status());
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: status_subscription::CancelParams,
        mut results: status_subscription::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_subscription(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The counter subscription the service hands back for `subscribeCounters`.
pub struct CounterSubscription<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for CounterSubscription<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> counter_subscription::Server for CounterSubscription<B>
where
    B: NetworkBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        _params: counter_subscription::PullParams,
        mut results: counter_subscription::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let counters = backend
                .pull_counters(subscription)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_counters(&counters, &mut builder.init_counters());
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: counter_subscription::CancelParams,
        mut results: counter_subscription::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_subscription(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// Write the domain's counters into the contract's `InterfaceCounters`.
fn write_counters(
    counters: &InterfaceCounters,
    builder: &mut crate::schema::network_capnp::interface_counters::Builder<'_>,
) {
    builder.set_bytes_in(counters.bytes_in);
    builder.set_bytes_out(counters.bytes_out);
    builder.set_packets_in(counters.packets_in);
    builder.set_packets_out(counters.packets_out);
    builder.set_errors_in(counters.errors_in);
    builder.set_errors_out(counters.errors_out);
    builder.set_drops_in(counters.drops_in);
    builder.set_drops_out(counters.drops_out);
    let mut sampled = builder.reborrow().init_sampled_at();
    sampled.set_nanoseconds(counters.sampled_at);
}

/// Write the domain's interface snapshot into the contract's
/// `NetworkInterface`. Only the fields the domain measured are written; the
/// rest stay absent rather than invented.
fn write_interface(
    item: &InterfaceSnapshot,
    builder: &mut crate::schema::network_capnp::network_interface::Builder<'_>,
) {
    let mut id = builder.reborrow().init_id();
    id.set_id(item.id.get());
    builder.set_name(&item.name);
    builder.set_type(&item.kind);
    let mut hardware = builder.reborrow().init_hw_address();
    hardware.set_address(&item.hardware_address.0);
    builder.set_up(item.up);
    builder.set_connected(item.connected);
    builder.set_mt_u(item.mtu);
}

/// Write the domain's wifi network into the contract's `WifiNetwork`.
fn write_wifi_network(
    item: &WifiNetwork,
    builder: &mut crate::schema::network_capnp::wifi_network::Builder<'_>,
) {
    builder.set_ssid(&item.ssid);
    let mut bssid = builder.reborrow().init_bssid();
    bssid.set_address(&item.bssid.0);
    builder.set_strength(item.strength_dbm);
    builder.set_frequency(item.frequency_hz);
    builder.set_security(&item.security);
    builder.set_hidden(item.hidden);
}

impl<B> net::Server for NetworkService<B>
where
    B: NetworkBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn get_interfaces(
        self: Rc<Self>,
        params: net::GetInterfacesParams,
        mut results: net::GetInterfacesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = backend
                .get_interfaces(pagination(params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_interfaces(page.items.len() as u32);
            for (index, item) in page.items.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                write_interface(item, &mut entry);
            }
            Ok(())
        }
    }

    fn get_status(
        self: Rc<Self>,
        params: net::GetStatusParams,
        mut results: net::GetStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let interface = NetworkInterfaceId::new(params.get_interface_id()?.get_id());
            let status = backend
                .get_status(interface)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            status.write_to(&mut builder.init_status());
            Ok(())
        }
    }

    fn is_mobile_data_enabled(
        self: Rc<Self>,
        _params: net::IsMobileDataEnabledParams,
        mut results: net::IsMobileDataEnabledResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let enabled = backend
                .is_mobile_data_enabled()
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_enabled(enabled);
            Ok(())
        }
    }

    fn is_data_roaming_enabled(
        self: Rc<Self>,
        _params: net::IsDataRoamingEnabledParams,
        mut results: net::IsDataRoamingEnabledResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let enabled = backend
                .is_data_roaming_enabled()
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_enabled(enabled);
            Ok(())
        }
    }

    fn is_networking_enabled(
        self: Rc<Self>,
        _params: net::IsNetworkingEnabledParams,
        mut results: net::IsNetworkingEnabledResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let enabled = backend
                .is_networking_enabled()
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_enabled(enabled);
            Ok(())
        }
    }

    fn get_counters(
        self: Rc<Self>,
        params: net::GetCountersParams,
        mut results: net::GetCountersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let interface = NetworkInterfaceId::new(params.get_interface_id()?.get_id());
            let window = crate::core::values::Duration::from_reader(&params.get_window()?);
            let counters = backend
                .get_counters(interface, window)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_counters(&counters, &mut builder.init_counters());
            Ok(())
        }
    }

    fn list_radio_states(
        self: Rc<Self>,
        params: net::ListRadioStatesParams,
        mut results: net::ListRadioStatesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = backend
                .list_radio_states(pagination(params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_radios(page.items.len() as u32);
            for (index, item) in page.items.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                item.write_to(&mut entry);
            }
            builder.set_total(page.total);
            Ok(())
        }
    }

    fn list_known_wifi_networks(
        self: Rc<Self>,
        params: net::ListKnownWifiNetworksParams,
        mut results: net::ListKnownWifiNetworksResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = backend
                .list_known_wifi_networks(pagination(params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_networks(page.items.len() as u32);
            for (index, item) in page.items.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                item.write_to(&mut entry);
            }
            builder.set_total(page.total);
            Ok(())
        }
    }

    fn get_routes(
        self: Rc<Self>,
        params: net::GetRoutesParams,
        mut results: net::GetRoutesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = backend
                .get_routes(pagination(params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_routes(page.items.len() as u32);
            for (index, item) in page.items.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                item.write_to(&mut entry);
            }
            builder.set_total(page.total);
            Ok(())
        }
    }

    fn get_proxy_config(
        self: Rc<Self>,
        _params: net::GetProxyConfigParams,
        mut results: net::GetProxyConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let config = backend
                .get_proxy_config()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            config.write_to(&mut builder.init_config());
            Ok(())
        }
    }

    fn wifi_scan(
        self: Rc<Self>,
        params: net::WifiScanParams,
        mut results: net::WifiScanResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = backend
                .wifi_scan(pagination(params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_networks(page.items.len() as u32);
            for (index, item) in page.items.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                write_wifi_network(item, &mut entry);
            }
            Ok(())
        }
    }

    // ── The subscriptions: the contract's own capabilities ───────────────

    fn subscribe_status(
        self: Rc<Self>,
        params: net::SubscribeStatusParams,
        mut results: net::SubscribeStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let interface = NetworkInterfaceId::new(params.get_interface_id()?.get_id());
            let subscription = backend
                .subscribe_status(interface)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = StatusSubscription {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: status_subscription::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn pull_status(
        self: Rc<Self>,
        params: net::PullStatusParams,
        mut results: net::PullStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp:: Error>> {
        async move {
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            // The pull rides the subscription the caller holds: the
            // capability is the handle (law #5), and the subscription's own
            // `pull` is where the backend is reached.
            let response = subscription.pull_request().send().promise.await?;
            let answer = response.get()?;
            let status = answer.get_status()?;
            results.get().set_status(status)?;
            Ok(())
        }
    }

    fn subscribe_counters(
        self: Rc<Self>,
        params: net::SubscribeCountersParams,
        mut results: net::SubscribeCountersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let interface = NetworkInterfaceId::new(params.get_interface_id()?.get_id());
            let subscription = backend
                .subscribe_counters(interface)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = CounterSubscription {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: counter_subscription::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn pull_counters(
        self: Rc<Self>,
        params: net::PullCountersParams,
        mut results: net::PullCountersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let subscription = params.get_subscription()?;
            let response = subscription.pull_request().send().promise.await?;
            let answer = response.get()?;
            let counters = answer.get_counters()?;
            results.get().set_counters(counters)?;
            Ok(())
        }
    }

    // ── The mutating verbs: the class decides, before the backend ────────

    fn wifi_connect(
        self: Rc<Self>,
        params: net::WifiConnectParams,
        mut results: net::WifiConnectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let ssid = crate::serve::text_of(params.get_ssid()?);
            let credential = crate::core::ids::SecretKeyId::new(params.get_credential()?.get_id());
            let outcome = gate(class, "wifiConnect", || {
                backend.wifi_connect(&ssid, credential)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn wifi_disconnect(
        self: Rc<Self>,
        _params: net::WifiDisconnectParams,
        mut results: net::WifiDisconnectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let outcome = gate(class, "wifiDisconnect", || backend.wifi_disconnect())?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_mobile_data(
        self: Rc<Self>,
        params: net::SetMobileDataParams,
        mut results: net::SetMobileDataResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let enabled = params.get_enabled();
            let outcome = gate(class, "setMobileData", || {
                backend.set_mobile_data_enabled(enabled)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_networking(
        self: Rc<Self>,
        params: net::SetNetworkingParams,
        mut results: net::SetNetworkingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let enabled = params.get_enabled();
            let outcome = gate(class, "setNetworking", || {
                backend.set_networking_enabled(enabled)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_radio_enabled(
        self: Rc<Self>,
        params: net::SetRadioEnabledParams,
        mut results: net::SetRadioEnabledResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let radio = crate::serve::text_of(params.get_radio()?);
            let enabled = params.get_enabled();
            let outcome = gate(class, "setRadioEnabled", || {
                backend.set_radio_enabled(&radio, enabled)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_interface_mtu(
        self: Rc<Self>,
        params: net::SetInterfaceMtuParams,
        mut results: net::SetInterfaceMtuResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let interface = NetworkInterfaceId::new(params.get_interface_id()?.get_id());
            let mtu = params.get_mtu();
            let outcome = gate(class, "setInterfaceMtu", || {
                backend.set_interface_mtu(interface, mtu)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn forget_wifi_network(
        self: Rc<Self>,
        params: net::ForgetWifiNetworkParams,
        mut results: net::ForgetWifiNetworkResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            let params = params.get()?;
            let ssid = crate::serve::text_of(params.get_ssid()?);
            let outcome = gate(class, "forgetWifiNetwork", || {
                backend.forget_wifi_network(&ssid)
            })?;
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    // ── The methods this backend does not serve ──────────────────────────
    //
    // One arm per contract method, so the answer is the matrix's answer and
    // never a silent absence. The arm's body is the matrix row, read as
    // data.

    fn connect(
        self: Rc<Self>,
        _params: net::ConnectParams,
        _results: net::ConnectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("connect")))
    }

    fn disconnect(
        self: Rc<Self>,
        _params: net::DisconnectParams,
        _results: net::DisconnectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("disconnect")))
    }

    fn wifi_set_auto_connect(
        self: Rc<Self>,
        _params: net::WifiSetAutoConnectParams,
        _results: net::WifiSetAutoConnectResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("wifiSetAutoConnect")))
    }

    fn wifi_set_mac_randomization(
        self: Rc<Self>,
        _params: net::WifiSetMacRandomizationParams,
        _results: net::WifiSetMacRandomizationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("wifiSetMacRandomization")))
    }

    fn set_data_roaming(
        self: Rc<Self>,
        _params: net::SetDataRoamingParams,
        _results: net::SetDataRoamingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setDataRoaming")))
    }

    fn set_vpn_profile(
        self: Rc<Self>,
        _params: net::SetVpnProfileParams,
        _results: net::SetVpnProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setVpnProfile")))
    }

    fn remove_vpn_profile(
        self: Rc<Self>,
        _params: net::RemoveVpnProfileParams,
        _results: net::RemoveVpnProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("removeVpnProfile")))
    }

    fn list_vpn_profiles(
        self: Rc<Self>,
        _params: net::ListVpnProfilesParams,
        _results: net::ListVpnProfilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listVpnProfiles")))
    }

    fn attach_vpn(
        self: Rc<Self>,
        _params: net::AttachVpnParams,
        _results: net::AttachVpnResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("attachVpn")))
    }

    fn detach_vpn(
        self: Rc<Self>,
        _params: net::DetachVpnParams,
        _results: net::DetachVpnResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("detachVpn")))
    }

    fn list_vpn_connections(
        self: Rc<Self>,
        _params: net::ListVpnConnectionsParams,
        _results: net::ListVpnConnectionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listVpnConnections")))
    }

    fn start_hotspot(
        self: Rc<Self>,
        _params: net::StartHotspotParams,
        _results: net::StartHotspotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("startHotspot")))
    }

    fn stop_hotspot(
        self: Rc<Self>,
        _params: net::StopHotspotParams,
        _results: net::StopHotspotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("stopHotspot")))
    }

    fn get_hotspot_status(
        self: Rc<Self>,
        _params: net::GetHotspotStatusParams,
        _results: net::GetHotspotStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getHotspotStatus")))
    }

    fn subscribe_hotspot(
        self: Rc<Self>,
        _params: net::SubscribeHotspotParams,
        _results: net::SubscribeHotspotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeHotspot")))
    }

    fn pull_hotspot(
        self: Rc<Self>,
        _params: net::PullHotspotParams,
        _results: net::PullHotspotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullHotspot")))
    }

    fn request_address_assignment(
        self: Rc<Self>,
        _params: net::RequestAddressAssignmentParams,
        _results: net::RequestAddressAssignmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("requestAddressAssignment")))
    }

    fn get_address_assignment(
        self: Rc<Self>,
        _params: net::GetAddressAssignmentParams,
        _results: net::GetAddressAssignmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getAddressAssignment")))
    }

    fn release_address_assignment(
        self: Rc<Self>,
        _params: net::ReleaseAddressAssignmentParams,
        _results: net::ReleaseAddressAssignmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("releaseAddressAssignment")))
    }

    fn subscribe_address_assignment(
        self: Rc<Self>,
        _params: net::SubscribeAddressAssignmentParams,
        _results: net::SubscribeAddressAssignmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeAddressAssignment")))
    }

    fn pull_address_assignment(
        self: Rc<Self>,
        _params: net::PullAddressAssignmentParams,
        _results: net::PullAddressAssignmentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullAddressAssignment")))
    }

    fn get_dns_servers(
        self: Rc<Self>,
        _params: net::GetDnsServersParams,
        _results: net::GetDnsServersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getDnsServers")))
    }

    fn set_dns_servers(
        self: Rc<Self>,
        _params: net::SetDnsServersParams,
        _results: net::SetDnsServersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setDnsServers")))
    }

    fn set_interface_qos(
        self: Rc<Self>,
        _params: net::SetInterfaceQosParams,
        _results: net::SetInterfaceQosResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setInterfaceQos")))
    }

    fn get_interface_qos(
        self: Rc<Self>,
        _params: net::GetInterfaceQosParams,
        _results: net::GetInterfaceQosResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getInterfaceQos")))
    }

    fn reset_counters(
        self: Rc<Self>,
        _params: net::ResetCountersParams,
        _results: net::ResetCountersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("resetCounters")))
    }

    fn check_captive_portal(
        self: Rc<Self>,
        _params: net::CheckCaptivePortalParams,
        _results: net::CheckCaptivePortalResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("checkCaptivePortal")))
    }

    fn subscribe_captive_portal(
        self: Rc<Self>,
        _params: net::SubscribeCaptivePortalParams,
        _results: net::SubscribeCaptivePortalResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeCaptivePortal")))
    }

    fn pull_captive_portal(
        self: Rc<Self>,
        _params: net::PullCaptivePortalParams,
        _results: net::PullCaptivePortalResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullCaptivePortal")))
    }

    fn up_interface(
        self: Rc<Self>,
        _params: net::UpInterfaceParams,
        _results: net::UpInterfaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("upInterface")))
    }

    fn down_interface(
        self: Rc<Self>,
        _params: net::DownInterfaceParams,
        _results: net::DownInterfaceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("downInterface")))
    }

    fn get_topology(
        self: Rc<Self>,
        _params: net::GetTopologyParams,
        _results: net::GetTopologyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getTopology")))
    }

    fn set_power_profile(
        self: Rc<Self>,
        _params: net::SetPowerProfileParams,
        _results: net::SetPowerProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setPowerProfile")))
    }

    fn get_power_profile(
        self: Rc<Self>,
        _params: net::GetPowerProfileParams,
        _results: net::GetPowerProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getPowerProfile")))
    }

    fn handoff(
        self: Rc<Self>,
        _params: net::HandoffParams,
        _results: net::HandoffResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("handoff")))
    }

    fn create_slice(
        self: Rc<Self>,
        _params: net::CreateSliceParams,
        _results: net::CreateSliceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("createSlice")))
    }

    fn remove_slice(
        self: Rc<Self>,
        _params: net::RemoveSliceParams,
        _results: net::RemoveSliceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("removeSlice")))
    }

    fn list_slices(
        self: Rc<Self>,
        _params: net::ListSlicesParams,
        _results: net::ListSlicesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("listSlices")))
    }

    fn get_telemetry(
        self: Rc<Self>,
        _params: net::GetTelemetryParams,
        _results: net::GetTelemetryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getTelemetry")))
    }

    fn subscribe_telemetry(
        self: Rc<Self>,
        _params: net::SubscribeTelemetryParams,
        _results: net::SubscribeTelemetryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("subscribeTelemetry")))
    }

    fn pull_telemetry(
        self: Rc<Self>,
        _params: net::PullTelemetryParams,
        _results: net::PullTelemetryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("pullTelemetry")))
    }

    fn ping(
        self: Rc<Self>,
        _params: net::PingParams,
        _results: net::PingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("ping")))
    }

    fn traceroute(
        self: Rc<Self>,
        _params: net::TracerouteParams,
        _results: net::TracerouteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("traceroute")))
    }

    fn set_firewall_rules(
        self: Rc<Self>,
        _params: net::SetFirewallRulesParams,
        _results: net::SetFirewallRulesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setFirewallRules")))
    }

    fn get_firewall_rules(
        self: Rc<Self>,
        _params: net::GetFirewallRulesParams,
        _results: net::GetFirewallRulesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getFirewallRules")))
    }

    fn get_time_sync(
        self: Rc<Self>,
        _params: net::GetTimeSyncParams,
        _results: net::GetTimeSyncResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getTimeSync")))
    }

    fn get_satellite_config(
        self: Rc<Self>,
        _params: net::GetSatelliteConfigParams,
        _results: net::GetSatelliteConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getSatelliteConfig")))
    }

    fn set_hotspot_client_filter(
        self: Rc<Self>,
        _params: net::SetHotspotClientFilterParams,
        _results: net::SetHotspotClientFilterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setHotspotClientFilter")))
    }

    fn get_hotspot_bandwidth_schedule(
        self: Rc<Self>,
        _params: net::GetHotspotBandwidthScheduleParams,
        _results: net::GetHotspotBandwidthScheduleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getHotspotBandwidthSchedule")))
    }

    fn set_hotspot_bandwidth_schedule(
        self: Rc<Self>,
        _params: net::SetHotspotBandwidthScheduleParams,
        _results: net::SetHotspotBandwidthScheduleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setHotspotBandwidthSchedule")))
    }

    fn send_dtn_bundle(
        self: Rc<Self>,
        _params: net::SendDtnBundleParams,
        _results: net::SendDtnBundleResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("sendDtnBundle")))
    }

    fn receive_dtn_bundles(
        self: Rc<Self>,
        _params: net::ReceiveDtnBundlesParams,
        _results: net::ReceiveDtnBundlesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("receiveDtnBundles")))
    }

    fn get_cellular_config(
        self: Rc<Self>,
        _params: net::GetCellularConfigParams,
        _results: net::GetCellularConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getCellularConfig")))
    }

    fn set_cellular_config(
        self: Rc<Self>,
        _params: net::SetCellularConfigParams,
        _results: net::SetCellularConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setCellularConfig")))
    }

    fn get_ipv6_config(
        self: Rc<Self>,
        _params: net::GetIpv6ConfigParams,
        _results: net::GetIpv6ConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("getIpv6Config")))
    }

    fn set_ipv6_config(
        self: Rc<Self>,
        _params: net::SetIpv6ConfigParams,
        _results: net::SetIpv6ConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setIpv6Config")))
    }

    fn wifi_nan_advertise(
        self: Rc<Self>,
        _params: net::WifiNanAdvertiseParams,
        _results: net::WifiNanAdvertiseResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("wifiNanAdvertise")))
    }

    fn wifi_nan_discover(
        self: Rc<Self>,
        _params: net::WifiNanDiscoverParams,
        _results: net::WifiNanDiscoverResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("wifiNanDiscover")))
    }

    fn nic_provider(
        self: Rc<Self>,
        _params: net::NicProviderParams,
        _results: net::NicProviderResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("nicProvider")))
    }

    fn set_routes(
        self: Rc<Self>,
        _params: net::SetRoutesParams,
        _results: net::SetRoutesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setRoutes")))
    }

    fn set_proxy_config(
        self: Rc<Self>,
        _params: net::SetProxyConfigParams,
        _results: net::SetProxyConfigResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(unavailable("setProxyConfig")))
    }
}

/// The network domain, as a served domain: the contract interface it
/// serves, the descriptor discovery publishes for it, and the per-connection
/// server.
pub struct NetworkDomain<B> {
    backend: Arc<B>,
}

impl<B> NetworkDomain<B>
where
    B: NetworkBackend + 'static,
{
    /// The network domain over this backend. The backend is shared: the
    /// same host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for NetworkDomain<B>
where
    B: NetworkBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        // The contract's own interface id, from the generated code.
        net::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::NETWORK_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "network"
    }

    fn category(&self) -> &'static str {
        "network"
    }

    /// The commands this domain publishes through the registry
    /// (cli.capnp). The engine publishes them by NAME; the method's ordinal
    /// and the schema's node tree come from the generated contract table and
    /// the schema tree — never from a literal written here.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "status",
                "Network",
                "getStatus",
                "the network's overall status",
                0,
                true,
            ),
            DomainCommand::new(
                "interfaces",
                "Network",
                "getInterfaces",
                "the host's interfaces",
                1,
                true,
            ),
            DomainCommand::new(
                "networking",
                "Network",
                "isNetworkingEnabled",
                "whether networking is enabled",
                2,
                true,
            ),
            DomainCommand::new(
                "radios",
                "Network",
                "listRadioStates",
                "the radio states",
                3,
                true,
            ),
            DomainCommand::new(
                "wifi scan",
                "Network",
                "wifiScan",
                "scan for wifi networks",
                4,
                true,
            ),
            DomainCommand::new(
                "wifi list",
                "Network",
                "listKnownWifiNetworks",
                "the known wifi networks",
                5,
                true,
            ),
            DomainCommand::new(
                "wifi connect",
                "Network",
                "wifiConnect",
                "connect to a wifi network",
                6,
                true,
            ),
            DomainCommand::new(
                "wifi disconnect",
                "Network",
                "wifiDisconnect",
                "disconnect from a wifi network",
                7,
                true,
            ),
            DomainCommand::new(
                "wifi forget",
                "Network",
                "forgetWifiNetwork",
                "forget a known wifi network",
                8,
                true,
            ),
            DomainCommand::new(
                "dns servers",
                "Network",
                "getDnsServers",
                "the configured DNS servers",
                9,
                true,
            ),
            DomainCommand::new(
                "routes",
                "Network",
                "getRoutes",
                "the routing table",
                10,
                true,
            ),
            DomainCommand::new(
                "firewall rules",
                "Network",
                "getFirewallRules",
                "the firewall rules",
                11,
                true,
            ),
            DomainCommand::new(
                "proxy",
                "Network",
                "getProxyConfig",
                "the proxy configuration",
                12,
                true,
            ),
            DomainCommand::new(
                "vpn",
                "Network",
                "listVpnProfiles",
                "the VPN profiles",
                13,
                true,
            ),
            DomainCommand::new(
                "ping",
                "Network",
                "ping",
                "ping a host",
                14,
                true,
            ),
            DomainCommand::new(
                "traceroute",
                "Network",
                "traceroute",
                "trace a route to a host",
                15,
                true,
            ),
            DomainCommand::new(
                "bandwidth",
                "Network",
                "getCounters",
                "the traffic counters",
                16,
                true,
            ),
        ]
    }

    /// The network schema's node tree: what a client that never compiled
    /// `network.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/network.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = NetworkService::new(Arc::clone(&self.backend), class);
        let client: net::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
