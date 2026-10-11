//! The **network** domain: per-interface status, connect/disconnect, the
//! host-global networking master switch, and radios — over one object-safe
//! `NetworkBackend` trait with 76 contract-faithful methods.
//!
//! What this domain is: the frozen `network.capnp` surface (`getInterfaces`
//! through `listRadioStates @75`) as an honest backend. What the Linux backend
//! actually does: reads NetworkManager over a small, self-contained D-Bus client
//! (no `nmcli`/`ip` shellouts) for the read surface — `isNetworkingEnabled`,
//! `getStatus` with the `hostState` diagnostic, interfaces, radios and counters
//! — and maps the mutating host-global verbs (`setNetworking`, `setRadioEnabled`,
//! the cellular/mobile-data home) onto the NM write methods behind a mockable
//! seam. What is honestly Unavailable, and why: ~61 pre-existing methods are the
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
    DnsServerInfo, DtnBundleInfo, FirewallRuleInfo, HotspotConfigEcho, HotspotStatusInfo,
    HostState, InterfaceCounters, InterfaceSnapshot, IpAddress, Ipv6ConfigInfo, MacAddress,
    NanPeerInfo, NicProviderHandle, Page, Pagination, QosRuleInfo, RadioState,
    SatelliteConfigEcho, SliceConfigEcho, SliceId, SliceInfo, SubscriptionId, TelemetryInfo,
    TimeSyncInfo, TopologyLinkInfo, TopologyNodeInfo, TracerouteHopInfo, VpnConnectionInfo,
    VpnProfileInfo, WifiNetwork, WifiNanConfigEcho,
};
