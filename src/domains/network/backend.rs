//! The `NetworkBackend` trait: one object-safe method per `Network` contract
//! method (all 76, `@0`–`@75`), each returning [`crate::core::Result`].
//!
//! ## Capability story (law #5)
//!
//! There is **no permission string on any method**. Authority is the *held
//! service reference*: a caller either holds a `Network` capability (the
//! host-injected root, or one minted at acquisition) and may call the whole
//! surface, or it does not and every call is refused before a backend is
//! reached. No method here takes a `&Cap<_>`; the single capability-shaped
//! accessor, `nicProvider @71`, is the contract's *acquisition* point — this
//! backend maps it to the NetworkManager D-Bus path it already holds (there is
//! no second acquisition), and returns `Unavailable` because the NIC driver
//! surface is not built in this milestone.
//!
//! ## Object safety
//!
//! Every method borrows `&self` and returns owned values, so the trait is
//! object-safe and dispatchable as `dyn NetworkBackend` at the transport seam.
//! `NetworkManager`-specific mutating methods (`set_networking_enabled`,
//! `set_radio_enabled`, domestic data switches) are implemented over a
//! *mockable host seam*; no host-mutating path runs during build or test.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::ids::SecretKeyId;
use crate::core::values::Duration;
use crate::core::Result;
use crate::core::Error;
use crate::core::ids::{NetworkInterfaceId, VpnConnectionId, VpnProfileId};

use super::types::{
    AddressAssignmentInfo, AddressAssignmentOptionsEcho, AddressAssignmentOutcome,
    BandwidthSlotInfo, CaptivePortalInfo, CellularConfigInfo, ConnectConfig, ConnStatus,
    DnsServerInfo, DtnBundleInfo, FirewallRuleInfo, HotspotConfigEcho, HotspotStatusInfo,
    InterfaceCounters, InterfaceSnapshot, IpAddress, Ipv6ConfigInfo, KnownWifiNetwork, NanPeerInfo,
    NicProviderHandle, Pagination, Page, ProxyConfigInfo, QosRuleInfo, RadioState, RouteInfo,
    SatelliteConfigEcho, SliceConfigEcho, SliceId, SliceInfo, SubscriptionId, TelemetryInfo,
    TimeSyncInfo, TopologyLinkInfo, TopologyNodeInfo, TracerouteHopInfo, VpnConnectionInfo,
    VpnProfileInfo, WifiNetwork, WifiNanConfigEcho,
};

/// A protocol-compliant network backend: one method per contract method, with
/// honest `Unavailable`-with-reason for everything the platform cannot serve.
pub trait NetworkBackend {
    // ─────────────────────────────────────────────────────────────────────
    // Implemented surface — the Linux (NetworkManager) backend overrides these.
    // ─────────────────────────────────────────────────────────────────────

    /// Enumerate interfaces (`getInterfaces @0`). `total` is stable across two
    /// identical calls and never exceeds the enumerated device count.
    fn get_interfaces(&self, page: Pagination) -> Result<Page<InterfaceSnapshot>>;

    /// Per-interface status (`getStatus @1`), including the `hostState` token
    /// that distinguishes *networking-disabled* / *radio-off* / *no-firmware* /
    /// *carrier-down* — the outage as a first-class answer.
    fn get_status(&self, interface: NetworkInterfaceId) -> Result<ConnStatus>;

    /// Mint a status subscription handle (`subscribeStatus @2`).
    fn subscribe_status(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId>;

    /// Pull the next status for a subscription (`pullStatus @3`), bounded by
    /// the caller's decision to stop.
    fn pull_status(&self, subscription: SubscriptionId) -> Result<ConnStatus>;

    /// Retire a subscription (`<Subscription>.cancel @1`).
    fn cancel_subscription(&self, subscription: SubscriptionId) -> Result<()>;

    /// Interface traffic counters for a window ending now (`getCounters @35`).
    fn get_counters(&self, interface: NetworkInterfaceId, window: Duration)
        -> Result<InterfaceCounters>;

    /// Mint a counter subscription handle (`subscribeCounters @37`).
    fn subscribe_counters(&self, interface: NetworkInterfaceId) -> Result<SubscriptionId>;

    /// Pull the next counters snapshot for a subscription (`pullCounters @38`).
    fn pull_counters(&self, subscription: SubscriptionId) -> Result<InterfaceCounters>;

    // ── Host-global networking (the master switch, `@72`–`@73`) ──────────

    /// Is host-global networking enabled? (`isNetworkingEnabled @73`.) Reads
    /// NetworkManager's `NetworkingEnabled` — the field that once persisted
    /// `false` and survived a reboot.
    fn is_networking_enabled(&self) -> Result<bool>;

    /// Enable or disable host-global networking (`setNetworking @72`). This
    /// writes host state (NM `Enable`); the live bring-back proof is founder-run.
    fn set_networking_enabled(&self, enabled: bool) -> Result<()>;

    // ── Radios (open token set, `@74`–`@75`) ─────────────────────────────

    /// Enable or disable a radio (`setRadioEnabled @74`). `radio` is an open
    /// token: "wifi" maps to NM `WirelessEnabled`, "cellular" routes to the one
    /// `set_mobile_data_enabled` home; an unknown radio is `invalidArgument`.
    /// Writes host state; live proof is founder-run.
    fn set_radio_enabled(&self, radio: &str, enabled: bool) -> Result<()>;

    /// List observable radio states (`listRadioStates @75`); `hardBlocked` and
    /// `softBlocked` are reported from the host, never conflated with `enabled`.
    fn list_radio_states(&self, page: Pagination) -> Result<Page<RadioState>>;

    // ── Cellular data (`@11`–`@14`) ──────────────────────────────────────

    /// Is mobile (cellular) data enabled? (`isMobileDataEnabled @12`.)
    fn is_mobile_data_enabled(&self) -> Result<bool>;

    /// Enable or disable mobile data (`setMobileData @11`), the one home the
    /// "cellular" radio token also routes to. Writes host state.
    fn set_mobile_data_enabled(&self, enabled: bool) -> Result<()>;

    /// Is data roaming enabled? (`isDataRoamingEnabled @14`.)
    fn is_data_roaming_enabled(&self) -> Result<bool>;

    /// Enable or disable data roaming (`setDataRoaming @13`). Writes host state.
    fn set_data_roaming_enabled(&self, enabled: bool) -> Result<()>;

    // ─────────────────────────────────────────────────────────────────────
    // Honestly Unavailable-with-reason on Linux in this milestone. Every one
    // answers `notSupported` at runtime with a reasonClass prefix that matches
    // its availability row (`domains::network::availability`). A platform that
    // can serve one overrides the method; the trait never lies about it.
    // ─────────────────────────────────────────────────────────────────────

    /// `connect @4` — WPA/WPA-EAP association is NetworkManager-mediated and
    /// CA-privileged; not built on the NM bridge in this milestone.
    fn connect(
        &self,
        _interface: NetworkInterfaceId,
        _network: &ConnectConfig,
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: connect routes through NetworkManager AddAndActivateConnection (polkit-gated; polkitd absent on this host)",
        ))
    }

    /// `disconnect @5`.
    fn disconnect(&self, _interface: NetworkInterfaceId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: disconnect routes through NetworkManager device disconnection (polkit-gated)",
        ))
    }

    /// `wifiScan @6` — an active scan drives NM `RequestScan` and enumerates
    /// APs; not built in this milestone.
    fn wifi_scan(&self, _page: Pagination) -> Result<Page<WifiNetwork>> {
        Err(Error::not_supported(
            "no-backend: active WiFi scan orchestration (NM RequestScan + AccessPoint enumeration) not implemented",
        ))
    }

    /// `wifiConnect @7` — credentials stay behind a `SecretKeyId`; the
    /// association path is polkit-gated and not built here.
    fn wifi_connect(&self, _ssid: &str, _credential: SecretKeyId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: wifiConnect routes through NetworkManager connection activation (polkit-gated)",
        ))
    }

    /// `wifiDisconnect @8`.
    fn wifi_disconnect(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: wifiDisconnect routes through NetworkManager (polkit-gated)",
        ))
    }

    /// `wifiSetAutoConnect @9`.
    fn wifi_set_auto_connect(&self, _ssid: &str, _enabled: bool) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: per-SSID autoconnect is a NetworkManager setting write (polkit-gated)",
        ))
    }

    /// `wifiSetMacRandomization @10`.
    fn wifi_set_mac_randomization(
        &self,
        _interface: NetworkInterfaceId,
        _enabled: bool,
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: MAC randomization is a NetworkManager setting write (polkit-gated)",
        ))
    }

    /// `setVpnProfile @15`.
    fn set_vpn_profile(&self, _profile: &VpnProfileInfo) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: VPN profile management (NM settings + VPN plugin) not implemented",
        ))
    }

    /// `removeVpnProfile @16`.
    fn remove_vpn_profile(&self, _profile_id: VpnProfileId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: VPN profile management (NM settings + VPN plugin) not implemented",
        ))
    }

    /// `listVpnProfiles @17`.
    fn list_vpn_profiles(&self, _page: Pagination) -> Result<Page<VpnProfileInfo>> {
        Err(Error::not_supported(
            "no-backend: VPN profile management (NM settings + VPN plugin) not implemented",
        ))
    }

    /// `attachVpn @18`.
    fn attach_vpn(&self, _profile_id: VpnProfileId) -> Result<VpnConnectionInfo> {
        Err(Error::not_supported(
            "no-backend: VPN activation (NM VPN plugin) not implemented",
        ))
    }

    /// `detachVpn @19`.
    fn detach_vpn(&self, _connection_id: VpnConnectionId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: VPN activation (NM VPN plugin) not implemented",
        ))
    }

    /// `listVpnConnections @20`.
    fn list_vpn_connections(&self, _page: Pagination) -> Result<Page<VpnConnectionInfo>> {
        Err(Error::not_supported(
            "no-backend: VPN activation (NM VPN plugin) not implemented",
        ))
    }

    /// `startHotspot @21`.
    fn start_hotspot(&self, _config: &HotspotConfigEcho) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: hotspot/tethering activation (NetworkManager hotspot) not implemented",
        ))
    }

    /// `stopHotspot @22`.
    fn stop_hotspot(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: hotspot/tethering activation (NetworkManager hotspot) not implemented",
        ))
    }

    /// `getHotspotStatus @23`.
    fn get_hotspot_status(&self) -> Result<HotspotStatusInfo> {
        Err(Error::not_supported(
            "no-backend: hotspot/tethering status (NetworkManager hotspot) not implemented",
        ))
    }

    /// `subscribeHotspot @24`.
    fn subscribe_hotspot(&self) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: hotspot/tethering status (NetworkManager hotspot) not implemented",
        ))
    }

    /// `pullHotspot @25`.
    fn pull_hotspot(&self, _subscription: SubscriptionId) -> Result<HotspotStatusInfo> {
        Err(Error::not_supported(
            "no-backend: hotspot/tethering status (NetworkManager hotspot) not implemented",
        ))
    }

    /// `requestAddressAssignment @26`.
    fn request_address_assignment(
        &self,
        _interface: NetworkInterfaceId,
        _options: &AddressAssignmentOptionsEcho,
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: DHCPv4/DHCPv6 request is internal to NetworkManager; not exposed here",
        ))
    }

    /// `getAddressAssignment @27`.
    fn get_address_assignment(
        &self,
        _interface: NetworkInterfaceId,
    ) -> Result<AddressAssignmentOutcome> {
        Err(Error::not_supported(
            "no-backend: DHCP assignment readout not implemented in this milestone",
        ))
    }

    /// `releaseAddressAssignment @28`.
    fn release_address_assignment(&self, _interface: NetworkInterfaceId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: DHCP release is internal to NetworkManager; not exposed here",
        ))
    }

    /// `subscribeAddressAssignment @29`.
    fn subscribe_address_assignment(
        &self,
        _interface: NetworkInterfaceId,
    ) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: DHCP assignment readout not implemented in this milestone",
        ))
    }

    /// `pullAddressAssignment @30`.
    fn pull_address_assignment(
        &self,
        _subscription: SubscriptionId,
    ) -> Result<AddressAssignmentInfo> {
        Err(Error::not_supported(
            "no-backend: DHCP assignment readout not implemented in this milestone",
        ))
    }

    /// `getDnsServers @31`.
    fn get_dns_servers(
        &self,
        _interface: NetworkInterfaceId,
        _page: Pagination,
    ) -> Result<Page<DnsServerInfo>> {
        Err(Error::not_supported(
            "no-backend: per-interface DNS readout not implemented in this milestone",
        ))
    }

    /// `setDnsServers @32`.
    fn set_dns_servers(
        &self,
        _interface: NetworkInterfaceId,
        _servers: &[DnsServerInfo],
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: DNS configuration write needs CAP_NET_ADMIN / NM+polkit (polkitd absent)",
        ))
    }

    /// `setInterfaceQos @33`.
    fn set_interface_qos(
        &self,
        _interface: NetworkInterfaceId,
        _rules: &[QosRuleInfo],
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: QoS/tc programming needs CAP_NET_ADMIN (nft absent; tc/netem not implemented)",
        ))
    }

    /// `getInterfaceQos @34`.
    fn get_interface_qos(
        &self,
        _interface: NetworkInterfaceId,
        _page: Pagination,
    ) -> Result<Page<QosRuleInfo>> {
        Err(Error::not_supported(
            "no-backend: QoS rule readout (tc/netem) not implemented in this milestone",
        ))
    }

    /// `resetCounters @36` — counter reset is a privileged write.
    fn reset_counters(&self, _interface: NetworkInterfaceId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: interface counter reset needs CAP_NET_ADMIN",
        ))
    }

    /// `checkCaptivePortal @39`.
    fn check_captive_portal(&self, _interface: NetworkInterfaceId) -> Result<CaptivePortalInfo> {
        Err(Error::not_supported(
            "no-backend: captive-portal probe (NM connectivity check) not implemented in this milestone",
        ))
    }

    /// `subscribeCaptivePortal @40`.
    fn subscribe_captive_portal(&self, _interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: captive-portal probe (NM connectivity check) not implemented in this milestone",
        ))
    }

    /// `pullCaptivePortal @41`.
    fn pull_captive_portal(&self, _subscription: SubscriptionId) -> Result<CaptivePortalInfo> {
        Err(Error::not_supported(
            "no-backend: captive-portal probe (NM connectivity check) not implemented in this milestone",
        ))
    }

    /// `upInterface @42`.
    fn up_interface(&self, _interface: NetworkInterfaceId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: bringing a link up needs CAP_NET_ADMIN (or NM+polkit; polkitd absent)",
        ))
    }

    /// `downInterface @43`.
    fn down_interface(&self, _interface: NetworkInterfaceId) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: taking a link down needs CAP_NET_ADMIN (or NM+polkit; polkitd absent)",
        ))
    }

    /// `getTopology @44`.
    fn get_topology(
        &self,
        _page: Pagination,
    ) -> Result<(Vec<TopologyNodeInfo>, Vec<TopologyLinkInfo>, u64)> {
        Err(Error::not_supported(
            "no-backend: topology discovery (ARP/LLDP synthesis) not implemented in this milestone",
        ))
    }

    /// `setPowerProfile @45`.
    fn set_power_profile(&self, _interface: NetworkInterfaceId, _profile: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: power-profile write routes through nl80211 (CAP_NET_ADMIN-gated)",
        ))
    }

    /// `getPowerProfile @46`.
    fn get_power_profile(&self, _interface: NetworkInterfaceId) -> Result<String> {
        Err(Error::not_supported(
            "no-backend: power-profile readout (nl80211) not implemented in this milestone",
        ))
    }

    /// `handoff @47`.
    fn handoff(
        &self,
        _from_interface: NetworkInterfaceId,
        _to_interface: NetworkInterfaceId,
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: seamless handoff orchestration not implemented in this milestone",
        ))
    }

    /// `createSlice @48`.
    fn create_slice(&self, _config: &SliceConfigEcho) -> Result<SliceId> {
        Err(Error::not_supported(
            "no-backend: network slicing (netns/VLAN) not implemented in this milestone",
        ))
    }

    /// `removeSlice @49`.
    fn remove_slice(&self, _slice_id: SliceId) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: network slicing (netns/VLAN) not implemented in this milestone",
        ))
    }

    /// `listSlices @50`.
    fn list_slices(&self, _page: Pagination) -> Result<Page<SliceInfo>> {
        Err(Error::not_supported(
            "no-backend: network slicing (netns/VLAN) not implemented in this milestone",
        ))
    }

    /// `getTelemetry @51`.
    fn get_telemetry(
        &self,
        _interface: NetworkInterfaceId,
        _duration: Duration,
    ) -> Result<TelemetryInfo> {
        Err(Error::not_supported(
            "no-backend: telemetry sampling (netlink/ethtool windows) not implemented in this milestone",
        ))
    }

    /// `subscribeTelemetry @52`.
    fn subscribe_telemetry(&self, _interface: NetworkInterfaceId) -> Result<SubscriptionId> {
        Err(Error::not_supported(
            "no-backend: telemetry sampling (netlink/ethtool windows) not implemented in this milestone",
        ))
    }

    /// `pullTelemetry @53`.
    fn pull_telemetry(&self, _subscription: SubscriptionId) -> Result<TelemetryInfo> {
        Err(Error::not_supported(
            "no-backend: telemetry sampling (netlink/ethtool windows) not implemented in this milestone",
        ))
    }

    /// `ping @54`.
    fn ping(&self, _target: &IpAddress) -> Result<(Duration, f32)> {
        Err(Error::not_supported(
            "no-backend: ICMP/UDP probe (ping_group_range allows ICMP, but the probe is not implemented)",
        ))
    }

    /// `traceroute @55`.
    fn traceroute(&self, _target: &IpAddress) -> Result<Vec<TracerouteHopInfo>> {
        Err(Error::not_supported(
            "no-backend: traceroute (raw/UDP tricks) not implemented in this milestone",
        ))
    }

    /// `setFirewallRules @56`.
    fn set_firewall_rules(
        &self,
        _interface: NetworkInterfaceId,
        _rules: &[FirewallRuleInfo],
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: firewall programming needs CAP_NET_ADMIN (iptables/nftables) — not implemented",
        ))
    }

    /// `getFirewallRules @57`.
    fn get_firewall_rules(
        &self,
        _interface: NetworkInterfaceId,
        _page: Pagination,
    ) -> Result<Page<FirewallRuleInfo>> {
        Err(Error::not_supported(
            "no-backend: firewall rule readout (iptables/nftables) not implemented in this milestone",
        ))
    }

    /// `getTimeSync @58`.
    fn get_time_sync(&self, _interface: NetworkInterfaceId) -> Result<TimeSyncInfo> {
        Err(Error::not_supported(
            "no-backend: NTP/PTP status readout not implemented in this milestone",
        ))
    }

    /// `getSatelliteConfig @59`.
    fn get_satellite_config(&self) -> Result<SatelliteConfigEcho> {
        Err(Error::not_supported(
            "not-applicable: no satellite modem present on this host",
        ))
    }

    /// `setHotspotClientFilter @60`.
    fn set_hotspot_client_filter(&self, _mac_addresses: &[String]) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: hotspot client filtering not implemented in this milestone",
        ))
    }

    /// `getHotspotBandwidthSchedule @61`.
    fn get_hotspot_bandwidth_schedule(&self) -> Result<Vec<BandwidthSlotInfo>> {
        Err(Error::not_supported(
            "no-backend: hotspot bandwidth scheduling not implemented in this milestone",
        ))
    }

    /// `setHotspotBandwidthSchedule @62`.
    fn set_hotspot_bandwidth_schedule(&self, _schedule: &[BandwidthSlotInfo]) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: hotspot bandwidth scheduling not implemented in this milestone",
        ))
    }

    /// `sendDtnBundle @63`.
    fn send_dtn_bundle(&self, _bundle: &DtnBundleInfo) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: DTN store-and-forward agent not present on this host",
        ))
    }

    /// `receiveDtnBundles @64`.
    fn receive_dtn_bundles(&self, _page: Pagination) -> Result<Page<DtnBundleInfo>> {
        Err(Error::not_supported(
            "no-backend: DTN store-and-forward agent not present on this host",
        ))
    }

    /// `getCellularConfig @65`.
    fn get_cellular_config(&self) -> Result<CellularConfigInfo> {
        Err(Error::not_supported(
            "not-applicable: no cellular modem on this host (ModemManager absent)",
        ))
    }

    /// `setCellularConfig @66`.
    fn set_cellular_config(&self, _config: &CellularConfigInfo) -> Result<()> {
        Err(Error::not_supported(
            "not-applicable: no cellular modem on this host (ModemManager absent)",
        ))
    }

    /// `getIpv6Config @67`.
    fn get_ipv6_config(&self, _interface: NetworkInterfaceId) -> Result<Ipv6ConfigInfo> {
        Err(Error::not_supported(
            "no-backend: IPv6 configuration readout not implemented in this milestone",
        ))
    }

    /// `setIpv6Config @68`.
    fn set_ipv6_config(
        &self,
        _interface: NetworkInterfaceId,
        _config: &Ipv6ConfigInfo,
    ) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: IPv6 configuration write needs NM+polkit / CAP_NET_ADMIN",
        ))
    }

    /// `wifiNanAdvertise @69`.
    fn wifi_nan_advertise(&self, _config: &WifiNanConfigEcho) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: Wi-Fi Aware (NAN) has no public host API; not implemented",
        ))
    }

    /// `wifiNanDiscover @70`.
    fn wifi_nan_discover(
        &self,
        _config: &WifiNanConfigEcho,
        _page: Pagination,
    ) -> Result<(Vec<NanPeerInfo>, u32)> {
        Err(Error::not_supported(
            "no-backend: Wi-Fi Aware (NAN) has no public host API; not implemented",
        ))
    }

    /// `nicProvider @71` — the capability accessor. Authority is the held
    /// service reference; no per-method `Cap` param. The NIC driver surface
    /// (netlink/ethtool/DPDK) is not built in this milestone.
    fn nic_provider(&self) -> Result<NicProviderHandle> {
        Err(Error::not_supported(
            "no-backend: NIC driver provider surface (netlink/ethtool/DPDK) not implemented",
        ))
    }

    // ── Configuration the command surface proved missing (`@76`–`@82`) ───────
    //
    // The seven methods the route pass added to the contract. Every one of them
    // has an honest answer on this backend, and the availability matrix names
    // which kind: REAL (kernel- or policy-answered, test-backed), DEGRADED
    // (a host-mutating verb whose mapping the mock proves and whose live proof
    // is founder-run), or UNAVAILABLE with a reasonClass and a reason.

    /// `setInterfaceMtu @76` — set one interface's MTU. Host-mutating: the
    /// kernel owns the attribute and writes it with `CAP_NET_ADMIN`. The read
    /// half (`getInterfaces`' `mtU`) is the store's `IFLA_MTU`, a kernel fact.
    fn set_interface_mtu(&self, _interface: NetworkInterfaceId, _mtu: u32) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: an MTU write is a netlink RTM_NEWLINK/IFLA_MTU or a NetworkManager \
             setting write (CAP_NET_ADMIN / polkit-gated); the read half is the kernel's own \
             IFLA_MTU in the state store",
        ))
    }

    /// `forgetWifiNetwork @77` — drop one remembered network. Host-mutating:
    /// the remembered set is NetworkManager's connection profiles, and a
    /// delete on it is polkit-gated. Carries no credential: forgetting a
    /// network removes the secret reference with the profile, it never
    /// returns one.
    fn forget_wifi_network(&self, _ssid: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: forgetting a network deletes a NetworkManager connection profile \
             (polkit-gated)",
        ))
    }

    /// `listKnownWifiNetworks @78` — the remembered-set read. NM policy state:
    /// the kernel does not own connection profiles, so this is one counted
    /// policy read, not a per-profile daemon crawl.
    fn list_known_wifi_networks(&self, _page: Pagination) -> Result<Page<KnownWifiNetwork>> {
        Err(Error::not_supported(
            "no-backend: NetworkManager connection-profile readout (Settings.ListConnections + \
             Connection.GetSettings) not wired",
        ))
    }

    /// `getRoutes @79` — the kernel's routing table. A kernel-answerable fact:
    /// `RTM_GETROUTE` reaches the store, so a read costs no daemon hop.
    fn get_routes(&self, _page: Pagination) -> Result<Page<RouteInfo>> {
        Err(Error::not_supported(
            "no-backend: routing-table readout (netlink RTM_GETROUTE) not wired into this backend",
        ))
    }

    /// `setRoutes @80` — replace one interface's routes. A full-table rewrite
    /// is a host-mutating `CAP_NET_ADMIN` verb; the read half (`getRoutes`) is
    /// the kernel's own table.
    fn set_routes(&self, _interface: NetworkInterfaceId, _routes: &[RouteInfo]) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: a route-table rewrite is a netlink RTM_NEWROUTE/RTM_DELROUTE sequence \
             (CAP_NET_ADMIN); not implemented — the read half, getRoutes, is the kernel's own table",
        ))
    }

    /// `getProxyConfig @81` — the host's own proxy. A file-level host fact the
    /// daemon does not own; see `platforms::linux` for the exact source.
    fn get_proxy_config(&self) -> Result<ProxyConfigInfo> {
        Err(Error::not_supported(
            "no-backend: host proxy readout (the /etc proxy block) not wired into this backend",
        ))
    }

    /// `setProxyConfig @82` — write the host's own proxy. Host-mutating and,
    /// on this platform, a root write to `/etc` (or a desktop settings
    /// daemon's business): smart-lib does not write `/etc` (charter law #8).
    fn set_proxy_config(&self, _config: &ProxyConfigInfo) -> Result<()> {
        Err(Error::not_supported(
            "no-privilege: the host proxy is written by the system administrator (/etc) or a \
             desktop settings daemon; smart-lib never writes /etc (charter law #8)",
        ))
    }

    /// Publish this backend's honest network capability tokens (law #6). The
    /// static detail lives in [`super::availability`]; this is a convenience so
    /// a caller holding a `&dyn NetworkBackend` can read the same claims.
    fn network_caps(&self) -> Vec<&'static str> {
        alloc::vec![
            "network.status.hostState",
            "network.host.networking",
            "network.host.radio",
            "network.counters",
        ]
    }

    /// A nameable reason a method is unavailable — used by tooling that mirrors
    /// the availability matrix. Not a permission string; a diagnostic label.
    fn last_unavailable_reason(&self, _method: &str) -> Option<&'static str> {
        None
    }
}
