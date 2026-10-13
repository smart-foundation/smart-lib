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
    Pagination, Page, RadioState, SubscriptionId,
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
        ssid: String::new(),
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

    fn wifi_scan(&self, page: Pagination) -> smart_lib::core::Result<Page<smart_lib::domains::network::types::WifiNetwork>> {
        self.seams.borrow_mut().push("wifi_scan".to_string());
        if self.scan_error {
            return Err(smart_lib::core::Error::not_supported(
                "no-backend: active WiFi scan orchestration (NM RequestScan + AccessPoint enumeration) not implemented",
            ));
        }
        let _ = page;
        Ok(Page {
            items: Vec::new(),
            total: 0,
        })
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
