use clap::{Parser, Subcommand};
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;
use smart_lib::domains::power::PowerBackend;

pub type Backend = dyn PowerBackend;

#[derive(Parser, Debug)]
pub struct PowerCmd {
    #[command(subcommand)]
    pub action: Option<PowerAction>,
}
#[derive(Subcommand, Debug)]
pub enum PowerAction {
    /// The host's battery state.
    Battery,
    /// The available power profiles.
    Profiles,
    /// Set the active power profile (host-mutating, gated).
    SetProfile { profile: u64 },
    /// Charging information.
    Charging,
    /// Fuel gauge reading.
    FuelGauge,
    /// Suspend the host (host-mutating, gated; usually unavailable).
    Suspend,
    /// Hibernate the host (host-mutating, gated; usually unavailable).
    Hibernate,
    /// Reboot the host (host-mutating, gated; usually unavailable).
    Reboot,
    /// Shut down the host (host-mutating, gated; usually unavailable).
    Shutdown,
    /// This backend's honest capability tokens.
    Caps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard { pub allowed: bool }

pub fn evaluate(cmd: &PowerCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(PowerAction::Battery) => {
            let b = backend.get_battery().map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("present".into(), Value::Bool(b.present)),
                ("percentage".into(), Value::Number(b.percentage as f64)),
                ("charging".into(), Value::Bool(b.charging)),
                ("plugged".into(), Value::Bool(b.plugged)),
            ]))
        }
        Some(PowerAction::Profiles) => {
            let ps = backend.get_profiles().map_err(SmartError::from)?;
            Ok(Value::List(ps.into_iter().map(|p| Value::Object(vec![
                ("id".into(), Value::Number(p.id as f64)),
                ("name".into(), Value::String(p.name)),
                ("description".into(), Value::String(p.description)),
                ("active".into(), Value::Bool(p.active)),
            ])).collect()))
        }
        Some(PowerAction::SetProfile { profile }) => {
            guard_host_write(guard, &format!("power set-profile {profile}"))?;
            backend.set_profile(*profile).map_err(SmartError::from)?;
            Ok(Value::Object(vec![("verb".into(),Value::String("set-profile".into())),("profile".into(),Value::Number(*profile as f64))]))
        }
        Some(PowerAction::Charging) => {
            let s = backend.get_charging_info().map_err(SmartError::from)?;
            Ok(Value::String(s))
        }
        Some(PowerAction::FuelGauge) => {
            let s = backend.get_fuel_gauge().map_err(SmartError::from)?;
            Ok(Value::String(s))
        }
        Some(PowerAction::Suspend) => { guard_host_write(guard, "power suspend")?; backend.suspend().map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("suspend".into()))])) }
        Some(PowerAction::Hibernate) => { guard_host_write(guard, "power hibernate")?; backend.hibernate().map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("hibernate".into()))])) }
        Some(PowerAction::Reboot) => { guard_host_write(guard, "power reboot")?; backend.reboot().map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("reboot".into()))])) }
        Some(PowerAction::Shutdown) => { guard_host_write(guard, "power shutdown")?; backend.shutdown().map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("shutdown".into()))])) }
        Some(PowerAction::Caps) => Ok(Value::List(vec![Value::String("power.read".into()), Value::String("power.write".into())])),
    }
}

pub fn run(cmd: &PowerCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw { println!("{}", render::render_action(cmd, &value, *format)); }
    Ok(value)
}

pub fn write_intent(cmd: &PowerCmd) -> Option<(String, String)> {
    match &cmd.action {
        Some(PowerAction::SetProfile { profile }) => Some(("set power profile".into(), format!("{profile}"))),
        Some(PowerAction::Suspend) => Some(("suspend host".into(), String::new())),
        Some(PowerAction::Hibernate) => Some(("hibernate host".into(), String::new())),
        Some(PowerAction::Reboot) => Some(("reboot host".into(), String::new())),
        Some(PowerAction::Shutdown) => Some(("shut down host".into(), String::new())),
        _ => None,
    }
}

pub mod render;
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed { return Ok(()); }
    Err(SmartError::not_supported(format!("refusing to run `{verb}`: it CHANGES host state. In the REPL confirm the prompt, or pass --force once you accept the write (the live bring-back proof is founder-run).")))
}
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let b = backend.get_battery().map_err(SmartError::from)?;
    let ps = backend.get_profiles().map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("battery".into(), Value::Object(vec![
            ("percentage".into(), Value::Number(b.percentage as f64)),
            ("charging".into(), Value::Bool(b.charging)),
        ])),
        ("profiles".into(), Value::List(ps.into_iter().map(|p| Value::Object(vec![
            ("id".into(), Value::Number(p.id as f64)),
            ("name".into(), Value::String(p.name)),
            ("active".into(), Value::Bool(p.active)),
        ])).collect())),
        ("caps".into(), Value::List(vec![Value::String("power.read".into()), Value::String("power.write".into())])),
    ]))
}
