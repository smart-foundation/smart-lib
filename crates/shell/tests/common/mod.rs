//! The shell suite's shared fixture: a scripted [`NetworkBackend`] over the
//! engine's owned types. The host is never touched (charter law #8): every
//! read is served from the script, and every host-mutating verb is recorded
//! so a test can prove the shell issues the right engine call.

#![allow(dead_code)]

use std::cell::RefCell;

use smart_lib::core::ids::{NetworkInterfaceId, SecretKeyId};
use smart_lib::core::values::Duration;
use smart_lib::domains::network::types::{
    ConnStatus, HostState, HostStateSource, InterfaceCounters, InterfaceSnapshot, MacAddress,
    Pagination, Page, RadioState, SubscriptionId, WifiNetwork, WifiWritePrivilege,
};

pub use smart_lib::domains::network::types::KnownWifiNetwork;
use smart_lib::domains::network::NetworkBackend;

/// A wifi link shaped like this host's wlp0s20f3: the interface whose type
/// the live verification once saw disagree between `getStatus` and
/// `getInterfaces` (bug 1).
pub fn wifi_iface() -> InterfaceSnapshot {
    InterfaceSnapshot {
        id: iface_id("wlp0s20f3"),
        name: "wlp0s20f3".to_string(),
        kind: "wifi".to_string(),
        hardware_address: MacAddress("98:59:7A:E4:A0:F2".to_string()),
        up: true,
        connected: true,
        mtu: 1500,
        managed: true,
    }
}

/// A wired link, like this host's enp0s31f6.
pub fn ethernet_iface() -> InterfaceSnapshot {
    InterfaceSnapshot {
        id: iface_id("enp0s31f6"),
        name: "enp0s31f6".to_string(),
        kind: "ethernet".to_string(),
        hardware_address: MacAddress("08:8F:C3:E1:21:71".to_string()),
        up: true,
        connected: false,
        mtu: 1500,
        managed: true,
    }
}

/// The same id derivation the engine's Linux backend uses (FNV-1a of the
/// ifname), so the shell's lookups behave exactly as they do live.
pub fn iface_id(name: &str) -> NetworkInterfaceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    NetworkInterfaceId::new(hash)
}

pub fn wifi_status() -> ConnStatus {
    ConnStatus {
        connected: true,
        interface: iface_id("wlp0s20f3"),
        kind: "wifi".to_string(),
        strength_dbm: 0,
        speed_bps: 0,
        quality: 0,
        metered: false,
        behind_captive: false,
        host_state: HostState::Managed,
        host_state_source: HostStateSource::Kernel,
        ssid: "VR-3245-0884".to_string(),
    }
}

/// The scripted engine. `writes` records every host-mutating call; `scan_supported`
/// flips the wifi-scan answer; `known` scripts the remembered-network read.
#[derive(Default)]
pub struct MockBackend {
    pub interfaces: Vec<InterfaceSnapshot>,
    pub radios: Vec<RadioState>,
    pub known: Vec<KnownWifiNetwork>,
    pub networking_enabled: bool,
    pub writes: RefCell<Vec<String>>,
    pub seams: RefCell<Vec<String>>,
    pub scan_error: bool,
    /// Scripted access-point set `wifi_scan` enumerates.
    pub scan: Vec<WifiNetwork>,
    /// The SSID the scripted wifi status carries (the active connection name).
    pub active_ssid: String,
    /// The scripted privilege verdict; `None` ⇒ the engine's own default
    /// (Unavailable — the authority could not be queried).
    pub wifi_privilege: Option<WifiWritePrivilege>,
}

impl MockBackend {
    /// The common fixture: one wifi, one ethernet, both radios on, nothing
    /// remembered.
    pub fn host() -> Self {
        MockBackend {
            interfaces: vec![ethernet_iface(), wifi_iface()],
            radios: vec![
                RadioState {
                    name: "wifi".to_string(),
                    enabled: true,
                    hard_blocked: false,
                    soft_blocked: false,
                },
                RadioState {
                    name: "cellular".to_string(),
                    enabled: false,
                    hard_blocked: false,
                    soft_blocked: false,
                },
            ],
            known: Vec::new(),
            networking_enabled: true,
            writes: RefCell::new(Vec::new()),
            seams: RefCell::new(Vec::new()),
            scan_error: false,
            scan: Vec::new(),
            active_ssid: "VR-3245-0884".to_string(),
            wifi_privilege: None,
        }
    }

    pub fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    pub fn seams(&self) -> Vec<String> {
        self.seams.borrow().clone()
    }
}

impl NetworkBackend for MockBackend {
    fn get_interfaces(&self, page: Pagination) -> smart_lib::core::Result<Page<InterfaceSnapshot>> {
        self.seams.borrow_mut().push("get_interfaces".to_string());
        let total = self.interfaces.len() as u64;
        let limit = page.effective_limit() as usize;
        Ok(Page {
            items: self.interfaces.iter().take(limit).cloned().collect(),
            total,
        })
    }

    fn get_status(&self, interface: NetworkInterfaceId) -> smart_lib::core::Result<ConnStatus> {
        self.seams.borrow_mut().push("get_status".to_string());
        let name = self
            .interfaces
            .iter()
            .find(|i| i.id == interface)
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let mut status = wifi_status();
        status.interface = interface;
        status.kind = if name == "enp0s31f6" {
            "ethernet".to_string()
        } else {
            "wifi".to_string()
        };
        // The active connection name is only carried on a wifi interface.
        status.ssid = if status.kind == "wifi" {
            self.active_ssid.clone()
        } else {
            String::new()
        };
        Ok(status)
    }

    fn subscribe_status(&self, interface: NetworkInterfaceId) -> smart_lib::core::Result<SubscriptionId> {
        self.seams.borrow_mut().push("subscribe_status".to_string());
        Ok(SubscriptionId(interface.get()))
    }

    fn pull_status(&self, subscription: SubscriptionId) -> smart_lib::core::Result<ConnStatus> {
        self.seams.borrow_mut().push("pull_status".to_string());
        let mut status = wifi_status();
        status.interface = NetworkInterfaceId::new(subscription.0);
        Ok(status)
    }

    fn cancel_subscription(&self, _subscription: SubscriptionId) -> smart_lib::core::Result<()> {
        self.seams.borrow_mut().push("cancel_subscription".to_string());
        Ok(())
    }

    fn get_counters(
        &self,
        _interface: NetworkInterfaceId,
        _window: Duration,
    ) -> smart_lib::core::Result<InterfaceCounters> {
        Ok(InterfaceCounters::default())
    }

    fn subscribe_counters(
        &self,
        _interface: NetworkInterfaceId,
    ) -> smart_lib::core::Result<SubscriptionId> {
        Ok(SubscriptionId(1))
    }

    fn pull_counters(
        &self,
        _subscription: SubscriptionId,
    ) -> smart_lib::core::Result<InterfaceCounters> {
        Ok(InterfaceCounters::default())
    }

    fn is_networking_enabled(&self) -> smart_lib::core::Result<bool> {
        self.seams.borrow_mut().push("is_networking_enabled".to_string());
        Ok(self.networking_enabled)
    }

    fn set_networking_enabled(&self, enabled: bool) -> smart_lib::core::Result<()> {
        self.writes.borrow_mut().push(format!("set_networking_enabled({enabled})"));
        Ok(())
    }

    fn set_radio_enabled(&self, radio: &str, enabled: bool) -> smart_lib::core::Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("set_radio_enabled({radio}, {enabled})"));
        Ok(())
    }

    fn list_radio_states(&self, page: Pagination) -> smart_lib::core::Result<Page<RadioState>> {
        self.seams.borrow_mut().push("list_radio_states".to_string());
        let total = self.radios.len() as u64;
        let limit = page.effective_limit() as usize;
        Ok(Page {
            items: self.radios.iter().take(limit).cloned().collect(),
            total,
        })
    }

    fn is_mobile_data_enabled(&self) -> smart_lib::core::Result<bool> {
        Ok(false)
    }

    fn set_mobile_data_enabled(&self, enabled: bool) -> smart_lib::core::Result<()> {
        self.writes.borrow_mut().push(format!("set_mobile_data_enabled({enabled})"));
        Ok(())
    }

    fn is_data_roaming_enabled(&self) -> smart_lib::core::Result<bool> {
        Ok(false)
    }

    fn set_data_roaming_enabled(&self, enabled: bool) -> smart_lib::core::Result<()> {
        self.writes.borrow_mut().push(format!("set_data_roaming_enabled({enabled})"));
        Ok(())
    }

    fn wifi_scan(&self, page: Pagination) -> smart_lib::core::Result<Page<WifiNetwork>> {
        self.seams.borrow_mut().push("wifi_scan".to_string());
        if self.scan_error {
            return Err(smart_lib::core::Error::not_supported(
                "no-backend: active WiFi scan orchestration (NM RequestScan + AccessPoint enumeration) not implemented",
            ));
        }
        let total = self.scan.len() as u64;
        let limit = page.effective_limit() as usize;
        Ok(Page {
            items: self.scan.iter().take(limit).cloned().collect(),
            total,
        })
    }

    fn measure_wifi_write_privilege(&self) -> smart_lib::core::Result<WifiWritePrivilege> {
        self.seams.borrow_mut().push("measure_wifi_write_privilege".to_string());
        match &self.wifi_privilege {
            Some(p) => Ok(p.clone()),
            None => Err(smart_lib::core::Error::unavailable(
                "no-backend: the privilege authority could not be queried on this backend",
            )),
        }
    }

    fn wifi_connect(&self, ssid: &str, _credential: SecretKeyId) -> smart_lib::core::Result<()> {
        self.seams.borrow_mut().push(format!("wifi_connect({ssid})"));
        Ok(())
    }

    fn wifi_disconnect(&self) -> smart_lib::core::Result<()> {
        self.seams.borrow_mut().push("wifi_disconnect".to_string());
        Ok(())
    }

    fn forget_wifi_network(&self, ssid: &str) -> smart_lib::core::Result<()> {
        self.writes.borrow_mut().push(format!("forget_wifi_network({ssid})"));
        Ok(())
    }

    fn list_known_wifi_networks(
        &self,
        page: Pagination,
    ) -> smart_lib::core::Result<Page<KnownWifiNetwork>> {
        self.seams.borrow_mut().push("list_known_wifi_networks".to_string());
        let total = self.known.len() as u64;
        let limit = page.effective_limit() as usize;
        Ok(Page {
            items: self.known.iter().take(limit).cloned().collect(),
            total,
        })
    }
}

/// A scripted [`ClipboardBackend`] over the engine's owned types. The host is
/// never touched (charter law #8): reads are served from an in-memory ledger,
/// and every host-mutating verb is recorded so a test proves the shell issues
/// the right engine call and routes it through the gate. It models a seat, so
/// `set` serves the copy back on `get` exactly as the real backend does.
#[derive(Default)]
pub struct MockClipboard {
    pub transport: String,
    pub can_read: bool,
    pub can_write: bool,
    pub content: std::cell::RefCell<Option<(String, Vec<u8>)>>,
    pub targets: Vec<String>,
    pub history: std::cell::RefCell<Vec<MockItem>>,
    pub writes: std::cell::RefCell<Vec<String>>,
    pub next_id: std::cell::Cell<u64>,
}

#[derive(Clone)]
pub struct MockItem {
    pub id: u64,
    pub text: String,
    pub pinned: bool,
}

#[derive(Default)]
pub struct MockClip;

impl MockClipboard {
    pub fn seat() -> Self {
        MockClipboard {
            transport: "scripted".to_string(),
            can_read: true,
            can_write: true,
            next_id: std::cell::Cell::new(1),
            ..Default::default()
        }
    }
}

impl smart_lib::domains::clipboard::ClipboardBackend for MockClipboard {
    fn clipboard_caps(&self) -> (String, bool, bool) {
        (self.transport.clone(), self.can_read, self.can_write)
    }

    fn get_content(
        &self,
        mime_type: &str,
    ) -> smart_lib::core::Result<smart_lib::domains::clipboard::types::ContentOutcome> {
        Ok(match self.content.borrow().clone() {
            Some((_, data)) if mime_type.starts_with("text") => {
                smart_lib::domains::clipboard::types::ContentOutcome::Content(
                    smart_lib::domains::clipboard::types::ClipboardContent {
                        mime_type: mime_type.to_string(),
                        data,
                    },
                )
            }
            _ => smart_lib::domains::clipboard::types::ContentOutcome::Empty,
        })
    }

    fn set_content(
        &self,
        content: smart_lib::domains::clipboard::types::ClipboardContent,
    ) -> smart_lib::core::Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("set_content(mime={})", content.mime_type));
        *self.content.borrow_mut() = Some((content.mime_type.clone(), content.data.clone()));
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.history.borrow_mut().push(MockItem {
            id,
            text: String::from_utf8_lossy(&content.data).into_owned(),
            pinned: false,
        });
        Ok(())
    }

    fn clear_content(&self) -> smart_lib::core::Result<()> {
        self.writes.borrow_mut().push("clear_content".to_string());
        *self.content.borrow_mut() = None;
        Ok(())
    }

    fn get_supported_mime_types(&self) -> smart_lib::core::Result<Vec<String>> {
        if self.targets.is_empty() {
            Err(smart_lib::core::Error::unavailable(
                "no-backend: the current selection has no owner to introspect",
            ))
        } else {
            Ok(self.targets.clone())
        }
    }

    fn get_history(
        &self,
        page: smart_lib::domains::clipboard::types::Pagination,
    ) -> smart_lib::core::Result<
        smart_lib::domains::clipboard::types::Page<smart_lib::domains::clipboard::types::ClipboardItem>,
    > {
        use smart_lib::domains::clipboard::types::ClipboardItem;
        let total = self.history.borrow().len() as u64;
        let limit = page.effective_limit() as usize;
        let items = self
            .history
            .borrow()
            .iter()
            .skip(page.offset as usize)
            .take(limit)
            .map(|m| ClipboardItem {
                id: smart_lib::core::ids::ClipboardId::new(m.id),
                timestamp: 1_000,
                mime_types: vec!["text/plain".to_string()],
                tags: Vec::new(),
                pinned: m.pinned,
                source_id: smart_lib::core::ids::ClipboardSourceId::new(1),
                size_bytes: m.text.len() as u64,
                text: m.text.clone(),
            })
            .collect();
        Ok(smart_lib::domains::clipboard::types::Page { items, total })
    }
}

/// A scripted [`smart_lib::domains::service::ServiceBackend`] over the engine's
/// owned types. The host is never touched (charter law #8): the supervisor's
/// state is served from the script, every host-mutating verb is recorded, and
/// the callers' credentials decide what the script answers — so the shell's
/// host-write gate and its honest rendering are proven without a supervisor.
pub struct MockService {
    pub supervisor: std::cell::RefCell<smart_lib::domains::service::types::SupervisorIdentity>,
    pub services: std::cell::RefCell<Vec<MockServiceEntry>>,
    pub can_read_state: std::cell::Cell<bool>,
    pub can_control: std::cell::Cell<bool>,
    pub writes: std::cell::RefCell<Vec<String>>,
}

/// One scripted service: the supervisor's own state words plus the measured
/// supervision facts beside it.
#[derive(Clone, Debug)]
pub struct MockServiceEntry {
    pub name: String,
    pub description: String,
    pub active: bool,
    pub failed: bool,
    pub wanted_up: bool,
    pub main_pid: u64,
    pub since_us: u64,
    pub enabled: bool,
    pub supervised: bool,
}

/// The same id derivation the engine's Linux service backend uses (FNV-1a over
/// the service name), so the scripted identities are the engine's own kind —
/// 64-bit hashes, the values a rounded `f64` would misrender.
pub fn scripted_service_id(name: &str) -> smart_lib::core::ids::ServiceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    smart_lib::core::ids::ServiceId::new(hash)
}

impl Default for MockService {
    fn default() -> Self {
        MockService {
            supervisor: std::cell::RefCell::new(smart_lib::domains::service::types::SupervisorIdentity {
                supervisor: "scripted".to_string(),
                service_root: "/tmp/scripted/service".to_string(),
                declared_root: "/tmp/scripted/sv".to_string(),
                state_root: "/tmp/scripted/run".to_string(),
            }),
            services: std::cell::RefCell::new(vec![
                MockServiceEntry {
                    name: "NetworkManager".to_string(),
                    description: "the network manager".to_string(),
                    active: true,
                    failed: false,
                    wanted_up: true,
                    main_pid: 4821,
                    since_us: 9_123_000_000,
                    enabled: true,
                    supervised: true,
                },
                MockServiceEntry {
                    name: "sshd".to_string(),
                    description: "secure shell daemon".to_string(),
                    active: false,
                    failed: false,
                    wanted_up: false,
                    main_pid: 0,
                    since_us: 400_000,
                    enabled: true,
                    supervised: true,
                },
                MockServiceEntry {
                    name: "dhcpcd".to_string(),
                    description: "dhcp client".to_string(),
                    active: false,
                    failed: true,
                    wanted_up: true,
                    main_pid: 0,
                    since_us: 2_000_000,
                    enabled: true,
                    supervised: true,
                },
                MockServiceEntry {
                    name: "cold".to_string(),
                    description: String::new(),
                    active: false,
                    failed: false,
                    wanted_up: false,
                    main_pid: 0,
                    since_us: 0,
                    enabled: false,
                    supervised: false,
                },
                MockServiceEntry {
                    name: "dbus".to_string(),
                    description: "the system message bus".to_string(),
                    active: true,
                    failed: false,
                    wanted_up: true,
                    main_pid: 812,
                    since_us: 9_123_000_000,
                    enabled: true,
                    supervised: true,
                },
            ]),
            can_read_state: std::cell::Cell::new(true),
            can_control: std::cell::Cell::new(true),
            writes: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl MockService {
    /// The status the script reads for one entry: the supervisor's own words,
    /// and the honest `unknown` when the callers' credentials cannot read the
    /// state at all.
    fn status_of(
        &self,
        entry: &MockServiceEntry,
    ) -> smart_lib::domains::service::types::ServiceStatus {
        use smart_lib::domains::service::types::ServiceStatus;
        if !self.can_read_state.get() {
            return ServiceStatus::Unknown;
        }
        if entry.active {
            ServiceStatus::Active
        } else if entry.failed {
            ServiceStatus::Failed
        } else {
            ServiceStatus::Inactive
        }
    }
}

impl smart_lib::domains::service::ServiceBackend for MockService {
    fn supervision_caps(
        &self,
    ) -> smart_lib::domains::service::types::SupervisionCapabilities {
        smart_lib::domains::service::types::SupervisionCapabilities {
            supervisor: self.supervisor.borrow().supervisor.clone(),
            can_enumerate: true,
            can_read_state: self.can_read_state.get(),
            // The mock serves the supervisor's own words directly, so the
            // exact half stands with the state read.
            can_read_exact_state: self.can_read_state.get(),
            can_control: self.can_control.get(),
        }
    }

    fn supervisor(
        &self,
    ) -> smart_lib::domains::service::types::SupervisorIdentity {
        self.supervisor.borrow().clone()
    }

    fn list_services(
        &self,
        page: smart_lib::domains::service::types::Pagination,
    ) -> smart_lib::core::Result<
        smart_lib::domains::service::types::Page<
            smart_lib::domains::service::types::ServiceInstanceInfo,
        >,
    > {
        use smart_lib::domains::service::types::{ServiceInstanceInfo, ServiceInstanceState};
        let services = self.services.borrow();
        let total = services.len() as u64;
        let limit = page.effective_limit() as usize;
        let items = services
            .iter()
            .skip(page.offset as usize)
            .take(limit)
            .map(|entry| {
                let mut state = ServiceInstanceState::default();
                state.status = self.status_of(entry);
                state.main_pid = if entry.active { entry.main_pid } else { 0 };
                state.since_us = entry.since_us;
                state.since_valid = entry.active;
                state.reason = if self.can_read_state.get() {
                    if entry.failed {
                        "the supervisor wants the service up and it is down".to_string()
                    } else if !entry.active {
                        "the supervisor reports the service down".to_string()
                    } else {
                        String::new()
                    }
                } else {
                    "no-privilege: the supervisor's state files are not readable by this caller"
                        .to_string()
                };
                state.metadata = vec![
                    ("supervisor".to_string(), "scripted".to_string()),
                    ("declared".to_string(), entry.supervised.to_string()),
                    ("supervised".to_string(), entry.supervised.to_string()),
                ];
                ServiceInstanceInfo {
                    id: scripted_service_id(&entry.name),
                    name: entry.name.clone(),
                    description: entry.description.clone(),
                    state,
                    enabled: entry.enabled,
                    on_demand: false,
                    manifest: 0,
                }
            })
            .collect();
        Ok(smart_lib::domains::service::types::Page { items, total })
    }

    fn get_service(
        &self,
        service_id: smart_lib::core::ids::ServiceId,
    ) -> smart_lib::core::Result<smart_lib::domains::service::types::ServiceInstanceInfo> {
        let page = self
            .list_services(smart_lib::domains::service::types::Pagination {
                offset: 0,
                limit: u32::MAX,
            })
            .expect("scripted listing");
        page.items
            .into_iter()
            .find(|info| info.id == service_id)
            .ok_or_else(|| smart_lib::core::Error::not_found())
    }

    fn resolve(
        &self,
        name: &str,
    ) -> smart_lib::core::Result<(smart_lib::core::ids::ServiceId, bool)> {
        let services = self.services.borrow();
        match services.iter().find(|e| e.name == name) {
            Some(entry) if entry.supervised => Ok((scripted_service_id(&entry.name), true)),
            Some(entry) if entry.enabled => Ok((scripted_service_id(&entry.name), true)),
            _ => Ok((smart_lib::core::ids::ServiceId::new(0), false)),
        }
    }

    fn get_health(
        &self,
        service_id: smart_lib::core::ids::ServiceId,
    ) -> smart_lib::core::Result<smart_lib::domains::service::types::ServiceHealth> {
        use smart_lib::domains::service::types::HealthState;
        let info = self.get_service(service_id)?;
        let state = match info.state.status {
            smart_lib::domains::service::types::ServiceStatus::Active => HealthState::Online,
            smart_lib::domains::service::types::ServiceStatus::Failed => HealthState::Offline,
            _ => HealthState::Unknown,
        };
        Ok(smart_lib::domains::service::types::ServiceHealth {
            service_id,
            state,
            reason: info.state.reason.clone(),
            quarantined: false,
            since_us: info.state.since_us,
            since_valid: info.state.since_valid,
            last_check_passed: false,
            last_check_at: 0,
            fault_count: 0,
            consecutive_faults: 0,
            restarts: 0,
        })
    }

    fn list_health(
        &self,
        page: smart_lib::domains::service::types::Pagination,
    ) -> smart_lib::core::Result<
        smart_lib::domains::service::types::Page<
            smart_lib::domains::service::types::ServiceHealth,
        >,
    > {
        let listing = self.list_services(page)?;
        let mut items = Vec::new();
        for info in &listing.items {
            if let Ok(health) = self.get_health(info.id) {
                items.push(health);
            }
        }
        Ok(smart_lib::domains::service::types::Page {
            items,
            total: listing.total,
        })
    }

    fn get_dependencies(
        &self,
        service_id: smart_lib::core::ids::ServiceId,
    ) -> smart_lib::core::Result<
        Vec<smart_lib::domains::service::types::ServiceDependency>,
    > {
        // The scripted supervisor declares one edge — the way runit's services
        // declare theirs in their own scripts — and nothing else.
        let services = self.services.borrow();
        let Some(entry) = services
            .iter()
            .find(|e| scripted_service_id(&e.name) == service_id)
        else {
            return Ok(Vec::new());
        };
        if entry.name != "NetworkManager" {
            return Ok(Vec::new());
        }
        // The dependency is the scripted `dbus` service, resolved by name the
        // way the shell renders a dependency's identity back to its name.
        let dbus = services
            .iter()
            .find(|e| e.name == "dbus")
            .expect("the scripted tree declares dbus");
        Ok(vec![
            smart_lib::domains::service::types::ServiceDependency {
                service_id: scripted_service_id(&dbus.name),
                dependency_type: smart_lib::domains::service::types::DependencyType::Required,
                version_constraint: String::new(),
                satisfied: true,
            },
        ])
    }

    fn subscribe_service_events(
        &self,
        _service_id: smart_lib::core::ids::ServiceId,
    ) -> smart_lib::core::Result<smart_lib::domains::service::types::SubscriptionId> {
        Ok(smart_lib::domains::service::types::SubscriptionId(1))
    }

    fn pull_service_events(
        &self,
        _subscription: smart_lib::domains::service::types::SubscriptionId,
        _max_events: u32,
    ) -> smart_lib::core::Result<Vec<smart_lib::domains::service::types::ServiceEvent>> {
        // The scripted supervisor reports one real transition: the failed
        // service. A subscription that starts with nothing reports what exists.
        let page = self
            .list_services(smart_lib::domains::service::types::Pagination {
                offset: 0,
                limit: 1,
            })
            .expect("scripted listing");
        Ok(page
            .items
            .iter()
            .map(|info| smart_lib::domains::service::types::ServiceEvent {
                payload: smart_lib::domains::service::types::ServiceEventPayload::StateChanged(
                    info.state.clone(),
                ),
                timestamp: 1_700_000_000_000_000_000,
                service_id: info.id,
            })
            .collect())
    }
}

impl smart_lib::domains::service::ServiceLifecycle for MockService {
    fn apply_lifecycle(
        &self,
        _service_id: smart_lib::core::ids::ServiceId,
        service_name: &str,
        verb: smart_lib::domains::service::LifecycleVerb,
    ) -> smart_lib::core::Result<()> {
        if !self.can_control.get() {
            return Err(smart_lib::core::Error::with_message(
                smart_lib::core::ErrorCode::PermissionDenied,
                "no-privilege: the supervisor's control path is not writable by this caller",
            ));
        }
        self.writes
            .borrow_mut()
            .push(format!("{} {service_name}", verb.as_token()));
        Ok(())
    }
}
