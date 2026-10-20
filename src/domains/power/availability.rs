//! power availability matrix.

pub const POWER_SCHEMA_ID: u64 = 11651590505119481870;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass { NoBackend, NoPrivilege, NotApplicable, DisabledByPolicy, Unknown }
impl ReasonClass { pub const fn as_str(self) -> &'static str { match self { ReasonClass::NoBackend => "no-backend", ReasonClass::NoPrivilege => "no-privilege", ReasonClass::NotApplicable => "not-applicable", ReasonClass::DisabledByPolicy => "disabled-by-policy", ReasonClass::Unknown => "unknown" } } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus { Real, Degraded, Unavailable }

#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability { pub name: &'static str, pub ordinal: u16, pub status: AvailStatus, pub reason_class: Option<ReasonClass>, pub reason: &'static str, pub l3_test: Option<&'static str> }
impl MethodAvailability {
  const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Real, reason_class: None, reason: "", l3_test: Some(l3) } }
  const fn degraded(name: &'static str, ordinal: u16, l3: &'static str, detail: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Degraded, reason_class: None, reason: detail, l3_test: Some(l3) } }
  const fn unavailable(name: &'static str, ordinal: u16, class: ReasonClass, reason: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Unavailable, reason_class: Some(class), reason, l3_test: None } }
}

pub const POWER_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("getBattery", 0, "l3_linux_power::core_works"),
    MethodAvailability::real("getProfiles", 1, "l3_linux_power::core_works"),
    MethodAvailability::degraded("setProfile", 2, "l3_linux_power::core_works", "cpufreq energy_performance_preference/scaling_governor write; host-mutating verb, live proof founder-run"),
    MethodAvailability::real("subscribeBattery", 3, "l3_linux_power::core_works"),
    MethodAvailability::unavailable("suspend", 4, ReasonClass::NoPrivilege, "no-privilege: host power control requires CAP_SYS_BOOT/CAP_SYS_ADMIN for this uid"),
    MethodAvailability::unavailable("hibernate", 5, ReasonClass::NoPrivilege, "no-privilege: host power control requires CAP_SYS_BOOT/CAP_SYS_ADMIN for this uid"),
    MethodAvailability::unavailable("reboot", 6, ReasonClass::NoPrivilege, "no-privilege: host power control requires CAP_SYS_BOOT/CAP_SYS_ADMIN for this uid"),
    MethodAvailability::unavailable("shutdown", 7, ReasonClass::NoPrivilege, "no-privilege: host power control requires CAP_SYS_BOOT/CAP_SYS_ADMIN for this uid"),
    MethodAvailability::unavailable("subscribePowerTelemetry", 8, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getPowerBudget", 9, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("setPowerBudget", 10, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::real("getChargingInfo", 11, "l3_linux_power::core_works"),
    MethodAvailability::unavailable("negotiateUsbPd", 12, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getUsbPdContract", 13, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("negotiateWirelessCharging", 14, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getGridEnergy", 15, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getCarbonIntensity", 16, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getPoEInfo", 17, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("configureCellBalancing", 18, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::unavailable("getCellBalancingStatus", 19, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
    MethodAvailability::real("getFuelGauge", 20, "l3_linux_power::core_works"),
    MethodAvailability::unavailable("predictThermalRunaway", 21, ReasonClass::NoBackend, "no native UPower/cpufreq surface on this host"),
];

pub const PLATFORM: &str = "linux";
pub const TRANSPORTS: &[&str] = &["InProcess"];
pub const CAPS: &[&str] = &[];

pub fn status_counts() -> (usize, usize, usize) {
    let mut r=0; let mut d=0; let mut u=0; for row in POWER_AVAILABILITY { match row.status { AvailStatus::Real=>r+=1, AvailStatus::Degraded=>d+=1, AvailStatus::Unavailable=>u+=1 } } (r,d,u)
}

pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    POWER_AVAILABILITY.iter().filter(|r| r.l3_test.is_some()).map(|r| (r.name, r.l3_test.unwrap()))
}

pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    POWER_AVAILABILITY.iter().find(|r| r.name == name)
}
