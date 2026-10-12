//! The network dispatch glue — the template every later domain copies
//! (05 §3: "network is the first domain served, not a special case").
//!
//! ## What one arm looks like
//!
//! ```text
//! frame bytes ──▶ contract params struct ──▶ NetworkBackend ──▶ contract
//!             (capnp read, the          (one method,     result struct
//!              method's own module)      statically       (capnp write,
//!                                        dispatched)       the method's
//!                                                          own module)
//! ```
//!
//! The ordinal is the contract's own, so the match is a **static** match:
//! one monomorphized arm per method, no lookup table on the hot path, no
//! `dyn`. The only allocation a call makes is the response frame's payload —
//! the contract's own result bytes, serialized once.
//!
//! ## The three answers a call can get
//!
//! 1. **The backend's answer** — the servable set (19 methods + the two
//!    subscription pseudo-ordinals). A backend error maps to the frame's
//!    error vocabulary with the contract's own code preserved (`T.Error`
//!    in the payload).
//! 2. **`permissionDenied` before the backend is reached** — the five
//!    mutating verbs on a [`CapabilityClass::ReadOnly`] connection. The
//!    class is the authority gate (law #5): the backend is never called, so
//!    a backend can never be reached through a capability the peer does not
//!    hold. The gate is data-shaped — the verb list lives with the verbs.
//! 3. **`notSupported` from the availability matrix** — everything not in
//!    the servable set reads [`NETWORK_AVAILABILITY`] by ordinal and answers
//!    with the row's own `reasonClass` + reason. Availability is data, not
//!    code: a row that changes changes the wire answer without a line of
//!    glue changing (law #6 — the wire never pretends).
//!
//! ## Subscriptions over the wire
//!
//! The contract's subscription params are capabilities (`StatusSubscription`,
//! `CounterSubscription`), and a capability cannot travel as frame bytes —
//! that is the transport layer's declared
//! [`CapabilityTransfer`](crate::transport::channel::CapabilityTransfer),
//! not a gap in this glue. So the wire path is:
//!
//! * `subscribeStatus @2` / `subscribeCounters @37` mint a server-side
//!   subscription id (the backend's own token) and answer with the **first
//!   event included** — the wire mirror of `Network::status_stream`. The
//!   service remembers which family each minted id belongs to, because the
//!   id is the in-band handle and a pull must know which backend verb it
//!   names.
//! * `SUB_PULL` (0xFFFE) reads the backend's `pull_*` under the caller's
//!   bound. A pull the backend answers `temporary` on is "nothing yet" —
//!   the response is ok with an **empty payload**; an empty payload is never
//!   a fabricated event.
//! * `SUB_CANCEL` (0xFFFF) retires the token through `cancel_subscription`.
//!
//! Drops stay where they already are: the backend surface counts what a
//! bounded pull collapsed (`LinuxNetworkBackend::dropped_updates`, proven by
//! `l3_linux_network::subscription_pulls_count_the_updates_they_collapse`).
//! This layer does not add a second drop counter — that would be a second
//! wire vocabulary.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::NetworkInterfaceId;
use crate::core::values::Duration;
use crate::core::{Error, ErrorCode};
use crate::domains::network::availability::{
    AvailStatus, NETWORK_AVAILABILITY, NETWORK_SCHEMA_ID, ReasonClass,
};
use crate::domains::network::backend::NetworkBackend;
use crate::domains::network::types::{
    ConnStatus, InterfaceCounters, InterfaceSnapshot, KnownWifiNetwork, Page, Pagination,
    ProxyConfigInfo, RadioState, RouteInfo, SubscriptionId,
};
use crate::schema::types_capnp;
/// The `Network` interface's generated method modules: `<method>_params`
/// and `<method>_results`, one pair per contract method.
use crate::schema::network_capnp::network as net;

use super::frame::{self, Response};
use super::{CapabilityClass, ServiceDispatch};

/// The network service's identity for the harness: the contract's own
/// compiled schema id plus the matrix this service answers unavailable from.
/// A unit struct — there is no per-service state, the connection loop owns
/// the state that exists.
pub struct NetworkService;

impl ServiceDispatch for NetworkService {
    fn schema_id() -> u64 {
        NETWORK_SCHEMA_ID
    }

    fn availability() -> &'static [crate::domains::network::availability::MethodAvailability] {
        NETWORK_AVAILABILITY
    }
}

/// Which family a minted subscription id belongs to — the in-band half of
/// the contract's capability handle (law #5: the id is the handle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionFamily {
    /// Minted by `subscribeStatus @2`, pulled through `pull_status`.
    Status,
    /// Minted by `subscribeCounters @37`, pulled through `pull_counters`.
    Counters,
}

/// Per-connection subscription bookkeeping: the ids this service minted and
/// the family each belongs to. Bounded by the connection's own subscriptions;
/// a token retired through `SUB_CANCEL` leaves the map.
#[derive(Debug, Default)]
pub struct Subscriptions {
    families: BTreeMap<u64, SubscriptionFamily>,
}

impl Subscriptions {
    /// An empty book (one per served connection).
    pub fn new() -> Self {
        Subscriptions {
            families: BTreeMap::new(),
        }
    }

    /// Record a freshly minted subscription.
    pub fn mint(&mut self, subscription: SubscriptionId, family: SubscriptionFamily) {
        self.families.insert(subscription.0, family);
    }

    /// The family a minted id belongs to, or `None` when this service never
    /// minted it (or it was cancelled) — an honest `notFound`, never a guess.
    pub fn family(&self, subscription: SubscriptionId) -> Option<SubscriptionFamily> {
        self.families.get(&subscription.0).copied()
    }

    /// Retire a token's bookkeeping.
    pub fn retire(&mut self, subscription: SubscriptionId) {
        self.families.remove(&subscription.0);
    }
}

/// Dispatch one request frame and produce the response frame. Total: every
/// input — a malformed frame, a foreign interface id, a contract ordinal
/// inside or outside the matrix, a pseudo-ordinal — produces exactly one
/// typed answer.
pub fn respond<B: NetworkBackend>(
    backend: &B,
    subscriptions: &mut Subscriptions,
    class: CapabilityClass,
    bytes: &[u8],
) -> Response {
    let request = match frame::parse_request(bytes) {
        Ok(request) => request,
        Err(error) => return Response::error(error.to_string()),
    };
    if request.interface_id != NetworkService::schema_id() {
        return Response::error(format!(
            "unknown-interface: {:#x} is not the interface this service serves \
             (it serves {:#x})",
            request.interface_id,
            NetworkService::schema_id()
        ));
    }
    match request.ordinal {
        // ── reads ───────────────────────────────────────────────────────
        0 => method_get_interfaces::<B>(backend, request.params),
        1 => method_get_status::<B>(backend, request.params),
        12 => method_is_mobile_data_enabled::<B>(backend),
        14 => method_is_data_roaming_enabled::<B>(backend),
        35 => method_get_counters::<B>(backend, request.params),
        73 => method_is_networking_enabled::<B>(backend),
        75 => method_list_radio_states::<B>(backend, request.params),
        78 => method_list_known_wifi_networks::<B>(backend, request.params),
        79 => method_get_routes::<B>(backend, request.params),
        81 => method_get_proxy_config::<B>(backend),

        // ── subscriptions ───────────────────────────────────────────────
        2 => method_subscribe_status::<B>(backend, subscriptions, request.params),
        37 => method_subscribe_counters::<B>(backend, subscriptions, request.params),
        frame::SUB_PULL => method_pull::<B>(backend, subscriptions, request.params),
        frame::SUB_CANCEL => method_cancel::<B>(backend, subscriptions, request.params),

        // ── the mutating verbs, gated by the connection's class ──────────
        11 => gate(class, "setMobileData", || {
            method_set_mobile_data::<B>(backend, request.params)
        }),
        72 => gate(class, "setNetworking", || {
            method_set_networking_enabled::<B>(backend, request.params)
        }),
        74 => gate(class, "setRadioEnabled", || {
            method_set_radio_enabled::<B>(backend, request.params)
        }),
        76 => gate(class, "setInterfaceMtu", || {
            method_set_interface_mtu::<B>(backend, request.params)
        }),
        77 => gate(class, "forgetWifiNetwork", || {
            method_forget_wifi_network::<B>(backend, request.params)
        }),

        // ── the contract's pull ordinals take a capability param ─────────
        3 | 38 => capability_pull_ordinal(request.ordinal),

        other => unavailable(other),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The capability gate (law #5: the class is the authority, checked first)
// ═══════════════════════════════════════════════════════════════════════

/// Run `verb` only on a control connection; a read-only connection is
/// refused **before** `then` is ever evaluated, so the backend is not
/// reached. The refusal is the contract's own vocabulary: `permissionDenied`
/// in the message prefix and in the payload's code.
fn gate<F>(class: CapabilityClass, verb: &str, then: F) -> Response
where
    F: FnOnce() -> Response,
{
    if class.is_control() {
        then()
    } else {
        denied(verb)
    }
}

/// The refusal answer for a mutating verb on a read-only connection.
fn denied(verb: &str) -> Response {
    let message = format!(
        "permissionDenied: {verb} is a control verb; this connection's capability \
         class is ReadOnly (uid 0 or CAP_NET_ADMIN grants Control)"
    );
    Response::error_with_payload(
        message.clone(),
        encode_t_error(ErrorCode::PermissionDenied, &message, false),
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Reads (@0, @1, @12, @14, @35, @73, @75, @78, @79, @81)
// ═══════════════════════════════════════════════════════════════════════

/// `getInterfaces @0` — the kernel link table, paginated.
fn method_get_interfaces<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::get_interfaces_params::Owned, _>(params, |params| {
        let Ok(pagination) = params.get_pagination() else {
            return missing_params("getInterfaces", "pagination");
        };
        match backend.get_interfaces(Pagination {
            offset: pagination.get_offset(),
            limit: pagination.get_limit(),
        }) {
            Ok(page) => Response::ok(encode_interfaces(&page)),
            Err(error) => error_response(&error),
        }
    })
}

/// `getStatus @1` — per-interface status, `hostState` included.
fn method_get_status<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::get_status_params::Owned, _>(params, |params| {
        let Ok(interface_id) = params.get_interface_id() else {
            return missing_params("getStatus", "interfaceId");
        };
        match backend.get_status(NetworkInterfaceId::new(interface_id.get_id())) {
            Ok(status) => Response::ok(encode_status(&status)),
            Err(error) => error_response(&error),
        }
    })
}

/// `isMobileDataEnabled @12`.
fn method_is_mobile_data_enabled<B: NetworkBackend>(backend: &B) -> Response {
    match backend.is_mobile_data_enabled() {
        Ok(enabled) => Response::ok(encode_is_mobile_data_enabled(enabled)),
        Err(error) => error_response(&error),
    }
}

/// `isDataRoamingEnabled @14`.
fn method_is_data_roaming_enabled<B: NetworkBackend>(backend: &B) -> Response {
    match backend.is_data_roaming_enabled() {
        Ok(enabled) => Response::ok(encode_is_data_roaming_enabled(enabled)),
        Err(error) => error_response(&error),
    }
}

/// `getCounters @35` — the kernel's own interface counters for a window
/// ending now.
fn method_get_counters<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::get_counters_params::Owned, _>(params, |params| {
        let Ok(interface_id) = params.get_interface_id() else {
            return missing_params("getCounters", "interfaceId");
        };
        let Ok(window) = params.get_window() else {
            return missing_params("getCounters", "window");
        };
        let interface = NetworkInterfaceId::new(interface_id.get_id());
        match backend.get_counters(interface, Duration::from_nanos(window.get_nanoseconds())) {
            Ok(counters) => Response::ok(encode_counters(&counters)),
            Err(error) => error_response(&error),
        }
    })
}

/// `isNetworkingEnabled @73` — the master switch.
fn method_is_networking_enabled<B: NetworkBackend>(backend: &B) -> Response {
    match backend.is_networking_enabled() {
        Ok(enabled) => Response::ok(encode_is_networking_enabled(enabled)),
        Err(error) => error_response(&error),
    }
}

/// `listRadioStates @75` — the observable radios, `hardBlocked` and
/// `softBlocked` reported separately from `enabled`.
fn method_list_radio_states<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::list_radio_states_params::Owned, _>(params, |params| {
        let Ok(pagination) = params.get_pagination() else {
            return missing_params("listRadioStates", "pagination");
        };
        match backend.list_radio_states(Pagination {
            offset: pagination.get_offset(),
            limit: pagination.get_limit(),
        }) {
            Ok(page) => Response::ok(encode_radio_states(&page)),
            Err(error) => error_response(&error),
        }
    })
}

/// `listKnownWifiNetworks @78` — the remembered-set read. Carries no
/// credential: the contract's own `KnownWifiNetwork` has no field that could
/// hold one.
fn method_list_known_wifi_networks<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::list_known_wifi_networks_params::Owned, _>(params, |params| {
        let Ok(pagination) = params.get_pagination() else {
            return missing_params("listKnownWifiNetworks", "pagination");
        };
        match backend.list_known_wifi_networks(Pagination {
            offset: pagination.get_offset(),
            limit: pagination.get_limit(),
        }) {
            Ok(page) => Response::ok(encode_known_networks(&page)),
            Err(error) => error_response(&error),
        }
    })
}

/// `getRoutes @79` — the kernel's routing table.
fn method_get_routes<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::get_routes_params::Owned, _>(params, |params| {
        let Ok(pagination) = params.get_pagination() else {
            return missing_params("getRoutes", "pagination");
        };
        match backend.get_routes(Pagination {
            offset: pagination.get_offset(),
            limit: pagination.get_limit(),
        }) {
            Ok(page) => Response::ok(encode_routes(&page)),
            Err(error) => error_response(&error),
        }
    })
}

/// `getProxyConfig @81` — the host's own proxy, file-level truth.
fn method_get_proxy_config<B: NetworkBackend>(backend: &B) -> Response {
    match backend.get_proxy_config() {
        Ok(config) => Response::ok(encode_proxy_config(&config)),
        Err(error) => error_response(&error),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Subscriptions (@2, @37, SUB_PULL, SUB_CANCEL)
// ═══════════════════════════════════════════════════════════════════════

/// `subscribeStatus @2` — mint a subscription and answer with the first
/// event included (the wire mirror of `Network::status_stream`).
fn method_subscribe_status<B: NetworkBackend>(
    backend: &B,
    subscriptions: &mut Subscriptions,
    params: &[u8],
) -> Response {
    with_params::<net::subscribe_status_params::Owned, _>(params, |params| {
        let Ok(interface_id) = params.get_interface_id() else {
            return missing_params("subscribeStatus", "interfaceId");
        };
        let interface = NetworkInterfaceId::new(interface_id.get_id());
        let subscription = match backend.subscribe_status(interface) {
            Ok(subscription) => subscription,
            Err(error) => return error_response(&error),
        };
        subscriptions.mint(subscription, SubscriptionFamily::Status);
        match backend.pull_status(subscription) {
            // The first event rides the subscribe answer, so the caller does
            // not pay a second round trip to see the current fact.
            Ok(status) => Response::ok(frame::payload_with_sub_id(
                subscription.0,
                &encode_status(&status),
            )),
            Err(error) => {
                // A minted token whose first pull failed must not survive as
                // a phantom: retire it, then answer the pull's own error.
                let _ = backend.cancel_subscription(subscription);
                subscriptions.retire(subscription);
                error_response(&error)
            }
        }
    })
}

/// `subscribeCounters @37` — the counters subscription, first event included.
fn method_subscribe_counters<B: NetworkBackend>(
    backend: &B,
    subscriptions: &mut Subscriptions,
    params: &[u8],
) -> Response {
    with_params::<net::subscribe_counters_params::Owned, _>(params, |params| {
        let Ok(interface_id) = params.get_interface_id() else {
            return missing_params("subscribeCounters", "interfaceId");
        };
        let interface = NetworkInterfaceId::new(interface_id.get_id());
        let subscription = match backend.subscribe_counters(interface) {
            Ok(subscription) => subscription,
            Err(error) => return error_response(&error),
        };
        subscriptions.mint(subscription, SubscriptionFamily::Counters);
        match backend.pull_counters(subscription) {
            Ok(counters) => Response::ok(frame::payload_with_sub_id(
                subscription.0,
                &encode_counters(&counters),
            )),
            Err(error) => {
                let _ = backend.cancel_subscription(subscription);
                subscriptions.retire(subscription);
                error_response(&error)
            }
        }
    })
}

/// `SUB_PULL` (0xFFFE) — read one subscription's next event under the
/// caller's bound. `params` are `[subId u64][limit u32]`.
fn method_pull<B: NetworkBackend>(
    backend: &B,
    subscriptions: &Subscriptions,
    params: &[u8],
) -> Response {
    let Ok(pull) = frame::parse_pull_params(params) else {
        return Response::error(format!(
            "bad-frame: SUB_PULL params are [subId u64][limit u32] = 12 bytes; got {}",
            params.len()
        ));
    };
    let subscription = SubscriptionId(pull.sub_id);
    let Some(family) = subscriptions.family(subscription) else {
        let message = format!(
            "notFound: subscription {} was not minted by this connection (or was cancelled)",
            pull.sub_id
        );
        return Response::error_with_payload(
            message.clone(),
            encode_t_error(ErrorCode::NotFound, &message, false),
        );
    };
    // `limit` caps the events this response may carry; 0 means one (the
    // contract's own pagination law, repeated: a zero bound is never a zero
    // answer). The backend's pull is the real bound — it collapses every
    // state since the last delivered one into the current answer and counts
    // what it collapsed (`dropped_updates`, proven by its l3 test) — so
    // today's pull model serves one event and the cap is honored by it.
    let _wanted = pull.limit.max(1);
    let event = match family {
        SubscriptionFamily::Status => backend
            .pull_status(subscription)
            .map(|status| encode_status(&status)),
        SubscriptionFamily::Counters => backend
            .pull_counters(subscription)
            .map(|counters| encode_counters(&counters)),
    };
    match event {
        Ok(bytes) => Response::ok(frame::payload_with_sub_id(pull.sub_id, &bytes)),
        // `temporary` from a pull is the backend's "nothing yet": an ok
        // response with an EMPTY payload. An empty payload is never a
        // fabricated event.
        Err(error) if error.code == ErrorCode::Temporary => Response::ok(Vec::new()),
        Err(error) => error_response(&error),
    }
}

/// `SUB_CANCEL` (0xFFFF) — retire a minted subscription.
fn method_cancel<B: NetworkBackend>(
    backend: &B,
    subscriptions: &mut Subscriptions,
    params: &[u8],
) -> Response {
    let Ok(sub_id) = frame::parse_cancel_params(params) else {
        return Response::error(format!(
            "bad-frame: SUB_CANCEL params are [subId u64] = 8 bytes; got {}",
            params.len()
        ));
    };
    let subscription = SubscriptionId(sub_id);
    if subscriptions.family(subscription).is_none() {
        let message = format!(
            "notFound: subscription {sub_id} was not minted by this connection (or was cancelled)"
        );
        return Response::error_with_payload(
            message.clone(),
            encode_t_error(ErrorCode::NotFound, &message, false),
        );
    }
    match backend.cancel_subscription(subscription) {
        Ok(()) => {
            subscriptions.retire(subscription);
            Response::ok(encode_void_ok())
        }
        Err(error) => error_response(&error),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The mutating verbs (@11, @72, @74, @76, @77) — control class only
// ═══════════════════════════════════════════════════════════════════════

/// `setMobileData @11`.
fn method_set_mobile_data<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::set_mobile_data_params::Owned, _>(params, |params| {
        void_result(backend.set_mobile_data_enabled(params.get_enabled()))
    })
}

/// `setNetworking @72` — the master switch. Writes host state; the live
/// bring-back proof is founder-run (law #8).
fn method_set_networking_enabled<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::set_networking_params::Owned, _>(params, |params| {
        void_result(backend.set_networking_enabled(params.get_enabled()))
    })
}

/// `setRadioEnabled @74` — an open radio token ("wifi", "cellular", ...).
fn method_set_radio_enabled<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::set_radio_enabled_params::Owned, _>(params, |params| {
        let Ok(radio) = params.get_radio() else {
            return missing_params("setRadioEnabled", "radio");
        };
        let radio = text_of(radio);
        void_result(backend.set_radio_enabled(&radio, params.get_enabled()))
    })
}

/// `setInterfaceMtu @76` — the MTU attribute is the kernel's own; the write
/// half is a control verb, the read half (`getInterfaces`' `mtU`) is a
/// kernel fact.
fn method_set_interface_mtu<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::set_interface_mtu_params::Owned, _>(params, |params| {
        let Ok(interface_id) = params.get_interface_id() else {
            return missing_params("setInterfaceMtu", "interfaceId");
        };
        let interface = NetworkInterfaceId::new(interface_id.get_id());
        void_result(backend.set_interface_mtu(interface, params.get_mtu()))
    })
}

/// `forgetWifiNetwork @77` — drops the remembered network; carries no
/// credential, because the contract's params carry none.
fn method_forget_wifi_network<B: NetworkBackend>(backend: &B, params: &[u8]) -> Response {
    with_params::<net::forget_wifi_network_params::Owned, _>(params, |params| {
        let Ok(ssid) = params.get_ssid() else {
            return missing_params("forgetWifiNetwork", "ssid");
        };
        let ssid = text_of(ssid);
        void_result(backend.forget_wifi_network(&ssid))
    })
}

/// A `T.VoidResult` method's answer: success rides the void result's
/// `success` arm, failure rides the frame's error channel with the
/// contract's own `T.Error` payload.
fn void_result(result: crate::core::Result<()>) -> Response {
    match result {
        Ok(()) => Response::ok(encode_void_ok()),
        Err(error) => error_response(&error),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The answers that are not the backend's
// ═══════════════════════════════════════════════════════════════════════

/// The availability answer for every ordinal the servable match does not
/// claim: the matrix row's own reasonClass + reason, read as data.
fn unavailable(ordinal: u16) -> Response {
    let row = NetworkService::availability()
        .iter()
        .find(|row| row.ordinal == ordinal);
    match row {
        Some(row) if matches!(row.status, AvailStatus::Unavailable) => {
            let class = row
                .reason_class
                .unwrap_or(ReasonClass::Unknown)
                .as_str();
            not_supported_response(class, row.reason)
        }
        // A REAL/DEGRADED row the wire does not serve (the two capability
        // pulls) is handled in the match before this arm; anything else here
        // is a caller naming an ordinal no row covers.
        Some(row) => {
            let message = format!(
                "invalidArgument: ordinal @{ordinal} ({}) is not served over this wire",
                row.name
            );
            Response::error_with_payload(
                message.clone(),
                encode_t_error(ErrorCode::InvalidArgument, &message, false),
            )
        }
        None => {
            let message =
                format!("invalidArgument: ordinal @{ordinal} is not a Network contract method");
            Response::error_with_payload(
                message.clone(),
                encode_t_error(ErrorCode::InvalidArgument, &message, false),
            )
        }
    }
}

/// The honest answer for the contract's pull ordinals: their parameter is a
/// capability (`StatusSubscription` / `CounterSubscription`), and a
/// capability cannot travel as frame bytes — so the wire's pulls are the
/// pseudo-ordinals. The answer names the supported path instead of
/// pretending the capability crossed.
fn capability_pull_ordinal(ordinal: u16) -> Response {
    let (verb, ordinal_name) = match ordinal {
        3 => ("pullStatus", "@3"),
        38 => ("pullCounters", "@38"),
        _ => ("pull", ""),
    };
    let message = format!(
        "invalidArgument: {verb} {ordinal_name} takes a subscription capability, which cannot \
         travel as frame bytes; over this wire, pulls ride SUB_PULL (0xFFFE) with the \
         service-minted subscription id"
    );
    Response::error_with_payload(
        message.clone(),
        encode_t_error(ErrorCode::InvalidArgument, &message, false),
    )
}

/// The `notSupported` answer the matrix prescribes: the row's reasonClass
/// token prefixes the row's reason, in the message and in the payload.
fn not_supported_response(class: &str, reason: &str) -> Response {
    let message = format!("{class}: {reason}");
    Response::error_with_payload(
        message.clone(),
        encode_t_error(ErrorCode::NotSupported, &message, false),
    )
}

/// A params struct arrived without a field the method requires. This is the
/// caller's protocol violation, answered with the contract's own code.
fn missing_params(method: &str, field: &str) -> Response {
    let message =
        format!("invalidArgument: {method} requires the {field} field in its params struct");
    Response::error_with_payload(
        message.clone(),
        encode_t_error(ErrorCode::InvalidArgument, &message, false),
    )
}

/// Map a backend error onto the frame's error channel: the error's own
/// message (which already begins with its reasonClass token, per the
/// backend's convention) and the contract's own `T.Error` payload with the
/// code preserved exactly (05 §3: the same total mapping core tests, now
/// exercised end-to-end).
fn error_response(error: &Error) -> Response {
    let message = error.to_string();
    Response::error_with_payload(
        message.clone(),
        encode_t_error(error.code, &message, error.retryable),
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Encoders: the contract's own result structs, serialized once
// ═══════════════════════════════════════════════════════════════════════

/// Serialize one filled message. The one allocation a dispatch makes beyond
/// the frame itself.
fn encode(message: capnp::message::Builder<capnp::message::HeapAllocator>) -> Vec<u8> {
    let mut out = Vec::new();
    capnp::serialize::write_message(&mut out, &message)
        .expect("serializing a capnp message into a Vec cannot fail");
    out
}

/// `getInterfaces`' result: the page of `NetworkInterface` structs plus the
/// stable total. `InterfaceSnapshot.managed` has no field in the contract's
/// `NetworkInterface`, so it is not invented here — the contract's own shape
/// is the wire.
fn encode_interfaces(page: &Page<InterfaceSnapshot>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::get_interfaces_results::Builder<'_>>();
        let mut list = results.reborrow().init_interfaces(page.items.len() as u32);
        for (index, item) in page.items.iter().enumerate() {
            let mut builder = list.reborrow().get(index as u32);
            let mut id = builder.reborrow().init_id();
            id.set_id(item.id.get());
            builder.set_name(&item.name);
            builder.set_type(&item.kind);
            let mut hw = builder.reborrow().init_hw_address();
            hw.set_address(&item.hardware_address.0);
            builder.set_up(item.up);
            builder.set_connected(item.connected);
            builder.set_mt_u(item.mtu);
        }
        // The contract's `getInterfaces` result carries no `total`: the
        // domain's page total has no wire home here, and it is not invented.
    }
    encode(message)
}

/// `getStatus`' result: the `InterfaceConnectionStatus` the domain type
/// already knows how to write.
fn encode_status(status: &ConnStatus) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::get_status_results::Builder<'_>>();
        let mut builder = results.reborrow().init_status();
        status.write_to(&mut builder);
    }
    encode(message)
}

/// `getCounters`' result.
fn encode_counters(counters: &InterfaceCounters) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::get_counters_results::Builder<'_>>();
        let mut builder = results.reborrow().init_counters();
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
    encode(message)
}

/// `listRadioStates`' result.
fn encode_radio_states(page: &Page<RadioState>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::list_radio_states_results::Builder<'_>>();
        let mut list = results.reborrow().init_radios(page.items.len() as u32);
        for (index, item) in page.items.iter().enumerate() {
            let mut builder = list.reborrow().get(index as u32);
            item.write_to(&mut builder);
        }
        results.set_total(page.total);
    }
    encode(message)
}

/// `listKnownWifiNetworks`' result — and the proof it carries no credential:
/// every field written is one the contract's `KnownWifiNetwork` declares.
fn encode_known_networks(page: &Page<KnownWifiNetwork>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::list_known_wifi_networks_results::Builder<'_>>();
        let mut list = results.reborrow().init_networks(page.items.len() as u32);
        for (index, item) in page.items.iter().enumerate() {
            let mut builder = list.reborrow().get(index as u32);
            item.write_to(&mut builder);
        }
        results.set_total(page.total);
    }
    encode(message)
}

/// `getRoutes`' result.
fn encode_routes(page: &Page<RouteInfo>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::get_routes_results::Builder<'_>>();
        let mut list = results.reborrow().init_routes(page.items.len() as u32);
        for (index, item) in page.items.iter().enumerate() {
            let mut builder = list.reborrow().get(index as u32);
            item.write_to(&mut builder);
        }
        results.set_total(page.total);
    }
    encode(message)
}

/// `getProxyConfig`' result, the bypass list's order and duplicates intact.
fn encode_proxy_config(config: &ProxyConfigInfo) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::get_proxy_config_results::Builder<'_>>();
        let mut builder = results.reborrow().init_config();
        config.write_to(&mut builder);
    }
    encode(message)
}

/// `isMobileDataEnabled`' result.
fn encode_is_mobile_data_enabled(enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::is_mobile_data_enabled_results::Builder<'_>>();
        results.set_enabled(enabled);
    }
    encode(message)
}

/// `isDataRoamingEnabled`' result.
fn encode_is_data_roaming_enabled(enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::is_data_roaming_enabled_results::Builder<'_>>();
        results.set_enabled(enabled);
    }
    encode(message)
}

/// `isNetworkingEnabled`' result.
fn encode_is_networking_enabled(enabled: bool) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut results = message.init_root::<net::is_networking_enabled_results::Builder<'_>>();
        results.set_enabled(enabled);
    }
    encode(message)
}

/// A mutating verb's success answer: the contract's own `T.VoidResult` with
/// its `success` arm set.
fn encode_void_ok() -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    {
        let mut result = message.init_root::<types_capnp::void_result::Builder<'_>>();
        result.set_success(());
    }
    encode(message)
}

/// Serialize the contract's `T.Error` for an error response's payload: the
/// same code, the same message, the honest retryable flag.
fn encode_t_error(code: ErrorCode, message: &str, retryable: bool) -> Vec<u8> {
    let mut message_builder = capnp::message::Builder::new_default();
    {
        let mut error = message_builder.init_root::<types_capnp::error::Builder<'_>>();
        error.set_code(code.into());
        error.set_message(message);
        error.set_retryable(retryable);
    }
    encode(message_builder)
}

// ═══════════════════════════════════════════════════════════════════════
// Decoder: the contract's own params structs, read once
// ═══════════════════════════════════════════════════════════════════════

/// Read a request's params bytes as the contract's param struct `T` and hand
/// the reader to `f`. The message lives exactly as long as the call — the
/// reader never escapes — and the caller's answer must be owned.
fn with_params<T, F>(bytes: &[u8], f: F) -> Response
where
    T: capnp::traits::Owned,
    F: FnOnce(&T::Reader<'_>) -> Response,
{
    let message = match capnp::serialize::read_message(
        &mut &bytes[..],
        capnp::message::ReaderOptions::new(),
    ) {
        Ok(message) => message,
        Err(error) => return error_response(&Error::from(error)),
    };
    let params = match message.get_root::<T::Reader<'_>>() {
        Ok(params) => params,
        Err(error) => return error_response(&Error::from(error)),
    };
    f(&params)
}

/// Read a contract `Text` field lossily into an owned string (a peer's
/// invalid UTF-8 is a protocol violation message validation should catch; it
/// reads as replacement characters here rather than fabricating a value).
fn text_of(text: capnp::text::Reader<'_>) -> String {
    String::from_utf8_lossy(text.as_bytes()).into_owned()
}
