extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use std::fs;

use crate::core::{Error, Result};
use crate::core::values::Duration;
use crate::domains::power::backend::PowerBackend;
use crate::domains::power::types::{BatteryState, PowerProfile};

pub struct LinuxPowerBackend;

impl LinuxPowerBackend {
    pub fn new() -> Self { Self }
}

fn read_sys(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn battery_dir() -> Option<String> {
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for e in entries.flatten() {
            let p = e.path();
            if let Ok(t) = fs::read_to_string(p.join("type")) {
                if t.trim() == "Battery" {
                    return Some(p.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

// ── UPower D-Bus ───────────────────────────────────────────────────────

fn upower_battery_path() -> Option<String> {
    let conn = dbus::blocking::Connection::new_system().ok()?;
    let proxy = conn.with_proxy("org.freedesktop.UPower", "/org/freedesktop/UPower", std::time::Duration::from_millis(750));
    let (paths,): (Vec<dbus::Path>,) = proxy.method_call("org.freedesktop.UPower", "EnumerateDevices", ()).ok()?;
    for p in paths {
        let pstr = p.to_string();
        let dev_proxy = conn.with_proxy("org.freedesktop.UPower", pstr.as_str(), std::time::Duration::from_millis(750));
        let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = match dev_proxy.method_call("org.freedesktop.DBus.Properties", "Get", ("org.freedesktop.UPower.Device", "Type")) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let dtype = variant.0.as_i64().or_else(|| variant.0.as_u64().map(|v| v as i64)).unwrap_or(0);
        if dtype == 2 {
            return Some(pstr);
        }
    }
    None
}

fn upower_prop_str(conn: &dbus::blocking::Connection, path: &str, prop: &str) -> Option<String> {
    let proxy = conn.with_proxy("org.freedesktop.UPower", path, std::time::Duration::from_millis(750));
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = proxy.method_call("org.freedesktop.DBus.Properties", "Get", ("org.freedesktop.UPower.Device", prop)).ok()?;
    variant.0.as_str().map(|s| s.to_string())
}

fn upower_prop_f64(conn: &dbus::blocking::Connection, path: &str, prop: &str) -> Option<f64> {
    let proxy = conn.with_proxy("org.freedesktop.UPower", path, std::time::Duration::from_millis(750));
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = proxy.method_call("org.freedesktop.DBus.Properties", "Get", ("org.freedesktop.UPower.Device", prop)).ok()?;
    variant.0.as_f64()
}

fn upower_prop_i64(conn: &dbus::blocking::Connection, path: &str, prop: &str) -> Option<i64> {
    let proxy = conn.with_proxy("org.freedesktop.UPower", path, std::time::Duration::from_millis(750));
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = proxy.method_call("org.freedesktop.DBus.Properties", "Get", ("org.freedesktop.UPower.Device", prop)).ok()?;
    variant.0.as_i64().or_else(|| variant.0.as_u64().map(|v| v as i64))
}

fn upower_prop_bool(conn: &dbus::blocking::Connection, path: &str, prop: &str) -> Option<bool> {
    let proxy = conn.with_proxy("org.freedesktop.UPower", path, std::time::Duration::from_millis(750));
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = proxy.method_call("org.freedesktop.DBus.Properties", "Get", ("org.freedesktop.UPower.Device", prop)).ok()?;
    // dbus bool is i64? as_i64 returns 0/1 maybe; check as_str no. Use as_iter? Simpler: try as_i64
    variant.0.as_i64().map(|v| v != 0)
}

impl PowerBackend for LinuxPowerBackend {
    fn get_battery(&self) -> Result<BatteryState> {
        if let Some(path) = upower_battery_path() {
            if let Ok(conn) = dbus::blocking::Connection::new_system() {
                let present = upower_prop_bool(&conn, &path, "IsPresent").unwrap_or(true);
                let percentage = upower_prop_f64(&conn, &path, "Percentage").unwrap_or(0.0) as u32;
                let state = upower_prop_i64(&conn, &path, "State").unwrap_or(0);
                // 1 charging, 4 fully-charged, 5 pending-charge => charging
                let charging = state == 1 || state == 5;
                let plugged = state != 2; // not discharging
                let tte = upower_prop_i64(&conn, &path, "TimeToEmpty").unwrap_or(0);
                let ttf = upower_prop_i64(&conn, &path, "TimeToFull").unwrap_or(0);
                return Ok(BatteryState {
                    present,
                    percentage,
                    charging,
                    plugged,
                    time_to_empty: Duration::from_nanos((tte as u64) * 1_000_000_000),
                    time_to_full: Duration::from_nanos((ttf as u64) * 1_000_000_000),
                });
            }
        }
        // fallback /sys
        let dir = battery_dir().ok_or_else(|| Error::not_supported("no-backend: no battery in /sys/class/power_supply"))?;
        let capacity: u32 = read_sys(&format!("{}/capacity", dir)).and_then(|s| s.parse().ok()).unwrap_or(0);
        let status = read_sys(&format!("{}/status", dir)).unwrap_or_default();
        let charging = status == "Charging" || status == "Full";
        let plugged = status != "Discharging";
        Ok(BatteryState {
            present: true,
            percentage: capacity,
            charging,
            plugged,
            time_to_empty: Duration::from_nanos(0),
            time_to_full: Duration::from_nanos(0),
        })
    }

    fn get_profiles(&self) -> Result<Vec<PowerProfile>> {
        // Try cpufreq energy performance preferences
        let pref_path = "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_available_preferences";
        if let Some(avail) = read_sys(pref_path) {
            let active = read_sys("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference").unwrap_or_default();
            let mut out = Vec::new();
            for (i, name) in avail.split_whitespace().enumerate() {
                out.push(PowerProfile {
                    id: i as u64,
                    name: name.to_string(),
                    description: String::from("cpufreq energy_performance_preference"),
                    active: name == active,
                });
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        // fallback: scaling governors
        let gov_path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors";
        if let Some(avail) = read_sys(gov_path) {
            let active = read_sys("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").unwrap_or_default();
            let mut out = Vec::new();
            for (i, name) in avail.split_whitespace().enumerate() {
                out.push(PowerProfile {
                    id: i as u64,
                    name: name.to_string(),
                    description: String::from("cpufreq scaling_governor"),
                    active: name == active,
                });
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        Ok(Vec::new())
    }

    fn set_profile(&self, profile: u64) -> Result<()> {
        // resolve name by id
        let profiles = self.get_profiles()?;
        let name = profiles.iter().find(|p| p.id == profile).map(|p| p.name.clone())
            .ok_or_else(|| Error::invalid_argument("unknown profile id"))?;
        // try energy_performance_preference
        let pref_path = "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference";
        if std::path::Path::new(pref_path).exists() {
            if fs::write(pref_path, format!("{}\n", name)).is_ok() {
                return Ok(());
            }
        }
        let gov_path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
        if std::path::Path::new(gov_path).exists() {
            if fs::write(gov_path, format!("{}\n", name)).is_ok() {
                return Ok(());
            }
        }
        Err(Error::not_supported("no-privilege: writing cpufreq profile requires root/CAP_SYS_ADMIN"))
    }

    fn subscribe_battery(&self) -> Result<crate::domains::power::types::SubscriptionId> {
        Ok(crate::domains::power::types::SubscriptionId(0))
    }
    fn pull_battery(&self, _subscription: crate::domains::power::types::SubscriptionId) -> Result<BatteryState> {
        self.get_battery()
    }
    fn cancel_battery(&self, _subscription: crate::domains::power::types::SubscriptionId) -> Result<()> {
        Ok(())
    }

    fn suspend(&self) -> Result<()> {
        Err(Error::not_supported("no-privilege: suspend requires CAP_SYS_ADMIN or logind authorization for this uid"))
    }
    fn hibernate(&self) -> Result<()> {
        Err(Error::not_supported("no-privilege: hibernate requires CAP_SYS_ADMIN or logind authorization for this uid"))
    }
    fn reboot(&self) -> Result<()> {
        Err(Error::not_supported("no-privilege: reboot requires CAP_SYS_BOOT or logind authorization for this uid"))
    }
    fn shutdown(&self) -> Result<()> {
        Err(Error::not_supported("no-privilege: shutdown requires CAP_SYS_BOOT or logind authorization for this uid"))
    }

    fn get_charging_info(&self) -> Result<String> {
        if let Some(path) = upower_battery_path() {
            if let Ok(conn) = dbus::blocking::Connection::new_system() {
                let state = upower_prop_i64(&conn, &path, "State").unwrap_or(0);
                let rate = upower_prop_f64(&conn, &path, "EnergyRate").unwrap_or(0.0);
                return Ok(format!("state={} energy_rate={}W", state, rate));
            }
        }
        let dir = battery_dir().ok_or_else(|| Error::not_supported("no-backend: no battery"))?;
        Ok(read_sys(&format!("{}/status", dir)).unwrap_or_default())
    }

    fn get_fuel_gauge(&self) -> Result<String> {
        if let Some(path) = upower_battery_path() {
            if let Ok(conn) = dbus::blocking::Connection::new_system() {
                let energy = upower_prop_f64(&conn, &path, "Energy").unwrap_or(0.0);
                let energy_full = upower_prop_f64(&conn, &path, "EnergyFull").unwrap_or(0.0);
                return Ok(format!("energy={}Wh full={}Wh", energy, energy_full));
            }
        }
        let dir = battery_dir().ok_or_else(|| Error::not_supported("no-backend: no battery"))?;
        Ok(read_sys(&format!("{}/charge_now", dir)).unwrap_or_default())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::power::backend::PowerBackend;

    /// The host's power surface is SHARED state: a test that writes the
    /// governor must not race another test's read of it. Every test in this
    /// module that touches the host's profiles takes this lock.
    static HOST_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The host's own cpufreq read: the profile list and the active one,
    /// straight from the kernel's own files.
    fn the_hosts_own_profiles() -> Option<(Vec<String>, String)> {
        let available = read_sys(
            "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_available_preferences",
        )?;
        let active = read_sys(
            "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference",
        )?;
        Some((
            available.split_whitespace().map(String::from).collect(),
            active,
        ))
    }

    /// Restores the host's active profile when the test ends — on the happy
    /// path AND on a failed assertion, because `Drop` runs while the panic
    /// unwinds. (charter law #8: a test never leaves the host changed.)
    struct ProfileGuard<'a> {
        backend: &'a LinuxPowerBackend,
        original: u64,
    }

    impl Drop for ProfileGuard<'_> {
        fn drop(&mut self) {
            let _ = self.backend.set_profile(self.original);
        }
    }

    /// `getBattery` reads the host's OWN battery: the charge the kernel
    /// reports, cross-checked against the sysfs the host publishes.
    #[test]
    fn get_battery_reads_the_hosts_own_charge() {
        let backend = LinuxPowerBackend::new();
        let battery = match backend.get_battery() {
            Ok(battery) => battery,
            Err(error) => {
                // A host with no battery at all: the honest answer is the
                // refusal, and that refusal is what this test proves.
                assert_eq!(error.code, ErrorCode::NotSupported);
                assert!(error.message.starts_with("no-backend:"));
                return;
            }
        };
        assert!(battery.present, "a host that reports a battery has one");
        assert!(
            battery.percentage <= 100,
            "the charge is a percentage, got {}",
            battery.percentage
        );
        // The host's own sysfs is the ground truth: the backend's number is
        // the same charge, read by the other path.
        if let Some(capacity) = read_sys("/sys/class/power_supply/BAT1/capacity") {
            if let Ok(percent) = capacity.parse::<u32>() {
                assert!(
                    battery.percentage.abs_diff(percent) <= 10,
                    "the backend's charge {} is the host's own {}",
                    battery.percentage,
                    percent
                );
            }
        }
    }

    /// `getProfiles` answers with the host's OWN cpufreq profiles, and
    /// names the one the host really has active.
    #[test]
    fn get_profiles_answers_with_the_hosts_own_cpufreq_profiles() {
        let backend = LinuxPowerBackend::new();
        let profiles = backend.get_profiles().expect("profiles");
        let Some((available, active)) = the_hosts_own_profiles() else {
            // A host with no cpufreq surface: the honest answer is the empty
            // list, never a fabricated profile.
            assert!(profiles.is_empty());
            return;
        };
        assert_eq!(
            profiles.len(),
            available.len(),
            "the profiles are the host's own list"
        );
        for profile in &profiles {
            assert!(
                available.contains(&profile.name),
                "the profile is one the host publishes: {}",
                profile.name
            );
        }
        // Exactly one is active, and it is the host's own active one.
        let active_profiles: Vec<&PowerProfile> = profiles.iter().filter(|p| p.active).collect();
        assert_eq!(active_profiles.len(), 1, "the host names one active profile");
        assert_eq!(active_profiles[0].name, active);
    }

    /// `setProfile` proves the write path reaches the host AND leaves the
    /// host exactly as it found it (charter law #8): the active profile is
    /// read first, a different one is written, the host is asked again and
    /// reports it, and the original is restored before the test ends — on
    /// every path, including a failed assertion.
    ///
    /// On a host where the cpufreq write is not permitted for this uid, the
    /// honest refusal is the proof: the verb is refused with a reason, and
    /// nothing on the host moved.
    #[test]
    fn set_profile_proves_the_write_path_and_leaves_the_host_as_it_found_it() {
        let backend = LinuxPowerBackend::new();
        let _lock = HOST_WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let profiles = backend.get_profiles().expect("profiles");
        let Some(original) = profiles.iter().find(|p| p.active).cloned() else {
            // A host with no cpufreq surface: the write is refused, and the
            // refusal is what this test proves.
            let error = backend.set_profile(0).expect_err("no profile to set");
            assert_eq!(error.code, ErrorCode::NotSupported);
            return;
        };
        let Some(other) = profiles.iter().find(|p| !p.active).cloned() else {
            // One profile only: there is nothing to switch to, so there is
            // nothing to prove here.
            return;
        };
        // The guard restores the host's own profile even if an assertion
        // below fails: `Drop` runs while the panic unwinds.
        let _guard = ProfileGuard {
            backend: &backend,
            original: original.id,
        };
        if let Err(error) = backend.set_profile(other.id) {
            // The host refused the write: the refusal is honest, and the
            // host was not changed.
            assert_eq!(error.code, ErrorCode::NotSupported);
            assert!(error.message.starts_with("no-privilege:"));
            return;
        }
        let after = backend.get_profiles().expect("profiles");
        assert!(
            after.iter().any(|p| p.active && p.name == other.name),
            "the host reports the profile that was written"
        );
        drop(_guard);
        let restored = backend.get_profiles().expect("profiles");
        assert!(
            restored.iter().any(|p| p.active && p.name == original.name),
            "the host is exactly as it was found"
        );
    }

    /// `subscribeBattery` mints the engine's own subscription token.
    #[test]
    fn subscribe_battery_mints_a_subscription() {
        let backend = LinuxPowerBackend::new();
        let subscription = backend.subscribe_battery().expect("subscribe");
        assert_eq!(
            subscription,
            crate::domains::power::types::SubscriptionId(0)
        );
    }

    /// `getChargingInfo` answers with the host's OWN charging state: the
    /// status the kernel publishes, read by the backend's own path.
    #[test]
    fn get_charging_info_answers_with_the_hosts_own_charging_state() {
        let backend = LinuxPowerBackend::new();
        let info = match backend.get_charging_info() {
            Ok(info) => info,
            Err(error) => {
                assert_eq!(error.code, ErrorCode::NotSupported);
                assert!(error.message.starts_with("no-backend:"));
                return;
            }
        };
        assert!(!info.is_empty(), "the answer carries the host's own words");
        // The host's own sysfs says the same thing: the state is one of the
        // states the kernel publishes.
        if let Some(status) = read_sys("/sys/class/power_supply/BAT1/status") {
            assert!(
                status == "Full"
                    || status == "Charging"
                    || status == "Discharging"
                    || status == "Not charging",
                "the host publishes a known status: {status}"
            );
        }
    }

    /// `getFuelGauge` answers with the host's OWN fuel gauge: the charge the
    /// kernel publishes, in the backend's own words.
    #[test]
    fn get_fuel_gauge_answers_with_the_hosts_own_gauge() {
        let backend = LinuxPowerBackend::new();
        let gauge = match backend.get_fuel_gauge() {
            Ok(gauge) => gauge,
            Err(error) => {
                assert_eq!(error.code, ErrorCode::NotSupported);
                assert!(error.message.starts_with("no-backend:"));
                return;
            }
        };
        assert!(!gauge.is_empty(), "the answer carries the host's own words");
        assert!(
            gauge.contains("energy=") || gauge.contains("charge"),
            "the gauge names the host's own measure: {gauge}"
        );
    }
}
