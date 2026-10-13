//! The **kernel-first** network read seam (charter law #9: kernel first, daemon
//! for policy, contract for consumers).
//!
//! Everything the kernel is authoritative for — links, addresses, routes,
//! per-link counters, carrier, radio blocks — is read here through the
//! maintained `rust-netlink` crate ecosystem (`netlink-sys` for the socket,
//! `netlink-packet-route`/`netlink-packet-core` for the wire types), behind the
//! mockable [`NetlinkReader`] seam. The lesson of the hand-rolled D-Bus
//! marshaller (three wire bugs, `knowledge/reports/09-network.md` §4) is the
//! reason there is no hand-rolled netlink marshaller here: one crate ecosystem,
//! one seam, mock-swapable.
//!
//! ## What this module answers that NetworkManager cannot
//!
//! The kernel reports, per link: existence, the ifname, the link type, MTU,
//! hardware address, the administrative `IFF_UP` flag, `IFLA_CARRIER`,
//! `IFLA_OPERSTATE`, and `IFLA_STATS64` counters. Those facts arrive *without a
//! daemon hop*, and they arrive as **events** (`RTM_NEWLINK`/`RTM_NEWADDR` on
//! the `RTMGRP_LINK`/`RTMGRP_IPV4_IFADDR`/`RTMGRP_IPV6_IFADDR` groups) so the
//! state store is fed, not polled.
//!
//! Two state-classification facts are derived here **from kernel inputs only**
//! (never from NM policy), so the store can tell kernel causes from policy
//! causes:
//!
//! * **carrier-down** — `IFLA_CARRIER == 0` (nothing on the wire).
//! * **no-firmware** — a wireless link that is *administratively up* while the
//!   kernel reports the operstate DOWN with no carrier and no radio block: the
//!   host asked the radio up and the hardware never came up. NetworkManager
//!   reaches its `FirmwareMissing` verdict by *probing* (it sets `IFF_UP` and
//!   watches whether the driver keeps the link up) — a host-mutating probe we
//!   are banned from making and do not need: the kernel's own stuck-link view
//!   is the read-only equivalent.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every operation here is a read: opening the netlink socket, `RTM_GETLINK` /
//! `RTM_GETADDR` / `RTM_GETROUTE` dumps, subscribing to the multicast groups,
//! and reading `/dev/rfkill`. Nothing in this module writes host state — the
//! mutating netlink verbs (`RTM_SETLINK`, `RTM_NEWADDR`, `RTM_NEWROUTE`) are
//! deliberately absent from the seam and remain NM's job, behind its own seam.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use netlink_packet_core::{
    ErrorMessage, NetlinkBuffer, NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_DUMP,
    NLM_F_REQUEST, NLMSG_ALIGNTO, Parseable,
};
use netlink_packet_route::{
    address::{AddressAttribute, AddressMessage},
    link::{LinkAttribute, LinkFlags, LinkInfo, LinkLayerType, LinkMessage, State},
    route::{RouteAddress, RouteAttribute, RouteMessage},
    AddressFamily, RouteNetlinkMessage,
};
use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{Error, Result};
use crate::domains::network::types::InterfaceCounters;

// ═══════════════════════════════════════════════════════════════════════
// Kernel-authoritative value types (the store's vocabulary)
// ═══════════════════════════════════════════════════════════════════════

/// Nanoseconds since the epoch, the contract's `T.Timestamp` unit. A zero means
/// "the clock could not be read" — never fabricated.
pub(crate) fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Where a link's facts came from — the transport hierarchy made visible. A
/// `Kernel` link was enumerated from the netlink link table; an `NmFallback`
/// link is a fact NetworkManager reported because the kernel did not (the
/// fallback path, counted where it is used).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkProvenance {
    /// The kernel's own `RTM_GETLINK`/`RTM_NEWLINK` answer.
    Kernel,
    /// The D-Bus fallback sweep (NM's device view) — used only when the kernel
    /// link table does not answer.
    NmFallback,
}

/// The link technology as an open token, derived from the kernel's `ARPHRD_*`
/// link type (the same vocabulary the netlink wire itself uses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Ethernet,
    Wifi,
    Loopback,
    /// A real kernel link whose type nobody has named yet — reported, never
    /// guessed as a known kind.
    Other,
}

impl LinkKind {
    pub fn as_token(self) -> &'static str {
        match self {
            LinkKind::Ethernet => "ethernet",
            LinkKind::Wifi => "wifi",
            LinkKind::Loopback => "loopback",
            LinkKind::Other => "unknown",
        }
    }

    fn from_link_layer_type(t: &LinkLayerType) -> Self {
        match t {
            LinkLayerType::Ether => LinkKind::Ethernet,
            LinkLayerType::Ieee80211 => LinkKind::Wifi,
            LinkLayerType::Loopback => LinkKind::Loopback,
            _ => LinkKind::Other,
        }
    }

    /// The NM `DeviceType` equivalent, used only on the fallback path (when the
    /// kernel link table could not answer and the NM device view fills in).
    pub(crate) fn from_nm_device_type(t: u32) -> Self {
        match t {
            1 => LinkKind::Ethernet,
            2 => LinkKind::Wifi,
            32 => LinkKind::Loopback,
            _ => LinkKind::Other,
        }
    }
}

/// The kernel's `IFLA_OPERSTATE` (`/sys/class/net/*/operstate`), as the link
/// table reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperState {
    Unknown,
    NotPresent,
    Down,
    LowerLayerDown,
    Testing,
    Dormant,
    Up,
    Other(u8),
}

impl OperState {
    /// The link is usable: operstate UP or DORMANT (a wifi associated but idle
    /// reports DORMANT — it is running, not down).
    pub fn is_running(self) -> bool {
        matches!(self, OperState::Up | OperState::Dormant)
    }
}

impl From<&State> for OperState {
    fn from(s: &State) -> Self {
        match s {
            State::Unknown => OperState::Unknown,
            State::NotPresent => OperState::NotPresent,
            State::Down => OperState::Down,
            State::LowerLayerDown => OperState::LowerLayerDown,
            State::Testing => OperState::Testing,
            State::Dormant => OperState::Dormant,
            State::Up => OperState::Up,
            State::Other(v) => OperState::Other(*v),
            // The crate enum is non-exhaustive; a future kernel operstate
            // arrives here as an unmapped numeric, never as a guess.
            _ => OperState::Other(0),
        }
    }
}

/// One link, as the kernel reports it via `RTM_GETLINK`/`RTM_NEWLINK`. This is
/// the contract-side snapshot the store serves reads from; no field here comes
/// from a daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct KernelLink {
    /// The kernel's ifindex (stable for the interface's lifetime).
    pub index: u32,
    /// The OS ifname — the stable key the contract's interface id derives from.
    pub name: String,
    /// Link technology from the kernel's `ARPHRD_*` type.
    pub kind: LinkKind,
    /// `IFLA_LINKINFO` kind for virtual links ("bridge", "vlan", "veth",
    /// "tun", "wwan", ...); empty for physical links that carry none.
    pub info_kind: String,
    /// `IFF_UP` — administratively up (someone asked it up).
    pub up: bool,
    /// `IFLA_CARRIER` — `Some(false)` is the kernel's "nothing on the wire";
    /// `None` when the kernel does not report a carrier for this link.
    pub carrier: Option<bool>,
    /// `IFLA_OPERSTATE`.
    pub oper_state: OperState,
    pub mtu: u32,
    /// `IFLA_ADDRESS`, rendered "AA:BB:CC:DD:EE:FF"; empty when absent.
    pub hardware_address: String,
    /// `IFLA_STATS64` counters with `sampled_at` = read time; `None` when the
    /// kernel reported none.
    pub stats: Option<InterfaceCounters>,
    /// Where these facts came from (kernel netlink, or the counted fallback).
    pub provenance: LinkProvenance,
}

impl KernelLink {
    pub fn is_wifi(&self) -> bool {
        self.kind == LinkKind::Wifi
    }

    pub fn is_running(&self) -> bool {
        self.oper_state.is_running()
    }

    /// The kernel-authoritative no-firmware marker: the link is administratively
    /// up, the kernel reports it DOWN with no carrier, and there is no radio
    /// block (rfkill) to explain the hold. See the module docs for why this is
    /// the read-only equivalent of NM's probe-derived `FirmwareMissing`.
    pub fn stalled(&self) -> bool {
        self.up && self.oper_state == OperState::Down && self.carrier == Some(false)
    }

    /// The kernel-visible cellular marker: the `wwan` link kind (the interface
    /// the wwan subsystem registers for a modem).
    pub fn is_cellular(&self) -> bool {
        self.info_kind == "wwan"
    }

    /// A radio link (wifi, or the wwan net device). On a radio the kernel's
    /// `IFLA_CARRIER` / operstate mean "associated", not "the wire is
    /// plugged", so the carrier-down and no-firmware verdicts read them
    /// differently than they read a wired link.
    pub fn is_radio(&self) -> bool {
        self.is_wifi() || self.is_cellular()
    }

    /// The open token for this link's technology: the kernel `ARPHRD_*` type,
    /// except that a `wwan` link reports `cellular` — the wwan subsystem's net
    /// device IS the cellular data interface (the same token the NM device view
    /// used before the correction, now derived from the kernel's `IFLA_LINKINFO`).
    pub fn kind_token(&self) -> &'static str {
        if self.is_cellular() {
            "cellular"
        } else {
            self.kind.as_token()
        }
    }
}

/// One address, as the kernel reports it via `RTM_GETADDR`/`RTM_NEWADDR`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelAddress {
    /// The owning link's ifindex.
    pub index: u32,
    /// The rendered address ("192.168.18.128" / "fe80::b692:…").
    pub address: String,
    pub prefix_len: u8,
    /// "ipv4" or "ipv6".
    pub family: String,
}

/// One route, as the kernel reports it via `RTM_GETROUTE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelRoute {
    /// The output interface's ifindex (`RTA_OIF`).
    pub index: u32,
    /// The destination network in CIDR text form ("10.0.0.0/8"); the default
    /// route renders "0.0.0.0/0" — the kernel's own spelling of "no
    /// destination", which is prefix length 0 and an absent `RTA_DESTINATION`.
    pub destination: String,
    /// The gateway text form; empty for on-link routes.
    pub gateway: String,
    /// The routing table id.
    pub table: u32,
    /// The route metric / priority.
    pub metric: u32,
}

/// The kernel radio-block view of one radio (`/dev/rfkill`), kept apart from NM
/// policy so `hardBlocked`/`softBlocked` are never conflated with `enabled`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RfkillState {
    /// Open token: "wifi", "cellular", "bluetooth".
    pub name: String,
    pub hard_blocked: bool,
    pub soft_blocked: bool,
}

/// One full kernel sweep: every link, address and route the kernel has, plus
/// the rfkill radio-block view. Deliberately NOT per-device — one sweep feeds
/// the whole store.
#[derive(Clone, Debug, Default)]
pub struct KernelSnapshot {
    pub links: Vec<KernelLink>,
    pub addresses: Vec<KernelAddress>,
    pub routes: Vec<KernelRoute>,
    pub rfkill: Vec<RfkillState>,
}

/// A kernel event. The link table is the store's source of truth, and these
/// are the notifications that keep it current without a read round trip.
#[derive(Clone, Debug, PartialEq)]
pub enum KernelEvent {
    /// `RTM_NEWLINK` — a link appeared or changed (the kernel sends the full
    /// attribute set, including counters).
    Link(KernelLink),
    /// `RTM_DELLINK` — the interface with this ifindex is gone.
    LinkGone(u32),
    /// `RTM_NEWADDR` — an address appeared or changed.
    Address(KernelAddress),
    /// `RTM_DELADDR` — the address is gone.
    AddressGone(KernelAddress),
}

// ═══════════════════════════════════════════════════════════════════════
// The mockable kernel seam
// ═══════════════════════════════════════════════════════════════════════

/// Everything the network backend needs from the kernel. Three methods, mirroring
/// the three ways the kernel is read: one synchronous sweep (the store's priming
/// and the answer for anything the store has not been told yet), the blocking
/// event stream (for a caller that parks until the kernel has news), and the
/// non-blocking drain (what the store calls on every read, so a read never waits
/// on the socket).
pub trait NetlinkReader {
    /// One synchronous dump sweep: every link, address, route and rfkill entry.
    fn snapshot(&self) -> Result<KernelSnapshot>;

    /// The next kernel event, blocking until one arrives. Errors are
    /// channel-shaped (the socket died / netlink is unavailable) and end a
    /// blocking wait.
    fn next_event(&self) -> Result<KernelEvent>;

    /// Whatever the kernel has already queued, up to `max` events, WITHOUT
    /// waiting. This is the form the store uses: a read drains what arrived and
    /// returns immediately, so the read path is neither blocking nor polling.
    ///
    /// An `Err` means the event channel is not trustworthy (the socket died, or
    /// the kernel overran its notification queue — `ENOBUFS`); the caller must
    /// re-prime from [`NetlinkReader::snapshot`], never trust a partial view.
    fn drain_events(&self, max: usize) -> Result<Vec<KernelEvent>>;

    /// The kernel's routing table as it stands NOW: a fresh `RTM_GETROUTE`
    /// dump. Routes are not event-fed (the module doc explains why), so a
    /// route read is a kernel dump — no daemon hop, and always current rather
    /// than a table that went stale while the link table stayed fresh.
    fn routes(&self) -> Result<Vec<KernelRoute>>;
}

// ═══════════════════════════════════════════════════════════════════════
// The production kernel seam: rtnetlink over netlink-sys
// ═══════════════════════════════════════════════════════════════════════

/// The multicast groups the event socket subscribes to. Link changes plus the
/// address changes (v4 and v6) — the notifications the store needs to stay
/// current. Neighbour/route groups are deliberately NOT subscribed: the store
/// reads routes on demand, and routing-table change storms are not status.
///
/// These are the kernel UAPI `RTMGRP_*` bitmasks (`linux/rtnetlink.h`), stable
/// since 2.6; the values match the `rtnetlink` crate's own `constants` module.
const RTMGRP_LINK: u32 = 1;
const RTMGRP_IPV4_IFADDR: u32 = 16;
const RTMGRP_IPV6_IFADDR: u32 = 256;

/// The default rfkill device (the kernel's own radio-block source; its netlink
/// group has no packet-crate support, so the char device is the read).
const DEFAULT_RFKILL_PATH: &str = "/dev/rfkill";

/// `ENOBUFS` (`linux/errno.h`): the kernel dropped netlink notifications because
/// the reader's socket buffer overran. The literal is used because this crate
/// does not depend on `libc` directly (netlink-sys does); the value is UAPI and
/// has never moved.
const ENOBUFS: i32 = 105;

/// The sysfs tree that carries the per-link facts rtnetlink does not: the
/// wireless marker below.
const SYSFS_NET_ROOT: &str = "/sys/class/net";

/// The production [`NetlinkReader`]: one socket for dumps, one socket bound to
/// the link/address multicast groups for events. Opening either socket failing
/// is not fatal — the reader reports `unavailable` per call and the backend's
/// reads fall back to NM (law #9's fallback, counted).
pub struct RealNetlinkReader {
    /// Request socket: `RTM_GETLINK`/`GETADDR`/`GETROUTE` dumps.
    dump: Option<Socket>,
    /// Event socket: bound to the RTMGRP_* groups, receives kernel events.
    events: Option<Socket>,
    /// Events already parsed off the socket but not yet consumed. One datagram
    /// can carry several netlink messages, so a blocking wait that takes the
    /// first must keep the rest (never silently dropped).
    pending: Mutex<Vec<KernelEvent>>,
    /// Why `dump`/`events` is absent, if so (reported on each call, never
    /// silently returning empty facts).
    open_error: Option<String>,
    /// The rfkill device to read (overridable for tests).
    rfkill_path: PathBuf,
    /// The sysfs root that carries the per-link wireless marker.
    sysfs_root: PathBuf,
    /// Request sequence counter, so a late reply to an old dump is skipped.
    seq: AtomicU32,
}

impl RealNetlinkReader {
    /// Open both sockets and bind the event socket to the link/address groups.
    pub fn new() -> Self {
        Self::with_roots(&PathBuf::from(DEFAULT_RFKILL_PATH), &PathBuf::from(SYSFS_NET_ROOT))
    }

    /// [`RealNetlinkReader::new`] with an explicit rfkill path (tests).
    pub fn with_rfkill(rfkill_path: &std::path::Path) -> Self {
        Self::with_roots(rfkill_path, &PathBuf::from(SYSFS_NET_ROOT))
    }

    /// [`RealNetlinkReader::new`] with explicit rfkill and sysfs roots (tests:
    /// a hermetic sysfs tree can carry the wireless marker).
    pub fn with_roots(rfkill_path: &std::path::Path, sysfs_root: &std::path::Path) -> Self {
        let dump = match Socket::new(NETLINK_ROUTE) {
            Ok(s) => Some(s),
            Err(e) => {
                return Self::unavailable(format!("netlink dump socket: {e}"), rfkill_path)
            }
        };
        let events = match Socket::new(NETLINK_ROUTE) {
            Ok(s) => Some(s),
            Err(e) => return Self::unavailable(format!("netlink event socket: {e}"), rfkill_path),
        };
        let mut reader = RealNetlinkReader {
            dump,
            events,
            pending: Mutex::new(Vec::new()),
            open_error: None,
            rfkill_path: rfkill_path.to_path_buf(),
            sysfs_root: sysfs_root.to_path_buf(),
            seq: AtomicU32::new(1),
        };
        if let Err(e) = reader.bind_events() {
            reader.open_error = Some(format!("netlink group bind: {e}"));
            reader.events = None;
        }
        reader
    }

    fn unavailable(reason: String, rfkill_path: &std::path::Path) -> Self {
        RealNetlinkReader {
            dump: None,
            events: None,
            pending: Mutex::new(Vec::new()),
            open_error: Some(reason),
            rfkill_path: rfkill_path.to_path_buf(),
            sysfs_root: PathBuf::from(SYSFS_NET_ROOT),
            seq: AtomicU32::new(1),
        }
    }

    /// Apply every per-link kernel fact the wire does not carry.
    fn enrich_link(&self, link: KernelLink) -> KernelLink {
        enrich_link(&self.sysfs_root, link)
    }

    /// Parse one datagram's events with the per-link kernel facts applied.
    ///
    /// BUG 1's fix. The kernel reports a wlan interface's `ARPHRD_*` type as
    /// `ether` — the wireless-ness lives in the phy, which rtnetlink does not
    /// carry — so the snapshot path upgrades a link's kind from sysfs
    /// (`/sys/class/net/<name>/wireless`). The EVENT path parsed the wire
    /// only, so an `RTM_NEWLINK` event silently re-typed a wifi interface as
    /// `ethernet` in the store: `getStatus` then answered `ethernet` while
    /// `getInterfaces` answered `wifi` — the two surfaces disagreed on the
    /// type's source. Both surfaces now read the type from the same place:
    /// the kernel's link type, plus the kernel's own sysfs marker.
    fn parse_events(&self, bytes: &[u8]) -> Result<Vec<KernelEvent>> {
        let events = parse_event_datagram(bytes)?;
        Ok(events
            .into_iter()
            .map(|event| match event {
                KernelEvent::Link(link) => KernelEvent::Link(self.enrich_link(link)),
                other => other,
            })
            .collect())
    }

    /// Take the next already-parsed event, or `None` when none is pending.
    fn take_pending(&self) -> Option<KernelEvent> {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            None
        } else {
            Some(pending.remove(0))
        }
    }

    /// Receive one datagram and fold every event it carries into `pending`,
    /// returning how many were added.
    fn receive_events(&self) -> Result<usize> {
        let Some(socket) = self.events.as_ref() else {
            return Err(Error::unavailable(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| "netlink event socket unavailable".to_string()),
            ));
        };
        let (bytes, _from) = socket
            .recv_from_full()
            .map_err(|e| Error::unavailable(format!("netlink event recv: {e}")))?;
        let events = self.parse_events(&bytes)?;
        let n = events.len();
        self.pending.lock().unwrap().extend(events);
        Ok(n)
    }

    fn bind_events(&mut self) -> std::io::Result<()> {
        let groups = RTMGRP_LINK | RTMGRP_IPV4_IFADDR | RTMGRP_IPV6_IFADDR;
        let Some(socket) = self.events.as_mut() else {
            return Ok(());
        };
        socket.bind(&SocketAddr::new(0, groups))
    }

    /// Send one `NLM_F_DUMP` request and collect its replies until `NLMSG_DONE`.
    /// A kernel `NLMSG_ERROR` is mapped to a `core::Error`; an unrelated reply
    /// (a stray sequence number) is skipped, never misread as data.
    ///
    /// One received datagram can carry several netlink messages — the kernel
    /// packs a dump's replies that way — so every message in the datagram is
    /// parsed. Parsing only the first silently loses links, which is exactly
    /// what the live read-only probe caught on a three-interface host.
    fn dump(&self, request: RouteNetlinkMessage) -> Result<Vec<RouteNetlinkMessage>> {
        let Some(socket) = self.dump.as_ref() else {
            return Err(Error::unavailable(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| "netlink dump socket unavailable".to_string()),
            ));
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut message = NetlinkMessage::from(request);
        message.header.sequence_number = seq;
        message.header.port_number = 0; // to the kernel
        message.header.flags = NLM_F_REQUEST | NLM_F_DUMP;
        message.finalize();
        let mut wire = alloc::vec![0u8; message.buffer_len()];
        message.serialize(&mut wire);
        socket
            .send_to(&wire, &SocketAddr::new(0, 0), 0)
            .map_err(|e| Error::failed(format!("netlink dump send: {e}")))?;

        let mut replies = Vec::new();
        loop {
            let (bytes, _from) = socket
                .recv_from_full()
                .map_err(|e| Error::failed(format!("netlink dump recv: {e}")))?;
            for parsed in parse_datagram(&bytes)? {
                if parsed.header.sequence_number != seq {
                    continue; // a stale reply from an earlier request
                }
                match parsed.payload {
                    NetlinkPayload::Done(_) => return Ok(replies),
                    NetlinkPayload::Error(err) => return Err(map_nlmsgerr(&err)),
                    NetlinkPayload::InnerMessage(inner) => replies.push(inner),
                    NetlinkPayload::Noop | NetlinkPayload::Overrun(_) => {}
                    // The payload enum is non-exhaustive (new message kinds
                    // arrive with the crate); anything else is skipped, not
                    // misread.
                    _ => {}
                }
            }
        }
    }
}

impl Default for RealNetlinkReader {
    fn default() -> Self {
        Self::new()
    }
}

impl NetlinkReader for RealNetlinkReader {
    fn snapshot(&self) -> Result<KernelSnapshot> {
        let links = self.dump(RouteNetlinkMessage::GetLink(LinkMessage::default()))?;
        let addresses = self.dump(RouteNetlinkMessage::GetAddress(AddressMessage::default()))?;
        let routes = self.dump(RouteNetlinkMessage::GetRoute(RouteMessage::default()))?;
        let (links, addresses, routes) = fold_dump_replies(links, addresses, routes);
        Ok(KernelSnapshot {
            links: links.into_iter().map(|l| self.enrich_link(l)).collect(),
            addresses,
            routes,
            rfkill: self.rfkill()?,
        })
    }

    fn routes(&self) -> Result<Vec<KernelRoute>> {
        let replies = self.dump(RouteNetlinkMessage::GetRoute(RouteMessage::default()))?;
        let (_, _, routes) = fold_dump_replies(Vec::new(), Vec::new(), replies);
        Ok(routes)
    }

    fn next_event(&self) -> Result<KernelEvent> {
        loop {
            if let Some(event) = self.take_pending() {
                return Ok(event);
            }
            if self.receive_events()? == 0 {
                // Not store-relevant (DONE / NOOP): wait for the next datagram.
                continue;
            }
        }
    }

    fn drain_events(&self, max: usize) -> Result<Vec<KernelEvent>> {
        let Some(socket) = self.events.as_ref() else {
            return Err(Error::unavailable(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| "netlink event socket unavailable".to_string()),
            ));
        };
        let mut events: Vec<KernelEvent> = Vec::new();
        // What a previous blocking wait already parsed is served first.
        events.extend(self.take_pending().into_iter());
        // The drain must never wait: the kernel's answer for "nothing queued" is
        // EAGAIN, which we read as an empty batch (not an error, not a block).
        socket
            .set_non_blocking(true)
            .map_err(|e| Error::failed(format!("netlink drain: {e}")))?;
        while events.len() < max {
            match socket.recv_from_full() {
                Ok((bytes, _from)) => match self.parse_events(&bytes) {
                    Ok(mut parsed) => events.append(&mut parsed),
                    Err(e) => {
                        let _ = socket.set_non_blocking(false);
                        return Err(e);
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // ENOBUFS: the kernel overran its notification queue. The view
                // we hold is now missing events, so the drain reports the channel
                // as untrustworthy and the caller re-primes from a full dump.
                Err(e) if e.raw_os_error() == Some(ENOBUFS) => {
                    let _ = socket.set_non_blocking(false);
                    return Err(Error::temporary(
                        "netlink event overrun (ENOBUFS): the kernel dropped notifications; a full re-dump is required",
                    ));
                }
                Err(e) => {
                    let _ = socket.set_non_blocking(false);
                    return Err(Error::temporary(format!("netlink event recv: {e}")));
                }
            }
        }
        let _ = socket.set_non_blocking(false);
        Ok(events)
    }
}

impl RealNetlinkReader {
    /// Read the kernel's rfkill device (a stream of 8-byte events, no header)
    /// and fold it into per-radio block states.
    fn rfkill(&self) -> Result<Vec<RfkillState>> {
        use std::io::Read;
        let mut buf = [0u8; 8 * 128];
        let n = match std::fs::File::open(&self.rfkill_path) {
            Ok(mut f) => f.read(&mut buf).unwrap_or(0),
            Err(_) => 0,
        };
        Ok(parse_rfkill_events(&buf[..n]))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The kernel WRITE seam (one verb: set a link's MTU).
//
// This is the second half of the kernel-first story, and it is deliberately
// tiny. `setInterfaceMtu @76` is the one contract verb whose home is
// unambiguous: the attribute lives on the link, the kernel reads it back as
// `IFLA_MTU`, and NetworkManager has no MTU setter of its own — it writes the
// same netlink attribute after a settings-connection update that would be a
// whole second surface. So the write goes to the kernel, through the same
// maintained crate ecosystem as every read, behind the same mockable-seam
// shape.
//
// Law #8 governs its use exactly as it governs the NM control verbs: nothing
// in build or test runs it (the production constructor is only reached from
// the daemon or the founder's probe), the mock proves the mapping, and the
// live proof is founder-run.
// ═══════════════════════════════════════════════════════════════════════

/// Everything the network backend needs to *write* to the kernel.
pub trait NetlinkWriter {
    /// Set one link's MTU (`RTM_NEWLINK` carrying `IFLA_MTU`). A host-mutating
    /// verb: the production implementation really does change the host.
    fn set_link_mtu(&self, index: u32, mtu: u32) -> Result<()>;
}

/// The production [`NetlinkWriter`]: one netlink socket, one `RTM_NEWLINK`
/// transaction per call.
pub struct RealNetlinkWriter {
    socket: Option<Socket>,
    open_error: Option<String>,
    seq: AtomicU32,
}

impl RealNetlinkWriter {
    /// Open the write socket. A socket that cannot be opened is not fatal:
    /// every call answers `unavailable` with the reason, and the caller can
    /// report the gap instead of pretending the write landed.
    pub fn new() -> Self {
        match Socket::new(NETLINK_ROUTE) {
            Ok(socket) => RealNetlinkWriter {
                socket: Some(socket),
                open_error: None,
                seq: AtomicU32::new(1),
            },
            Err(e) => RealNetlinkWriter {
                socket: None,
                open_error: Some(format!("netlink write socket: {e}")),
                seq: AtomicU32::new(1),
            },
        }
    }

    /// Send one message with `NLM_F_ACK` and wait for the kernel's answer. An
    /// `NLMSG_ERROR` with no code is an ACK (success); a non-zero code is the
    /// kernel's own refusal, mapped verbatim.
    fn transact(&self, request: RouteNetlinkMessage) -> Result<()> {
        let Some(socket) = self.socket.as_ref() else {
            return Err(Error::unavailable(
                self.open_error
                    .clone()
                    .unwrap_or_else(|| "netlink write socket unavailable".to_string()),
            ));
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut message = NetlinkMessage::from(request);
        message.header.sequence_number = seq;
        message.header.port_number = 0;
        message.header.flags = NLM_F_REQUEST | NLM_F_ACK;
        message.finalize();
        let mut wire = alloc::vec![0u8; message.buffer_len()];
        message.serialize(&mut wire);
        socket
            .send_to(&wire, &SocketAddr::new(0, 0), 0)
            .map_err(|e| Error::failed(format!("netlink write send: {e}")))?;
        loop {
            let (bytes, _from) = socket
                .recv_from_full()
                .map_err(|e| Error::failed(format!("netlink write recv: {e}")))?;
            for parsed in parse_datagram(&bytes)? {
                if parsed.header.sequence_number != seq {
                    continue; // a stale reply to an earlier transaction
                }
                match parsed.payload {
                    // The ACK: an NLMSG_ERROR with no code is the kernel
                    // saying "accepted", not an error.
                    NetlinkPayload::Error(ref err) if err.code.is_none() => return Ok(()),
                    NetlinkPayload::Error(ref err) => return Err(map_nlmsgerr(err)),
                    _ => {}
                }
            }
        }
    }
}

impl Default for RealNetlinkWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl NetlinkWriter for RealNetlinkWriter {
    fn set_link_mtu(&self, index: u32, mtu: u32) -> Result<()> {
        let mut link = LinkMessage::default();
        link.header.index = index;
        link.attributes = alloc::vec![LinkAttribute::Mtu(mtu)];
        self.transact(RouteNetlinkMessage::NewLink(link))
    }
}

/// Return the rfkill entries discovered on the host (public, for the read-only
/// live proof and for callers that want the raw radio-block view).
pub fn read_rfkill_readonly() -> Result<Vec<RfkillState>> {
    RealNetlinkReader::new().rfkill()
}

// ═══════════════════════════════════════════════════════════════════════
// The scripted kernel behind the seam (tests only).
//
// Shared by this module's wire-parsing tests and by the backend's store tests
// in `linux/network.rs`, so the mock-kernel vocabulary exists once. Every
// fixture is hermetic (law #8): no socket, no /dev, no host state.
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
pub(crate) struct MockNetlink {
    /// The snapshot the next `snapshot()` answers.
    pub(crate) snapshot: KernelSnapshot,
    /// The queued events; an empty queue ends a pump batch.
    pub(crate) events: Mutex<Vec<KernelEvent>>,
    /// An injectable channel fault, returned by every seam method.
    pub(crate) fault: Mutex<Option<Error>>,
}

#[cfg(test)]
impl MockNetlink {
    /// A scripted kernel answering `snapshot` with the given facts.
    pub(crate) fn with_snapshot(snapshot: KernelSnapshot) -> Self {
        MockNetlink {
            snapshot,
            events: Mutex::new(Vec::new()),
            fault: Mutex::new(None),
        }
    }

    /// Queue one kernel event for the pump to apply.
    pub(crate) fn queue_event(&self, event: KernelEvent) {
        self.events.lock().unwrap().push(event);
    }

    /// Inject a channel fault (the socket died / netlink is unavailable).
    pub(crate) fn fail_with(&self, error: Error) {
        *self.fault.lock().unwrap() = Some(error);
    }

    /// Clear an injected fault: the scripted kernel answers again (the channel
    /// came back), which is what a store's recovery path must notice.
    pub(crate) fn clear_failure(&self) {
        *self.fault.lock().unwrap() = None;
    }

    fn check(&self) -> Result<()> {
        match self.fault.lock().unwrap().as_ref() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
impl NetlinkReader for MockNetlink {
    fn snapshot(&self) -> Result<KernelSnapshot> {
        self.check()?;
        Ok(self.snapshot.clone())
    }

    fn routes(&self) -> Result<Vec<KernelRoute>> {
        self.check()?;
        Ok(self.snapshot.routes.clone())
    }

    fn next_event(&self) -> Result<KernelEvent> {
        self.check()?;
        let mut events = self.events.lock().unwrap();
        if events.is_empty() {
            return Err(Error::temporary("mock netlink: no events queued"));
        }
        Ok(events.remove(0))
    }

    fn drain_events(&self, max: usize) -> Result<Vec<KernelEvent>> {
        self.check()?;
        let mut events = self.events.lock().unwrap();
        let n = events.len().min(max);
        Ok(events.drain(..n).collect())
    }
}

/// A shareable handle on a [`MockNetlink`]. A test that must queue kernel
/// events AFTER the store took the reader (a subscription that spans events, a
/// carrier that drops mid-read) keeps this clone and drives the scripted kernel
/// through it. The production reader is never shared: the store owns its socket.
#[cfg(test)]
impl NetlinkReader for std::sync::Arc<MockNetlink> {
    fn snapshot(&self) -> Result<KernelSnapshot> {
        (**self).snapshot()
    }

    fn routes(&self) -> Result<Vec<KernelRoute>> {
        (**self).routes()
    }

    fn next_event(&self) -> Result<KernelEvent> {
        (**self).next_event()
    }

    fn drain_events(&self, max: usize) -> Result<Vec<KernelEvent>> {
        (**self).drain_events(max)
    }
}

/// A scripted kernel write seam (tests only): records the MTU writes a backend
/// asks for, injects channel faults, and — like every mock here — never
/// touches the host (law #8).
#[cfg(test)]
pub(crate) struct MockNetlinkWriter {
    pub(crate) writes: Mutex<Vec<(u32, u32)>>,
    pub(crate) fault: Mutex<Option<Error>>,
}

#[cfg(test)]
impl MockNetlinkWriter {
    /// A writer with nothing recorded yet.
    pub(crate) fn new() -> Self {
        MockNetlinkWriter {
            writes: Mutex::new(Vec::new()),
            fault: Mutex::new(None),
        }
    }

    /// Inject a write fault (the socket died, the kernel refused).
    pub(crate) fn fail_with(&self, error: Error) {
        *self.fault.lock().unwrap() = Some(error);
    }

    fn check(&self) -> Result<()> {
        match self.fault.lock().unwrap().as_ref() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
impl NetlinkWriter for MockNetlinkWriter {
    fn set_link_mtu(&self, index: u32, mtu: u32) -> Result<()> {
        self.check()?;
        self.writes.lock().unwrap().push((index, mtu));
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Wire parsing (the maintained crates do the marshalling; this maps their
// types onto the store's vocabulary)
// ═══════════════════════════════════════════════════════════════════════

/// Parse one `LinkMessage` (a dump reply or an `RTM_NEWLINK` event) into the
/// store's link record.
pub fn parse_link_message(msg: &LinkMessage) -> KernelLink {
    let mut link = KernelLink {
        index: msg.header.index,
        name: String::new(),
        kind: LinkKind::from_link_layer_type(&msg.header.link_layer_type),
        info_kind: String::new(),
        up: msg.header.flags.contains(LinkFlags::Up),
        carrier: None,
        oper_state: OperState::from(&State::Unknown),
        mtu: 0,
        hardware_address: String::new(),
        stats: None,
        provenance: LinkProvenance::Kernel,
    };
    for attr in &msg.attributes {
        match attr {
            LinkAttribute::IfName(name) => link.name = name.clone(),
            LinkAttribute::Mtu(mtu) => link.mtu = *mtu,
            LinkAttribute::Address(bytes) => link.hardware_address = render_hardware_address(bytes),
            LinkAttribute::Carrier(c) => link.carrier = Some(*c != 0),
            LinkAttribute::OperState(s) => link.oper_state = OperState::from(s),
            LinkAttribute::Stats(s) => {
                link.stats = Some(InterfaceCounters {
                    bytes_in: u64::from(s.rx_bytes),
                    bytes_out: u64::from(s.tx_bytes),
                    packets_in: u64::from(s.rx_packets),
                    packets_out: u64::from(s.tx_packets),
                    errors_in: u64::from(s.rx_errors),
                    errors_out: u64::from(s.tx_errors),
                    drops_in: u64::from(s.rx_dropped),
                    drops_out: u64::from(s.tx_dropped),
                    sampled_at: now_nanos(),
                })
            }
            LinkAttribute::Stats64(s) => {
                link.stats = Some(InterfaceCounters {
                    bytes_in: s.rx_bytes,
                    bytes_out: s.tx_bytes,
                    packets_in: s.rx_packets,
                    packets_out: s.tx_packets,
                    errors_in: s.rx_errors,
                    errors_out: s.tx_errors,
                    drops_in: s.rx_dropped,
                    drops_out: s.tx_dropped,
                    sampled_at: now_nanos(),
                })
            }
            LinkAttribute::LinkInfo(infos) => {
                for info in infos {
                    if let LinkInfo::Kind(kind) = info {
                        link.info_kind = kind.to_string();
                    }
                }
            }
            _ => {}
        }
    }
    link
}

/// Parse one `AddressMessage` into the store's address record.
pub fn parse_address_message(msg: &AddressMessage) -> KernelAddress {
    let mut address = KernelAddress {
        index: msg.header.index,
        address: String::new(),
        prefix_len: msg.header.prefix_len,
        family: match msg.header.family {
            netlink_packet_route::AddressFamily::Inet => "ipv4",
            _ => "ipv6",
        }
        .to_string(),
    };
    for attr in &msg.attributes {
        if let AddressAttribute::Address(ip) = attr {
            address.address = ip.to_string();
        }
    }
    address
}

/// Parse one `RouteMessage` into the store's route record. The destination is
/// rendered in the contract's CIDR spelling: the address the kernel named plus
/// the prefix length from the message header. A default route — no
/// `RTA_DESTINATION`, prefix length 0 — renders "0.0.0.0/0" (or "::/0"), which
/// is the kernel's own truth, not a convention invented here.
pub fn parse_route_message(msg: &RouteMessage) -> KernelRoute {
    let mut route = KernelRoute {
        index: 0,
        destination: String::new(),
        gateway: String::new(),
        table: u32::from(msg.header.table),
        metric: 0,
    };
    for attr in &msg.attributes {
        match attr {
            RouteAttribute::Destination(dest) => route.destination = render_route_address(dest),
            RouteAttribute::Gateway(gw) => route.gateway = render_route_address(gw),
            RouteAttribute::Oif(index) => route.index = *index,
            RouteAttribute::Priority(metric) => route.metric = *metric,
            _ => {}
        }
    }
    if !route.destination.is_empty() {
        route.destination = format!("{}/{}", route.destination, msg.header.destination_prefix_length);
    } else if msg.header.destination_prefix_length == 0 {
        // No destination attribute: the default route of this address family.
        route.destination = match msg.header.address_family {
            AddressFamily::Inet6 => "::/0".to_string(),
            _ => "0.0.0.0/0".to_string(),
        };
    }
    route
}

/// Fold the three dumps' replies into the store's snapshot vocabulary.
///
/// The kernel answers a `RTM_GETLINK`/`RTM_GETADDR`/`RTM_GETROUTE` dump with the
/// `RTM_NEWLINK`/`RTM_NEWADDR`/`RTM_NEWROUTE` message kinds — the same messages
/// an event carries — so the fold accepts both shapes (a daemon or a test may
/// echo the request kind back) and skips anything else, never misreading a
/// stray message as data.
fn fold_dump_replies(
    link_replies: Vec<RouteNetlinkMessage>,
    address_replies: Vec<RouteNetlinkMessage>,
    route_replies: Vec<RouteNetlinkMessage>,
) -> (Vec<KernelLink>, Vec<KernelAddress>, Vec<KernelRoute>) {
    let mut links = Vec::new();
    for message in link_replies {
        match message {
            RouteNetlinkMessage::NewLink(link) | RouteNetlinkMessage::GetLink(link) => {
                links.push(parse_link_message(&link))
            }
            _ => {}
        }
    }
    let mut addresses = Vec::new();
    for message in address_replies {
        match message {
            RouteNetlinkMessage::NewAddress(addr) | RouteNetlinkMessage::GetAddress(addr) => {
                addresses.push(parse_address_message(&addr))
            }
            _ => {}
        }
    }
    let mut routes = Vec::new();
    for message in route_replies {
        match message {
            RouteNetlinkMessage::NewRoute(route) | RouteNetlinkMessage::GetRoute(route) => {
                routes.push(parse_route_message(&route))
            }
            _ => {}
        }
    }
    (links, addresses, routes)
}

/// The kernel's own wireless marker for one link: a wlan device has a
/// `/sys/class/net/<name>/wireless` directory (and a `phy80211` link to its phy).
/// rtnetlink's `ARPHRD_*` link type is `ether` for a normal wlan interface — the
/// wireless-ness lives in the phy, not in the link type — so without this marker
/// the "wifi" token would be wrong for exactly the interfaces the incident was
/// about. A sysfs read is a kernel read: no daemon hop, and no guess.
fn is_wireless(sysfs_root: &std::path::Path, name: &str) -> bool {
    sysfs_root.join(name).join("wireless").is_dir()
}

/// Apply the per-link kernel facts the wire does not carry. A `wwan` link keeps
/// its cellular identity (the wwan net device IS the cellular interface).
fn enrich_link(sysfs_root: &std::path::Path, mut link: KernelLink) -> KernelLink {
    if !link.is_cellular() && is_wireless(sysfs_root, &link.name) {
        link.kind = LinkKind::Wifi;
    }
    link
}

/// The NLMSG alignment stride (the kernel pads every netlink message to 4).
fn nlmsg_align(length: usize) -> usize {
    let align = usize::from(NLMSG_ALIGNTO);
    let rem = length % align;
    if rem == 0 {
        length
    } else {
        length + (align - rem)
    }
}

/// Parse EVERY netlink message in one received datagram, in order.
///
/// One datagram can carry several messages — the kernel packs a dump's replies
/// into as few datagrams as fit, and the crate's `deserialize` parses exactly
/// one message per buffer (ignoring whatever follows it). The stride between
/// messages is the header's own length field, the same field the crate itself
/// emits; reading it to slice the datagram is framing, not a second marshaller.
/// A length that does not fit the datagram is an error, never a truncation.
fn parse_datagram(bytes: &[u8]) -> Result<Vec<NetlinkMessage<RouteNetlinkMessage>>> {
    let mut messages = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        // A trailing fragment shorter than a netlink header is alignment
        // padding (the kernel pads every message to 4 bytes), not a truncated
        // message: stop cleanly rather than fail the whole datagram.
        if rest.len() < 16 {
            break;
        }
        let buffer = NetlinkBuffer::new_checked(rest)
            .map_err(|e| Error::failed(format!("netlink parse header: {e}")))?;
        let header = netlink_packet_core::NetlinkHeader::parse(&buffer)
            .map_err(|e| Error::failed(format!("netlink parse header: {e}")))?;
        let length = header.length as usize;
        // A netlink header is 16 bytes; anything shorter is a desync, and a
        // message longer than the datagram is a truncated read.
        if length < 16 || length > rest.len() {
            return Err(Error::failed(format!(
                "netlink framing: message length {length} does not fit the {} bytes left",
                rest.len()
            )));
        }
        let message = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&rest[..length])
            .map_err(|e| Error::failed(format!("netlink parse: {e}")))?;
        messages.push(message);
        offset += nlmsg_align(length);
    }
    Ok(messages)
}

/// Parse one received datagram (a dump reply or a multicast event) into the
/// store's event vocabulary. `Ok(vec![])` means "nothing store-relevant"
/// (`NLMSG_DONE`, `NLMSG_NOOP`, a route dump reply); an `NLMSG_ERROR` NACK is a
/// mapped `core::Error`. Several events may come back from one datagram.
pub fn parse_event_datagram(bytes: &[u8]) -> Result<Vec<KernelEvent>> {
    let mut events = Vec::new();
    for message in parse_datagram(bytes)? {
        match message.payload {
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link))
            | NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetLink(link)) => {
                events.push(KernelEvent::Link(parse_link_message(&link)))
            }
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelLink(link)) => {
                events.push(KernelEvent::LinkGone(link.header.index))
            }
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress(addr))
            | NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetAddress(addr)) => {
                events.push(KernelEvent::Address(parse_address_message(&addr)))
            }
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelAddress(addr)) => {
                events.push(KernelEvent::AddressGone(parse_address_message(&addr)))
            }
            // A route dump reply: the store does not model routes as events.
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetRoute(_route)) => {}
            NetlinkPayload::Error(err) => return Err(map_nlmsgerr(&err)),
            _ => {}
        }
    }
    Ok(events)
}

/// The single-message form of [`parse_event_datagram`]: the first store-relevant
/// event in one netlink message, or `None` when the message carries none.
pub fn parse_event_packet(bytes: &[u8]) -> Result<Option<KernelEvent>> {
    let mut events = parse_event_datagram(bytes)?;
    Ok(if events.is_empty() { None } else { Some(events.remove(0)) })
}

fn map_nlmsgerr(err: &ErrorMessage) -> Error {
    match err.code {
        Some(code) => Error::failed(format!("netlink kernel error: -{code}")),
        None => Error::failed("netlink kernel error (no code)"),
    }
}

/// Render `IFLA_ADDRESS` bytes as the colon-hex form the contract's
/// `T.MacAddress` uses (the same rendering NM reports).
fn render_hardware_address(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

fn render_route_address(addr: &RouteAddress) -> String {
    match addr {
        RouteAddress::Inet(v4) => v4.to_string(),
        RouteAddress::Inet6(v6) => v6.to_string(),
        RouteAddress::Other(bytes) => format!("{bytes:?}"),
        RouteAddress::Mpls(_) => "mpls".to_string(),
        _ => String::new(),
    }
}

/// Parse a batch of kernel `struct rfkill_event` records (8 bytes each:
/// `{ u32 idx; u8 type; u8 op; u8 soft; u8 hard; }`) into per-radio states,
/// keeping `hard` and `soft` separate from one another. Types mapped: 1=wifi,
/// 2=bluetooth, 5=cellular; others dropped (no honest name).
pub fn parse_rfkill_events(buf: &[u8]) -> Vec<RfkillState> {
    #[derive(Default)]
    struct Acc {
        name: &'static str,
        hard: bool,
        soft: bool,
    }
    let name_for = |t: u8| -> &'static str {
        match t {
            1 => "wifi",
            2 => "bluetooth",
            5 => "cellular",
            _ => "unknown",
        }
    };
    let mut map: alloc::collections::BTreeMap<u8, Acc> = alloc::collections::BTreeMap::new();
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let kind = buf[off + 4];
        let soft = buf[off + 6] != 0;
        let hard = buf[off + 7] != 0;
        let e = map.entry(kind).or_default();
        e.name = name_for(kind);
        e.hard = hard;
        e.soft = soft;
        off += 8;
    }
    map.into_values()
        .filter(|a| a.name != "unknown")
        .map(|a| RfkillState {
            name: a.name.to_string(),
            hard_blocked: a.hard,
            soft_blocked: a.soft,
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Hermetic, mock-based behavior tests (law #8: none touch the host).
//
// The fixtures are built with the crate ecosystem itself (production
// serialization), including raw `IFLA_*` attributes where the crates keep the
// type non-exhaustive — the same technique the crates' own test suites use.
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use netlink_packet_core::DefaultNla;
    use netlink_packet_route::address::{AddressAttribute, AddressMessage};
    use netlink_packet_route::link::{
        LinkAttribute, LinkFlags, LinkLayerType, LinkMessage,
    };
    use netlink_packet_route::AddressFamily;
    use netlink_packet_route::link::InfoKind;

    /// The kernel's `ifinfomsg` link type constants (UAPI `ARPHRD_*`).
    const ARPHRD_ETHER: u16 = 1;
    const ARPHRD_LOOPBACK: u16 = 772;
    const ARPHRD_IEEE80211: u16 = 801;

    /// `IFLA_STATS64`, the link-attribute type the kernel's 25×u64 counter
    /// block travels under (fixtures build it as a raw attribute; the crate
    /// parses it back into `LinkAttribute::Stats64`, the production arm).
    const IFLA_STATS64: u16 = 23;

    /// A buildable link: a wifi link, administratively up, carrier off, operstate
    /// DOWN — the kernel's "asked up, hardware never came up" shape.
    fn wifi_stalled_link() -> LinkMessage {
        let mut msg = LinkMessage::default();
        msg.header.link_layer_type = LinkLayerType::from(ARPHRD_IEEE80211);
        msg.header.flags = LinkFlags::Up;
        msg.attributes = alloc::vec![
            LinkAttribute::IfName("wlp0s20f3".to_string()),
            LinkAttribute::Mtu(1500),
            LinkAttribute::Address(alloc::vec![0x98, 0x59, 0x7a, 0xe4, 0xa0, 0xf2]),
            LinkAttribute::Carrier(0),
            LinkAttribute::OperState(State::from(2u8) /* IF_OPER_DOWN */),
        ];
        msg
    }

    /// A buildable link: ethernet, up, no carrier, operstate DOWN (unplugged
    /// cable with the host asking it up).
    fn ethernet_carrier_down_link() -> LinkMessage {
        let mut msg = LinkMessage::default();
        msg.header.link_layer_type = LinkLayerType::from(ARPHRD_ETHER);
        msg.header.flags = LinkFlags::Up | LinkFlags::LowerUp;
        msg.attributes = alloc::vec![
            LinkAttribute::IfName("enp0s31f6".to_string()),
            LinkAttribute::Mtu(1500),
            LinkAttribute::Address(alloc::vec![0x08, 0x8f, 0xc3, 0xe1, 0x21, 0x71]),
            LinkAttribute::Carrier(0),
            LinkAttribute::OperState(State::from(2u8)),
        ];
        msg
    }

    /// A buildable loopback link: operstate UP, the shape `/sys/class/net/lo`
    /// reports on every host.
    fn loopback_link() -> LinkMessage {
        let mut msg = LinkMessage::default();
        msg.header.index = 1;
        msg.header.link_layer_type = LinkLayerType::from(ARPHRD_LOOPBACK);
        msg.attributes = alloc::vec![
            LinkAttribute::IfName("lo".to_string()),
            LinkAttribute::Mtu(65_536),
            LinkAttribute::OperState(State::from(6u8) /* IF_OPER_UP */),
        ];
        msg
    }

    /// The 26×u64 `IFLA_STATS64` block with the contract's 8 counters in the
    /// kernel's field order.
    fn stats64_payload() -> Vec<u8> {
        // rx_packets, tx_packets, rx_bytes, tx_bytes, rx_errors, tx_errors,
        // rx_dropped, tx_dropped, then the counters the domain does not map.
        let values: [u64; 8] = [90001, 90002, 11_223_344, 55_667_788, 1, 2, 3, 4];
        let mut payload = Vec::new();
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 8..26 {
            payload.extend_from_slice(&0u64.to_le_bytes());
        }
        payload
    }
    fn serialize(inner: RouteNetlinkMessage) -> Vec<u8> {
        let mut message = NetlinkMessage::from(inner);
        message.header.flags = NLM_F_REQUEST;
        message.finalize();
        let mut wire = alloc::vec![0u8; message.buffer_len()];
        message.serialize(&mut wire);
        wire
    }

    #[test]
    fn link_message_parses_the_kernel_facts() {
        let link = parse_link_message(&wifi_stalled_link());
        assert_eq!(link.name, "wlp0s20f3");
        assert_eq!(link.kind, LinkKind::Wifi);
        assert_eq!(link.kind.as_token(), "wifi");
        assert_eq!(link.mtu, 1500);
        assert_eq!(link.hardware_address, "98:59:7A:E4:A0:F2");
        assert!(link.up);
        assert_eq!(link.carrier, Some(false));
        assert_eq!(link.oper_state, OperState::Down);
        // WiFi, admin-up, operstate down, no carrier: the kernel's
        // no-firmware marker — no NM involved.
        assert!(link.stalled());
        assert_eq!(link.provenance, LinkProvenance::Kernel);
    }

    #[test]
    fn carrier_down_arrives_as_a_kernel_fact() {
        let link = parse_link_message(&ethernet_carrier_down_link());
        assert_eq!(link.kind.as_token(), "ethernet");
        assert_eq!(link.carrier, Some(false), "IFLA_CARRIER == 0 is nothing-on-the-wire");
        assert_eq!(link.oper_state, OperState::Down);
    }

    /// The loopback link is a first-class kernel link — the store enumerates
    /// it, and its kind token is the open "loopback".
    #[test]
    fn loopback_link_parses_as_loopback() {
        let mut msg = LinkMessage::default();
        msg.header.link_layer_type = LinkLayerType::from(ARPHRD_LOOPBACK);
        msg.attributes = alloc::vec![
            LinkAttribute::IfName("lo".to_string()),
            LinkAttribute::Mtu(65536),
            LinkAttribute::OperState(State::from(6u8) /* IF_OPER_UP */),
        ];
        let link = parse_link_message(&msg);
        assert_eq!(link.kind, LinkKind::Loopback);
        assert_eq!(link.kind.as_token(), "loopback");
        assert_eq!(link.kind_token(), "loopback");
        assert!(link.is_running(), "lo reports operstate UP");
    }

    /// A `wwan` link reports the "cellular" token — the wwan net dev is the
    /// cellular data interface, derived from the kernel's `IFLA_LINKINFO`.
    #[test]
    fn wwan_link_reports_the_cellular_token() {
        let mut msg = LinkMessage::default();
        msg.header.link_layer_type = LinkLayerType::from(ARPHRD_ETHER);
        msg.attributes = alloc::vec![
            LinkAttribute::IfName("wwan0".to_string()),
            LinkAttribute::LinkInfo(alloc::vec![LinkInfo::Kind(InfoKind::Wwan)]),
        ];
        let link = parse_link_message(&msg);
        assert!(link.is_cellular());
        assert_eq!(link.kind_token(), "cellular");
    }

    #[test]
    fn stats64_attribute_maps_to_the_contract_counters() {
        let mut msg = ethernet_carrier_down_link();
        msg.attributes.push(LinkAttribute::Other(DefaultNla::new(
            IFLA_STATS64,
            stats64_payload(),
        )));
        // Through the production wire path: the crate's parser turns the
        // IFLA_STATS64 attribute into its typed form, which the store's parse
        // maps onto the contract counters.
        let wire = serialize(RouteNetlinkMessage::NewLink(msg));
        match parse_event_datagram(&wire).unwrap().remove(0) {
            KernelEvent::Link(link) => {
                let stats = link.stats.expect("IFLA_STATS64 was present");
                assert_eq!(stats.packets_in, 90001);
                assert_eq!(stats.packets_out, 90002);
                assert_eq!(stats.bytes_in, 11_223_344);
                assert_eq!(stats.bytes_out, 55_667_788);
                assert_eq!(stats.errors_in, 1);
                assert_eq!(stats.errors_out, 2);
                assert_eq!(stats.drops_in, 3);
                assert_eq!(stats.drops_out, 4);
                assert!(stats.sampled_at > 0);
            }
            other => panic!("expected a Link event, got {other:?}"),
        }
    }

    #[test]
    fn a_link_without_stats_reports_none_not_zero() {
        let link = parse_link_message(&ethernet_carrier_down_link());
        assert!(link.stats.is_none(), "absent IFLA_STATS64 is None, never a zeroed counter");
    }

    /// BUG 1's hermetic lock. The kernel reports a wlan interface's link type
    /// as `ARPHRD_ETHER` — the wireless-ness lives in the phy, not the link
    /// type — so the SNAPSHOT path upgrades the kind from sysfs
    /// (`/sys/class/net/<name>/wireless`). The EVENT path parsed the wire
    /// only, so an `RTM_NEWLINK` event silently re-typed a wifi interface as
    /// `ethernet` in the store: `getStatus` then answered `ethernet` while
    /// `getInterfaces` answered `wifi` — the two surfaces disagreed on the
    /// type's source. Both surfaces now read the type from the same place,
    /// and this test locks the event path to it.
    #[test]
    fn event_links_report_the_same_kind_the_snapshot_would() {
        use crate::platforms::linux::netlink::{NetlinkReader, RealNetlinkReader};
        use std::path::Path;

        let root = std::env::temp_dir().join(format!("smart-lib-bug1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("wlp0s20f3").join("wireless")).unwrap();
        std::fs::create_dir_all(root.join("enp0s31f6")).unwrap();
        let reader = RealNetlinkReader::with_roots(Path::new("/nonexistent-rfkill"), &root);

        // The kernel's own wire for a wlan link: ARPHRD_ETHER, no 80211 type.
        let mut wifi = LinkMessage::default();
        wifi.header.link_layer_type = LinkLayerType::from(ARPHRD_ETHER);
        wifi.header.index = 3;
        wifi.attributes = alloc::vec![LinkAttribute::IfName("wlp0s20f3".to_string())];
        let wire = serialize(RouteNetlinkMessage::NewLink(wifi.clone()));
        match reader.parse_events(&wire).unwrap().remove(0) {
            KernelEvent::Link(link) => {
                assert_eq!(link.name, "wlp0s20f3");
                assert_eq!(
                    link.kind,
                    LinkKind::Wifi,
                    "the event path must enrich exactly like the snapshot path"
                );
                assert_eq!(link.kind_token(), "wifi");
            }
            other => panic!("expected a Link event, got {other:?}"),
        }
        // A wired link keeps its kind — the enrichment adds, never overrides.
        let mut ether = wifi.clone();
        ether.attributes = alloc::vec![LinkAttribute::IfName("enp0s31f6".to_string())];
        let wire = serialize(RouteNetlinkMessage::NewLink(ether));
        match reader.parse_events(&wire).unwrap().remove(0) {
            KernelEvent::Link(link) => assert_eq!(link.kind, LinkKind::Ethernet),
            other => panic!("expected a Link event, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn event_packets_parse_from_the_wire() {        // A NEWLINK event (through the production serializer).
        let wire = serialize(RouteNetlinkMessage::NewLink(wifi_stalled_link()));
        match parse_event_datagram(&wire).unwrap().remove(0) {
            KernelEvent::Link(link) => {
                assert_eq!(link.name, "wlp0s20f3");
                assert!(link.stalled());
            }
            other => panic!("expected a Link event, got {other:?}"),
        }
        // A DELLINK event: the store learns the ifindex is gone.
        let mut gone = wifi_stalled_link();
        gone.header.index = 7;
        let wire = serialize(RouteNetlinkMessage::DelLink(gone));
        assert_eq!(
            parse_event_datagram(&wire).unwrap(),
            alloc::vec![KernelEvent::LinkGone(7)]
        );
        // NLMSG_DONE is not store-relevant: a 16-byte header with no payload.
        let done: [u8; 16] = [16, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(parse_event_datagram(&done).unwrap().is_empty());
    }

    /// The kernel answers a `RTM_GETLINK` dump with `RTM_NEWLINK` messages (the
    /// same kinds an event carries). The fold must map those — the live probe
    /// caught a snapshot that only accepted the request kind and therefore
    /// reported an empty link table on a host that has three.
    #[test]
    fn dump_replies_fold_the_new_message_kinds_into_the_snapshot() {
        // Real wire bytes for a NEWLINK, NEWADDR and NEWROUTE reply, the way the
        // kernel sends them.
        let mut v4 = AddressMessage::default();
        v4.header.family = AddressFamily::Inet;
        v4.header.prefix_len = 24;
        v4.header.index = 2;
        v4.attributes = alloc::vec![AddressAttribute::Address(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 18, 128)),
        )];
        let mut route = RouteMessage::default();
        route.header.table = 254;
        route.attributes = alloc::vec![
            RouteAttribute::Destination(RouteAddress::Inet(std::net::Ipv4Addr::new(0, 0, 0, 0))),
            RouteAttribute::Oif(2),
        ];
        let wire_link = serialize(RouteNetlinkMessage::NewLink(ethernet_carrier_down_link()));
        let wire_addr = serialize(RouteNetlinkMessage::NewAddress(v4));
        let wire_route = serialize(RouteNetlinkMessage::NewRoute(route));
        let inner = |wire: Vec<u8>| -> RouteNetlinkMessage {
            match NetlinkMessage::<RouteNetlinkMessage>::deserialize(&wire).unwrap().payload {
                NetlinkPayload::InnerMessage(m) => m,
                other => panic!("expected an inner message, got {other:?}"),
            }
        };

        let (links, addresses, routes) =
            fold_dump_replies(alloc::vec![inner(wire_link)], alloc::vec![inner(wire_addr)], alloc::vec![inner(wire_route)]);
        assert_eq!(links.len(), 1, "a NEWLINK reply is a link, not skipped");
        assert_eq!(links[0].name, "enp0s31f6");
        assert_eq!(links[0].index, 0, "the fixture link's ifindex");
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].address, "192.168.18.128");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].index, 2);
        // A stray message kind is skipped, never misread as data.
        let (skipped, _, _) = fold_dump_replies(
            alloc::vec![RouteNetlinkMessage::GetRoute(RouteMessage::default())],
            Vec::new(),
            Vec::new(),
        );
        assert!(skipped.is_empty(), "a GETROUTE in the link dump is not a link");
    }

    #[test]
    fn address_messages_parse_both_families() {
        let mut v4 = AddressMessage::default();
        v4.header.family = AddressFamily::Inet;
        v4.header.prefix_len = 24;
        v4.header.index = 3;
        v4.attributes = alloc::vec![AddressAttribute::Address(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 18, 128)),
        )];
        let addr = parse_address_message(&v4);
        assert_eq!(addr.address, "192.168.18.128");
        assert_eq!(addr.prefix_len, 24);
        assert_eq!(addr.family, "ipv4");
        let wire = serialize(RouteNetlinkMessage::NewAddress(v4));
        assert_eq!(
            parse_event_datagram(&wire).unwrap(),
            alloc::vec![KernelEvent::Address(addr)]
        );
    }

    #[test]
    fn dump_requests_carry_the_dump_flag() {
        // The request the production code sends, parsed back by the same
        // crates: a GetLink message with NLM_F_DUMP set.
        let mut request = NetlinkMessage::from(RouteNetlinkMessage::GetLink(LinkMessage::default()));
        request.header.flags = NLM_F_REQUEST | NLM_F_DUMP;
        request.header.sequence_number = 42;
        request.finalize();
        let mut wire = alloc::vec![0u8; request.buffer_len()];
        request.serialize(&mut wire);
        let parsed = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&wire).unwrap();
        assert_eq!(parsed.header.sequence_number, 42);
        assert!(parsed.header.flags & NLM_F_DUMP != 0);
        assert!(matches!(
            parsed.payload,
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetLink(_))
        ));
    }

    /// The wireless marker decides the kind token for a wlan device, and it is a
    /// KERNEL fact: `/sys/class/net/<name>/wireless` exists for a wlan interface
    /// while rtnetlink's `ARPHRD_*` type is `ether` (the live probe reported
    /// `kind=ethernet` for the box's wifi until this marker landed). Hermetic: a
    /// throwaway sysfs tree, no socket.
    #[test]
    fn the_wireless_marker_names_a_wlan_device_that_the_wire_calls_ether() {
        let dir = std::env::temp_dir().join(format!("smart-lib-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("wlp0s20f3").join("wireless")).unwrap();
        std::fs::create_dir_all(dir.join("enp0s31f6")).unwrap();
        std::fs::create_dir_all(dir.join("wwan0")).unwrap();

        let ether = parse_link_message(&ethernet_carrier_down_link());
        let wifi = enrich_link(&dir, KernelLink {
            name: "wlp0s20f3".to_string(),
            ..ether.clone()
        });
        assert_eq!(wifi.kind, LinkKind::Wifi, "the wireless marker names the wlan");
        assert_eq!(wifi.kind_token(), "wifi");
        // A wired link keeps its identity, and so does the wwan net device.
        assert_eq!(enrich_link(&dir, ether).kind, LinkKind::Ethernet);
        let wwan = enrich_link(
            &dir,
            KernelLink {
                name: "wwan0".to_string(),
                info_kind: "wwan".to_string(),
                kind: LinkKind::Ethernet,
                ..parse_link_message(&wifi_stalled_link())
            },
        );
        assert!(wwan.is_cellular());
        assert_eq!(wwan.kind_token(), "cellular", "the wwan net device stays cellular");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The regression the read-only live probe caught: the kernel packs several
    /// netlink messages into ONE datagram, and a parser that reads only the
    /// first silently loses the rest (this host's `enp0s31f6` vanished from the
    /// link table that way). Every message in a datagram must be parsed.
    #[test]
    fn one_datagram_carries_several_events_and_all_are_parsed() {
        let mut datagram = Vec::new();
        datagram.extend_from_slice(&serialize(RouteNetlinkMessage::NewLink(loopback_link())));
        datagram.extend_from_slice(&serialize(RouteNetlinkMessage::NewLink(wifi_stalled_link())));
        datagram.extend_from_slice(&serialize(RouteNetlinkMessage::NewLink(ethernet_carrier_down_link())));
        let events = parse_event_datagram(&datagram).unwrap();
        assert_eq!(events.len(), 3, "all three links in the datagram survive");
        match &events[2] {
            KernelEvent::Link(link) => assert_eq!(link.name, "enp0s31f6"),
            other => panic!("expected the third link, got {other:?}"),
        }
        // The dump fold sees the same multi-message datagram shape.
        let messages = parse_datagram(&datagram).unwrap();
        assert_eq!(messages.len(), 3);
        let inner: Vec<RouteNetlinkMessage> = messages
            .into_iter()
            .filter_map(|m| match m.payload {
                NetlinkPayload::InnerMessage(inner) => Some(inner),
                _ => None,
            })
            .collect();
        let (links, addresses, routes) = fold_dump_replies(inner, Vec::new(), Vec::new());
        assert_eq!(links.len(), 3, "a packed dump reply folds every link");
        assert!(addresses.is_empty() && routes.is_empty());
    }

    /// A truncated or corrupt framing length is an error, never a silent
    /// truncation that drops links.
    #[test]
    fn a_bad_framing_length_is_an_error_not_a_truncation() {
        // 16-byte header claiming a 400-byte message in a 20-byte buffer.
        let mut bogus = alloc::vec![0u8; 20];
        bogus[0..4].copy_from_slice(&400u32.to_le_bytes());
        assert!(parse_datagram(&bogus).is_err());
        // A trailing fragment (fewer than 16 bytes) is ignored, not misread.
        let mut padded = serialize(RouteNetlinkMessage::NewLink(loopback_link()));
        padded.extend_from_slice(&[0u8; 4]);
        assert_eq!(parse_datagram(&padded).unwrap().len(), 1, "the 4-byte pad is not a message");
    }

    #[test]
    fn rfkill_events_split_hard_and_soft() {
        // event: idx=u32, type=1(wifi), op=2, soft=1, hard=0
        let mut wifi = [0u8; 8];
        wifi[4] = 1; // type wifi
        wifi[5] = 2; // op change
        wifi[6] = 1; // soft=1
        wifi[7] = 0; // hard=0
        let events = parse_rfkill_events(&wifi);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "wifi");
        assert!(events[0].soft_blocked);
        assert!(!events[0].hard_blocked);
        // A truncated tail is dropped, not misparsed.
        assert!(parse_rfkill_events(&wifi[..5]).is_empty());
    }

    /// The mock seam's non-blocking drain: a bounded batch, in order, and an
    /// empty queue answers an empty batch (never a block, never an error).
    #[test]
    fn mock_drain_returns_a_bounded_batch_in_order() {
        let mock = MockNetlink::with_snapshot(KernelSnapshot::default());
        for i in 1..=4u32 {
            mock.queue_event(KernelEvent::LinkGone(i));
        }
        assert_eq!(mock.drain_events(2).unwrap(), alloc::vec![KernelEvent::LinkGone(1), KernelEvent::LinkGone(2)]);
        assert_eq!(mock.drain_events(8).unwrap().len(), 2, "the rest arrive in the next drain");
        assert!(mock.drain_events(8).unwrap().is_empty(), "an empty queue is an empty batch");
        // A channel fault on the drain is the caller's re-prime signal.
        mock.fail_with(Error::temporary("mock netlink: channel fault"));
        assert!(mock.drain_events(4).is_err());
    }

    /// The kernel's routing table renders in the contract's CIDR spelling: the
    /// address the kernel named plus the prefix length from the message header.
    /// A default route — no `RTA_DESTINATION`, prefix length 0 — renders
    /// "0.0.0.0/0", which is the kernel's own truth rather than a convention
    /// invented at this layer.
    #[test]
    fn routes_render_in_the_contract_cidr_spelling() {
        // A /24 subnet route through a gateway.
        let mut subnet = RouteMessage::default();
        subnet.header.address_family = AddressFamily::Inet;
        subnet.header.destination_prefix_length = 24;
        subnet.header.table = 254;
        subnet.attributes = alloc::vec![
            RouteAttribute::Destination(RouteAddress::Inet(std::net::Ipv4Addr::new(10, 0, 0, 0))),
            RouteAttribute::Gateway(RouteAddress::Inet(std::net::Ipv4Addr::new(192, 168, 18, 1))),
            RouteAttribute::Oif(2),
            RouteAttribute::Priority(100),
        ];
        let route = parse_route_message(&subnet);
        assert_eq!(route.destination, "10.0.0.0/24");
        assert_eq!(route.gateway, "192.168.18.1");
        assert_eq!(route.metric, 100);
        assert_eq!(route.index, 2);
        assert_eq!(route.table, 254);

        // The default route: no destination attribute, prefix length 0.
        let mut default = RouteMessage::default();
        default.header.address_family = AddressFamily::Inet;
        default.header.destination_prefix_length = 0;
        default.header.table = 254;
        default.attributes = alloc::vec![
            RouteAttribute::Gateway(RouteAddress::Inet(std::net::Ipv4Addr::new(192, 168, 18, 1))),
            RouteAttribute::Oif(2),
        ];
        let route = parse_route_message(&default);
        assert_eq!(route.destination, "0.0.0.0/0", "the default route is the kernel's own spelling");
        assert_eq!(route.gateway, "192.168.18.1");

        // An IPv6 default route spells "::/0", never a v4 guess.
        let mut default6 = RouteMessage::default();
        default6.header.address_family = AddressFamily::Inet6;
        default6.header.destination_prefix_length = 0;
        let route = parse_route_message(&default6);
        assert_eq!(route.destination, "::/0");
        assert!(route.gateway.is_empty(), "an on-link route has no gateway");

        // Through the production wire path too (the crate's own serializer).
        let wire = serialize(RouteNetlinkMessage::NewRoute(subnet));
        let inner = match NetlinkMessage::<RouteNetlinkMessage>::deserialize(&wire).unwrap().payload {
            NetlinkPayload::InnerMessage(m) => m,
            other => panic!("expected an inner message, got {other:?}"),
        };
        match inner {
            RouteNetlinkMessage::NewRoute(route) => {
                assert_eq!(parse_route_message(&route).destination, "10.0.0.0/24")
            }
            other => panic!("expected NewRoute, got {other:?}"),
        }
    }

    /// The MTU write request is a real `RTM_NEWLINK` carrying `IFLA_MTU` on the
    /// right ifindex — built by the production serializer and parsed back by the
    /// same crates, so a request that the kernel would reject is caught here
    /// rather than on the founder's host.
    #[test]
    fn the_mtu_write_request_is_a_real_rtm_newlink() {
        let mut link = LinkMessage::default();
        link.header.index = 3;
        link.attributes = alloc::vec![LinkAttribute::Mtu(9000)];
        let request = RouteNetlinkMessage::NewLink(link);
        // The flags the production `transact` sets, built here explicitly so
        // the assertion below tests the writer's real request shape.
        let mut message = NetlinkMessage::from(request);
        message.header.flags = NLM_F_REQUEST | NLM_F_ACK;
        message.finalize();
        let mut wire = alloc::vec![0u8; message.buffer_len()];
        message.serialize(&mut wire);
        let parsed = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&wire).unwrap();
        assert!(parsed.header.flags & NLM_F_ACK != 0, "a write asks the kernel to ACK");
        assert!(parsed.header.flags & NLM_F_REQUEST != 0);
        match parsed.payload {
            NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link)) => {
                assert_eq!(link.header.index, 3);
                assert!(
                    link.attributes.contains(&LinkAttribute::Mtu(9000)),
                    "the request must carry IFLA_MTU, not a whole link re-declaration"
                );
                assert_eq!(link.attributes.len(), 1, "and only that attribute");
            }
            other => panic!("expected NewLink, got {other:?}"),
        }
    }

    /// The scripted write seam records what a backend asked for, and a fault on
    /// it reaches the caller unchanged (error injection stays total over the
    /// write path too).
    #[test]
    fn the_mock_writer_records_and_can_fault() {
        let writer = MockNetlinkWriter::new();
        writer.set_link_mtu(3, 9000).unwrap();
        writer.set_link_mtu(2, 1500).unwrap();
        assert_eq!(*writer.writes.lock().unwrap(), alloc::vec![(3, 9000), (2, 1500)]);
        for code in ErrorCode::ALL {
            writer.fail_with(Error::new(code));
            assert_eq!(writer.set_link_mtu(3, 1500).unwrap_err().code, code);
        }
    }

    /// A read-only live probe of the kernel's link table over a real netlink    /// socket. Law #8: reads only (RTM_GETLINK/GETADDR/GETROUTE dumps +
    /// /dev/rfkill). `#[ignore]`d so the default hermetic suite never touches
    /// the host; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "read-only live netlink probe; run with `cargo test -- --ignored`"]
    fn live_netlink_dump_reports_the_kernel_table() {
        let reader = RealNetlinkReader::new();
        let snap = reader
            .snapshot()
            .expect("the kernel answered RTM_GETLINK (netlink is the always-present primitive)");
        std::println!(
            "live dump: {} links {:?}, {} addresses, {} routes",
            snap.links.len(),
            snap.links.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
            snap.addresses.len(),
            snap.routes.len()
        );
        assert!(!snap.links.is_empty(), "the kernel always has at least lo");
        assert!(
            snap.links.iter().any(|l| l.name == "lo"),
            "loopback must appear in the kernel link table"
        );
        for link in &snap.links {
            assert!(!link.name.is_empty());
            assert!(link.mtu > 0, "{} reported an mtu", link.name);
        }
        assert!(
            snap.addresses.iter().any(|a| a.address == "127.0.0.1"),
            "loopback's address must appear in the address table"
        );
        // Every address belongs to a link the same snapshot enumerated.
        for addr in &snap.addresses {
            assert!(
                snap.links.iter().any(|l| l.index == addr.index),
                "address on ifindex {} has no enclosing link",
                addr.index
            );
        }
    }
}
