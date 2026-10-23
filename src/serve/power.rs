#![allow(missing_docs)]
//! The power domain, served through the contract's generated `Server` trait.
extern crate std;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::rc::Rc;
use std::sync::Arc;
use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use crate::core::ErrorCode;
use crate::domains::power::availability;
use crate::domains::power::backend::PowerBackend;
use crate::schema::power_capnp::power as pwr;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct PowerService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for PowerService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> PowerService<B>
where
    B: PowerBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's power state, and this connection holds the read-only class"),
    ))
}

fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Power contract method"),
        )),
    }
}

fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

impl<B> pwr::Server for PowerService<B>
where
    B: PowerBackend + 'static,
{
    fn get_battery(
        self: Rc<Self>,
        _params: pwr::GetBatteryParams,
        _results: pwr::GetBatteryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getBattery") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_profiles(
        self: Rc<Self>,
        _params: pwr::GetProfilesParams,
        _results: pwr::GetProfilesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getProfiles") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_profile(
        self: Rc<Self>,
        _params: pwr::SetProfileParams,
        _results: pwr::SetProfileResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setProfile") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_battery(
        self: Rc<Self>,
        _params: pwr::SubscribeBatteryParams,
        _results: pwr::SubscribeBatteryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeBattery") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn suspend(
        self: Rc<Self>,
        _params: pwr::SuspendParams,
        _results: pwr::SuspendResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("suspend") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn hibernate(
        self: Rc<Self>,
        _params: pwr::HibernateParams,
        _results: pwr::HibernateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("hibernate") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn reboot(
        self: Rc<Self>,
        _params: pwr::RebootParams,
        _results: pwr::RebootResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("reboot") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn shutdown(
        self: Rc<Self>,
        _params: pwr::ShutdownParams,
        _results: pwr::ShutdownResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("shutdown") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_power_telemetry(
        self: Rc<Self>,
        _params: pwr::SubscribePowerTelemetryParams,
        _results: pwr::SubscribePowerTelemetryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribePowerTelemetry") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_power_budget(
        self: Rc<Self>,
        _params: pwr::GetPowerBudgetParams,
        _results: pwr::GetPowerBudgetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPowerBudget") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_power_budget(
        self: Rc<Self>,
        _params: pwr::SetPowerBudgetParams,
        _results: pwr::SetPowerBudgetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setPowerBudget") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_charging_info(
        self: Rc<Self>,
        _params: pwr::GetChargingInfoParams,
        _results: pwr::GetChargingInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getChargingInfo") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn negotiate_usb_pd(
        self: Rc<Self>,
        _params: pwr::NegotiateUsbPdParams,
        _results: pwr::NegotiateUsbPdResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("negotiateUsbPd") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_usb_pd_contract(
        self: Rc<Self>,
        _params: pwr::GetUsbPdContractParams,
        _results: pwr::GetUsbPdContractResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getUsbPdContract") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn negotiate_wireless_charging(
        self: Rc<Self>,
        _params: pwr::NegotiateWirelessChargingParams,
        _results: pwr::NegotiateWirelessChargingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("negotiateWirelessCharging") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_grid_energy(
        self: Rc<Self>,
        _params: pwr::GetGridEnergyParams,
        _results: pwr::GetGridEnergyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getGridEnergy") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_carbon_intensity(
        self: Rc<Self>,
        _params: pwr::GetCarbonIntensityParams,
        _results: pwr::GetCarbonIntensityResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getCarbonIntensity") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_po_e_info(
        self: Rc<Self>,
        _params: pwr::GetPoEInfoParams,
        _results: pwr::GetPoEInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPoEInfo") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn configure_cell_balancing(
        self: Rc<Self>,
        _params: pwr::ConfigureCellBalancingParams,
        _results: pwr::ConfigureCellBalancingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("configureCellBalancing") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_cell_balancing_status(
        self: Rc<Self>,
        _params: pwr::GetCellBalancingStatusParams,
        _results: pwr::GetCellBalancingStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getCellBalancingStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_fuel_gauge(
        self: Rc<Self>,
        _params: pwr::GetFuelGaugeParams,
        _results: pwr::GetFuelGaugeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getFuelGauge") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn predict_thermal_runaway(
        self: Rc<Self>,
        _params: pwr::PredictThermalRunawayParams,
        _results: pwr::PredictThermalRunawayResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("predictThermalRunaway") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
}

pub struct PowerDomain<B> {
    backend: Arc<B>,
}

impl<B> PowerDomain<B>
where
    B: PowerBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for PowerDomain<B>
where
    B: PowerBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        pwr::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::POWER_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "power"
    }

    fn category(&self) -> &'static str {
        "power"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "power battery",
                "Power",
                "getBattery",
                "getBattery",
                0,
                true,
            )
            .with_row(matrix_row("getBattery")),            DomainCommand::new(
                "power profiles",
                "Power",
                "getProfiles",
                "getProfiles",
                1,
                true,
            )
            .with_row(matrix_row("getProfiles")),            DomainCommand::new(
                "power set-profile",
                "Power",
                "setProfile",
                "setProfile",
                2,
                true,
            )
            .with_row(matrix_row("setProfile")),            DomainCommand::new(
                "power subscribe-battery",
                "Power",
                "subscribeBattery",
                "subscribeBattery",
                3,
                true,
            )
            .with_row(matrix_row("subscribeBattery")),            DomainCommand::new(
                "power suspend",
                "Power",
                "suspend",
                "suspend",
                4,
                true,
            )
            .with_row(matrix_row("suspend")),            DomainCommand::new(
                "power hibernate",
                "Power",
                "hibernate",
                "hibernate",
                5,
                true,
            )
            .with_row(matrix_row("hibernate")),            DomainCommand::new(
                "power reboot",
                "Power",
                "reboot",
                "reboot",
                6,
                true,
            )
            .with_row(matrix_row("reboot")),            DomainCommand::new(
                "power shutdown",
                "Power",
                "shutdown",
                "shutdown",
                7,
                true,
            )
            .with_row(matrix_row("shutdown")),            DomainCommand::new(
                "power subscribe-power-telemetry",
                "Power",
                "subscribePowerTelemetry",
                "subscribePowerTelemetry",
                8,
                true,
            )
            .with_row(matrix_row("subscribePowerTelemetry")),            DomainCommand::new(
                "power get-power-budget",
                "Power",
                "getPowerBudget",
                "getPowerBudget",
                9,
                true,
            )
            .with_row(matrix_row("getPowerBudget")),            DomainCommand::new(
                "power set-power-budget",
                "Power",
                "setPowerBudget",
                "setPowerBudget",
                10,
                true,
            )
            .with_row(matrix_row("setPowerBudget")),            DomainCommand::new(
                "power charging",
                "Power",
                "getChargingInfo",
                "getChargingInfo",
                11,
                true,
            )
            .with_row(matrix_row("getChargingInfo")),            DomainCommand::new(
                "power negotiate-usb-pd",
                "Power",
                "negotiateUsbPd",
                "negotiateUsbPd",
                12,
                true,
            )
            .with_row(matrix_row("negotiateUsbPd")),            DomainCommand::new(
                "power get-usb-pd-contract",
                "Power",
                "getUsbPdContract",
                "getUsbPdContract",
                13,
                true,
            )
            .with_row(matrix_row("getUsbPdContract")),            DomainCommand::new(
                "power negotiate-wireless-charging",
                "Power",
                "negotiateWirelessCharging",
                "negotiateWirelessCharging",
                14,
                true,
            )
            .with_row(matrix_row("negotiateWirelessCharging")),            DomainCommand::new(
                "power get-grid-energy",
                "Power",
                "getGridEnergy",
                "getGridEnergy",
                15,
                true,
            )
            .with_row(matrix_row("getGridEnergy")),            DomainCommand::new(
                "power get-carbon-intensity",
                "Power",
                "getCarbonIntensity",
                "getCarbonIntensity",
                16,
                true,
            )
            .with_row(matrix_row("getCarbonIntensity")),            DomainCommand::new(
                "power get-po-e-info",
                "Power",
                "getPoEInfo",
                "getPoEInfo",
                17,
                true,
            )
            .with_row(matrix_row("getPoEInfo")),            DomainCommand::new(
                "power configure-cell-balancing",
                "Power",
                "configureCellBalancing",
                "configureCellBalancing",
                18,
                true,
            )
            .with_row(matrix_row("configureCellBalancing")),            DomainCommand::new(
                "power get-cell-balancing-status",
                "Power",
                "getCellBalancingStatus",
                "getCellBalancingStatus",
                19,
                true,
            )
            .with_row(matrix_row("getCellBalancingStatus")),            DomainCommand::new(
                "power fuel-gauge",
                "Power",
                "getFuelGauge",
                "getFuelGauge",
                20,
                true,
            )
            .with_row(matrix_row("getFuelGauge")),            DomainCommand::new(
                "power predict-thermal-runaway",
                "Power",
                "predictThermalRunaway",
                "predictThermalRunaway",
                21,
                true,
            )
            .with_row(matrix_row("predictThermalRunaway")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/power.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = PowerService::new(Arc::clone(&self.backend), class);
        let client: pwr::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
