//! The **network** domain: per-interface status, connect/disconnect, the
//! host-global networking master switch, and radios — over one object-safe
//! `NetworkBackend` trait with 76 contract-faithful methods.
//!
//! What this domain is: the frozen `network.capnp` surface (`getInterfaces`
//! through `listRadioStates @75`) as an honest backend.
//!
//! ## The transport hierarchy on this backend (law #9)
//!
//! What the Linux backend actually does: reads the **kernel first** — links,
//! addresses, routes, counters, carrier — over netlink
//! (`platforms::linux::netlink`'s `NetlinkReader` seam), event-fed into a local
//! state store, so a status/interfaces/counters read costs no daemon hop.
//! NetworkManager is used for what the kernel does not own (the host-global
//! networking switch, radio policy, per-device management, connection profiles)
//! and for the control (mutating) verbs only NM exposes — over the `NmClient`
//! seam, which is now policy + control only. The D-Bus path is a **counted
//! fallback**: a read that misses the store issues exactly one fallback sweep,
//! every subsequent read is local. No `nmcli`/`ip` shellouts on either path.
//! What is honestly Unavailable, and why: ~61 pre-existing methods are the
//! subsystem/privileged surface (VPN, hotspot, DHCP, QoS, firewall, slicing,
//! DTN, NAN, the NIC driver) that Linux needs real services or `CAP_NET_ADMIN`
//! for; each returns `notSupported` with a nameable reason. The domain exists
//! because a switched-off host was once indistinguishable from a dead interface;
//! `hostState` is how this backend says why a device is silent.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Network;
pub use availability::{
    claimed_l3_tests, status_counts, AvailStatus, MethodAvailability, NETWORK_AVAILABILITY,
    NETWORK_SCHEMA_ID, ReasonClass,
};
pub use backend::NetworkBackend;
pub use types::{
    AddressAssignmentInfo, AddressAssignmentOptionsEcho, AddressAssignmentOutcome,
    BandwidthSlotInfo, CaptivePortalInfo, CellularConfigInfo, ConnectConfig, ConnStatus,
    DnsServerInfo, DtnBundleInfo, FirewallRuleInfo, HostState, HostStateSource, HotspotConfigEcho,
    HotspotStatusInfo, InterfaceCounters, InterfaceSnapshot, IpAddress, Ipv6ConfigInfo, MacAddress,
    NanPeerInfo, NicProviderHandle, Page, Pagination, QosRuleInfo, RadioState,
    SatelliteConfigEcho, SliceConfigEcho, SliceId, SliceInfo, SubscriptionId, TelemetryInfo,
    TimeSyncInfo, TopologyLinkInfo, TopologyNodeInfo, TracerouteHopInfo, VpnConnectionInfo,
    VpnProfileInfo, WifiNetwork, WifiNanConfigEcho,
};
