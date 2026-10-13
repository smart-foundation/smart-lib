//! `network` — the domain command module, rewritten onto the NEW engine.
//!
//! What changed from the original shell's module: it renders from the
//! engine's OWNED types (`InterfaceSnapshot`, `ConnStatus`, `RadioState`,
//! `KnownWifiNetwork`) instead of capnp `Reader<'static>` views (which also
//! removes the old `Box::leak` pathology), and its fake arms are real engine
//! calls: `wifi on`/`wifi off` route to `setRadioEnabled`, `wifi connect` /
//! `disconnect` / `forget` / `scan` route to their contract verbs — and when
//! the engine refuses (polkit-gated on this host), the refusal is the
//! answer, rendered honestly. Host-mutating verbs are gated: the operator
//! must ask for them (`--force` one-shot, a confirmation in the REPL), and
//! their live proof is the founder's (charter law #8).

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

#[cfg(feature = "domain-network")]
use smart_lib::domains::network::NetworkBackend;

/// The engine seam this shell speaks to — the corrected architecture's rule:
/// the shell embeds its engine in-process through the per-domain backend
/// trait object. Without the feature the seam is the unit type, so every
/// signature still compiles (and every path answers honestly that the
/// engine half is not built).
#[cfg(feature = "domain-network")]
pub type Backend = dyn NetworkBackend;
#[cfg(not(feature = "domain-network"))]
pub type Backend = ();

/// `smart network` — connectivity, radios, and the wifi verbs.
#[derive(Parser, Debug)]
pub struct NetworkCmd {
    #[command(subcommand)]
    pub action: Option<NetworkAction>,
}

#[derive(Subcommand, Debug)]
pub enum NetworkAction {
    /// The interface table with the host's online state (the default view).
    Status,
    /// Enumerate every interface the kernel reports.
    Interfaces,
    /// The observable radio states (enabled, hard-blocked, soft-blocked).
    Radios,
    /// One interface: its type and why it is (or is not) usable.
    Interface {
        name: String,
        /// `up`/`down` — not wired on this backend; answers honestly.
        state: Option<String>,
    },
    /// The wifi radio: remembered networks, scan, connect, forget, on/off.
    Wifi {
        #[command(subcommand)]
        action: Option<WifiAction>,
    },
}

#[derive(Subcommand, Debug)]
pub enum WifiAction {
    /// The remembered networks (the engine's own profile read).
    List,
    /// An active scan (the engine's answer decides what is possible).
    Scan,
    /// Connect to a remembered or new network.
    Connect {
        ssid: String,
        /// The credential — carried to the engine as a secret reference,
        /// never stored by the shell.
        password: Option<String>,
    },
    /// Disconnect the active wifi connection.
    Disconnect,
    /// Forget a remembered network.
    Forget { ssid: String },
    /// Enable the wifi radio (a real engine call).
    On,
    /// Disable the wifi radio (a real engine call).
    Off,
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot
/// mode passes the `--force` flag through).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(
    cmd: &NetworkCmd,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    #[cfg(feature = "domain-network")]
    {
        evaluate_network(cmd, backend, guard)
    }
    #[cfg(not(feature = "domain-network"))]
    {
        let _ = (cmd, backend, guard);
        Err(SmartError::unavailable(
            "the shell was built without the domain-network feature — rebuild with default \
             features for the network domain",
        ))
    }
}

/// Run the command: evaluate, render in the requested format, and hand the
/// value back (the REPL renders a piped result itself; a one-shot command's
/// own rendering already happened here).
pub fn run(
    cmd: &NetworkCmd,
    format: &OutputFormat,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    #[cfg(feature = "domain-network")]
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

#[cfg(feature = "domain-network")]
pub mod render;

#[cfg(feature = "domain-network")]
fn evaluate_network(
    cmd: &NetworkCmd,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    use smart_lib::domains::network::types::Pagination;

    let all = Pagination { offset: 0, limit: u32::MAX };
    match &cmd.action {
        None | Some(NetworkAction::Interfaces) => {
            let interfaces = backend.get_interfaces(all)?;
            render::interfaces_value(&interfaces.items)
        }
        Some(NetworkAction::Status) => {
            // The full status: the host-networking master switch, the radios,
            // and every interface with its four-state hostState — the two
            // reads that answer "why is a device silent" without guessing.
            let interfaces = backend.get_interfaces(all)?;
            let radios = backend.list_radio_states(all)?;
            let networking = backend.is_networking_enabled()?;
            let mut rows = Vec::with_capacity(interfaces.items.len());
            for iface in &interfaces.items {
                let status = backend.get_status(iface.id)?;
                rows.push(render::status_value(iface, &status));
            }
            Ok(Value::Object(vec![
                ("networking".into(), Value::Bool(networking)),
                (
                    "radios".into(),
                    Value::List(radios.items.iter().map(render::radio_value).collect()),
                ),
                ("interfaces".into(), Value::List(rows)),
            ]))
        }
        Some(NetworkAction::Radios) => {
            let radios = backend.list_radio_states(all)?;
            Ok(Value::List(
                radios.items.iter().map(render::radio_value).collect(),
            ))
        }
        Some(NetworkAction::Interface { name, state }) => {
            let interfaces = backend.get_interfaces(all)?;
            let Some(iface) = interfaces.items.iter().find(|i| i.name == *name) else {
                return Err(SmartError::NotFound);
            };
            if let Some(requested) = state {
                // There is no per-interface up/down verb on this backend: the
                // honest answer names the switches that do exist.
                return Err(SmartError::not_supported(format!(
                    "interface {name} {requested}: not wired — this backend exposes no \
                     per-interface up/down verb; the host switches are \
                     `network wifi on|off` and the engine's setNetworking"
                )));
            }
            let status = backend.get_status(iface.id)?;
            Ok(render::status_value(iface, &status))
        }
        Some(NetworkAction::Wifi { action }) => match action {
            None => {
                let radios = backend.list_radio_states(all)?;
                let Some(wifi) = radios.items.iter().find(|r| r.name == "wifi") else {
                    return Err(SmartError::unavailable(
                        "no wifi radio reported by the host (the engine enumerated none)",
                    ));
                };
                Ok(render::radio_value(wifi))
            }
            Some(WifiAction::List) => {
                let known = backend.list_known_wifi_networks(all)?;
                Ok(Value::List(
                    known.items.iter().map(render::known_value).collect(),
                ))
            }
            Some(WifiAction::Scan) => {
                // A real engine call: on this host the engine answers
                // honestly that the active scan is not built (and that
                // refusal is rendered, never papered over).
                let found = backend.wifi_scan(all)?;
                Ok(Value::List(
                    found.items.iter().map(render::wifi_value).collect(),
                ))
            }
            Some(WifiAction::Connect { ssid, password }) => {
                guard_host_write(guard, "wifi connect", ssid)?;
                // The credential stays a reference: the shell never keeps it,
                // and the engine routes association through its own path.
                let credential = smart_lib::core::ids::SecretKeyId::new(
                    stable_secret_ref(ssid, password.as_deref()),
                );
                backend.wifi_connect(ssid, credential)?;
                Ok(render::verb_value("wifiConnect", &format!("{ssid} acknowledged")))
            }
            Some(WifiAction::Disconnect) => {
                guard_host_write(guard, "wifi disconnect", "")?;
                backend.wifi_disconnect()?;
                Ok(render::verb_value("wifiDisconnect", "acknowledged"))
            }
            Some(WifiAction::Forget { ssid }) => {
                guard_host_write(guard, "wifi forget", ssid)?;
                backend.forget_wifi_network(ssid)?;
                Ok(render::verb_value("wifiForget", &format!("{ssid} forgotten")))
            }
            Some(WifiAction::On) | Some(WifiAction::Off) => {
                let enabled = matches!(action, Some(WifiAction::On));
                // Read first: if the radio is already in the requested state
                // the engine read is the whole answer and nothing is written.
                let radios = backend.list_radio_states(all)?;
                let current = radios
                    .items
                    .iter()
                    .find(|r| r.name == "wifi")
                    .map(|r| r.enabled);
                if current == Some(enabled) {
                    return Ok(render::verb_value(
                        "setRadioEnabled",
                        &format!(
                            "wifi already {} (engine read — nothing written)",
                            if enabled { "enabled" } else { "disabled" }
                        ),
                    ));
                }
                guard_host_write(
                    guard,
                    if enabled { "wifi on" } else { "wifi off" },
                    "",
                )?;
                backend.set_radio_enabled("wifi", enabled)?;
                Ok(render::verb_value(
                    "setRadioEnabled",
                    &format!("wifi {} acknowledged", if enabled { "on" } else { "off" }),
                ))
            }
        },
    }
}

/// A stable, non-reversible reference for a credential: the shell hands the
/// engine a `SecretKeyId` and keeps no password itself. The reference is
/// derived from the SSID (deterministic across calls) — it names the secret
/// slot; the engine owns the material.
fn stable_secret_ref(ssid: &str, password: Option<&str>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ssid.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if password.is_some() {
        hash ^= 0x5deece66d;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The host-write gate: a mutating verb proceeds only when the caller has
/// accepted the write (the REPL asked, or one-shot `--force` was passed).
/// Law #8 lives here.
fn guard_host_write(guard: WriteGuard, verb: &str, target: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    let what = if target.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {target}")
    };
    Err(SmartError::not_supported(format!(
        "refusing to run `{what}`: it CHANGES host state. In the REPL confirm the prompt, or \
         pass --force once you accept the write (the live bring-back proof is founder-run)."
    )))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{NetworkAction as Action, NetworkCmd as Cmd};
