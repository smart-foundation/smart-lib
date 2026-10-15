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

// ═══ the width oracle, shared by the domain command suites ════════════════
//
// Hand-written rather than calling back into the renderer: an assertion that
// measures with the code it is testing proves nothing. The locked copy lives in
// `render_width.rs` (the widget-level proof); this one is the same terminal's
// own arithmetic for the domain suites, so a domain table and the widget suite
// measure with one rule.
//
//   East-Asian **W**ide / **F**ullwidth → **2** columns;
//   **everything else, INCLUDING the East-Asian Ambiguous class → 1**.

/// Skip an escape sequence whose leading ESC was already taken. None of it
/// draws a column.
pub fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            while let Some(c) = chars.next() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    return;
                }
            }
        }
        Some(']') => {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\u{07}' {
                    return;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    return;
                }
            }
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

/// The display columns one line occupies, by the terminal's own arithmetic.
pub fn columns(s: &str) -> usize {
    let mut w = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
            continue;
        }
        w += match c {
            '日' | '本' | '語' => 2,
            // Emoji-presentation marks — the domain headers' own glyphs.
            '📁' | '🔍' | '🌳' | '🔎' | '🏷' | '📄' | '💽' | '🗄' | '🔒' | '🔐' | '🕰' | '🌿'
            | '📦' | '🧬' | '👁' | '📍' | '🗑' | '🕘' | '🚀' | '🧵' | '🩺' | '📊' | '🔑' | '✎'
            | '⚙' | '📋' | '🌐' | '🔗' | '📈' | '📜' | '📐' | '🚧' | '🧭' | '📡' | '🔌' | '📶'
            | '⛔' | '⏩' | '⌚' | '▶' => 2,
            // Everything else is ONE column on this terminal — ASCII, the
            // box-drawing borders, and the whole East-Asian Ambiguous class the
            // shell's meaning glyphs belong to:
            //   ◉ ○ ● ◌ ◐ (state dots)   ✗ ✓ ⚙ ❤ (marks)
            //   — … ↑ ↓ ↔ ← → (null/ellipsis/arrows)   · • █ ░ ▣ ▤ ◍ ◈ ▢ ＋ － ～
            _ => 1,
        };
    }
    w
}

/// The text of a line with its escapes removed, so a border glyph can be
/// located in what the terminal draws.
pub fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
            continue;
        }
        out.push(c);
    }
    out
}

/// The display column of every border glyph on a line.
pub fn border_columns(line: &str) -> Vec<usize> {
    let p = plain(line);
    let mut out = Vec::new();
    let mut w = 0usize;
    for c in p.chars() {
        if matches!(c, '│' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼') {
            out.push(w);
        }
        w += columns(&c.to_string());
    }
    out
}

/// Assert the rendering is pixel-perfect: every boxed line is the same display
/// width, and every border glyph sits at the same display column.
pub fn assert_borders_hold(rendered: &str) {
    let lines: Vec<&str> = rendered
        .lines()
        .filter(|l| l.contains('│') || l.contains('┌') || l.contains('└'))
        .collect();
    assert!(
        !lines.is_empty(),
        "the rendering must contain a table: {rendered}"
    );
    let mut widths = Vec::new();
    let mut layouts = Vec::new();
    for line in &lines {
        widths.push(columns(line));
        layouts.push(border_columns(line));
    }
    let first_w = widths[0];
    for (i, w) in widths.iter().enumerate() {
        assert_eq!(
            *w, first_w,
            "line {i} is {w} columns wide, but the table is {first_w}: {}",
            lines[i]
        );
    }
    let first_l = &layouts[0];
    for (i, l) in layouts.iter().enumerate() {
        assert_eq!(
            l, first_l,
            "line {i}'s borders sit at {l:?}, not {first_l:?}: {}",
            lines[i]
        );
    }
}

#[cfg(feature = "domain-files")]
#[cfg(feature = "domain-files")]
pub mod files_fixture {
    use super::*;

    //
    // A `FilesBackend` over the engine's OWNED types, serving the host's answers
    // from a script: the listing, the statx metadata, the mmap read window, the
    // inotify events, the statvfs totals and the store's own objects. The host is
    // never touched (charter law #8), and every host-mutating verb is recorded so a
    // test can prove the shell issued the engine call it was supposed to.

    use smart_lib::domains::files::types as files_types;
    use smart_lib::domains::files::{AuditSubscription, FilesBackend, WatchSubscription};

    /// One scripted directory entry: the kernel's own words for one name.
    #[derive(Clone, Debug)]
    pub struct MockFileEntry {
        pub name: String,
        pub path: String,
        pub kind: files_types::FileKind,
        pub size: u64,
        pub modified: u64,
    }

    /// The scripted files backend. `writes` records every host-mutating verb, so
    /// the shell's gate is proven by what the engine was (or was not) asked to do.
    pub struct MockFiles {
        pub cwd: String,
        pub entries: Vec<MockFileEntry>,
        /// The bytes the handle surface serves for a read.
        pub content: Vec<u8>,
        /// The kernel's own inotify records a watch pulls (a queue that drains).
        pub events: std::cell::RefCell<Vec<files_types::FileEvent>>,
        pub writes: std::cell::RefCell<Vec<String>>,
        pub caps: Vec<&'static str>,
        /// Whether `statFile` answers the contract's error arm.
        pub stat_missing: bool,
    }

    impl Default for MockFiles {
        fn default() -> Self {
            MockFiles {
                cwd: "/tmp/scripted".to_string(),
                entries: vec![
                    MockFileEntry {
                        name: "notes.txt".to_string(),
                        path: "/tmp/scripted/notes.txt".to_string(),
                        kind: files_types::FileKind::Regular,
                        size: 12,
                        modified: 1_700_000_000_000_000_000,
                    },
                    MockFileEntry {
                        name: "src".to_string(),
                        path: "/tmp/scripted/src".to_string(),
                        kind: files_types::FileKind::Directory,
                        size: 4096,
                        modified: 1_700_000_001_000_000_000,
                    },
                    MockFileEntry {
                        name: "big.bin".to_string(),
                        path: "/tmp/scripted/big.bin".to_string(),
                        kind: files_types::FileKind::Regular,
                        size: 3_145_728,
                        modified: 1_700_000_002_000_000_000,
                    },
                ],
                content: b"the scripted bytes\n".to_vec(),
                events: std::cell::RefCell::new(vec![files_types::FileEvent {
                    id: smart_lib::core::ids::EventId::new(7),
                    timestamp: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_003_000_000_000,
                    ),
                    kind: files_types::FileEventKind::Create,
                    path: "/tmp/scripted/notes.txt".to_string(),
                    old_path: String::new(),
                    metadata: Vec::new(),
                }]),
                writes: std::cell::RefCell::new(Vec::new()),
                caps: vec!["files.read", "files.write", "files.watch", "files.mounts", "files.content"],
                stat_missing: false,
            }
        }
    }

    impl MockFiles {
        /// The entries the script serves, as the engine's own type.
        fn entry(&self, index: usize) -> files_types::DirectoryEntry {
            let scripted = &self.entries[index];
            files_types::DirectoryEntry {
                name: scripted.name.clone(),
                path: scripted.path.clone(),
                kind: scripted.kind,
                size: scripted.size,
                modified: smart_lib::core::values::Timestamp::from_nanos(scripted.modified),
                metadata: vec![
                    ("created".to_string(), scripted.modified.to_string()),
                    ("accessed".to_string(), scripted.modified.to_string()),
                ],
            }
        }
    }

    impl FilesBackend for MockFiles {
        fn list_directory(
            &self,
            path: &str,
            filter: &files_types::DirectoryFilter,
        ) -> smart_lib::core::Result<files_types::Page<files_types::DirectoryEntry>> {
            if path != self.cwd {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: {path} is not a directory the scripted host serves"),
                ));
            }
            let mut entries: Vec<files_types::DirectoryEntry> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    let entry = self.entry(*index);
                    if !filter.include_hidden && entry.name.starts_with('.') {
                        return false;
                    }
                    if !filter.patterns.is_empty()
                        && !filter.patterns.iter().any(|p| glob(p, &entry.name))
                    {
                        return false;
                    }
                    true
                })
                .map(|(index, _)| self.entry(index))
                .collect();
            match filter.sort_by {
                files_types::SortField::Size => {
                    entries.sort_by_key(|e| e.size);
                }
                files_types::SortField::Modified => {
                    entries.sort_by_key(|e| e.modified.as_nanos());
                }
                _ => entries.sort_by(|a, b| a.name.cmp(&b.name)),
            }
            if matches!(filter.sort_order, files_types::SortOrder::Descending) {
                entries.reverse();
            }
            Ok(files_types::Page {
                total: entries.len() as u64,
                items: entries,
            })
        }

        fn stat_file(&self, path: &str) -> smart_lib::core::Result<files_types::FileInfoResult> {
            let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
                return Ok(files_types::FileInfoResult::Error(
                    smart_lib::core::Error::with_message(
                        smart_lib::core::ErrorCode::NotFound,
                        format!("not-found: statx({path}) answered ENOENT"),
                    ),
                ));
            };
            Ok(files_types::FileInfoResult::Info(files_types::FileInfo {
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
                created: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                modified: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                accessed: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                mime_type: "text/plain".to_string(),
                content_hash: Vec::new(),
                metadata: vec![
                    ("name".to_string(), entry.name.clone()),
                    ("inode".to_string(), "4242".to_string()),
                    ("linkCount".to_string(), "1".to_string()),
                    ("uid".to_string(), "1000".to_string()),
                    ("gid".to_string(), "1000".to_string()),
                    ("blocks".to_string(), "8".to_string()),
                    ("permissions".to_string(), "644".to_string()),
                    ("changed".to_string(), entry.modified.to_string()),
                ],
            }))
        }

        fn open_file(
            &self,
            _path: &str,
            request: &files_types::OpenRequest,
        ) -> smart_lib::core::Result<files_types::FileHandle> {
            self.writes
                .borrow_mut()
                .push(format!("open({})", request.mode.as_token()));
            Ok(files_types::FileHandle::new(
                smart_lib::core::ids::FileHandleId::new(11),
            ))
        }

        fn read_file(
            &self,
            _handle: &files_types::FileHandle,
            offset: u64,
            size: u32,
        ) -> smart_lib::core::Result<Vec<u8>> {
            let start = offset as usize;
            let end = start.saturating_add(size as usize).min(self.content.len());
            if start >= self.content.len() {
                return Ok(Vec::new());
            }
            Ok(self.content[start..end].to_vec())
        }

        fn close_file(&self, _handle: &files_types::FileHandle) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push("close".to_string());
            Ok(())
        }

        fn watch_directory(&self, path: &str) -> smart_lib::core::Result<WatchSubscription> {
            if path != self.cwd {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::InvalidArgument,
                    format!("not-found: {path} is not a directory the scripted host serves"),
                ));
            }
            self.writes.borrow_mut().push("watch".to_string());
            Ok(WatchSubscription(3))
        }

        fn pull_watch(
            &self,
            _subscription: WatchSubscription,
            max_events: u32,
        ) -> smart_lib::core::Result<Vec<files_types::FileEvent>> {
            // The kernel's own pending queue drains: a record is delivered once,
            // never replayed to the next pull.
            let bound = max_events.max(1) as usize;
            let mut events = self.events.borrow_mut();
            let count = events.len().min(bound);
            Ok(events.drain(0..count).collect())
        }

        fn cancel_watch(&self, _subscription: WatchSubscription) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push("cancel-watch".to_string());
            Ok(())
        }

        fn get_disk_usage(&self, path: &str) -> smart_lib::core::Result<files_types::DiskUsage> {
            if path != self.cwd {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: {path} is not a directory the scripted host serves"),
                ));
            }
            Ok(files_types::DiskUsage {
                total: 100 * 1024 * 1024,
                used: 40 * 1024 * 1024,
                free: 60 * 1024 * 1024,
                percent_used: 40.0,
                mount_point: "/tmp".to_string(),
                mount_id: smart_lib::core::ids::MountId::new(36),
                filesystem_id: smart_lib::core::ids::FilesystemId::new(1),
                metadata: vec![
                    ("files".to_string(), "1000".to_string()),
                    ("filesFree".to_string(), "600".to_string()),
                    ("mountOptions".to_string(), "rw,nosuid,relatime".to_string()),
                ],
            })
        }

        fn get_file_metadata(
            &self,
            path: &str,
        ) -> smart_lib::core::Result<files_types::ExtendedMetadata> {
            let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: {path} is not a file the scripted host serves"),
                ));
            };
            Ok(files_types::ExtendedMetadata {
                path: entry.path.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                size: entry.size,
                created: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                modified: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                accessed: smart_lib::core::values::Timestamp::from_nanos(entry.modified),
                owner: "scripted".to_string(),
                mime_type: "text/plain".to_string(),
                target: String::new(),
                is_hidden: false,
                is_read_only: false,
                is_executable: true,
                content_hash: Vec::new(),
                compressed_size: 0,
                encryption_key: smart_lib::core::ids::KeyId::new(0),
                metadata: vec![
                    ("inode".to_string(), "4242".to_string()),
                    ("linkCount".to_string(), "1".to_string()),
                    ("ownerUid".to_string(), "1000".to_string()),
                    ("permissions".to_string(), "755".to_string()),
                    ("created".to_string(), entry.modified.to_string()),
                ],
            })
        }

        fn get_file_hash(
            &self,
            _path: &str,
            algorithm: &str,
        ) -> smart_lib::core::Result<files_types::FileHashResult> {
            if algorithm != "sha256" {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::InvalidArgument,
                    format!("unsupported: this backend implements sha256, not {algorithm}"),
                ));
            }
            Ok(files_types::FileHashResult::Hash(vec![0xab; 32]))
        }

        fn get_file_versions(
            &self,
            _path: &str,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::FileVersion>> {
            Ok(files_types::Page {
                items: vec![files_types::FileVersion {
                    version_id: 1,
                    content_hash: vec![0xcd; 32],
                    created_at: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_004_000_000_000,
                    ),
                    author: smart_lib::core::ids::AppId::new(9),
                    message: "the scripted version".to_string(),
                    size: 12,
                    metadata: Vec::new(),
                }],
                total: 1,
            })
        }

        fn store_content_addressed(
            &self,
            path: &str,
        ) -> smart_lib::core::Result<files_types::ContentRefResult> {
            self.writes.borrow_mut().push(format!("store({path})"));
            Ok(files_types::ContentRefResult::Ref(
                files_types::ContentAddressedRef {
                    hash: vec![0xef; 32],
                    algorithm: "sha256".to_string(),
                    size_bytes: 12,
                    metadata: Vec::new(),
                },
            ))
        }

        fn deduplicate(&self, path: &str) -> smart_lib::core::Result<(u64, u32)> {
            self.writes.borrow_mut().push(format!("dedupe({path})"));
            Ok((0, 1))
        }

        fn list_branches(
            &self,
            _path: &str,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::FileBranch>> {
            Ok(files_types::Page {
                items: Vec::new(),
                total: 0,
            })
        }

        fn get_special_path(
            &self,
            path: files_types::SpecialPath,
        ) -> smart_lib::core::Result<files_types::TextResult> {
            Ok(files_types::TextResult::Value(match path {
                files_types::SpecialPath::Home => "/tmp/scripted/home".to_string(),
                _ => String::new(),
            }))
        }

        fn list_trash(
            &self,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::TrashEntry>> {
            Ok(files_types::Page {
                items: vec![files_types::TrashEntry {
                    entry_id: "1".to_string(),
                    original_path: "/tmp/scripted/gone.txt".to_string(),
                    deleted_at: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_005_000_000_000,
                    ),
                    size: 12,
                    metadata: Vec::new(),
                }],
                total: 1,
            })
        }

        fn get_recent_files(
            &self,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::RecentFile>> {
            Ok(files_types::Page {
                items: vec![files_types::RecentFile {
                    path: "file:///tmp/scripted/notes.txt".to_string(),
                    mime_type: "text/plain".to_string(),
                    timestamp: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_006_000_000_000,
                    ),
                    metadata: Vec::new(),
                }],
                total: 1,
            })
        }

        fn get_default_app(
            &self,
            mime_type: &str,
        ) -> smart_lib::core::Result<files_types::DefaultAppResult> {
            if mime_type != "text/plain" {
                return Ok(files_types::DefaultAppResult::Error(
                    smart_lib::core::Error::with_message(
                        smart_lib::core::ErrorCode::Unavailable,
                        format!("not-applicable: the scripted host names no default app for {mime_type}"),
                    ),
                ));
            }
            Ok(files_types::DefaultAppResult::App(
                files_types::DefaultAppInfo {
                    app_id: smart_lib::core::ids::AppId::new(5),
                    name: "Scripted Editor".to_string(),
                    icon: Vec::new(),
                    metadata: vec![("desktopFile".to_string(), "scripted.desktop".to_string())],
                },
            ))
        }

        fn search_files(
            &self,
            query: &files_types::FileSearchQuery,
        ) -> smart_lib::core::Result<files_types::Page<files_types::FileSearchResult>> {
            let items = self
                .entries
                .iter()
                .filter(|e| query.pattern.is_empty() || glob(&query.pattern, &e.name))
                .map(|e| files_types::FileSearchResult {
                    path: e.path.clone(),
                    name: e.name.clone(),
                    kind: e.kind,
                    size: e.size,
                    modified: smart_lib::core::values::Timestamp::from_nanos(e.modified),
                    score: 1.0,
                    metadata: Vec::new(),
                })
                .collect();
            Ok(files_types::Page { items, total: self.entries.len() as u64 })
        }

        fn search_file_content(
            &self,
            query: &files_types::ContentSearchQuery,
        ) -> smart_lib::core::Result<files_types::Page<files_types::ContentSearchResult>> {
            let text = String::from_utf8_lossy(&self.content).to_string();
            let items = text
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains(&query.query))
                .map(|(index, line)| files_types::ContentSearchResult {
                    path: format!("{}/notes.txt", self.cwd),
                    snippet: line.to_string(),
                    score: 1.0,
                    line_number: index as u32 + 1,
                    metadata: Vec::new(),
                })
                .collect();
            Ok(files_types::Page { items, total: 1 })
        }

        fn compare_files(
            &self,
            left: &str,
            right: &str,
        ) -> smart_lib::core::Result<files_types::ComparisonResult> {
            let identical = left == right;
            Ok(files_types::ComparisonResult {
                identical,
                diff: if identical {
                    Vec::new()
                } else {
                    b"--- left\n+++ right\n".to_vec()
                },
                metadata: vec![("size".to_string(), self.content.len().to_string())],
            })
        }

        fn get_filesystem_health(
            &self,
            _path: &str,
        ) -> smart_lib::core::Result<files_types::FilesystemHealthResult> {
            Ok(files_types::FilesystemHealthResult::Health(
                files_types::FilesystemHealth {
                    status: "healthy".to_string(),
                    last_check: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_007_000_000_000,
                    ),
                    next_scheduled: smart_lib::core::values::Timestamp::from_nanos(
                        1_700_000_008_000_000_000,
                    ),
                    errors: 0,
                    warnings: 0,
                    details: "the scripted host's measured half".to_string(),
                    metadata: Vec::new(),
                },
            ))
        }

        fn get_quota(&self, _path: &str) -> smart_lib::core::Result<files_types::FilesystemQuota> {
            Ok(files_types::FilesystemQuota {
                max_size_bytes: 100 * 1024 * 1024,
                max_files: 1000,
                current_size_bytes: 40 * 1024 * 1024,
                current_files: 400,
                soft_limit_percent: 80,
            })
        }

        fn get_locks(
            &self,
            _path: &str,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::FileLock>> {
            Ok(files_types::Page {
                items: Vec::new(),
                total: 0,
            })
        }

        fn walk_tree(
            &self,
            path: &str,
            _max_depth: u32,
            _page: files_types::Pagination,
        ) -> smart_lib::core::Result<files_types::Page<files_types::DirectoryEntry>> {
            if path != self.cwd {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: {path} is not a directory the scripted host serves"),
                ));
            }
            Ok(files_types::Page {
                total: self.entries.len() as u64,
                items: self.entries.iter().enumerate().map(|(i, _)| self.entry(i)).collect(),
            })
        }

        fn get_current_directory(&self) -> smart_lib::core::Result<String> {
            Ok(self.cwd.clone())
        }

        fn files_caps(&self) -> Vec<&'static str> {
            self.caps.clone()
        }

        // ── the host-mutating surface: recorded, never performed on the host ────

        fn create_directory(&self, path: &str, parents: bool) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("mkdir({path}, {parents})"));
            Ok(())
        }

        fn touch(&self, path: &str) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("touch({path})"));
            Ok(())
        }

        fn remove_file(&self, path: &str, recursive: bool) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("rm({path}, {recursive})"));
            Ok(())
        }

        fn rename_file(&self, old_path: &str, new_path: &str) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("mv({old_path}, {new_path})"));
            Ok(())
        }

        fn copy_file(&self, src: &str, dst: &str) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("cp({src}, {dst})"));
            Ok(())
        }

        fn create_file(
            &self,
            path: &str,
            content: &[u8],
            parents: bool,
        ) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("write({path}, {}, {parents})", content.len()));
            Ok(())
        }

        fn link_file(&self, target: &str, link: &str, hard: bool) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("link({target}, {link}, {hard})"));
            Ok(())
        }

        fn lock_file(
            &self,
            request: &files_types::LockFileRequest,
        ) -> smart_lib::core::Result<files_types::FileLock> {
            self.writes.borrow_mut().push(format!("lock({})", request.path));
            Ok(files_types::FileLock {
                lock_id: 1,
                handle: files_types::FileHandle {
                    id: smart_lib::core::ids::FileHandleId::new(1),
                    metadata: vec![("path".to_string(), request.path.clone())],
                },
                kind: request.kind,
                owner: request.owner.clone(),
                acquired_at: smart_lib::core::values::Timestamp::from_nanos(1_700_000_009_000_000_000),
                expires_at: smart_lib::core::values::Timestamp::UNIX_EPOCH,
                metadata: Vec::new(),
            })
        }

        fn unlock_file(&self, lock_id: u64) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("unlock({lock_id})"));
            Ok(())
        }

        fn subscribe_audit(
            &self,
            _filter: &files_types::FileAuditFilter,
        ) -> smart_lib::core::Result<AuditSubscription> {
            Ok(AuditSubscription(1))
        }

        fn pull_audit(
            &self,
            _subscription: AuditSubscription,
            _max_entries: u32,
        ) -> smart_lib::core::Result<Vec<files_types::FileAuditEntry>> {
            Ok(Vec::new())
        }

        fn cancel_audit(&self, _subscription: AuditSubscription) -> smart_lib::core::Result<()> {
            Ok(())
        }
    }

    /// The scripted host's own glob: the plain-name pattern the backend matches
    /// with (so the shell's tests exercise the same rule the engine's walk uses).
    pub fn glob(pattern: &str, name: &str) -> bool {
        if pattern == "*" || pattern.is_empty() {
            return true;
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return name.ends_with(suffix);
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return name.starts_with(prefix);
        }
        pattern == name
    }

}
#[cfg(feature = "domain-files")]
pub use files_fixture::*;

#[cfg(feature = "domain-process")]
pub mod process_fixture {
    use super::*;

    //
    // A `ProcessBackend` over the engine's OWNED types, serving the host's `/proc`
    // answers from a script: the enumeration, one process's row, the measured
    // accounting, the threads, the environment (the host's own NUL pairs), the
    // memory map's rows and limits, the aggregates, and the measured signal and
    // ptrace verdicts. The host is never touched (charter law #8) and every
    // host-mutating verb is recorded, so the shell's gate is proven by what the
    // engine was (or was not) asked to do.

    use smart_lib::domains::process::types as proc_types;
    use smart_lib::domains::process::{ExitSubscription, ProcessBackend, ProcessSubscription};

    /// One scripted process row: the kernel's own words for one pid.
    #[derive(Clone, Debug)]
    pub struct MockProcessRow {
        pub pid: u64,
        pub ppid: u64,
        pub name: String,
        pub command: String,
        pub state: proc_types::ExecutionState,
        pub uid: u64,
        pub threads: u32,
        pub memory_usage: u64,
        pub cpu_nanos: u64,
    }

    /// The scripted process backend.
    pub struct MockProcess {
        pub rows: Vec<MockProcessRow>,
        pub env: Vec<(String, String)>,
        /// The host's own `/proc/<pid>/maps` rows.
        pub maps: Vec<proc_types::MemoryMapEntry>,
        pub writes: std::cell::RefCell<Vec<String>>,
        /// The measured signal verdict (the host's own kill(pid, 0) answer).
        pub signal_privilege: bool,
        /// The measured ptrace verdict.
        pub ptrace: (bool, String),
    }

    impl Default for MockProcess {
        fn default() -> Self {
            MockProcess {
                rows: vec![
                    MockProcessRow {
                        pid: 1,
                        ppid: 0,
                        name: "runit".to_string(),
                        command: "runit".to_string(),
                        state: proc_types::ExecutionState::Blocked,
                        uid: 0,
                        threads: 1,
                        memory_usage: 92 * 1024,
                        cpu_nanos: 1_050_000_000,
                    },
                    MockProcessRow {
                        pid: 42,
                        ppid: 1,
                        name: "smart".to_string(),
                        command: "target/debug/smart service".to_string(),
                        state: proc_types::ExecutionState::Active,
                        uid: 1000,
                        threads: 4,
                        memory_usage: 24 * 1024 * 1024,
                        cpu_nanos: 350_000_000,
                    },
                    MockProcessRow {
                        pid: 777,
                        ppid: 1,
                        name: "kworker/0:1".to_string(),
                        command: "[kworker/0:1]".to_string(),
                        state: proc_types::ExecutionState::Blocked,
                        uid: 0,
                        threads: 1,
                        memory_usage: 0,
                        cpu_nanos: 4_000_000,
                    },
                ],
                env: vec![
                    ("LANG".to_string(), "en_ZA.UTF-8".to_string()),
                    ("HOME".to_string(), "/tmp/scripted".to_string()),
                ],
                maps: vec![proc_types::MemoryMapEntry {
                    start: 0x1000,
                    end: 0x3000,
                    perms: proc_types::MemoryPerm::Read,
                    name: "scripted.bin".to_string(),
                    backing: String::new(),
                    metadata: vec![("perms".to_string(), "r--p".to_string())],
                }],
                writes: std::cell::RefCell::new(Vec::new()),
                signal_privilege: true,
                ptrace: (
                    false,
                    "MEASURED: /proc/sys/kernel/yama/ptrace_scope is 1 on this host".to_string(),
                ),
            }
        }
    }

    impl MockProcess {
        fn info(&self, pid: u64) -> Option<proc_types::ProcessInfo> {
            self.rows.iter().find(|row| row.pid == pid).map(|row| proc_types::ProcessInfo {
                pid: smart_lib::core::ids::ProcessId::new(row.pid),
                ppid: smart_lib::core::ids::ProcessId::new(row.ppid),
                name: row.name.clone(),
                command: row.command.clone(),
                args: row.command.split(' ').map(str::to_string).collect(),
                state: row.state,
                cpu_usage: 0.0,
                memory_usage: row.memory_usage,
                threads: row.threads,
                start_time: smart_lib::core::values::Timestamp::from_nanos(1_700_000_000_000_000_000),
                user: smart_lib::core::ids::UserId::new(row.uid),
                groups: Vec::new(),
            })
        }
    }

    impl ProcessBackend for MockProcess {
        fn list_processes(
            &self,
            filter: &proc_types::ProcessFilter,
            _page: proc_types::Pagination,
        ) -> smart_lib::core::Result<proc_types::Page<proc_types::ProcessInfo>> {
            let infos: Vec<proc_types::ProcessInfo> = self
                .rows
                .iter()
                .filter(|row| filter.name.is_empty() || row.name.contains(&filter.name))
                .filter(|row| filter.user.get() == 0 || filter.user.get() == row.uid)
                .filter(|row| {
                    matches!(filter.state, proc_types::ExecutionState::Unknown) || filter.state == row.state
                })
                .filter_map(|row| self.info(row.pid))
                .collect();
            Ok(proc_types::Page {
                total: infos.len() as u64,
                items: infos,
            })
        }

        fn get_process(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<proc_types::ProcessInfo> {
            self.info(pid.get()).ok_or_else(|| {
                smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: /proc/{} answered ENOENT", pid.get()),
                )
            })
        }

        fn get_process_status(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<proc_types::ProcessStatus> {
            let info = self.get_process(pid)?;
            Ok(proc_types::ProcessStatus {
                pid: info.pid,
                name: info.name,
                state: info.state,
                exit_status: proc_types::ExitStatus::Exited(0),
                uptime: smart_lib::core::values::Duration::from_nanos(90_000_000_000),
                memory_bytes: info.memory_usage,
                cpu_time: smart_lib::core::values::Duration::from_nanos(
                    self.rows
                        .iter()
                        .find(|row| row.pid == pid.get())
                        .map(|row| row.cpu_nanos)
                        .unwrap_or(0),
                ),
                last_failure: None,
            })
        }

        fn get_environment(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<Vec<(String, String)>> {
            if pid.get() == 1 {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::PermissionDenied,
                    "no-privilege: /proc/1/environ: Permission denied (os error 13)",
                ));
            }
            Ok(self.env.clone())
        }

        fn get_threads(
            &self,
            pid: smart_lib::core::ids::ProcessId,
            _page: proc_types::Pagination,
        ) -> smart_lib::core::Result<proc_types::Page<proc_types::ThreadInfo>> {
            let info = self.get_process(pid)?;
            let threads = (0..info.threads)
                .map(|index| proc_types::ThreadInfo {
                    id: smart_lib::core::ids::ThreadId::new(info.pid.get() * 100 + u64::from(index)),
                    name: format!("{}-{}", info.name, index),
                    state: proc_types::ExecutionState::Blocked,
                    cpu_usage: 0.0,
                })
                .collect::<Vec<_>>();
            Ok(proc_types::Page {
                total: threads.len() as u64,
                items: threads,
            })
        }

        fn get_memory_map(
            &self,
            pid: smart_lib::core::ids::ProcessId,
            _page: proc_types::Pagination,
        ) -> smart_lib::core::Result<proc_types::Page<proc_types::MemoryMapEntry>> {
            if pid.get() == 1 {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::PermissionDenied,
                    "no-privilege: /proc/1/maps: Permission denied (os error 13)",
                ));
            }
            Ok(proc_types::Page {
                total: self.maps.len() as u64,
                items: self.maps.clone(),
            })
        }

        fn get_accounting(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<proc_types::ResourceUsage> {
            let info = self.get_process(pid)?;
            Ok(proc_types::ResourceUsage {
                cpu_time: smart_lib::core::values::Duration::from_nanos(
                    self.rows
                        .iter()
                        .find(|row| row.pid == pid.get())
                        .map(|row| row.cpu_nanos)
                        .unwrap_or(0),
                ),
                memory_bytes: info.memory_usage,
                peak_memory_bytes: info.memory_usage,
                io_read_bytes: 4096,
                io_write_bytes: 0,
                network_rx_bytes: 0,
                network_tx_bytes: 0,
                acquired_at: smart_lib::core::values::Timestamp::from_nanos(1_700_000_001_000_000_000),
            })
        }

        fn get_stats(
            &self,
            _user: smart_lib::core::ids::UserId,
        ) -> smart_lib::core::Result<proc_types::ProcessStats> {
            let mut stats = proc_types::ProcessStats::default();
            for row in &self.rows {
                stats.total_processes += 1;
                match row.state {
                    proc_types::ExecutionState::Active => stats.active += 1,
                    proc_types::ExecutionState::Blocked => stats.blocked += 1,
                    proc_types::ExecutionState::Suspended => stats.suspended += 1,
                    proc_types::ExecutionState::Terminated => stats.terminated += 1,
                    _ => {}
                }
                stats.total_cpu_usage += 0.5;
                stats.total_memory_usage += row.memory_usage;
            }
            Ok(stats)
        }

        fn list_process_groups(
            &self,
            _page: proc_types::Pagination,
        ) -> smart_lib::core::Result<proc_types::Page<proc_types::ProcessGroup>> {
            let group = proc_types::ProcessGroup {
                group_id: smart_lib::core::ids::ProcessGroupId::new(1),
                name: "pgid-1".to_string(),
                processes: self.rows.iter().map(|r| smart_lib::core::ids::ProcessId::new(r.pid)).collect(),
                session_id: 1,
                foreground: false,
            };
            Ok(proc_types::Page {
                total: 1,
                items: vec![group],
            })
        }

        fn get_process_tree(
            &self,
            pid: smart_lib::core::ids::ProcessId,
            _max_depth: u32,
        ) -> smart_lib::core::Result<proc_types::ProcessTree> {
            let info = self.get_process(pid)?;
            let children = self
                .rows
                .iter()
                .filter(|row| row.ppid == pid.get())
                .filter_map(|row| self.info(row.pid))
                .map(|child| proc_types::ProcessTree {
                    pid: child.pid,
                    name: child.name,
                    state: child.state,
                    children: Vec::new(),
                })
                .collect();
            Ok(proc_types::ProcessTree {
                pid: info.pid,
                name: info.name,
                state: info.state,
                children,
            })
        }

        fn get_resource_limits(
            &self,
            _pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<proc_types::Quota> {
            Ok(proc_types::Quota {
                memory_bytes: 8 * 1024 * 1024 * 1024,
                cpu_percent: 100.0,
                io_read_bytes_per_sec: 0,
                io_write_bytes_per_sec: 0,
                pids: 512,
                wall_clock_budget: smart_lib::core::values::Duration::from_nanos(0),
            })
        }

        fn measure_signal_privilege(
            &self,
            _pid: smart_lib::core::ids::ProcessId,
            _signal: proc_types::SignalKind,
        ) -> smart_lib::core::Result<bool> {
            Ok(self.signal_privilege)
        }

        fn measure_ptrace_privilege(&self) -> smart_lib::core::Result<(bool, String)> {
            Ok(self.ptrace.clone())
        }

        fn signal_process(
            &self,
            pid: smart_lib::core::ids::ProcessId,
            signal: proc_types::SignalKind,
        ) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("signal({}, {})", pid.get(), signal.as_token()));
            Ok(())
        }

        fn attach(&self, pid: smart_lib::core::ids::ProcessId) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("attach({})", pid.get()));
            Ok(())
        }

        fn wait(
            &self,
            pid: smart_lib::core::ids::ProcessId,
            _timeout: smart_lib::core::values::Duration,
        ) -> smart_lib::core::Result<proc_types::ExitStatus> {
            self.writes.borrow_mut().push(format!("wait({})", pid.get()));
            Ok(proc_types::ExitStatus::Exited(0))
        }

        fn subscribe_process(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<ProcessSubscription> {
            self.get_process(pid)?;
            Ok(ProcessSubscription(1))
        }

        fn pull_process(
            &self,
            _subscription: ProcessSubscription,
            max_infos: u32,
        ) -> smart_lib::core::Result<Vec<proc_types::ProcessInfo>> {
            Ok(self
                .rows
                .iter()
                .take(max_infos.max(1) as usize)
                .filter_map(|row| self.info(row.pid))
                .collect())
        }

        fn cancel_process(&self, _subscription: ProcessSubscription) -> smart_lib::core::Result<()> {
            Ok(())
        }

        fn subscribe_death(
            &self,
            pid: smart_lib::core::ids::ProcessId,
        ) -> smart_lib::core::Result<ExitSubscription> {
            self.get_process(pid)?;
            Ok(ExitSubscription(1))
        }

        fn pull_exit(
            &self,
            _subscription: ExitSubscription,
            _max_notifications: u32,
        ) -> smart_lib::core::Result<Vec<proc_types::ExitNotification>> {
            Ok(Vec::new())
        }

        fn cancel_exit(&self, _subscription: ExitSubscription) -> smart_lib::core::Result<()> {
            Ok(())
        }

        fn process_caps(&self) -> Vec<&'static str> {
            vec!["process.read", "process.subscribe", "process.signal"]
        }
    }

}
#[cfg(feature = "domain-process")]
pub use process_fixture::*;
#[cfg(feature = "domain-settings")]
pub mod settings_fixture {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};
    use smart_lib::core::values::Timestamp;
    use smart_lib::domains::settings::types as settings_types;
    use smart_lib::domains::settings::SettingsBackend;

    /// One scripted schema key: the host's own declaration.
    pub fn scripted_key(
        name: &str,
        variant: &str,
        default: Option<&str>,
    ) -> settings_types::SchemaKey {
        settings_types::SchemaKey {
            name: name.to_string(),
            variant_type: variant.to_string(),
            default_value: default.map(str::to_string),
            range: None,
            summary: Some(format!("the scripted {name}")),
            description: None,
        }
    }

    /// The scripted settings backend: the host's own schema source and values
    /// served from a script, with the host's own change signal modelled as a
    /// queue a watched subscription drains.
    pub struct MockSettings {
        pub schemas: Vec<settings_types::SchemaInfo>,
        pub values:
            std::sync::Mutex<std::collections::HashMap<(String, String), settings_types::SettingValue>>,
        pub pending: std::cell::RefCell<Vec<settings_types::SettingChange>>,
        pub version: AtomicU64,
        pub subscriptions:
            std::sync::Mutex<std::collections::HashMap<u64, (String, String, u64)>>,
        pub writes: std::cell::RefCell<Vec<String>>,
        next_subscription: AtomicU64,
    }

    impl Default for MockSettings {
        fn default() -> Self {
            let mut values = std::collections::HashMap::new();
            values.insert(
                ("org.example.shell".to_string(), "font-name".to_string()),
                settings_types::SettingValue::String("Sans 10".to_string()),
            );
            values.insert(
                ("org.example.shell".to_string(), "clock-show-seconds".to_string()),
                settings_types::SettingValue::Boolean(false),
            );
            values.insert(
                ("org.example.shell".to_string(), "workspaces".to_string()),
                settings_types::SettingValue::Integer(4),
            );
            let values = std::sync::Mutex::new(values);
            MockSettings {
                schemas: vec![settings_types::SchemaInfo {
                    id: "org.example.shell".to_string(),
                    path: Some("/org/example/shell/".to_string()),
                    relocatable: false,
                    keys: vec![
                        scripted_key("font-name", "s", Some("'Sans 10'")),
                        scripted_key("clock-show-seconds", "b", Some("false")),
                        scripted_key("workspaces", "i", Some("4")),
                        scripted_key("accent", "s", None),
                    ],
                }],
                values,
                pending: std::cell::RefCell::new(Vec::new()),
                version: AtomicU64::new(1),
                subscriptions: std::sync::Mutex::new(std::collections::HashMap::new()),
                writes: std::cell::RefCell::new(Vec::new()),
                next_subscription: AtomicU64::new(1),
            }
        }
    }

    impl MockSettings {
        /// The host's own change, as the host's own signal would deliver it.
        pub fn host_change(&self, domain: &str, key: &str, value: settings_types::SettingValue) {
            let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
            self.pending
                .borrow_mut()
                .push(settings_types::SettingChange {
                    domain: domain.to_string(),
                    key: key.to_string(),
                    value,
                    timestamp: Timestamp::from_nanos(1_700_000_000_000_000_000),
                    actor_id: "host:changed".to_string(),
                    version,
                });
        }
    }

    impl SettingsBackend for MockSettings {
        fn get(
            &self,
            domain: &str,
            key: &str,
        ) -> smart_lib::core::Result<settings_types::SettingResult> {
            match self.values.lock().unwrap().get(&(domain.to_string(), key.to_string())) {
                Some(value) => Ok(settings_types::SettingResult::Value(value.clone())),
                None => Ok(settings_types::SettingResult::NotSet),
            }
        }

        fn set(
            &self,
            domain: &str,
            key: &str,
            value: &settings_types::SettingValue,
        ) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("set({domain} {key})"));
            self.values
                .lock()
                .unwrap()
                .insert((domain.to_string(), key.to_string()), value.clone());
            self.host_change(domain, key, value.clone());
            Ok(())
        }

        fn get_all(
            &self,
            domain: &str,
            _page: settings_types::Pagination,
        ) -> smart_lib::core::Result<settings_types::SettingMap> {
            let Some(schema) = self.schemas.iter().find(|s| s.id == domain) else {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotFound,
                    format!("not-found: the scripted host publishes no schema named {domain}"),
                ));
            };
            let entries = schema
                .keys
                .iter()
                .map(|key| settings_types::SettingMapEntry {
                    key: key.name.clone(),
                    value: self
                        .values
                        .lock()
                        .unwrap()
                        .get(&(domain.to_string(), key.name.clone()))
                        .cloned()
                        .unwrap_or(settings_types::SettingValue::Boolean(false)),
                })
                .collect();
            Ok(settings_types::SettingMap { entries })
        }

        fn list_domains(
            &self,
            _page: settings_types::Pagination,
        ) -> smart_lib::core::Result<settings_types::Page<String>> {
            let ids: Vec<String> = self.schemas.iter().map(|s| s.id.clone()).collect();
            Ok(settings_types::Page {
                total: ids.len() as u64,
                items: ids,
            })
        }

        fn watch(
            &self,
            domain: &str,
            key: &str,
        ) -> smart_lib::core::Result<smart_lib::domains::settings::SettingsSubscription> {
            let id = self.next_subscription.fetch_add(1, Ordering::Relaxed);
            self.subscriptions
                .lock()
                .unwrap()
                .insert(id, (domain.to_string(), key.to_string(), 0));
            Ok(smart_lib::domains::settings::SettingsSubscription(id))
        }

        fn watch_domain(
            &self,
            domain: &str,
        ) -> smart_lib::core::Result<smart_lib::domains::settings::SettingsSubscription> {
            let id = self.next_subscription.fetch_add(1, Ordering::Relaxed);
            self.subscriptions
                .lock()
                .unwrap()
                .insert(id, (domain.to_string(), String::new(), 0));
            Ok(smart_lib::domains::settings::SettingsSubscription(id))
        }

        fn pull_settings(
            &self,
            subscription: smart_lib::domains::settings::SettingsSubscription,
            max_events: u32,
        ) -> smart_lib::core::Result<Vec<settings_types::SettingChange>> {
            let mut subscriptions = self.subscriptions.lock().unwrap();
            let Some((domain, key, drained)) = subscriptions.get_mut(&subscription.0) else {
                return Err(smart_lib::core::Error::not_found()
                    .with_context("settings subscription"));
            };
            let domain = domain.clone();
            let key = key.clone();
            let mut drained = *drained;
            let bound = max_events.max(1) as usize;
            let mut out = Vec::new();
            for change in self.pending.borrow().iter() {
                if out.len() >= bound {
                    break;
                }
                if change.domain != domain {
                    continue;
                }
                if !key.is_empty() && change.key != key {
                    continue;
                }
                if change.version <= drained {
                    continue;
                }
                drained = change.version;
                out.push(change.clone());
            }
            if let Some(slot) = subscriptions.get_mut(&subscription.0) {
                slot.2 = drained;
            }
            Ok(out)
        }

        fn cancel_settings(
            &self,
            subscription: smart_lib::domains::settings::SettingsSubscription,
        ) -> smart_lib::core::Result<()> {
            self.subscriptions
                .lock()
                .unwrap()
                .remove(&subscription.0);
            Ok(())
        }

        fn get_version(&self, _domain: &str, _key: &str) -> smart_lib::core::Result<u64> {
            Ok(self.version.load(Ordering::Relaxed))
        }

        fn get_history(
            &self,
            domain: &str,
            key: &str,
            _page: settings_types::Pagination,
        ) -> smart_lib::core::Result<settings_types::Page<settings_types::SettingChange>> {
            let items = self
                .pending
                .borrow()
                .iter()
                .filter(|change| change.domain == domain && change.key == key)
                .cloned()
                .collect::<Vec<_>>();
            Ok(settings_types::Page {
                total: items.len() as u64,
                items,
            })
        }

        fn get_settings_diff(
            &self,
            domain: &str,
            since_version: u64,
        ) -> smart_lib::core::Result<(Vec<settings_types::SettingChange>, u64)> {
            let version = self.version.load(Ordering::Relaxed);
            let items = self
                .pending
                .borrow()
                .iter()
                .filter(|change| change.domain == domain && change.version > since_version)
                .cloned()
                .collect();
            Ok((items, version))
        }

        fn list_by_category(
            &self,
            category: &str,
            _page: settings_types::Pagination,
        ) -> smart_lib::core::Result<settings_types::Page<settings_types::SettingMapEntry>> {
            let values = self.values.lock().unwrap();
            let entries = values
                .iter()
                .filter(|((domain, _), _)| domain.contains(category))
                .map(|((domain, key), value)| settings_types::SettingMapEntry {
                    key: format!("{domain} {key}"),
                    value: value.clone(),
                })
                .collect::<Vec<_>>();
            Ok(settings_types::Page {
                total: entries.len() as u64,
                items: entries,
            })
        }

        fn bulk_get(
            &self,
            domain: &str,
            keys: &[&str],
        ) -> smart_lib::core::Result<Vec<settings_types::SettingMapEntry>> {
            Ok(keys
                .iter()
                .map(|key| settings_types::SettingMapEntry {
                    key: (*key).to_string(),
                    value: self
                        .values
                        .lock()
                        .unwrap()
                        .get(&(domain.to_string(), (*key).to_string()))
                        .cloned()
                        .unwrap_or(settings_types::SettingValue::Boolean(false)),
                })
                .collect())
        }

        fn bulk_set(
            &self,
            domain: &str,
            entries: &[settings_types::SettingMapEntry],
        ) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("bulk-set({domain}, {})", entries.len()));
            Ok(())
        }

        fn describe(
            &self,
            domain: &str,
        ) -> smart_lib::core::Result<Option<settings_types::SchemaInfo>> {
            Ok(self.schemas.iter().find(|s| s.id == domain).cloned())
        }

        fn describe_all(&self) -> smart_lib::core::Result<Vec<settings_types::SchemaInfo>> {
            Ok(self.schemas.clone())
        }

        fn settings_caps(&self) -> Vec<&'static str> {
            vec!["settings.read", "settings.subscribe", "settings.write"]
        }
    }
}

#[cfg(feature = "domain-settings")]
pub use settings_fixture::*;

#[cfg(feature = "domain-logging")]
pub mod logging_fixture {
    use super::*;

    use smart_lib::core::values::Timestamp;
    use smart_lib::domains::logging::types as log_types;
    use smart_lib::domains::logging::LoggingBackend;
    use smart_lib::platforms::linux::logging::HostSink;

    /// The scripted logging backend over the engine's OWNED types: the host's
    /// own sinks with their measured verdicts, this process's retained
    /// entries, and the contract's subscription interface. The host is never
    /// touched (charter law #8) and every host-touching verb is recorded.
    pub struct MockLogging {
        pub sinks: Vec<HostSink>,
        pub entries: std::cell::RefCell<Vec<log_types::LogEntry>>,
        pub levels: std::cell::RefCell<std::collections::BTreeMap<String, log_types::Level>>,
        pub sampling: std::cell::RefCell<std::collections::BTreeMap<String, f32>>,
        pub sequence: std::cell::Cell<u64>,
        pub writes: std::cell::RefCell<Vec<String>>,
        next_subscription: std::cell::Cell<u64>,
        subscriptions: std::cell::RefCell<std::collections::HashMap<u64, log_types::LogFilter>>,
    }

    impl Default for MockLogging {
        fn default() -> Self {
            let mut entries = Vec::new();
            for (index, (level, message)) in [
                (log_types::Level::Info, "the scripted host booted"),
                (log_types::Level::Warn, "a disk is filling up"),
                (log_types::Level::Error, "a service refused a start"),
                (log_types::Level::Debug, "a debug line nobody asked to drop"),
            ]
            .into_iter()
            .enumerate()
            {
                entries.push(log_types::LogEntry {
                    id: smart_lib::core::ids::EventId::new(index as u64 + 1),
                    sequence: index as u64 + 1,
                    timestamp: Timestamp::from_nanos(1_700_000_000_000_000_000 + index as u64),
                    level,
                    domain: "app".to_string(),
                    message: message.to_string(),
                    source: Vec::new(),
                    subsystem: smart_lib::core::ids::SubsystemId::new(0),
                    metadata: Vec::new(),
                    correlation_id: String::new(),
                    trace_id: String::new(),
                    span_id: String::new(),
                    parent_span_id: String::new(),
                });
            }
            MockLogging {
                sinks: vec![
                    HostSink {
                        domain: "Xorg.0.log".to_string(),
                        path: "/var/log/Xorg.0.log".to_string(),
                        readable: true,
                        measured_reason: String::new(),
                    },
                    HostSink {
                        domain: "service:sshd".to_string(),
                        path: "/run/runit/supervise.sshd-log".to_string(),
                        readable: false,
                        measured_reason:
                            "/run/runit/supervise.sshd-log: Permission denied (os error 13)"
                                .to_string(),
                    },
                ],
                entries: std::cell::RefCell::new(entries),
                levels: std::cell::RefCell::new(std::collections::BTreeMap::new()),
                sampling: std::cell::RefCell::new(std::collections::BTreeMap::new()),
                sequence: std::cell::Cell::new(4),
                writes: std::cell::RefCell::new(Vec::new()),
                next_subscription: std::cell::Cell::new(1),
                subscriptions: std::cell::RefCell::new(std::collections::HashMap::new()),
            }
        }
    }

    impl MockLogging {
        /// The host's own sink lines for one sink (the scripted file): the
        /// lines this host's file carries, bounded by the caller's number.
        fn sink_entries(&self, _domain: &str, limit: u32) -> Vec<log_types::LogEntry> {
            self.entries
                .borrow()
                .iter()
                .take(limit.max(1) as usize)
                .cloned()
                .collect()
        }
    }

    impl LoggingBackend for MockLogging {
        fn log(
            &self,
            level: log_types::Level,
            domain: &str,
            message: &str,
        ) -> smart_lib::core::Result<log_types::LogOutcome> {
            self.writes
                .borrow_mut()
                .push(format!("log({}, {})", level.as_token(), domain));
            self.sequence.set(self.sequence.get() + 1);
            let sequence = self.sequence.get();
            self.entries.borrow_mut().push(log_types::LogEntry {
                id: smart_lib::core::ids::EventId::new(sequence),
                sequence,
                timestamp: Timestamp::from_nanos(1_700_000_009_000_000_000),
                level,
                domain: domain.to_string(),
                message: message.to_string(),
                source: Vec::new(),
                subsystem: smart_lib::core::ids::SubsystemId::new(0),
                metadata: Vec::new(),
                correlation_id: String::new(),
                trace_id: String::new(),
                span_id: String::new(),
                parent_span_id: String::new(),
            });
            Ok(log_types::LogOutcome::Accepted)
        }

        fn get_entries(
            &self,
            filter: &log_types::LogFilter,
            page: log_types::Pagination,
        ) -> smart_lib::core::Result<log_types::Page<log_types::LogEntry>> {
            // A host sink's own domain token reads the sink's own lines; the
            // scripted host answers its own EACCES for the root-only one.
            if filter.domain == "service:sshd" {
                return Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::Unavailable,
                    "no-privilege: the host's log sink is not readable by this caller: \
                     /run/runit/supervise.sshd-log: Permission denied (os error 13)",
                ));
            }
            if filter.domain == "Xorg.0.log" {
                let items = self.sink_entries(&filter.domain, page.effective_limit());
                return Ok(log_types::Page {
                    total: items.len() as u64,
                    items,
                });
            }
            let matched: Vec<log_types::LogEntry> = self
                .entries
                .borrow()
                .iter()
                .filter(|entry| filter.matches(entry))
                .cloned()
                .collect();
            let start = page.offset as usize;
            let end = start
                .saturating_add(page.effective_limit() as usize)
                .min(matched.len());
            let items = if start >= matched.len() {
                Vec::new()
            } else {
                matched[start..end].to_vec()
            };
            Ok(log_types::Page {
                total: matched.len() as u64,
                items,
            })
        }

        fn subscribe(
            &self,
            filter: &log_types::LogFilter,
        ) -> smart_lib::core::Result<log_types::SubscriptionId> {
            let id = self.next_subscription.get();
            self.next_subscription.set(id + 1);
            self.subscriptions
                .borrow_mut()
                .insert(id, filter.clone());
            Ok(log_types::SubscriptionId(id))
        }

        fn pull(
            &self,
            subscription: log_types::SubscriptionId,
            max_entries: u32,
        ) -> smart_lib::core::Result<Vec<log_types::LogEntry>> {
            let Some(filter) = self.subscriptions.borrow().get(&subscription.0).cloned() else {
                return Err(smart_lib::core::Error::not_found()
                    .with_context("log subscription"));
            };
            let delivered = self
                .entries
                .borrow()
                .iter()
                .map(|entry| entry.sequence)
                .max()
                .unwrap_or(0);
            let _ = delivered;
            let mut out: Vec<log_types::LogEntry> = self
                .entries
                .borrow()
                .iter()
                .filter(|entry| entry.sequence > 4 && filter.matches(entry))
                .take(max_entries.max(1) as usize)
                .cloned()
                .collect();
            out.sort_by_key(|entry| entry.sequence);
            Ok(out)
        }

        fn cancel(&self, subscription: log_types::SubscriptionId) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("cancel({})", subscription.0));
            self.subscriptions.borrow_mut().remove(&subscription.0);
            Ok(())
        }

        fn set_level(
            &self,
            domain: &str,
            level: log_types::Level,
        ) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("set-level({domain}, {})", level.as_token()));
            self.levels
                .borrow_mut()
                .insert(domain.to_string(), level);
            Ok(())
        }

        fn get_level(&self, domain: &str) -> smart_lib::core::Result<log_types::Level> {
            Ok(self
                .levels
                .borrow()
                .get(domain)
                .copied()
                .unwrap_or(log_types::Level::Info))
        }

        fn clear(&self, domain: &str) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push(format!("clear({domain})"));
            self.entries
                .borrow_mut()
                .retain(|entry| !domain.is_empty() && entry.domain != domain);
            Ok(())
        }

        fn export(
            &self,
            filter: &log_types::LogFilter,
            mime_type: &str,
        ) -> smart_lib::core::Result<Vec<u8>> {
            self.writes
                .borrow_mut()
                .push(format!("export({mime_type})"));
            let held = self.entries.borrow();
            let matched: Vec<log_types::LogEntry> = held
                .iter()
                .filter(|entry| filter.matches(entry))
                .cloned()
                .collect();
            drop(held);
            let text = matched
                .iter()
                .map(|entry| {
                    format!("{} {}: {}", entry.level.as_token(), entry.domain, entry.message)
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(text.into_bytes())
        }

        fn set_sampling_rate(&self, domain: &str, rate: f32) -> smart_lib::core::Result<()> {
            self.writes
                .borrow_mut()
                .push(format!("sampling({domain}, {rate})"));
            self.sampling
                .borrow_mut()
                .insert(domain.to_string(), rate);
            Ok(())
        }

        fn list_domains(
            &self,
            page: log_types::Pagination,
        ) -> smart_lib::core::Result<log_types::Page<log_types::LogDomain>> {
            let mut names: std::collections::BTreeMap<String, (log_types::Level, f32, String)> =
                std::collections::BTreeMap::new();
            for entry in self.entries.borrow().iter() {
                names
                    .entry(entry.domain.clone())
                    .or_insert((
                        log_types::Level::Info,
                        1.0,
                        "this process's retained entries".to_string(),
                    ));
            }
            for (domain, rate) in self.sampling.borrow().iter() {
                names
                    .entry(domain.clone())
                    .or_insert((
                        log_types::Level::Info,
                        1.0,
                        "configured on this backend".to_string(),
                    ));
                names.get_mut(domain).unwrap().1 = *rate;
            }
            for sink in &self.sinks {
                let description = if sink.readable {
                    format!("host log file {}", sink.path)
                } else {
                    format!(
                        "host log file {} (not readable by this caller: {})",
                        sink.path, sink.measured_reason
                    )
                };
                names
                    .entry(sink.domain.clone())
                    .or_insert((log_types::Level::Custom, 1.0, description));
            }
            let all: Vec<log_types::LogDomain> = names
                .into_iter()
                .map(|(name, (level, rate, description))| log_types::LogDomain {
                    name,
                    level,
                    description,
                    sampling_rate: rate,
                })
                .collect();
            let start = page.offset as usize;
            let end = start
                .saturating_add(page.effective_limit() as usize)
                .min(all.len());
            let items = if start >= all.len() {
                Vec::new()
            } else {
                all[start..end].to_vec()
            };
            Ok(log_types::Page {
                total: all.len() as u64,
                items,
            })
        }

        fn logging_caps(&self) -> Vec<&'static str> {
            vec!["logging.write", "logging.read", "logging.subscribe", "logging.export"]
        }
    }

    /// The contract's severity tokens, named for the tests.
    pub mod log_types_of {
        use smart_lib::domains::logging::types::Level;
        pub fn info() -> Level {
            Level::Info
        }
        pub fn warn() -> Level {
            Level::Warn
        }
    }

    /// The contract seam the scripted backend satisfies.
    impl shell::domains::logging::LoggingSeam for MockLogging {}

    /// The sink surface the scripted backend publishes beside the contract.
    impl shell::domains::logging::SinkSurface for MockLogging {
        fn host_sinks(&self) -> Vec<HostSink> {
            self.sinks.clone()
        }
        fn refresh_host_sinks(&self) {
            self.writes.borrow_mut().push("refresh".to_string());
        }
    }
}

#[cfg(feature = "domain-logging")]
pub use logging_fixture::*;

#[cfg(feature = "domain-clock")]
pub mod clock_fixture {
    use super::*;

    use smart_lib::core::values::Timestamp;
    use smart_lib::domains::clock::types as clock_types;
    use smart_lib::domains::clock::{AlarmService, ClockBackend};

    /// The scripted clock backend over the engine's OWNED types: the kernel's
    /// own clock readings, the timerfds this process armed, the alarm store,
    /// and the measured sync words. The host is never touched (charter law #8)
    /// and every removal verb is recorded, so the shell's gate is proven by
    /// what the engine was (or was not) asked to do.
    pub struct MockClock {
        pub realtime: u64,
        pub monotonic: u64,
        pub boot_time: u64,
        pub resolution: u64,
        pub source: String,
        pub sync: clock_types::ClockSyncInfo,
        pub evidence: String,
        pub security: clock_types::ClockSecurityInfo,
        pub domains: Vec<clock_types::ClockDomainInfo>,
        /// The timers this process armed, as the kernel's own table.
        pub timers: std::sync::Mutex<std::collections::HashMap<u64, clock_types::TimerSpec>>,
        pub alarms:
            std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<u64, clock_types::Alarm>>>,
        pub next_timer: std::cell::Cell<u64>,
        pub next_alarm: std::cell::Cell<u64>,
        pub writes: std::cell::RefCell<Vec<String>>,
        pub virtual_clocks: std::sync::Mutex<std::collections::HashMap<u64, clock_types::VirtualClockConfig>>,
    }

    impl Default for MockClock {
        fn default() -> Self {
            MockClock {
                realtime: 1_700_000_000_000_000_000,
                monotonic: 900_000_000_000,
                boot_time: 1_699_999_000_000_000_000,
                resolution: 1,
                source: "tsc".to_string(),
                sync: clock_types::ClockSyncInfo {
                    synced: false,
                    source: "chrony".to_string(),
                    stratum: 0,
                    last_sync: Timestamp::UNIX_EPOCH,
                    offset: smart_lib::core::values::Duration::from_nanos(0),
                },
                evidence: "pid 711 runs chronyd: the host runs a synchronization daemon, and that \
                           is the measured fact; the daemon's own sync state is not readable by \
                           this uid (/run/chrony is root-only on this host)"
                    .to_string(),
                security: clock_types::ClockSecurityInfo {
                    tampered: false,
                    secure_source: false,
                    hardware_rng: true,
                },
                domains: vec![
                    clock_types::ClockDomainInfo {
                        domain: clock_types::ClockDomainKind::Realtime,
                        name: "CLOCK_REALTIME".to_string(),
                        resolution: smart_lib::core::values::Duration::from_nanos(1),
                        available: true,
                    },
                    clock_types::ClockDomainInfo {
                        domain: clock_types::ClockDomainKind::Monotonic,
                        name: "CLOCK_MONOTONIC".to_string(),
                        resolution: smart_lib::core::values::Duration::from_nanos(1),
                        available: true,
                    },
                ],
                timers: std::sync::Mutex::new(std::collections::HashMap::new()),
                alarms: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::BTreeMap::new(),
                )),
                next_timer: std::cell::Cell::new(1),
                next_alarm: std::cell::Cell::new(1),
                writes: std::cell::RefCell::new(Vec::new()),
                virtual_clocks: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    impl ClockBackend for MockClock {
        fn info(&self) -> smart_lib::core::Result<clock_types::ClockInfo> {
            Ok(clock_types::ClockInfo {
                monotonic: Timestamp::from_nanos(self.monotonic),
                realtime: Timestamp::from_nanos(self.realtime),
                resolution: smart_lib::core::values::Duration::from_nanos(self.resolution),
                source_name: self.source.clone(),
                boot_time: Timestamp::from_nanos(self.boot_time),
                sync: self.sync.clone(),
                security: self.security,
                domains: self.domains.clone(),
            })
        }

        fn monotonic_now(&self) -> smart_lib::core::Result<Timestamp> {
            Ok(Timestamp::from_nanos(self.monotonic))
        }

        fn realtime_now(&self) -> smart_lib::core::Result<Timestamp> {
            Ok(Timestamp::from_nanos(self.realtime))
        }

        fn create_timer(
            &self,
            spec: &clock_types::TimerSpec,
        ) -> smart_lib::core::Result<clock_types::TimerHandle> {
            let id = self.next_timer.get();
            self.next_timer.set(id + 1);
            self.writes
                .borrow_mut()
                .push(format!("timer({id}, {})", spec.label));
            self.timers.lock().unwrap().insert(id, spec.clone());
            Ok(clock_types::TimerHandle {
                id: smart_lib::core::ids::TimerId::new(id),
                label: spec.label.clone(),
            })
        }

        fn cancel_timer(
            &self,
            timer: &clock_types::TimerHandle,
        ) -> smart_lib::core::Result<()> {
            let removed = self.timers.lock().unwrap().remove(&timer.id.get());
            self.writes
                .borrow_mut()
                .push(format!("cancel-timer({})", timer.id.get()));
            match removed {
                Some(_) => Ok(()),
                None => Err(smart_lib::core::Error::not_found()
                    .with_context("timer")),
            }
        }

        fn check_timer(
            &self,
            timer: &clock_types::TimerHandle,
        ) -> smart_lib::core::Result<bool> {
            match self.timers.lock().unwrap().get(&timer.id.get()) {
                Some(_) => Ok(true),
                None => Err(smart_lib::core::Error::not_found().with_context("timer")),
            }
        }

        fn subscribe_timers(
            &self,
            _filter: &clock_types::TimerFilter,
        ) -> smart_lib::core::Result<clock_types::SubscriptionId> {
            self.writes.borrow_mut().push("subscribe-timers".to_string());
            Ok(clock_types::SubscriptionId(9))
        }

        fn pull_timers(
            &self,
            _subscription: clock_types::SubscriptionId,
        ) -> smart_lib::core::Result<Vec<clock_types::TimerEvent>> {
            Ok(vec![clock_types::TimerEvent {
                timer: clock_types::TimerHandle {
                    id: smart_lib::core::ids::TimerId::new(1),
                    label: "scripted".to_string(),
                },
                fired_at: Timestamp::from_nanos(self.monotonic + 1),
                // The kernel's own expiration count, never a fabricated 1.
                count: 3,
            }])
        }

        fn cancel_timers(
            &self,
            _subscription: clock_types::SubscriptionId,
        ) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push("cancel-timers".to_string());
            Ok(())
        }

        fn sleep(
            &self,
            _duration: smart_lib::core::values::Duration,
        ) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push("sleep".to_string());
            Ok(())
        }

        fn sleep_cancellable(
            &self,
            _duration: smart_lib::core::values::Duration,
        ) -> smart_lib::core::Result<clock_types::SleepHandle> {
            Ok(clock_types::SleepHandle { id: 5 })
        }

        fn cancel_sleep(&self, _handle: clock_types::SleepHandle) -> smart_lib::core::Result<()> {
            self.writes.borrow_mut().push("cancel-sleep".to_string());
            Ok(())
        }

        fn list_clock_domains(
            &self,
        ) -> smart_lib::core::Result<Vec<clock_types::ClockDomainInfo>> {
            Ok(self.domains.clone())
        }

        fn get_clock_sync(&self) -> smart_lib::core::Result<clock_types::ClockSyncInfo> {
            Ok(self.sync.clone())
        }

        fn get_clock_drift(
            &self,
            _domain1: clock_types::ClockDomainKind,
            _domain2: clock_types::ClockDomainKind,
        ) -> smart_lib::core::Result<clock_types::ClockDriftInfo> {
            Ok(clock_types::ClockDriftInfo {
                drift: smart_lib::core::values::Duration::from_nanos(12),
                measured_at: Timestamp::from_nanos(self.realtime),
                accuracy: smart_lib::core::values::Duration::from_nanos(1),
            })
        }

        fn create_virtual_clock(
            &self,
            config: &clock_types::VirtualClockConfig,
        ) -> smart_lib::core::Result<clock_types::VirtualClockHandle> {
            let id = self.virtual_clocks.lock().unwrap().len() as u64 + 1;
            self.virtual_clocks
                .lock()
                .unwrap()
                .insert(id, config.clone());
            Ok(clock_types::VirtualClockHandle {
                id,
                name: config.name.clone(),
            })
        }

        fn advance_virtual_clock(
            &self,
            clock: &clock_types::VirtualClockHandle,
            duration: smart_lib::core::values::Duration,
        ) -> smart_lib::core::Result<Timestamp> {
            let now = self
                .virtual_clocks
                .lock()
                .unwrap()
                .get(&clock.id)
                .map(|_| self.realtime + duration.as_nanos())
                .unwrap_or(self.realtime);
            Ok(Timestamp::from_nanos(now))
        }

        fn get_clock_quality(
            &self,
            source_name: &str,
        ) -> smart_lib::core::Result<clock_types::ClockQualityMetrics> {
            Ok(clock_types::ClockQualityMetrics {
                source_name: source_name.to_string(),
                accuracy: smart_lib::core::values::Duration::from_nanos(100),
                stability: smart_lib::core::values::Duration::from_nanos(10),
                reliability: 0.99,
                measured_at: Timestamp::from_nanos(self.realtime),
                sample_count: 32,
            })
        }

        fn persist_timer(
            &self,
            _timer: &clock_types::TimerHandle,
            _persistence: &clock_types::TimerPersistence,
        ) -> smart_lib::core::Result<()> {
            Err(smart_lib::core::Error::with_message(
                smart_lib::core::ErrorCode::Unavailable,
                "not-applicable: this host carries no timer-persistence store",
            ))
        }

        fn list_persisted_timers(
            &self,
            _page: clock_types::Pagination,
        ) -> smart_lib::core::Result<clock_types::Page<clock_types::TimerHandle>> {
            Ok(clock_types::Page {
                items: Vec::new(),
                total: 0,
            })
        }

        fn alarms(&self) -> smart_lib::core::Result<Box<dyn AlarmService>> {
            Ok(Box::new(MockAlarmService {
                alarms: std::sync::Arc::clone(&self.alarms),
                next_id: self.next_alarm.clone(),
            }))
        }

        fn clock_caps(&self) -> Vec<&'static str> {
            vec!["clock.realtime", "clock.monotonic", "clock.timers", "clock.alarms"]
        }
    }

    /// The scripted alarm store, acquired through `alarms @19`.
    pub struct MockAlarmService {
        pub alarms:
            std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<u64, clock_types::Alarm>>>,
        pub next_id: std::cell::Cell<u64>,
    }

    impl AlarmService for MockAlarmService {
        fn create_alarm(
            &self,
            alarm: &clock_types::Alarm,
        ) -> smart_lib::core::Result<smart_lib::core::ids::AlarmId> {
            let id = smart_lib::core::ids::AlarmId::new(self.next_id.get());
            self.next_id.set(id.get() + 1);
            let mut stored = alarm.clone();
            stored.id = id;
            self.alarms.lock().unwrap().insert(id.get(), stored);
            Ok(id)
        }

        fn delete_alarm(
            &self,
            id: smart_lib::core::ids::AlarmId,
        ) -> smart_lib::core::Result<()> {
            match self.alarms.lock().unwrap().remove(&id.get()) {
                Some(_) => Ok(()),
                None => Err(smart_lib::core::Error::not_found()),
            }
        }

        fn get_alarm(
            &self,
            id: smart_lib::core::ids::AlarmId,
        ) -> smart_lib::core::Result<clock_types::Alarm> {
            self.alarms
                .lock()
                .unwrap()
                .get(&id.get())
                .cloned()
                .ok_or_else(smart_lib::core::Error::not_found)
        }

        fn list_alarms(
            &self,
            _page: clock_types::Pagination,
        ) -> smart_lib::core::Result<clock_types::Page<clock_types::Alarm>> {
            let items: Vec<clock_types::Alarm> =
                self.alarms.lock().unwrap().values().cloned().collect();
            Ok(clock_types::Page {
                total: items.len() as u64,
                items,
            })
        }

        fn snooze_alarm(
            &self,
            id: smart_lib::core::ids::AlarmId,
            _duration: smart_lib::core::values::Duration,
        ) -> smart_lib::core::Result<()> {
            match self.alarms.lock().unwrap().get_mut(&id.get()) {
                Some(_) => Ok(()),
                None => Err(smart_lib::core::Error::not_found()),
            }
        }

        fn get_next_alarm(&self) -> smart_lib::core::Result<clock_types::Alarm> {
            self.alarms
                .lock()
                .unwrap()
                .values()
                .next()
                .cloned()
                .ok_or_else(smart_lib::core::Error::not_found)
        }
    }

    /// The sync-evidence surface the scripted backend publishes beside the
    /// contract: the host's own measured words.
    impl shell::domains::clock::SyncEvidenceSurface for MockClock {
        fn sync_evidence(&self) -> String {
            self.evidence.clone()
        }
    }

    /// The contract seam the scripted backend satisfies.
    impl shell::domains::clock::ClockSeam for MockClock {}
}

#[cfg(feature = "domain-clock")]
pub use clock_fixture::*;

#[cfg(feature = "domain-time")]
pub mod time_fixture {
    use super::*;

    use smart_lib::domains::time::types as time_types;
    use smart_lib::domains::time::TimeBackend;

    /// The scripted time backend over the engine's OWNED types: the kernel's
    /// own wall-clock reading, the contract's TAI converters (the same constant
    /// the contract carries) and the arithmetic calendar kernels, with the
    /// Hebrew and Chinese astronomies refused the way the host refuses them.
    /// The host is never touched (charter law #8).
    pub struct MockTime {
        pub now: time_types::DateTime,
    }

    impl Default for MockTime {
        fn default() -> Self {
            MockTime {
                now: time_types::DateTime {
                    year: 2026,
                    month: 10,
                    day: 15,
                    hour: 12,
                    minute: 30,
                    second: 45,
                    millisecond: 500,
                    weekday: 4,
                    yearday: 288,
                    timezone: "Africa/Johannesburg".to_string(),
                    utc_offset_seconds: 7_200,
                },
            }
        }
    }

    impl TimeBackend for MockTime {
        fn get_date_time(&self) -> smart_lib::core::Result<time_types::DateTime> {
            Ok(self.now.clone())
        }

        fn verify_hsm_timestamp(
            &self,
            _hsm: &time_types::HsmTimestamp,
        ) -> smart_lib::core::Result<Option<bool>> {
            Err(smart_lib::core::Error::with_message(
                smart_lib::core::ErrorCode::NotSupported,
                "no-backend: this host exposes no hardware security module (no TPM character \
                 device under /dev, no PKCS#11 module), so an HSM attestation cannot be verified",
            ))
        }

        fn convert_calendar(
            &self,
            date: &time_types::HistoricalDate,
            _from: time_types::CalendarSystem,
            to: time_types::CalendarSystem,
        ) -> smart_lib::core::Result<time_types::HistoricalDate> {
            use time_types::CalendarSystem as C;
            // The scripted arithmetic: a real offset per calendar, so the
            // shell's rendering of a conversion is provable.
            match to {
                C::Julian => Ok(time_types::HistoricalDate::gregorian(
                    date.year,
                    date.month - 1,
                    date.day,
                )),
                C::Islamic => Ok(time_types::HistoricalDate::gregorian(
                    date.year - 578,
                    5,
                    3,
                )),
                C::Ethiopian => Ok(time_types::HistoricalDate::gregorian(
                    date.year - 7,
                    2,
                    5,
                )),
                other => Err(smart_lib::core::Error::with_message(
                    smart_lib::core::ErrorCode::NotSupported,
                    format!(
                        "no-backend: the {:?} calendar is not carried by this backend; the \
                         contract's `custom` outlet is the way a backend names one it does not \
                         carry",
                        other.as_token()
                    ),
                )),
            }
        }

        fn add_calendar_units(
            &self,
            date: &time_types::HistoricalDate,
            units: &[time_types::CalendarUnit],
        ) -> smart_lib::core::Result<time_types::HistoricalDate> {
            let total = units.iter().map(|u| u.amount).sum::<i32>();
            let mut result = date.clone();
            // The month-end clamp the contract's own arithmetic carries.
            let mut month = i64::from(result.month) + i64::from(total);
            let mut year = result.year;
            while month > 12 {
                month -= 12;
                year += 1;
            }
            while month < 1 {
                month += 12;
                year -= 1;
            }
            result.year = year;
            result.month = month as u8;
            let last = last_day_of_month(year, month as u8);
            if result.day > last {
                result.day = last;
            }
            Ok(result)
        }

        fn subtract_calendar_units(
            &self,
            date: &time_types::HistoricalDate,
            units: &[time_types::CalendarUnit],
        ) -> smart_lib::core::Result<time_types::HistoricalDate> {
            let negated: Vec<time_types::CalendarUnit> = units
                .iter()
                .map(|unit| time_types::CalendarUnit {
                    kind: unit.kind,
                    amount: -unit.amount,
                })
                .collect();
            self.add_calendar_units(date, &negated)
        }

        fn diff_calendar_dates(
            &self,
            date1: &time_types::HistoricalDate,
            date2: &time_types::HistoricalDate,
        ) -> smart_lib::core::Result<time_types::CalendarDiff> {
            let days = time_types::days_from_civil(date1.year, u32::from(date1.month), u32::from(date1.day))
                - time_types::days_from_civil(date2.year, u32::from(date2.month), u32::from(date2.day));
            Ok(time_types::CalendarDiff {
                years: (days / 365) as i32,
                months: ((days % 365) / 30) as i32,
                days: (days % 30) as i32,
                total_days: days,
                negative: days < 0,
            })
        }

        fn convert_to_tai(
            &self,
            utc: smart_lib::core::values::Timestamp,
        ) -> smart_lib::core::Result<time_types::TaiTimestamp> {
            // The contract's own constant: TAI = UTC + 37 s, in seconds and
            // nanoseconds since the Unix epoch.
            let total = utc.as_nanos() + u64::from(time_types::LEAP_SECONDS_TAI_MINUS_UTC) * 1_000_000_000;
            Ok(time_types::TaiTimestamp {
                tai_seconds: total / 1_000_000_000,
                nanoseconds: (total % 1_000_000_000) as u32,
            })
        }

        fn convert_from_tai(
            &self,
            tai: time_types::TaiTimestamp,
        ) -> smart_lib::core::Result<smart_lib::core::values::Timestamp> {
            let total = tai.tai_seconds * 1_000_000_000 + u64::from(tai.nanoseconds);
            let utc = total - u64::from(time_types::LEAP_SECONDS_TAI_MINUS_UTC) * 1_000_000_000;
            Ok(smart_lib::core::values::Timestamp::from_nanos(utc))
        }

        fn time_caps(&self) -> Vec<&'static str> {
            vec!["time.datetime", "time.calendar", "time.tai"]
        }
    }

    /// The last day of one month, in the arithmetic calendars' own rule.
    fn last_day_of_month(year: i64, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ => {
                if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                    29
                } else {
                    28
                }
            }
        }
    }
}

#[cfg(feature = "domain-time")]
pub use time_fixture::*;
