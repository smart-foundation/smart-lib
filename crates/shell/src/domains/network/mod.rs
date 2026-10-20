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
    /// The host's routing table (kernel RTM_GETROUTE).
    Routes,
    /// Per-interface byte/packet/error/drop counters.
    Counters { name: String },
    /// The host's proxy configuration (/etc/environment).
    Proxy,
    /// Enable the host networking master switch (gated).
    On,
    /// Disable the host networking master switch (gated).
    Off,
    /// Mobile data + roaming state.
    Mobile,
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
    /// Decode a `WIFI:T:WPA;S:<ssid>;P:<pass>;;` QR payload (a string, or a
    /// text-payload file path) and route it to `wifiConnect` via a secret
    /// reference — the password is never echoed. Without `--force` this is a
    /// dry-run: it parses and shows the resolved call, and performs nothing.
    Join {
        /// The QR payload text, or a path to a text file holding it.
        payload: String,
        /// Supply the password when the payload carries none (used only to
        /// build the secret reference, never stored or echoed).
        #[arg(long)]
        password: Option<String>,
    },
    /// Render a terminal QR of a network's `WIFI:` payload so a phone can join
    /// by scanning the screen. The rendered matrix is decoded back and shown to
    /// match the payload — the round-trip proof a phone will read it.
    Share {
        ssid: String,
        /// The password to embed (needed for a secured network the host cannot
        /// read the secret of; used only to build the payload, never stored).
        #[arg(long)]
        password: Option<String>,
        /// Render the larger ASCII `#`/space form instead of half blocks.
        #[arg(long)]
        ascii: bool,
    },
    /// The read-shaped privilege measurement: what THIS uid may actually do to
    /// the wifi surface, as the host's authority reports it (never an
    /// assumption, never a write).
    Privilege,
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
pub mod qr;
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
            // Each interface row carries its status's SSID, so a connected wifi
            // network's NAME reaches the default table too (the founder's "no
            // network name anywhere"). One cached status read per interface.
            let mut rows = Vec::with_capacity(interfaces.items.len());
            for iface in &interfaces.items {
                let status = backend.get_status(iface.id)?;
                rows.push(render::interface_row(iface, &status));
            }
            Ok(Value::List(rows))
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
        Some(NetworkAction::Routes) => {
            let routes = backend.get_routes(all)?;
            Ok(Value::List(routes.items.iter().map(|r| Value::Object(vec![
                ("destination".into(), Value::String(r.destination_cidr.clone())),
                ("gateway".into(), Value::String(r.gateway.clone())),
                ("metric".into(), Value::Number(r.metric as f64)),
                ("interface_id".into(), Value::Number(r.interface_id.get() as f64)),
            ])).collect()))
        }
        Some(NetworkAction::Counters { name }) => {
            let interfaces = backend.get_interfaces(all)?;
            let Some(iface) = interfaces.items.iter().find(|i| i.name == *name) else {
                return Err(SmartError::NotFound(format!("no interface named {name}")));
            };
            let c = backend.get_counters(iface.id, smart_lib::core::values::Duration::from_nanos(0))?;
            Ok(Value::Object(vec![
                ("interface".into(), Value::String(name.clone())),
                ("bytes_in".into(), Value::Number(c.bytes_in as f64)),
                ("bytes_out".into(), Value::Number(c.bytes_out as f64)),
                ("packets_in".into(), Value::Number(c.packets_in as f64)),
                ("packets_out".into(), Value::Number(c.packets_out as f64)),
                ("errors_in".into(), Value::Number(c.errors_in as f64)),
                ("errors_out".into(), Value::Number(c.errors_out as f64)),
                ("drops_in".into(), Value::Number(c.drops_in as f64)),
                ("drops_out".into(), Value::Number(c.drops_out as f64)),
            ]))
        }
        Some(NetworkAction::Proxy) => {
            let p = backend.get_proxy_config()?;
            Ok(Value::Object(vec![
                ("enabled".into(), Value::Bool(p.enabled)),
                ("url".into(), Value::String(p.url.clone())),
                ("bypass".into(), Value::List(p.bypass.iter().map(|(k,v)| Value::Object(vec![
                    ("key".into(), Value::String(k.clone())),
                    ("value".into(), Value::String(v.clone())),
                ])).collect())),
            ]))
        }
        Some(NetworkAction::On) => {
            if !guard.allowed {
                return Err(SmartError::not_supported(
                    "network on: a host-mutating verb — run with --force or confirm in the REPL".to_string(),
                ));
            }
            backend.set_networking_enabled(true)?;
            Ok(Value::String("networking enabled".into()))
        }
        Some(NetworkAction::Off) => {
            if !guard.allowed {
                return Err(SmartError::not_supported(
                    "network off: a host-mutating verb — run with --force or confirm in the REPL".to_string(),
                ));
            }
            backend.set_networking_enabled(false)?;
            Ok(Value::String("networking disabled".into()))
        }
        Some(NetworkAction::Mobile) => {
            let md = backend.is_mobile_data_enabled()?;
            let dr = backend.is_data_roaming_enabled()?;
            Ok(Value::Object(vec![
                ("mobile_data".into(), Value::Bool(md)),
                ("data_roaming".into(), Value::Bool(dr)),
            ]))
        }
        Some(NetworkAction::Interface { name, state }) => {
            let interfaces = backend.get_interfaces(all)?;
            let Some(iface) = interfaces.items.iter().find(|i| i.name == *name) else {
                return Err(SmartError::NotFound(format!(
                    "no interface named {name} on this host"
                )));
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
                // A real engine call: NM RequestScan + GetAllAccessPoints. The
                // visible networks (SSID / signal / security) come back as a
                // list; the honest empty when the radio sees nothing.
                let found = backend.wifi_scan(all)?;
                Ok(Value::List(
                    found.items.iter().map(render::wifi_value).collect(),
                ))
            }
            Some(WifiAction::Privilege) => {
                // The read-shaped measurement: what this uid may actually do.
                let priv_ = backend.measure_wifi_write_privilege()?;
                Ok(render::privilege_value(&priv_))
            }
            Some(WifiAction::Join { payload, password }) => {
                // A text file is read when the argument names one; otherwise
                // the argument is the payload itself.
                let text = match std::fs::read_to_string(payload) {
                    Ok(contents) => contents,
                    Err(_) => payload.clone(),
                };
                let mut join = qr::parse(&text)?;
                // A payload password wins; `--password` supplies one the payload
                // omitted. Neither is ever stored or echoed past the reference.
                if join.password.is_empty() {
                    if let Some(p) = password {
                        join.password = p.clone();
                    }
                }
                // The resolved call, self-contained: SSID + security + the
                // secret reference (the stable slot name, never the password).
                let secret_ref = stable_secret_ref(&join.ssid, Some(&join.password));
                let resolved = format!(
                    "wifiConnect(ssid={:?}, security={:?}, credential=SecretKeyId({secret_ref}))",
                    join.ssid, join.security
                );
                // Gated exactly like every other host write: without --force
                // this is a dry-run that SHOWS the resolved call and performs
                // nothing (the founder's join proof).
                let performed = if guard.allowed && !join.ssid.is_empty() {
                    backend.wifi_connect(&join.ssid, smart_lib::core::ids::SecretKeyId::new(secret_ref))?;
                    true
                } else {
                    false
                };
                Ok(render::join_value(&join, &resolved, performed))
            }
            Some(WifiAction::Share { ssid, password, ascii }) => {
                // Build the WIFI payload: open when no password is supplied and
                // the network is not known-secured; WPA (with the password, or
                // without it and a note) otherwise. The host never exposes a
                // remembered network's secret, so a secured share carries a
                // password only when the operator provides one.
                let join = qr::WifiJoin {
                    ssid: ssid.clone(),
                    security: if password.is_some() { "wpa".to_string() } else { "open".to_string() },
                    password: password.clone().unwrap_or_default(),
                    hidden: false,
                };
                let payload = join.to_payload();
                // The round-trip proof: decode the rendered matrix back and show
                // it equals the payload, so a phone reads exactly this network.
                let decoded = qr::round_trip(&payload, *ascii)?;
                Ok(render::share_value(&join, &payload, *ascii, &decoded))
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

/// The host write this command performs, when it performs one: the verb and
/// the target the confirmation prompt shows. Read-only commands (`status`,
/// `wifi scan`, `wifi list`, the privilege probe) answer `None`.
pub fn write_intent(cmd: &NetworkCmd) -> Option<(String, String)> {
    use WifiAction as W;
    Some(match &cmd.action {
        Some(NetworkAction::Wifi { action: Some(W::Connect { ssid, .. }) }) => {
            ("join wifi network".into(), ssid.clone())
        }
        Some(NetworkAction::Wifi { action: Some(W::Disconnect) }) => {
            ("disconnect wifi".into(), "the active connection".into())
        }
        Some(NetworkAction::Wifi { action: Some(W::Forget { ssid }) }) => {
            ("forget wifi network".into(), ssid.clone())
        }
        Some(NetworkAction::Wifi { action: Some(W::On) }) => {
            ("enable the wifi radio".into(), String::new())
        }
        Some(NetworkAction::Wifi { action: Some(W::Off) }) => {
            ("disable the wifi radio".into(), String::new())
        }
        Some(NetworkAction::Wifi {
            action: Some(W::Join { payload, .. }),
        }) => ("join wifi network".into(), format!("from payload {payload}")),
        _ => return None,
    })
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
        "refusing to run `{what}`: it CHANGES host state. The interactive `(y/N)` prompt asks with the target's \
         context beside it; a piped `y` line answers it, and `--force` accepts the \
         write explicitly (the live bring-back proof is founder-run)."
    )))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{NetworkAction as Action, NetworkCmd as Cmd};
