//! Owned value types the network domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen contract.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam. It is
//! ergonomic (owned, `Copy` where the contract allows) for the methods this
//! milestone implements, and a compact faithful echo for the methods that are
//! honestly `Unavailable`-with-reason on Linux. The `metadata` / `StringMap`
//! bookkeeping the contract carries is intentionally NOT mirrored here — the
//! generated struct still owns it (core makes the same call for `T.Error`), and
//! a backend that needs it uses `crate::schema::network_capnp` directly.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::{NetworkInterfaceId, VpnConnectionId, VpnProfileId};
use crate::core::values::Duration;
use crate::schema::network_capnp;

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`).
///
/// The contract's rule (conformance §2.3): `limit == 0` means "one", never
/// "all"; `offset + limit` may never exceed `total`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule: 0 means "one".
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A one-page window over an unbounded list, with the stable `total` the
/// contract promises across two identical calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

/// An IP address, mirroring `T.IpAddress`'s union minus the rarely-used
/// `custom` arm (a backend-specific blob is backend detail, carried by the
/// generated struct).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
    Text(String),
}

impl IpAddress {
    /// The empty / "not present" address (the neutral value a caller reads when
    /// no address was reported — distinct from a measured 0.0.0.0).
    pub fn none() -> Self {
        IpAddress::Text(String::new())
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, IpAddress::Text(s) if s.is_empty())
    }
}

impl Default for IpAddress {
    /// The neutral (empty) address: an absent address, distinct from a measured
    /// `0.0.0.0`. Reading it as a real address is the fabrication this avoids.
    fn default() -> Self {
        IpAddress::Text(String::new())
    }
}

impl core::fmt::Display for IpAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IpAddress::V4(o) => f.write_fmt(format_args!("{}.{}.{}.{}", o[0], o[1], o[2], o[3])),
            IpAddress::V6(o) => {
                for (i, b) in o.iter().enumerate() {
                    if i > 0 {
                        f.write_str(":")?;
                    }
                    f.write_fmt(format_args!("{b:02x}"))?;
                }
                Ok(())
            }
            IpAddress::Text(s) => f.write_str(s),
        }
    }
}

/// A MAC / link-layer address (`T.MacAddress`: colon-separated hex text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacAddress(pub String);

impl core::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// WHY an interface is or is not usable — the incident's whole reason for
/// existing. Open tokens on the wire (`InterfaceConnectionStatus.hostState`);
/// an enum in Rust so the four diagnostic states are *distinct typed values*
/// the compiler cannot collapse. Never conflate policy (radio-off,
/// networking-disabled) with a physical fact (no-firmware, carrier-down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostState {
    /// The device is managed and its state is the host's own doing.
    Managed,
    /// The device is present but unmanaged by the network stack.
    Unmanaged,
    /// Host-global networking is switched off (the master switch, above every
    /// per-connection state).
    NetworkingDisabled,
    /// The interface's radio is switched off by policy or a soft/hard block.
    RadioOff,
    /// No carrier (cable unplugged, AP out of range, link-layer has no signal).
    CarrierDown,
    /// The device lacks firmware and cannot come up.
    NoFirmware,
    /// The host could not classify the state — an honest last resort, never a
    /// fabricated `managed`.
    Unknown,
}

impl HostState {
    /// The exact open tokens the contract carries, so the mapping is auditable.
    pub const ALL: [HostState; 7] = [
        HostState::Managed,
        HostState::Unmanaged,
        HostState::NetworkingDisabled,
        HostState::RadioOff,
        HostState::CarrierDown,
        HostState::NoFirmware,
        HostState::Unknown,
    ];

    /// The contract token for this state.
    pub const fn as_token(self) -> &'static str {
        match self {
            HostState::Managed => "managed",
            HostState::Unmanaged => "unmanaged",
            HostState::NetworkingDisabled => "networking-disabled",
            HostState::RadioOff => "radio-off",
            HostState::CarrierDown => "carrier-down",
            HostState::NoFirmware => "no-firmware",
            HostState::Unknown => "unknown",
        }
    }

    /// Parse a token. An unrecognised token (a 2046 vocabulary this build does
    /// not know) maps to [`HostState::Unknown`] — the token set is open, so an
    /// unknown token is tolerated, never rejected and never guessed as a known
    /// state.
    pub fn from_token(token: &str) -> HostState {
        match token {
            "managed" => HostState::Managed,
            "unmanaged" => HostState::Unmanaged,
            "networking-disabled" => HostState::NetworkingDisabled,
            "radio-off" => HostState::RadioOff,
            "carrier-down" => HostState::CarrierDown,
            "no-firmware" => HostState::NoFirmware,
            _ => HostState::Unknown,
        }
    }

    /// Read the domain state directly out of a generated `hostState` Text.
    pub fn from_reader(reader: &network_capnp::interface_connection_status::Reader<'_>) -> Self {
        match reader.get_host_state() {
            Ok(text) => HostState::from_token(core::str::from_utf8(text.as_bytes()).unwrap_or("")),
            Err(_) => HostState::Unknown,
        }
    }
}

/// WHERE a `HostState` value came from — the transport hierarchy (charter law
/// #9: kernel first, daemon for policy, contract for consumers) made
/// auditable. `Kernel` states are the kernel's own facts read over netlink
/// (carrier-down, no-firmware); `NmPolicy` states are NetworkManager policy
/// read over the counted D-Bus fallback (networking-disabled, radio-off,
/// unmanaged). A consumer that reads the state with its source can tell a
/// dead wire from a switched-off host without guessing which layer lied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostStateSource {
    /// The kernel's own fact (netlink): the wire, the firmware, the operstate.
    Kernel,
    /// NetworkManager policy over the D-Bus fallback: the master switch, the
    /// radio enables, per-device management.
    NmPolicy,
}

impl HostStateSource {
    /// The token a diagnostic surface prints.
    pub const fn as_token(self) -> &'static str {
        match self {
            HostStateSource::Kernel => "kernel",
            HostStateSource::NmPolicy => "nm-policy",
        }
    }

    /// Where a `HostState` decided its verdict, as a pure function of the state
    /// (used when a status is read back off the contract wire, which carries the
    /// token but not the source): the two states only the kernel can decide are
    /// kernel-sourced, everything else was decided by NetworkManager policy.
    pub const fn of(state: HostState) -> Self {
        match state {
            HostState::CarrierDown | HostState::NoFirmware => HostStateSource::Kernel,
            _ => HostStateSource::NmPolicy,
        }
    }
}

/// Per-interface connection status, the honest subset this milestone reports
/// (`InterfaceConnectionStatus` minus the WiFi sub-struct, whose `ssid` this
/// domain surfaces directly).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnStatus {
    pub connected: bool,
    pub interface: NetworkInterfaceId,
    /// Link technology as an open token ("ethernet", "wifi", ...).
    pub kind: String,
    /// dBm signal strength (0 when not measurable, not a fabricated 0).
    pub strength_dbm: i32,
    /// Speed in bits per second (0 = unknown).
    pub speed_bps: u64,
    /// Quality 0..100.
    pub quality: u8,
    pub metered: bool,
    pub behind_captive: bool,
    /// WHY the interface is or is not usable — the four-state surface.
    pub host_state: HostState,
    /// WHERE that verdict came from — the transport hierarchy (law #9) made
    /// auditable on the answer itself: `carrier-down` / `no-firmware` are the
    /// kernel's own facts (netlink); `networking-disabled` / `radio-off` /
    /// `unmanaged` are NetworkManager policy (the counted D-Bus fallback). The
    /// contract wire carries the `hostState` token only, so this is the local
    /// enrichment that survives the round-trip via [`HostStateSource::of`].
    pub host_state_source: HostStateSource,
    /// WiFi SSID when connected and the link is WiFi; empty otherwise.
    pub ssid: String,
}

impl ConnStatus {
    /// Write into a generated `InterfaceConnectionStatus` builder.
    pub fn write_to(&self, builder: &mut network_capnp::interface_connection_status::Builder<'_>) {
        builder.set_connected(self.connected);
        builder.reborrow().init_interface().set_id(self.interface.get());
        builder.set_type(&self.kind);
        builder.set_strength(self.strength_dbm);
        builder.set_speed(self.speed_bps);
        builder.set_quality(self.quality);
        builder.set_metered(self.metered);
        builder.set_behind_captive(self.behind_captive);
        builder.set_host_state(self.host_state.as_token());
        // The contract folds WiFi specifics (ssid) into the `wifi` sub-struct;
        // this domain surfaces the ssid directly and routes it through there.
        if !self.ssid.is_empty() {
            builder.reborrow().init_wifi().set_ssid(&self.ssid);
        }
    }

    /// Read from a generated `InterfaceConnectionStatus` reader.
    pub fn from_reader(reader: &network_capnp::interface_connection_status::Reader<'_>) -> Self {
        let ssid = reader
            .get_wifi()
            .ok()
            .and_then(|w| w.get_ssid().ok())
            .and_then(|t| core::str::from_utf8(t.as_bytes()).ok().map(ToString::to_string))
            .unwrap_or_default();
        ConnStatus {
            connected: reader.get_connected(),
            interface: NetworkInterfaceId::new(reader.get_interface().map(|i| i.get_id()).unwrap_or(0)),
            kind: reader
                .get_type()
                .ok()
                .and_then(|t| core::str::from_utf8(t.as_bytes()).ok().map(ToString::to_string))
                .unwrap_or_default(),
            strength_dbm: reader.get_strength(),
            speed_bps: reader.get_speed(),
            quality: reader.get_quality(),
            metered: reader.get_metered(),
            behind_captive: reader.get_behind_captive(),
            host_state: HostState::from_reader(reader),
            host_state_source: HostStateSource::of(HostState::from_reader(reader)),
            ssid,
        }
    }
}

/// One enumerable network interface (`NetworkInterface`, honest subset).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: NetworkInterfaceId,
    pub name: String,
    pub kind: String,
    pub hardware_address: MacAddress,
    pub up: bool,
    pub connected: bool,
    pub mtu: u32,
    pub managed: bool,
}

/// The observable state of one radio (`RadioState`) — policy `enabled`,
/// physical `hard_blocked`, software `soft_blocked`, each reported separately
/// and never conflated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadioState {
    /// Open token: "wifi", "cellular", "bluetooth", or a radio not yet named.
    pub name: String,
    pub enabled: bool,
    pub hard_blocked: bool,
    pub soft_blocked: bool,
}

impl RadioState {
    pub fn write_to(&self, builder: &mut network_capnp::radio_state::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_enabled(self.enabled);
        builder.set_hard_blocked(self.hard_blocked);
        builder.set_soft_blocked(self.soft_blocked);
    }
}

/// A WiFi network seen in a scan (`WifiNetwork`, honest subset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: MacAddress,
    pub strength_dbm: i32,
    pub frequency_hz: u32,
    pub security: String,
    pub hidden: bool,
}

/// Interface traffic counters (`InterfaceCounters`), a window ending at
/// `sampled_at`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterfaceCounters {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub errors_in: u64,
    pub errors_out: u64,
    pub drops_in: u64,
    pub drops_out: u64,
    /// Nanoseconds since epoch (the contract's `T.Timestamp` unit).
    pub sampled_at: u64,
}

/// An opaque handle minted by `subscribe_*`. The `NetworkBackend` is a pull
/// model: subscribing mints a token, pulling reads the current value under the
/// caller's stated bound, and the token is retired on cancel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

// ── Compact faithful echoes for the honestly-unavailable surface ───────────
//
// These carry the fields a caller would name; the metadata/StringMap the
// generated structs own is the codegen wave's job (law #7). Every method
// consuming one of these is `Unavailable`-with-reason on Linux in this
// milestone, so their bodies are never reached here — they exist so the trait
// has one honest, well-typed method per contract method.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VpnProfileInfo {
    pub id: VpnProfileId,
    pub name: String,
    pub kind: String,
    pub autoconnect: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VpnConnectionInfo {
    pub id: VpnConnectionId,
    pub profile_id: VpnProfileId,
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotspotStatusInfo {
    pub active: bool,
    pub band: String,
    pub ssid: String,
    pub connected_clients: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotspotConfigEcho {
    pub band: String,
    pub security: String,
    pub ssid: String,
    pub max_clients: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressAssignmentOptionsEcho {
    pub requested: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressAssignmentOutcome {
    Assigned(AddressAssignmentInfo),
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressAssignmentInfo {
    pub interface: NetworkInterfaceId,
    pub family: String,
    pub address: IpAddress,
    pub prefix_len: u8,
    pub gateway: IpAddress,
    pub mechanism: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DnsServerInfo {
    pub address: IpAddress,
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QosRuleInfo {
    pub dscp: u8,
    pub protocol: String,
    pub ceiling_kbps: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TracerouteHopInfo {
    pub hop: u8,
    pub address: IpAddress,
    pub hostname: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirewallRuleInfo {
    pub name: String,
    pub enabled: bool,
    pub protocol: String,
    pub action: String,
    /// How many packets this rule has matched (`FirewallRule.hitCount @13`).
    ///
    /// The contract's law, repeated here because it is the one unforgivable
    /// place to get it wrong: **a counter is measured by the host or it is
    /// absent.** A backend that cannot count reports 0 only when it knows the
    /// count is zero, and says so in metadata. This field exists on the type
    /// so a firewall backend has somewhere honest to put the number; it is
    /// never fabricated here (the Linux backend cannot read the host's
    /// firewall tables at all in this milestone, so `getFirewallRules` answers
    /// `notSupported` rather than an empty rule list with invented counters).
    pub hit_count: u64,
}

impl FirewallRuleInfo {
    /// Write into a generated `FirewallRule` builder. `hit_count` is written
    /// verbatim: 0 is the kernel's measured zero or the row's neutral value,
    /// and the reason class a caller reads to tell them apart lives in the
    /// availability matrix, not in a silent field.
    pub fn write_to(&self, builder: &mut network_capnp::firewall_rule::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_enabled(self.enabled);
        builder.set_protocol(&self.protocol);
        builder.set_action(firewall_action_from_token(&self.action));
        builder.set_hit_count(self.hit_count);
    }

    /// Read from a generated `FirewallRule` reader.
    pub fn from_reader(reader: &network_capnp::firewall_rule::Reader<'_>) -> Self {
        FirewallRuleInfo {
            name: text_field(reader.get_name()),
            enabled: reader.get_enabled(),
            protocol: text_field(reader.get_protocol()),
            action: firewall_action_token(reader.get_action().unwrap_or(network_capnp::FirewallAction::Custom)).to_string(),
            hit_count: reader.get_hit_count(),
        }
    }
}

/// The contract's `FirewallAction` enum from a host-spelled open token. An
/// unrecognised token — a 2046 action nobody has named — maps to `custom`,
/// the enum's own open member, never to a guessed action.
pub fn firewall_action_from_token(token: &str) -> network_capnp::FirewallAction {
    match token {
        "accept" => network_capnp::FirewallAction::Accept,
        "drop" => network_capnp::FirewallAction::Drop,
        "reject" => network_capnp::FirewallAction::Reject,
        "log" => network_capnp::FirewallAction::Log,
        _ => network_capnp::FirewallAction::Custom,
    }
}

/// The open token for a contract `FirewallAction`, so a read-back row spells
/// its action the way the host did.
pub fn firewall_action_token(action: network_capnp::FirewallAction) -> &'static str {
    match action {
        network_capnp::FirewallAction::Accept => "accept",
        network_capnp::FirewallAction::Drop => "drop",
        network_capnp::FirewallAction::Reject => "reject",
        network_capnp::FirewallAction::Log => "log",
        network_capnp::FirewallAction::Custom => "custom",
    }
}

/// The MEASURED result of the read-shaped privilege probe: what this uid may
/// actually do to the wifi control surface, as the host's authorisation
/// authority reports it — never an assumption and never a host write. Each
/// verb carries the one of three honest answers the authority's
/// `CheckAuthorization` reply can give.
///
/// The network name (`ssid`) and the probe's verdict travel together because a
/// connected wifi status and the authority to change it are read the same way:
/// measured, with the measured words kept verbatim.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiWritePrivilege {
    /// The `network-control` action (activate / disconnect / radio control):
    /// `"yes"` = authorised, `"no"` = refused, `"challenge"` = would prompt,
    /// `"unknown"` = the authority answered but not in a shape we map.
    pub network_control: String,
    /// The action adding a NEW connection needs (`settings.modify.system`):
    /// the verb `wifiConnect` to a network the host does not remember.
    pub modify_connection: String,
    /// Whether the authority answered at all. `false` ⇒ it could not be reached
    /// and NO verdict was measured (the columns stay empty, never guessed).
    pub measured: bool,
    /// The uid the verdict was issued for (read from `/proc/self/status`).
    pub uid: u32,
    /// The full measured detail, one line per action — the verbatim evidence.
    pub detail: String,
}

/// One network the host remembers (`KnownWifiNetwork`): the bookkeeping half
/// of `wifiConnect` — what is known, whether it joins automatically, and when
/// it last did.
///
/// **The credential is NOT here and can never be here.** `KnownWifiNetwork`
/// carries `ssid`, `autoConnect`, `lastConnected` and an open `security`
/// token; the secret stays a `T.SecretKeyId` reference owned by the host's
/// secret material, exactly as `wifiConnect @7`'s parameter does. A struct
/// with no credential field cannot leak one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownWifiNetwork {
    pub ssid: String,
    pub auto_connect: bool,
    /// The contract's `T.Timestamp` unit: nanoseconds since the epoch, 0 when
    /// the host did not report when it last connected (never fabricated).
    pub last_connected: u64,
    /// Security as an open token ("wpa3", "wpa2", "open", or a scheme nobody
    /// has named yet).
    pub security: String,
}

impl KnownWifiNetwork {
    /// Write into a generated `KnownWifiNetwork` builder.
    pub fn write_to(&self, builder: &mut network_capnp::known_wifi_network::Builder<'_>) {
        builder.set_ssid(&self.ssid);
        builder.set_auto_connect(self.auto_connect);
        builder.reborrow().init_last_connected().set_nanoseconds(self.last_connected);
        builder.set_security(&self.security);
    }

    /// Read from a generated `KnownWifiNetwork` reader.
    pub fn from_reader(reader: &network_capnp::known_wifi_network::Reader<'_>) -> Self {
        KnownWifiNetwork {
            ssid: text_field(reader.get_ssid()),
            auto_connect: reader.get_auto_connect(),
            last_connected: reader
                .get_last_connected()
                .map(|t| t.get_nanoseconds())
                .unwrap_or(0),
            security: text_field(reader.get_security()),
        }
    }
}

/// One entry in the host's routing table (`Route`): the destination in CIDR
/// text, the next hop (empty for a directly-connected route), the metric, and
/// the interface the kernel would send it out of.
///
/// The whole table for an interface is read with `getRoutes` and replaced
/// with `setRoutes` — the same shape as the firewall surface, for the same
/// reason: a full-set mutator is honest about being one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteInfo {
    /// "10.0.0.0/8"; "0.0.0.0/0" for the default route (the kernel's own
    /// spelling of "no destination", which is prefix length 0).
    pub destination_cidr: String,
    /// Next hop; empty for an on-link route.
    pub gateway: String,
    pub metric: u32,
    pub interface_id: NetworkInterfaceId,
}

impl RouteInfo {
    /// Write into a generated `Route` builder.
    pub fn write_to(&self, builder: &mut network_capnp::route::Builder<'_>) {
        builder.set_destination_cidr(&self.destination_cidr);
        builder.set_gateway(&self.gateway);
        builder.set_metric(self.metric);
        builder.reborrow().init_interface_id().set_id(self.interface_id.get());
    }

    /// Read from a generated `Route` reader.
    pub fn from_reader(reader: &network_capnp::route::Reader<'_>) -> Self {
        RouteInfo {
            destination_cidr: text_field(reader.get_destination_cidr()),
            gateway: text_field(reader.get_gateway()),
            metric: reader.get_metric(),
            interface_id: NetworkInterfaceId::new(
                reader.get_interface_id().map(|i| i.get_id()).unwrap_or(0),
            ),
        }
    }
}

/// The host's own proxy configuration (`ProxyConfig`) — not a fetch override
/// (`web.capnp` owns the fetch-scoped one). `url` is an open token
/// ("http://host:port", "socks5://host:port", or a scheme that does not exist
/// yet); `bypass` is the open set of destinations that never use it.
///
/// `bypass` is carried as ordered pairs rather than a `T.StringMap` because
/// the host sources spell it as a list (comma-separated `no_proxy`), and a map
/// would silently collapse duplicates the host actually keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyConfigInfo {
    pub enabled: bool,
    pub url: String,
    pub bypass: Vec<(String, String)>,
}

impl ProxyConfigInfo {
    /// Write into a generated `ProxyConfig` builder.
    pub fn write_to(&self, builder: &mut network_capnp::proxy_config::Builder<'_>) {
        builder.set_enabled(self.enabled);
        builder.set_url(&self.url);
        let mut entries = builder.reborrow().init_bypass().init_entries(self.bypass.len() as u32);
        for (i, (key, value)) in self.bypass.iter().enumerate() {
            let mut entry = entries.reborrow().get(i as u32);
            entry.set_key(key);
            entry.set_value(value);
        }
    }

    /// Read from a generated `ProxyConfig` reader.
    pub fn from_reader(reader: &network_capnp::proxy_config::Reader<'_>) -> Self {
        let mut bypass = Vec::new();
        if let Ok(entries) = reader.get_bypass().and_then(|m| m.get_entries()) {
            for entry in entries.iter() {
                bypass.push((text_field(entry.get_key()), text_field(entry.get_value())));
            }
        }
        ProxyConfigInfo {
            enabled: reader.get_enabled(),
            url: text_field(reader.get_url()),
            bypass,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeSyncInfo {
    pub offset: Duration,
    pub offset_measured: bool,
    pub stratum: u32,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SatelliteConfigEcho {
    pub constellation: String,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BandwidthSlotInfo {
    pub start_hour: u8,
    pub end_hour: u8,
    pub max_kbps: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DtnBundleInfo {
    pub bundle_id: String,
    pub destination: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellularConfigInfo {
    pub band: String,
    pub carrier: String,
    pub sa_mode: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ipv6ConfigInfo {
    pub slaac_enabled: bool,
    pub privacy_extensions: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WifiNanConfigEcho {
    pub service_name: String,
    pub mode: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NanPeerInfo {
    pub peer_id: String,
    pub name: String,
    pub rssi: i32,
}

/// A network-slice identifier (`NetworkSliceId { id :UInt64 }`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SliceId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SliceInfo {
    pub slice_id: SliceId,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopologyNodeInfo {
    pub interface: NetworkInterfaceId,
    pub label: String,
    pub reachable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopologyLinkInfo {
    pub source: NetworkInterfaceId,
    pub target: NetworkInterfaceId,
    pub link_type: String,
}

/// The captive-portal state (`CaptivePortalState`, honest subset).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptivePortalInfo {
    /// Open stage token ("noPortal", "portalPending", "signedIn", ...).
    pub stage: String,
    pub login_url: String,
}

/// Sampled telemetry (`NetworkTelemetry`, honest subset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TelemetryInfo {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub avg_latency: Duration,
}

/// An opaque NIC-driver provider handle (`nicProvider @71`). The contract's
/// capability accessor; this milestone does not build the driver surface, so it
/// is only ever handed back when a driver backend exists (otherwise
/// `Unavailable`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NicProviderHandle(pub u64);

/// The connect target (`NetworkConfig`, credentials stay as a `SecretKeyId`
/// reference — never cleartext).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectConfig {
    pub ssid: String,
    pub security: String,
    pub hidden: bool,
    pub remember: bool,
    pub auto_connect: bool,
}

/// The network-slice creation config (`SliceConfig`, honest subset).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SliceConfigEcho {
    pub name: String,
}

// ── Conversions to the frozen contract for the implemented seam ────────────

/// Read a `Text` field into an owned string. An absent or non-UTF-8 field is
/// the empty string: the same equivalence the writers below emit, so the
/// round-trip stays total (a peer's invalid UTF-8 is a protocol violation the
/// message validation catches, never fabricated over here).
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Read the domain host state out of a generated `hostState` Text field.
///
/// A non-UTF-8 `Text` is a peer protocol violation (message validation should
/// have caught it); it reads as [`HostState::Unknown`] rather than fabricating a
/// known state.
pub fn host_state_from_text(bytes: &[u8]) -> HostState {
    HostState::from_token(core::str::from_utf8(bytes).unwrap_or(""))
}
