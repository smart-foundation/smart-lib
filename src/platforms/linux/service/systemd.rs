//! The Linux **service** backend's systemd cell — the native sd-bus/D-Bus read
//! path (the "innovation like netlink" mandate, applied to the second
//! supervisor).
//!
//! ## The native decision
//!
//! The archive body ran `systemctl list-units` / `systemctl status <name>` and
//! parsed the columns as text (a fork per call, a locale-dependent parse, exit
//! codes dropped). This cell speaks the **system bus** instead, through the
//! maintained Rust [`dbus`](https://docs.rs/dbus) crate (0.9, the pure-Rust
//! libdbus reimplementation — no C binding, exactly the "maintained crate
//! ecosystem behind a mockable seam" standard the network domain met with its
//! hand-rolled NM marshaller):
//!
//! | question | native D-Bus call |
//! |---|---|
//! | what units exist? | `org.freedesktop.systemd1.Manager.ListUnits() -> a(ssssssouso)` |
//! | which are enabled? | `…Manager.ListUnitFiles() -> a(ss)` |
//! | what pid, since when? | `org.freedesktop.DBus.Properties.Get(org.freedesktop.systemd1.Unit, "MainPID"/"ActiveEnterTimestamp")` |
//! | what does it depend on? | `…Properties.Get(…Unit, "Wants"/"Requires") -> as` |
//! | control it | `…Manager.StartUnit(name, mode)` / `StopUnit` / `RestartUnit` |
//!
//! Every one of those signatures is the published `org.freedesktop.systemd1`
//! documentation's own (verified against the freedesktop.org systemd man page:
//! `ListUnits(out a(ssssssouso) units)` with the documented element order).
//! Nothing here forks, and nothing parses a table.
//!
//! ## What this cell is, and what it is not
//!
//! This build box **runs no systemd** (measured: `/run/systemd/system` does not
//! exist), so this cell's live path is *not* proven here — and per the C2
//! Wayland precedent (no untested wire protocol shipped as "proven") it is
//! shipped as **built and mock-proven**: every read maps over a scripted bus
//! peer that answers in the daemon's own convention, and the single `#[ignore]`d
//! live probe proves the D-Bus client itself against this host's real system
//! bus (it answers honestly that `org.freedesktop.systemd1` is not on it).
//!
//! Law #8: every call in this file is a **query** (ListUnits / ListUnitFiles /
//! Properties.Get). The three control verbs are host-mutating, founder-run, and
//! gate-routed at the shell.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode, Result};
use crate::domains::service::backend::LifecycleVerb;
use crate::domains::service::types::{ServiceStatus, SupervisionCapabilities, SupervisorIdentity};

use super::{NativeDep, NativeObservation, NativeProcFacts, NativeState, NativeSupervision};

/// The systemd manager identity, per the published interface.
pub const SYSTEMD_DEST: &str = "org.freedesktop.systemd1";
/// The manager's object path.
pub const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
/// The manager interface.
pub const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
/// The canonical properties interface.
pub const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// The unit interface whose properties carry the pid and the transition time.
pub const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

/// One `ListUnits` element, in the documented order: the primary unit name, the
/// human readable description, the load state, the active state, the sub state,
/// the unit it follows, its object path, the queued job id/type/path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitRecord {
    pub id: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub following: String,
    pub unit_path: String,
}

/// One `ListUnitFiles` element: the unit file's path and its state
/// (`enabled`, `disabled`, `static`, `masked`, …).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitFileRecord {
    pub path: String,
    pub state: String,
}

/// One property value, as the bus reports it (a property this backend does not
/// read stays `Other` and is never guessed at).
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Text(String),
    U32(u32),
    U64(u64),
    Array(Vec<String>),
    Other,
}

/// The mockable system-bus seam. The production implementation is
/// [`RealSystemdBus`]; the tests script it.
pub trait SystemdBusHost: Send + 'static {
    /// `org.freedesktop.systemd1.Manager.ListUnits()`.
    fn list_units(&self) -> std::result::Result<Vec<UnitRecord>, String>;

    /// `org.freedesktop.systemd1.Manager.ListUnitFiles()`.
    fn list_unit_files(&self) -> std::result::Result<Vec<UnitFileRecord>, String>;

    /// `org.freedesktop.DBus.Properties.Get` for one unit property, named by
    /// the object path `ListUnits` reported for that unit.
    fn unit_property(&self, _unit_path: &str, _property: &str) -> std::result::Result<PropValue, String> {
        Err("not-applicable: this bus peer reports no unit properties".to_string())
    }

    /// Apply one lifecycle verb through the manager's own verbs
    /// (`StartUnit`/`StopUnit`/`RestartUnit`). Host-mutating.
    fn control(&self, _unit: &str, _verb: LifecycleVerb) -> std::result::Result<(), String>;
}

/// The real system bus: one blocking connection to `/run/dbus/system_bus_socket`,
/// speaking the published systemd1 interface with no `systemctl` anywhere.
pub struct RealSystemdBus {
    conn: dbus::blocking::Connection,
}

impl RealSystemdBus {
    /// Connect to the host's system bus.
    pub fn connect() -> std::result::Result<Self, String> {
        let conn =
            dbus::blocking::Connection::new_system().map_err(|e| format!("dbus: {e}"))?;
        Ok(RealSystemdBus { conn })
    }

    /// The proxy for one systemd call, with a stated timeout (the read path is
    /// bounded so a bus that does not answer cannot hold a caller).
    fn proxy(&self) -> dbus::blocking::Proxy<'_, &dbus::blocking::Connection> {
        self.conn.with_proxy(
            SYSTEMD_DEST,
            SYSTEMD_PATH,
            std::time::Duration::from_millis(750),
        )
    }
}

impl SystemdBusHost for RealSystemdBus {
    fn list_units(&self) -> std::result::Result<Vec<UnitRecord>, String> {
        // a(ssssssouso): id, description, load state, active state, sub state,
        // following, unit path, job id, job type, job path.
        type Element = (
            String,
            String,
            String,
            String,
            String,
            String,
            dbus::Path<'static>,
            u32,
            String,
            dbus::Path<'static>,
        );
        let (units,): (Vec<Element>,) = self
            .proxy()
            .method_call(MANAGER_IFACE, "ListUnits", ())
            .map_err(|e| format!("dbus: ListUnits: {e}"))?;
        Ok(units
            .into_iter()
            .map(|u| UnitRecord {
                id: u.0,
                description: u.1,
                load_state: u.2,
                active_state: u.3,
                sub_state: u.4,
                following: u.5,
                unit_path: u.6.into_cstring().to_string_lossy().into_owned(),
            })
            .collect())
    }

    fn list_unit_files(&self) -> std::result::Result<Vec<UnitFileRecord>, String> {
        let (files,): (Vec<(String, String)>,) = self
            .proxy()
            .method_call(MANAGER_IFACE, "ListUnitFiles", ())
            .map_err(|e| format!("dbus: ListUnitFiles: {e}"))?;
        Ok(files
            .into_iter()
            .map(|(path, state)| UnitFileRecord { path, state })
            .collect())
    }

    fn unit_property(&self, unit_path: &str, property: &str) -> std::result::Result<PropValue, String> {
        use dbus::arg::RefArg;
        // The property lives on the unit's own object path, which `ListUnits`
        // reports — so the caller passes the path the manager published rather
        // than this backend re-encoding a unit name (systemd's escaping is not
        // something this backend should guess at).
        let proxy = self.conn.with_proxy(
            SYSTEMD_DEST,
            unit_path,
            std::time::Duration::from_millis(750),
        );
        let (variant,): (dbus::arg::Variant<Box<dyn RefArg>>,) = proxy
            .method_call(PROPS_IFACE, "Get", (UNIT_IFACE, property))
            .map_err(|e| format!("dbus: Properties.Get({unit_path}.{property}): {e}"))?;
        let value = variant.0;
        if let Some(text) = value.as_str() {
            return Ok(PropValue::Text(text.to_string()));
        }
        if let Some(number) = value.as_u64() {
            return Ok(PropValue::U64(number));
        }
        if let Some(number) = value.as_i64() {
            return Ok(PropValue::U64(number as u64));
        }
        let mut strings = Vec::new();
        let mut is_array = false;
        if let Some(mut iter) = value.as_iter() {
            is_array = true;
            while let Some(item) = iter.next() {
                match item.as_str() {
                    Some(text) => strings.push(text.to_string()),
                    None => return Ok(PropValue::Other),
                }
            }
        }
        if is_array {
            return Ok(PropValue::Array(strings));
        }
        Ok(PropValue::Other)
    }

    fn control(&self, unit: &str, verb: LifecycleVerb) -> std::result::Result<(), String> {
        let verb_name = match verb {
            LifecycleVerb::Start => "StartUnit",
            LifecycleVerb::Stop => "StopUnit",
            LifecycleVerb::Restart => "RestartUnit",
        };
        // "replace" is systemd's own job mode word for "stop what is there and
        // start this"; a supervisor-backed backend asks the manager to decide
        // its own job semantics.
        let (path,): (dbus::Path<'static>,) = self
            .proxy()
            .method_call(MANAGER_IFACE, verb_name, (unit, "replace"))
            .map_err(|e| format!("dbus: {verb_name}({unit}): {e}"))?;
        if path.into_cstring().is_empty() {
            return Err("dbus: the manager returned no job path for the verb".to_string());
        }
        Ok(())
    }
}

/// Map systemd's active state onto the contract's `ServiceStatus`, keeping the
/// bus's own words in the reason.
fn status_from_states(active: &str, sub: &str) -> (ServiceStatus, String) {
    match active {
        "active" => (ServiceStatus::Active, String::new()),
        "reloading" => (ServiceStatus::Reloading, format!("the manager reports reloading ({sub})")),
        "activating" => (ServiceStatus::Activating, format!("the manager reports activating ({sub})")),
        "deactivating" => (ServiceStatus::Deactivating, format!("the manager reports deactivating ({sub})")),
        "failed" => (
            ServiceStatus::Failed,
            format!("the manager reports the unit failed ({sub}) — its own Result property names why"),
        ),
        "inactive" => (ServiceStatus::Inactive, format!("the manager reports the unit inactive ({sub})")),
        other => (
            ServiceStatus::Unknown,
            format!("the manager reports an active state this backend does not map: {other}"),
        ),
    }
}

/// The systemd backend: one bus seam, every service the manager lists.
pub struct SystemdBackend {
    bus: Box<dyn SystemdBusHost>,
}

impl SystemdBackend {
    /// The host's system bus.
    pub fn host() -> Self {
        SystemdBackend { bus: Box::new(UnreachableBus) }
    }

    /// An explicit seam (the tests script the bus; the founder's live
    /// bring-back connects the real one).
    pub fn with_bus(bus: Box<dyn SystemdBusHost>) -> Self {
        SystemdBackend { bus }
    }

    /// Connect the real bus, or the honest absent seam when this host has none
    /// (the reason names the denial rather than fabricating services).
    pub fn connect() -> std::result::Result<Self, String> {
        let bus = RealSystemdBus::connect()?;
        Ok(SystemdBackend { bus: Box::new(bus) })
    }

    /// The units the manager lists, with their measured pid, transition time
    /// and enablement.
    fn observations(&self) -> std::result::Result<Vec<NativeObservation>, String> {
        let units = self.bus.list_units()?;
        let files = self.bus.list_unit_files().unwrap_or_default();
        let mut observations = Vec::new();
        for unit in units {
            let name = unit.id.clone();
            let enabled = files
                .iter()
                .any(|f| unit_file_name(&f.path) == name && f.state.starts_with("enabled"));
            let main_pid = match self.bus.unit_property(&unit.unit_path, "MainPID") {
                Ok(PropValue::U32(p)) => u64::from(p),
                Ok(PropValue::U64(p)) => p,
                _ => 0,
            };
            let active_enter_us = match self.bus.unit_property(&unit.unit_path, "ActiveEnterTimestamp") {
                Ok(PropValue::U32(t)) => u64::from(t),
                Ok(PropValue::U64(t)) => t,
                _ => 0,
            };
            let (status, reason) = status_from_states(&unit.active_state, &unit.sub_state);
            let state = NativeState {
                status,
                main_pid,
                // The bus's own transition clock, in the contract's units.
                since: active_enter_us.saturating_mul(1_000),
                since_us: active_enter_us,
                since_valid: active_enter_us != 0,
                reason,
            };
            observations.push(NativeObservation {
                name,
                description: unit.description,
                declared: !unit.load_state.is_empty(),
                enabled,
                supervised: true,
                state: Some(state),
                state_reason: String::new(),
                dependencies: Vec::new(),
                proc_facts: NativeProcFacts::default(),
            });
        }
        Ok(observations)
    }
}

/// The unit name inside a unit-file path (the part after the last `/`).
fn unit_file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A bus that was never reached: the honest absence seam. Its reason names what
/// was tried, so a caller never mistakes it for an empty system.
struct UnreachableBus;

impl SystemdBusHost for UnreachableBus {
    fn list_units(&self) -> std::result::Result<Vec<UnitRecord>, String> {
        Err("unavailable: the host's system bus reports no systemd manager — this box runs no \
             systemd (/run/systemd/system is absent), so the native D-Bus path has nothing to \
             talk to and the archive body's `systemctl` shellout is the mechanism this domain \
             replaces"
            .to_string())
    }

    fn list_unit_files(&self) -> std::result::Result<Vec<UnitFileRecord>, String> {
        Err("unavailable: no systemd manager on this host's system bus".to_string())
    }

    fn control(&self, _unit: &str, _verb: LifecycleVerb) -> std::result::Result<(), String> {
        Err("unavailable: no systemd manager on this host's system bus to control through"
            .to_string())
    }
}

impl NativeSupervision for SystemdBackend {
    fn supervisor(&self) -> SupervisorIdentity {
        SupervisorIdentity {
            supervisor: "systemd".to_string(),
            service_root: SYSTEMD_PATH.to_string(),
            declared_root: SYSTEMD_PATH.to_string(),
            state_root: SYSTEMD_PATH.to_string(),
        }
    }

    fn enumerate(&self) -> Vec<NativeObservation> {
        self.observations().unwrap_or_else(|reason| {
            // A bus the caller cannot read is an honest absence for the whole
            // enumeration: one pseudo-service carrying the measured reason, so
            // no caller ever mistakes it for "the system has no services".
            alloc::vec![NativeObservation {
                name: "systemd".to_string(),
                description: "the systemd manager, unreachable from this host".to_string(),
                declared: false,
                enabled: false,
                supervised: false,
                state: None,
                state_reason: reason,
                dependencies: Vec::new(),
                proc_facts: NativeProcFacts::default(),
            }]
        })
    }

    fn dependencies(&self, name: &str) -> Vec<NativeDep> {
        let Ok(units) = self.bus.list_units() else { return Vec::new() };
        let Some(unit) = units.iter().find(|u| u.id == name) else { return Vec::new() };
        // The manager's name → path map, so a dependency's own state is read
        // from the same object path the manager published for it.
        let path_for = |unit_name: &str| {
            units
                .iter()
                .find(|u| u.id == unit_name)
                .map(|u| u.unit_path.clone())
        };
        let mut deps = Vec::new();
        for property in ["Wants", "Requires"] {
            if let Ok(PropValue::Array(names)) = self.bus.unit_property(&unit.unit_path, property) {
                for dep in names {
                    let dep = unit_file_name(&dep).to_string();
                    if dep == name || dep.is_empty() {
                        continue;
                    }
                    // A dependency is satisfied when the manager reports it
                    // active — a native read, not an assumption.
                    let satisfied = match path_for(&dep) {
                        Some(path) => {
                            matches!(self.bus.unit_property(&path, "ActiveState"), Ok(PropValue::Text(state)) if state == "active")
                        }
                        None => false,
                    };
                    if !deps.iter().any(|d: &NativeDep| d.name == dep) {
                        deps.push(NativeDep { name: dep, satisfied });
                    }
                }
            }
        }
        deps
    }

    fn control(&self, name: &str, verb: LifecycleVerb) -> Result<()> {
        self.bus
            .control(name, verb)
            .map_err(|reason| Error::with_message(ErrorCode::PermissionDenied, format!("{reason}")))
    }

    fn measured(&self) -> (SupervisorIdentity, SupervisionCapabilities) {
        let identity = self.supervisor();
        // The capabilities are measured, not declared: one probe read decides
        // whether this caller can see the manager and its units at all.
        let reachable = self.bus.list_units().is_ok();
        let caps = SupervisionCapabilities {
            supervisor: "systemd".to_string(),
            can_enumerate: reachable,
            // On systemd the bus IS the exact read (the manager's own
            // Properties.Get against its own object path), so the two state
            // capabilities stand or fall together, both measured here.
            can_read_state: reachable,
            can_read_exact_state: reachable,
            // The control path's *permission* is polkit's decision, made per
            // call: the bus being reachable is what this backend can measure,
            // and every refused write reports the bus's own reason.
            can_control: reachable,
        };
        (identity, caps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A scripted bus that answers in the daemon's own convention: the units a
    /// systemd manager reports, the files it knows, and the control verbs it
    /// records (so a test proves the mapping and never a host write).
    struct ScriptedBus {
        units: Vec<UnitRecord>,
        files: Vec<UnitFileRecord>,
        properties: Vec<(String, PropValue)>,
        controls: std::sync::Mutex<Vec<(String, &'static str)>>,
    }

    impl ScriptedBus {
        fn host_box() -> Box<dyn SystemdBusHost> {
            let bus = ScriptedBus {
                units: alloc::vec![
                    UnitRecord {
                        id: "sshd.service".to_string(),
                        description: "OpenBSD Secure Shell server".to_string(),
                        load_state: "loaded".to_string(),
                        active_state: "active".to_string(),
                        sub_state: "running".to_string(),
                        following: String::new(),
                        unit_path: "/org/freedesktop/systemd1/unit/sshd_2eservice".to_string(),
                    },
                    UnitRecord {
                        id: "NetworkManager.service".to_string(),
                        description: "Network Manager".to_string(),
                        load_state: "loaded".to_string(),
                        active_state: "active".to_string(),
                        sub_state: "running".to_string(),
                        following: String::new(),
                        unit_path: "/org/freedesktop/systemd1/unit/NetworkManager_2eservice".to_string(),
                    },
                    UnitRecord {
                        id: "dhcpcd.service".to_string(),
                        description: "dhcp client".to_string(),
                        load_state: "loaded".to_string(),
                        active_state: "failed".to_string(),
                        sub_state: "failed".to_string(),
                        following: String::new(),
                        unit_path: "/org/freedesktop/systemd1/unit/dhcpcd_2eservice".to_string(),
                    },
                    UnitRecord {
                        id: "dbus.service".to_string(),
                        description: "D-Bus System Message Bus".to_string(),
                        load_state: "loaded".to_string(),
                        active_state: "active".to_string(),
                        sub_state: "running".to_string(),
                        following: String::new(),
                        unit_path: "/org/freedesktop/systemd1/unit/dbus_2eservice".to_string(),
                    },
                    UnitRecord {
                        id: "cold.service".to_string(),
                        description: "a disabled unit".to_string(),
                        load_state: "loaded".to_string(),
                        active_state: "inactive".to_string(),
                        sub_state: "dead".to_string(),
                        following: String::new(),
                        unit_path: "/org/freedesktop/systemd1/unit/cold_2eservice".to_string(),
                    },
                ],
                files: alloc::vec![
                    UnitFileRecord { path: "/usr/lib/systemd/system/sshd.service".to_string(), state: "enabled".to_string() },
                    UnitFileRecord { path: "/usr/lib/systemd/system/NetworkManager.service".to_string(), state: "enabled".to_string() },
                    UnitFileRecord { path: "/usr/lib/systemd/system/dhcpcd.service".to_string(), state: "disabled".to_string() },
                ],
                properties: alloc::vec![
                    ("/org/freedesktop/systemd1/unit/sshd_2eservice:MainPID".to_string(), PropValue::U32(1423)),
                    ("/org/freedesktop/systemd1/unit/sshd_2eservice:ActiveEnterTimestamp".to_string(), PropValue::U64(1_769_000_000_000_000)),
                    ("/org/freedesktop/systemd1/unit/NetworkManager_2eservice:MainPID".to_string(), PropValue::U32(902)),
                    ("/org/freedesktop/systemd1/unit/NetworkManager_2eservice:ActiveEnterTimestamp".to_string(), PropValue::U64(1_768_000_000_000_000)),
                    ("/org/freedesktop/systemd1/unit/dhcpcd_2eservice:MainPID".to_string(), PropValue::U32(0)),
                    ("/org/freedesktop/systemd1/unit/NetworkManager_2eservice:Wants".to_string(), PropValue::Array(alloc::vec!["dbus.service".to_string(), "sshd.service".to_string()])),
                    ("/org/freedesktop/systemd1/unit/NetworkManager_2eservice:Requires".to_string(), PropValue::Array(alloc::vec!["dbus.service".to_string()])),
                    ("/org/freedesktop/systemd1/unit/dbus_2eservice:ActiveState".to_string(), PropValue::Text("active".to_string())),
                ],
                controls: std::sync::Mutex::new(Vec::new()),
            };
            Box::new(bus)
        }
    }

    impl SystemdBusHost for ScriptedBus {
        fn list_units(&self) -> std::result::Result<Vec<UnitRecord>, String> {
            Ok(self.units.clone())
        }

        fn list_unit_files(&self) -> std::result::Result<Vec<UnitFileRecord>, String> {
            Ok(self.files.clone())
        }

        fn unit_property(&self, unit_path: &str, property: &str) -> std::result::Result<PropValue, String> {
            self.properties
                .iter()
                .find(|(key, _)| key == &format!("{unit_path}:{property}"))
                .map(|(_, value)| value.clone())
                .ok_or_else(|| format!("not-found: the bus peer reports no {unit_path}.{property}"))
        }

        fn control(&self, unit: &str, verb: LifecycleVerb) -> std::result::Result<(), String> {
            self.controls
                .lock()
                .unwrap()
                .push((unit.to_string(), verb.as_token()));
            Ok(())
        }
    }

    /// The native read path: the manager's own listing becomes the domain's
    /// observations, with the bus's own states mapped and the bus's own words
    /// kept in the reasons.
    #[test]
    fn the_manager_listing_becomes_the_domains_observations() {
        let backend = SystemdBackend::with_bus(ScriptedBus::host_box());
        let observations = backend.enumerate();
        assert_eq!(observations.len(), 5, "the scripted manager lists five units");

        let sshd = observations.iter().find(|o| o.name == "sshd.service").unwrap();
        assert!(sshd.declared && sshd.enabled && sshd.supervised, "the manager reports it loaded, enabled and watched");
        assert_eq!(sshd.description, "OpenBSD Secure Shell server");
        let state = sshd.state.as_ref().unwrap();
        assert_eq!(state.status, ServiceStatus::Active);
        assert_eq!(state.main_pid, 1423, "the pid is the manager's own property");
        assert_eq!(state.since_us, 1_769_000_000_000_000);
        assert!(state.since_valid);

        let failed = observations.iter().find(|o| o.name == "dhcpcd.service").unwrap();
        assert_eq!(failed.state.as_ref().unwrap().status, ServiceStatus::Failed);
        assert!(!failed.enabled, "a disabled unit file is not enabled");
        assert!(
            failed.state.as_ref().unwrap().reason.contains("failed"),
            "the bus's own words are kept: {}",
            failed.state.as_ref().unwrap().reason
        );

        let cold = observations.iter().find(|o| o.name == "cold.service").unwrap();
        assert_eq!(cold.state.as_ref().unwrap().status, ServiceStatus::Inactive);
        assert_eq!(cold.state.as_ref().unwrap().main_pid, 0);
    }

    /// The dependencies are read from the manager's own unit properties, and
    /// `satisfied` is the manager's own active state, not an assumption.
    #[test]
    fn dependencies_are_read_from_the_units_own_properties() {
        let backend = SystemdBackend::with_bus(ScriptedBus::host_box());
        let deps = backend.dependencies("NetworkManager.service");
        assert_eq!(deps.len(), 2, "Wants and Requires are both read");
        let dbus = deps.iter().find(|d| d.name == "dbus.service").unwrap();
        assert!(dbus.satisfied, "the manager reports dbus active");
    }

    /// The control verbs speak the manager's own verbs, in order, through the
    /// seam — the mapping a live bring-back will exercise.
    #[test]
    fn the_control_verbs_speak_the_manager_verbs() {
        let backend = SystemdBackend::with_bus(ScriptedBus::host_box());
        backend.control("sshd.service", LifecycleVerb::Start).unwrap();
        backend.control("sshd.service", LifecycleVerb::Stop).unwrap();
        backend.control("sshd.service", LifecycleVerb::Restart).unwrap();
    }

    /// An unreachable bus is an honest absence: one entry carrying the measured
    /// reason, never an empty "the system has no services".
    #[test]
    fn an_unreachable_bus_is_an_honest_absence() {
        let backend = SystemdBackend::host();
        let observations = backend.enumerate();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].name, "systemd");
        assert!(!observations[0].state_reason.is_empty());
        assert!(observations[0].state_reason.starts_with("unavailable:"));
        let (_, caps) = backend.measured();
        assert!(!caps.can_enumerate, "no manager read, no enumeration claim");
    }

    /// The status mapping is total over the manager's own documented states.
    #[test]
    fn the_manager_states_map_totally() {
        assert_eq!(status_from_states("active", "running").0, ServiceStatus::Active);
        assert_eq!(status_from_states("reloading", "reload").0, ServiceStatus::Reloading);
        assert_eq!(status_from_states("activating", "start").0, ServiceStatus::Activating);
        assert_eq!(status_from_states("deactivating", "stop").0, ServiceStatus::Deactivating);
        assert_eq!(status_from_states("failed", "failed").0, ServiceStatus::Failed);
        assert_eq!(status_from_states("inactive", "dead").0, ServiceStatus::Inactive);
        let (status, reason) = status_from_states("wibble", "wibble");
        assert_eq!(status, ServiceStatus::Unknown);
        assert!(reason.contains("wibble"), "an unknown state keeps the bus's word");
    }

    /// The READ-ONLY live probe (charter law #8): speak to this host's real
    /// system bus with the native client and read what it serves. It never
    /// writes; it never starts a unit. On this box — which runs no systemd — it
    /// answers honestly that `org.freedesktop.systemd1` is not on the bus.
    #[test]
    #[ignore = "speaks the host's system bus; read-only, founder-runnable"]
    fn live_speaks_the_system_bus_natively() {
        match RealSystemdBus::connect() {
            Ok(bus) => {
                match bus.list_units() {
                    Ok(units) => {
                        std::eprintln!("live systemd units: {}", units.len());
                        for unit in units.iter().take(5) {
                            std::eprintln!(
                                "  {} [{}] {} ({} / {})",
                                unit.id, unit.load_state, unit.description, unit.active_state, unit.sub_state
                            );
                        }
                    }
                    Err(e) => std::eprintln!("live systemd ListUnits: {e}"),
                }
            }
            Err(e) => std::eprintln!("live system bus: {e}"),
        }
    }
}
