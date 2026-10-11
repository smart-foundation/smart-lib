//! The network domain availability matrix: one row per `Network` contract
//! method, published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3Test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! This table is the contract with the availability generator and `cli.capnp`'s
//! `listUnavailable`: every non-`REAL` row here is answered at runtime by the
//! method returning `notSupported` whose message begins with the row's
//! `reasonClass` (see [`super::backend`]).

/// The compiled schema file ID, read from `network.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const NETWORK_SCHEMA_ID: u64 = 0xa1b2c3d4e5f6000f;

/// One of the four availability reasons (`cli.capnp` `AvailabilityReason`,
/// re-used rather than a second grammar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// No implementation exists for this (domain, platform).
    NoBackend,
    /// The host primitive exists but the caller's credentials do not reach it.
    NoPrivilege,
    /// The concept does not exist on this platform.
    NotApplicable,
    /// Exists, held back deliberately.
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    /// The exact token the matrix, the tool reason and the CLI agree on.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// Per-method status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Passes its full behaviour on this platform; an `l3Test` proves it.
    Real,
    /// Answers, but not completely; `reason` names the degraded capability and
    /// an `l3Test` still backs it.
    Degraded,
    /// Answers `notSupported` with a `reasonClass`-prefixed reason.
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract method name (`network.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves).
    pub ordinal: u16,
    pub status: AvailStatus,
    /// The reason class, exactly when `status == UNAVAILABLE`.
    pub reason_class: Option<ReasonClass>,
    /// For `UNAVAILABLE`: why. For `DEGRADED`: the named degraded capability.
    /// Empty for `REAL`.
    pub reason: &'static str,
    /// The L3 test that ran and passed, required when `status != UNAVAILABLE`.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    const fn real(name: &'static str, ordinal: u16, l3_test: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3_test),
        }
    }

    const fn degraded(name: &'static str, ordinal: u16, l3_test: &'static str, detail: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3_test),
        }
    }

    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The published availability matrix for the network domain on Linux.
///
/// One row per `Network` contract method (`@0`–`@75`), plus the domain's two
/// own-subscription helpers (`cancelSubscription`, and `subscribe/pull` are
/// already contract rows). The `l3_test` ids name the always-on mock-driven
/// tests in [`crate::platforms::linux::network`] that exercise this backend's
/// mapping; the read-only live probe that reads real NetworkManager state over
/// D-Bus is a separate, `#[ignore]`d, founder-runnable check.
pub const NETWORK_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("getInterfaces", 0, "l3_linux_network::get_interfaces_and_radio_states"),
    MethodAvailability::real("getStatus", 1, "l3_linux_network::get_status_four_states_distinct"),
    MethodAvailability::real("subscribeStatus", 2, "l3_linux_network::status_subscription_bounded_pull"),
    MethodAvailability::real("pullStatus", 3, "l3_linux_network::status_subscription_bounded_pull"),
    MethodAvailability::real("cancelSubscription", 0xFFFF, "l3_linux_network::cancel_retires_token"), // domain helper (StatusSubscription.cancel @1)
    MethodAvailability::unavailable("connect", 4, ReasonClass::NoPrivilege, "WPA association routes through NetworkManager AddAndActivateConnection (polkit-gated)"),
    MethodAvailability::unavailable("disconnect", 5, ReasonClass::NoPrivilege, "disconnection routes through NetworkManager (polkit-gated)"),
    MethodAvailability::unavailable("wifiScan", 6, ReasonClass::NoBackend, "active scan orchestration (NM RequestScan + AccessPoint enumeration) not implemented"),
    MethodAvailability::unavailable("wifiConnect", 7, ReasonClass::NoPrivilege, "wifi connect routes through NetworkManager activation (polkit-gated)"),
    MethodAvailability::unavailable("wifiDisconnect", 8, ReasonClass::NoPrivilege, "wifi disconnect routes through NetworkManager (polkit-gated)"),
    MethodAvailability::unavailable("wifiSetAutoConnect", 9, ReasonClass::NoPrivilege, "per-SSID autoconnect is a NetworkManager setting write (polkit-gated)"),
    MethodAvailability::unavailable("wifiSetMacRandomization", 10, ReasonClass::NoPrivilege, "MAC randomization is a NetworkManager setting write (polkit-gated)"),
    MethodAvailability::degraded("setMobileData", 11, "l3_linux_network::set_radio_cellular_routes_to_mobile_data", "implemented over NM WwanEnabled; host write authority gated by NM polkit (polkitd absent on this box) — founder live proof"),
    MethodAvailability::real("isMobileDataEnabled", 12, "l3_linux_network::mobile_data_enabled_reads_wwan"),
    MethodAvailability::unavailable("setDataRoaming", 13, ReasonClass::NotApplicable, "no cellular modem on this host (data roaming is a ModemManager setting; ModemManager absent)"),
    MethodAvailability::real("isDataRoamingEnabled", 14, "l3_linux_network::data_roaming_enabled_reads_wwan"),
    MethodAvailability::unavailable("setVpnProfile", 15, ReasonClass::NoBackend, "VPN profile management (NM settings + VPN plugin) not implemented"),
    MethodAvailability::unavailable("removeVpnProfile", 16, ReasonClass::NoBackend, "VPN profile management (NM settings + VPN plugin) not implemented"),
    MethodAvailability::unavailable("listVpnProfiles", 17, ReasonClass::NoBackend, "VPN profile management (NM settings + VPN plugin) not implemented"),
    MethodAvailability::unavailable("attachVpn", 18, ReasonClass::NoBackend, "VPN activation (NM VPN plugin) not implemented"),
    MethodAvailability::unavailable("detachVpn", 19, ReasonClass::NoBackend, "VPN activation (NM VPN plugin) not implemented"),
    MethodAvailability::unavailable("listVpnConnections", 20, ReasonClass::NoBackend, "VPN activation (NM VPN plugin) not implemented"),
    MethodAvailability::unavailable("startHotspot", 21, ReasonClass::NoBackend, "hotspot/tethering activation (NetworkManager hotspot) not implemented"),
    MethodAvailability::unavailable("stopHotspot", 22, ReasonClass::NoBackend, "hotspot/tethering activation (NetworkManager hotspot) not implemented"),
    MethodAvailability::unavailable("getHotspotStatus", 23, ReasonClass::NoBackend, "hotspot/tethering status (NetworkManager hotspot) not implemented"),
    MethodAvailability::unavailable("subscribeHotspot", 24, ReasonClass::NoBackend, "hotspot/tethering status (NetworkManager hotspot) not implemented"),
    MethodAvailability::unavailable("pullHotspot", 25, ReasonClass::NoBackend, "hotspot/tethering status (NetworkManager hotspot) not implemented"),
    MethodAvailability::unavailable("requestAddressAssignment", 26, ReasonClass::NoBackend, "DHCP request is internal to NetworkManager; not exposed here"),
    MethodAvailability::unavailable("getAddressAssignment", 27, ReasonClass::NoBackend, "DHCP assignment readout not implemented"),
    MethodAvailability::unavailable("releaseAddressAssignment", 28, ReasonClass::NoBackend, "DHCP release is internal to NetworkManager; not exposed here"),
    MethodAvailability::unavailable("subscribeAddressAssignment", 29, ReasonClass::NoBackend, "DHCP assignment readout not implemented"),
    MethodAvailability::unavailable("pullAddressAssignment", 30, ReasonClass::NoBackend, "DHCP assignment readout not implemented"),
    MethodAvailability::unavailable("getDnsServers", 31, ReasonClass::NoBackend, "per-interface DNS readout not implemented"),
    MethodAvailability::unavailable("setDnsServers", 32, ReasonClass::NoPrivilege, "DNS configuration write needs CAP_NET_ADMIN / NM+polkit (polkitd absent)"),
    MethodAvailability::unavailable("setInterfaceQos", 33, ReasonClass::NoPrivilege, "QoS/tc programming needs CAP_NET_ADMIN (nft absent; tc/netem not implemented)"),
    MethodAvailability::unavailable("getInterfaceQos", 34, ReasonClass::NoBackend, "QoS rule readout (tc/netem) not implemented"),
    MethodAvailability::real("getCounters", 35, "l3_linux_network::get_counters_from_sysfs"),
    MethodAvailability::unavailable("resetCounters", 36, ReasonClass::NoPrivilege, "interface counter reset needs CAP_NET_ADMIN"),
    MethodAvailability::real("subscribeCounters", 37, "l3_linux_network::counter_subscription_bounded_pull"),
    MethodAvailability::real("pullCounters", 38, "l3_linux_network::counter_subscription_bounded_pull"),
    MethodAvailability::unavailable("checkCaptivePortal", 39, ReasonClass::NoBackend, "captive-portal probe (NM connectivity check) not implemented"),
    MethodAvailability::unavailable("subscribeCaptivePortal", 40, ReasonClass::NoBackend, "captive-portal probe (NM connectivity check) not implemented"),
    MethodAvailability::unavailable("pullCaptivePortal", 41, ReasonClass::NoBackend, "captive-portal probe (NM connectivity check) not implemented"),
    MethodAvailability::unavailable("upInterface", 42, ReasonClass::NoPrivilege, "bringing a link up needs CAP_NET_ADMIN (or NM+polkit; polkitd absent)"),
    MethodAvailability::unavailable("downInterface", 43, ReasonClass::NoPrivilege, "taking a link down needs CAP_NET_ADMIN (or NM+polkit; polkitd absent)"),
    MethodAvailability::unavailable("getTopology", 44, ReasonClass::NoBackend, "topology discovery (ARP/LLDP synthesis) not implemented"),
    MethodAvailability::unavailable("setPowerProfile", 45, ReasonClass::NoPrivilege, "power-profile write routes through nl80211 (CAP_NET_ADMIN-gated)"),
    MethodAvailability::unavailable("getPowerProfile", 46, ReasonClass::NoBackend, "power-profile readout (nl80211) not implemented"),
    MethodAvailability::unavailable("handoff", 47, ReasonClass::NoBackend, "seamless handoff orchestration not implemented"),
    MethodAvailability::unavailable("createSlice", 48, ReasonClass::NoBackend, "network slicing (netns/VLAN) not implemented"),
    MethodAvailability::unavailable("removeSlice", 49, ReasonClass::NoBackend, "network slicing (netns/VLAN) not implemented"),
    MethodAvailability::unavailable("listSlices", 50, ReasonClass::NoBackend, "network slicing (netns/VLAN) not implemented"),
    MethodAvailability::unavailable("getTelemetry", 51, ReasonClass::NoBackend, "telemetry sampling (netlink/ethtool windows) not implemented"),
    MethodAvailability::unavailable("subscribeTelemetry", 52, ReasonClass::NoBackend, "telemetry sampling (netlink/ethtool windows) not implemented"),
    MethodAvailability::unavailable("pullTelemetry", 53, ReasonClass::NoBackend, "telemetry sampling (netlink/ethtool windows) not implemented"),
    MethodAvailability::unavailable("ping", 54, ReasonClass::NoBackend, "ICMP/UDP probe not implemented (ping_group_range allows ICMP)"),
    MethodAvailability::unavailable("traceroute", 55, ReasonClass::NoBackend, "traceroute (raw/UDP tricks) not implemented"),
    MethodAvailability::unavailable("setFirewallRules", 56, ReasonClass::NoPrivilege, "firewall programming needs CAP_NET_ADMIN (iptables/nftables)"),
    MethodAvailability::unavailable("getFirewallRules", 57, ReasonClass::NoBackend, "firewall rule readout (iptables/nftables) not implemented"),
    MethodAvailability::unavailable("getTimeSync", 58, ReasonClass::NoBackend, "NTP/PTP status readout not implemented"),
    MethodAvailability::unavailable("getSatelliteConfig", 59, ReasonClass::NotApplicable, "no satellite modem present on this host"),
    MethodAvailability::unavailable("setHotspotClientFilter", 60, ReasonClass::NoBackend, "hotspot client filtering not implemented"),
    MethodAvailability::unavailable("getHotspotBandwidthSchedule", 61, ReasonClass::NoBackend, "hotspot bandwidth scheduling not implemented"),
    MethodAvailability::unavailable("setHotspotBandwidthSchedule", 62, ReasonClass::NoBackend, "hotspot bandwidth scheduling not implemented"),
    MethodAvailability::unavailable("sendDtnBundle", 63, ReasonClass::NoBackend, "DTN store-and-forward agent not present on this host"),
    MethodAvailability::unavailable("receiveDtnBundles", 64, ReasonClass::NoBackend, "DTN store-and-forward agent not present on this host"),
    MethodAvailability::unavailable("getCellularConfig", 65, ReasonClass::NotApplicable, "no cellular modem on this host (ModemManager absent)"),
    MethodAvailability::unavailable("setCellularConfig", 66, ReasonClass::NotApplicable, "no cellular modem on this host (ModemManager absent)"),
    MethodAvailability::unavailable("getIpv6Config", 67, ReasonClass::NoBackend, "IPv6 configuration readout not implemented"),
    MethodAvailability::unavailable("setIpv6Config", 68, ReasonClass::NoPrivilege, "IPv6 configuration write needs NM+polkit / CAP_NET_ADMIN"),
    MethodAvailability::unavailable("wifiNanAdvertise", 69, ReasonClass::NoBackend, "Wi-Fi Aware (NAN) has no public host API"),
    MethodAvailability::unavailable("wifiNanDiscover", 70, ReasonClass::NoBackend, "Wi-Fi Aware (NAN) has no public host API"),
    MethodAvailability::unavailable("nicProvider", 71, ReasonClass::NoBackend, "NIC driver provider surface (netlink/ethtool/DPDK) not implemented"),
    MethodAvailability::degraded("setNetworking", 72, "l3_linux_network::set_networking_calls_enable", "implemented over NM Enable; host write authority gated by NM polkit (polkitd absent on this box) — founder live proof"),
    MethodAvailability::real("isNetworkingEnabled", 73, "l3_linux_network::is_networking_enabled_reads_property"),
    MethodAvailability::degraded("setRadioEnabled", 74, "l3_linux_network::set_radio_wifi_calls_wireless_enabled", "implemented over NM WirelessEnabled; host write authority gated by NM polkit (polkitd absent on this box) — founder live proof"),
    MethodAvailability::real("listRadioStates", 75, "l3_linux_network::get_interfaces_and_radio_states"),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records (supported, not merely
/// linked). Network status/radio are transport-independent reads/writes.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published network capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "network.status.hostState",
    "network.host.networking",
    "network.host.radio",
    "network.counters",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in NETWORK_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims — surfaced so the test suite can assert
/// each one exists (a REAL claim with a vanished test is a FAIL).
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    NETWORK_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}
