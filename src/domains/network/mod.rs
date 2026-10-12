//! The **network** domain: per-interface status, connect/disconnect, the
//! host-global networking master switch, radios, the routing table, and the
//! known-network / proxy bookkeeping — over one object-safe `NetworkBackend`
//! trait with 83 contract-faithful methods (`@0`–`@82`).
//!
//! What this domain is: the frozen `network.capnp` surface (`getInterfaces`
//! through `setProxyConfig @82`) as an honest backend.
//!
//! ## The transport hierarchy on this backend (law #9)
//!
//! What the Linux backend actually does: reads the **kernel first** — links,
//! addresses, routes, counters, carrier — over netlink
//! (`platforms::linux::netlink`'s `NetlinkReader` seam), event-fed into a local
//! state store, so a status/interfaces/counters/routes read costs no daemon
//! hop. NetworkManager is used for what the kernel does not own (the
//! host-global networking switch, radio policy, per-device management, the
//! remembered-network profiles) and for the control (mutating) verbs only NM
//! exposes — over the `NmClient` seam, which is policy + control only. The
//! D-Bus path is a **counted fallback**: a read that misses the store issues
//! exactly one fallback sweep, every subsequent read is local. No `nmcli`/`ip`
//! shellouts on either path.
//! What is honestly Unavailable, and why: the subsystem/privileged surface
//! (VPN, hotspot, DHCP, QoS, firewall, slicing, DTN, NAN, the NIC driver, and
//! the three mutating route/proxy/MTU writes) returns `notSupported` with a
//! nameable reason. The domain exists because a switched-off host was once
//! indistinguishable from a dead interface; `hostState` is how this backend
//! says why a device is silent.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Network;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    NETWORK_AVAILABILITY, NETWORK_SCHEMA_ID, ReasonClass,
};
pub use backend::NetworkBackend;
pub use types::{
    firewall_action_from_token, firewall_action_token, AddressAssignmentInfo,
    AddressAssignmentOptionsEcho, AddressAssignmentOutcome, BandwidthSlotInfo, CaptivePortalInfo,
    CellularConfigInfo, ConnectConfig, ConnStatus, DnsServerInfo, DtnBundleInfo, FirewallRuleInfo,
    HostState, HostStateSource, HotspotConfigEcho, HotspotStatusInfo, InterfaceCounters,
    InterfaceSnapshot, IpAddress, Ipv6ConfigInfo, KnownWifiNetwork, MacAddress, NanPeerInfo,
    NicProviderHandle, Page, Pagination, ProxyConfigInfo, QosRuleInfo, RadioState, RouteInfo,
    SatelliteConfigEcho, SliceConfigEcho, SliceId, SliceInfo, SubscriptionId, TelemetryInfo,
    TimeSyncInfo, TopologyLinkInfo, TopologyNodeInfo, TracerouteHopInfo, VpnConnectionInfo,
    VpnProfileInfo, WifiNetwork, WifiNanConfigEcho,
};
