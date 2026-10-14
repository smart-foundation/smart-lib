//! The network domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, `↑↓`, aligned tables. Every renderer reads the ENGINE'S OWNED
//! TYPES (`InterfaceSnapshot`, `ConnStatus`, `RadioState`, `KnownWifiNetwork`,
//! `WifiNetwork`) — there is no capnp reader here, so nothing can leak and
//! nothing can desynchronise from what the engine measured.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::{pad_to, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::network::types::{
    ConnStatus, InterfaceSnapshot, KnownWifiNetwork, RadioState, WifiNetwork, WifiWritePrivilege,
};

/// The widest a radio's state cell can be — `⛔ hard-blocked`, measured as the
/// terminal draws it (the mark is Wide, the rest ASCII: 14 columns). The
/// state column is padded to this, so a blocked radio and an idle one put
/// their `hard=` / `soft=` boxes on the same columns.
const RADIO_STATE_COLUMN: usize = 14;

/// The value a `network` / `network interfaces` command hands the pipe engine.
pub fn interfaces_value(items: &[InterfaceSnapshot]) -> Result<Value, crate::error::SmartError> {
    Ok(Value::List(items.iter().map(|i| Value::Object(vec![
        ("name".into(), Value::String(i.name.clone())),
        ("type".into(), Value::String(i.kind.clone())),
        ("up".into(), Value::Bool(i.up)),
        ("connected".into(), Value::Bool(i.connected)),
        ("mtu".into(), Value::Number(i.mtu as f64)),
        ("managed".into(), Value::Bool(i.managed)),
        ("mac".into(), Value::String(i.hardware_address.0.clone())),
    ])).collect()))
}

/// One interface's full status record (type + the four-state hostState).
pub fn status_value(iface: &InterfaceSnapshot, status: &ConnStatus) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(iface.name.clone())),
        ("type".into(), Value::String(status.kind.clone())),
        ("connected".into(), Value::Bool(status.connected)),
        ("host_state".into(), Value::String(status.host_state.as_token().to_string())),
        (
            "host_state_source".into(),
            Value::String(status.host_state_source.as_token().to_string()),
        ),
        ("up".into(), Value::Bool(iface.up)),
        ("mtu".into(), Value::Number(iface.mtu as f64)),
        ("managed".into(), Value::Bool(iface.managed)),
        ("mac".into(), Value::String(iface.hardware_address.0.clone())),
        ("ssid".into(), Value::String(status.ssid.clone())),
    ])
}

/// One radio's observable state.
pub fn radio_value(radio: &RadioState) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(radio.name.clone())),
        ("enabled".into(), Value::Bool(radio.enabled)),
        ("hard_blocked".into(), Value::Bool(radio.hard_blocked)),
        ("soft_blocked".into(), Value::Bool(radio.soft_blocked)),
    ])
}

/// One interface's enumeration row, enriched with its status SSID so the
/// default table carries the connected network's NAME (the founder's "no
/// network name anywhere"). The same shape `interfaces_value` produces, plus
/// the SSID from the interface's status.
pub fn interface_row(iface: &InterfaceSnapshot, status: &ConnStatus) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(iface.name.clone())),
        ("type".into(), Value::String(iface.kind.clone())),
        ("up".into(), Value::Bool(iface.up)),
        ("connected".into(), Value::Bool(iface.connected)),
        ("mtu".into(), Value::Number(iface.mtu as f64)),
        ("managed".into(), Value::Bool(iface.managed)),
        ("mac".into(), Value::String(iface.hardware_address.0.clone())),
        ("ssid".into(), Value::String(status.ssid.clone())),
    ])
}

/// The read-shaped privilege measurement, as a value: the measured verdict for
/// each verb, `measured`, the uid, and the verbatim detail line.
pub fn privilege_value(priv_: &WifiWritePrivilege) -> Value {
    Value::Object(vec![
        ("measured".into(), Value::Bool(priv_.measured)),
        ("network_control".into(), Value::String(priv_.network_control.clone())),
        ("modify_connection".into(), Value::String(priv_.modify_connection.clone())),
        ("uid".into(), Value::Number(priv_.uid as f64)),
        ("detail".into(), Value::String(priv_.detail.clone())),
    ])
}

/// A decoded `join`: the SSID, security, hidden flag, the resolved engine call
/// (SSID + a secret REFERENCE, never the password), and whether it was
/// performed (gated) or shown as a dry-run.
pub fn join_value(
    join: &crate::domains::network::qr::WifiJoin,
    resolved: &str,
    performed: bool,
) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(join.ssid.clone())),
        ("security".into(), Value::String(join.security.clone())),
        ("hidden".into(), Value::Bool(join.hidden)),
        ("needs_password".into(), Value::Bool(join.needs_password())),
        ("resolved_call".into(), Value::String(resolved.to_string())),
        ("performed".into(), Value::Bool(performed)),
    ])
}

/// A `share`: the payload, the round-trip proof (the decoded payload that
/// equals it), and whether the render is ASCII. The QR itself is drawn by the
/// human renderer.
pub fn share_value(
    join: &crate::domains::network::qr::WifiJoin,
    payload: &str,
    ascii: bool,
    decoded: &str,
) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(join.ssid.clone())),
        ("security".into(), Value::String(join.security.clone())),
        ("payload".into(), Value::String(payload.to_string())),
        ("ascii".into(), Value::Bool(ascii)),
        ("round_trip".into(), Value::String(decoded.to_string())),
        ("round_trip_ok".into(), Value::Bool(decoded == payload)),
    ])
}

/// One remembered network. The credential is not here and can never be here:
/// the engine's type has no field for one.
pub fn known_value(network: &KnownWifiNetwork) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(network.ssid.clone())),
        ("auto_connect".into(), Value::Bool(network.auto_connect)),
        ("last_connected".into(), Value::String(format_last_connected(network.last_connected))),
        ("security".into(), Value::String(network.security.clone())),
    ])
}

/// One scanned access point.
pub fn wifi_value(network: &WifiNetwork) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(network.ssid.clone())),
        ("bssid".into(), Value::String(network.bssid.0.clone())),
        ("strength_dbm".into(), Value::Number(network.strength_dbm as f64)),
        ("frequency_hz".into(), Value::Number(network.frequency_hz as f64)),
        ("security".into(), Value::String(network.security.clone())),
    ])
}

/// A wifi frequency in Hz labelled by its band: the 2.4 / 3.6 / 5 / 6 GHz
/// ranges, from the AP's own channel. The label is derived from the measured
/// frequency, never guessed.
fn band_label(hz: u32) -> String {
    let ghz = hz as f64 / 1e9;
    let band = if ghz < 2.5 {
        "2.4 GHz"
    } else if ghz < 3.7 {
        "3.6 GHz"
    } else if ghz < 5.9 {
        "5 GHz"
    } else {
        "6 GHz"
    };
    band.to_string()
}

/// A control verb's echo: what the engine acknowledged.
pub fn verb_value(verb: &str, result: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("result".into(), Value::String(result.to_string())),
    ])
}

fn yes_no(b: bool) -> String {
    if b {
        format!("{}", "yes".green())
    } else {
        format!("{}", "no".dimmed())
    }
}

/// A `lastConnected` timestamp (nanoseconds since the epoch) as a UTC
/// `YYYY-MM-DD HH:MMZ`, or `never` when the host reported none — "not
/// connected yet", never a fabricated date. The civil-from-days conversion is
/// the standard proleptic-Gregorian algorithm, so the date is the host's own
/// clock reading, not a third-party date crate.
fn format_last_connected(nanos: u64) -> String {
    if nanos == 0 {
        return "never".to_string();
    }
    let secs = (nanos / 1_000_000_000) as i64;
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}Z")
}

/// Days since 1970-01-01 → calendar (year, month, day), the civil-from-days
/// proleptic-Gregorian mapping (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

fn up_glyph(up: bool) -> &'static str {
    if up {
        "↑↓"
    } else {
        "—"
    }
}

/// Render one command's value in the requested format. Human is the
/// aesthetic; Json and Raw are the machine forms over the same value.
pub fn render_action(
    cmd: &super::NetworkCmd,
    value: &Value,
    format: OutputFormat,
) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::NetworkCmd, value: &Value) -> String {
    use super::NetworkAction::*;
    use super::WifiAction as W;
    match &cmd.action {
        None | Some(Interfaces) => {
            let mut out = String::new();
            out.push_str(&format!("  {} {}\n", "🌐".blue().bold(), "Network".blue().bold()));
            let online = match value {
                Value::List(items) => items.iter().any(|i| i.field_bool("connected")),
                _ => false,
            };
            out.push_str(&format!(
                "  ├─ State    {}\n",
                if online { "◉ Online".green() } else { "○ Offline".dimmed() }
            ));
            out.push_str("  └─ Interfaces\n");
            out.push_str(&interfaces_table(value));
            out
        }
        Some(Status) => render_status_view(value),
        Some(Radios) => {
            let mut out = String::new();
            out.push_str(&format!("  {} {}\n", "📡".blue().bold(), "Radios".blue().bold()));
            out.push_str(&radios_table(value));
            out
        }
        Some(Interface { name, .. }) => {
            let Value::Object(pairs) = value else {
                return format!("  {} interface {} not found", "✗".red(), name);
            };
            let field = |k: &str| -> String {
                pairs
                    .iter()
                    .find(|(pk, _)| pk == k)
                    .map(|(_, v)| v.display())
                    .unwrap_or_default()
            };
            let mut out = String::new();
            out.push_str(&format!("  {} {} {}\n", "🔌".blue().bold(), name.blue().bold(), "— interface".dimmed()));
            out.push_str(&format!("  ├─ Type          {}\n", field("type")));
            out.push_str(&format!("  ├─ Connected     {}\n", yes_no(value.field_bool("connected"))));
            // The network name — the founder's exact complaint, "there is no
            // network name anywhere". Shown only when the host measured one; a
            // line is never invented for a disconnected or wired link.
            if !value.field_str("ssid").is_empty() {
                out.push_str(&format!("  ├─ Network       {} {}\n", "◉".green(), value.field_str("ssid").green().bold()));
            }
            out.push_str(&format!(
                "  ├─ Host state    {} ({})\n",
                field("host_state"),
                field("host_state_source").dimmed()
            ));
            out.push_str(&format!("  ├─ Admin up      {}\n", yes_no(value.field_bool("up"))));
            out.push_str(&format!("  ├─ MTU           {}\n", field("mtu")));
            out.push_str(&format!("  ├─ Managed       {}\n", yes_no(value.field_bool("managed"))));
            out.push_str(&format!("  └─ MAC           {}\n", field("mac").dimmed()));
            out
        }
        Some(Wifi { action }) => match action {
            None => {
                let Value::Object(pairs) = value else {
                    return String::new();
                };
                let field = |k: &str| -> String {
                    pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                };
                let _name = field("name");
                let enabled = pairs
                    .iter()
                    .find(|(k, _)| k == "enabled")
                    .map(|(_, v)| matches!(v, Value::Bool(true)))
                    .unwrap_or(false);
                let hard = pairs
                    .iter()
                    .find(|(k, _)| k == "hard_blocked")
                    .map(|(_, v)| matches!(v, Value::Bool(true)))
                    .unwrap_or(false);
                let soft = pairs
                    .iter()
                    .find(|(k, _)| k == "soft_blocked")
                    .map(|(_, v)| matches!(v, Value::Bool(true)))
                    .unwrap_or(false);
                format!(
                    "  {} wifi radio — {}\n  ├─ enabled      {}\n  ├─ hard-blocked {}\n  └─ soft-blocked {}",
                    "📶".blue().bold(),
                    if hard {
                        "⛔ hard-blocked".red()
                    } else if enabled {
                        "◉ on".green()
                    } else {
                        "○ off".dimmed()
                    },
                    yes_no(enabled),
                    yes_no(hard),
                    yes_no(soft),
                )
            }
            Some(W::List) => {
                let mut out = String::new();
                out.push_str(&format!("  {} {}\n", "📡".blue().bold(), "Known Networks".blue().bold()));
                let Value::List(items) = value else {
                    return out;
                };
                if items.is_empty() {
                    out.push_str(&format!(
                        "  └─ {} — the engine enumerated no remembered networks (an honest empty)\n",
                        "○".dimmed()
                    ));
                    return out;
                }
                let mut table = Table::new(vec![
                    "SSID".into(),
                    "SECURITY".into(),
                    "AUTOCONNECT".into(),
                    "LAST CONNECTED".into(),
                ]);
                for item in items {
                    if let Value::Object(pairs) = item {
                        let f = |k: &str| -> String {
                            pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                        };
                        table = table.row(vec![
                            f("ssid"),
                            f("security"),
                            f("auto_connect"),
                            f("last_connected"),
                        ]);
                    }
                }
                out.push_str(&table.render());
                out.push('\n');
                out
            }
            Some(W::Scan) => {
                // The visible networks as a table: SSID / signal(dBm) /
                // security / band — the real scan NM answered.
                let mut out = String::new();
                out.push_str(&format!("  {} {}\n", "📶".blue().bold(), "Visible Networks".blue().bold()));
                let Value::List(items) = value else {
                    return out;
                };
                if items.is_empty() {
                    out.push_str(&format!(
                        "  └─ {} — the radio reported no visible access points (an honest empty)\n",
                        "○".dimmed()
                    ));
                    return out;
                }
                let mut table = Table::new(vec![
                    "SSID".into(),
                    "SIGNAL".into(),
                    "SECURITY".into(),
                    "BAND".into(),
                    "BSSID".into(),
                ]);
                for item in items {
                    if let Value::Object(pairs) = item {
                        let f = |k: &str| -> String {
                            pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                        };
                        let dbm: i32 = f("strength_dbm").parse().unwrap_or(0);
                        let mhz: u32 = f("frequency_hz").parse().unwrap_or(0);
                        table = table.row(vec![
                            f("ssid"),
                            format!("{dbm} dBm"),
                            f("security"),
                            band_label(mhz),
                            f("bssid"),
                        ]);
                    }
                }
                out.push_str(&table.render());
                out.push('\n');
                out
            }
            Some(W::Privilege) => {
                let Value::Object(pairs) = value else {
                    return String::new();
                };
                let field = |k: &str| -> String {
                    pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                };
                let measured = matches!(pairs.iter().find(|(k, _)| k == "measured").map(|(_, v)| v), Some(Value::Bool(true)));
                let verdict = |word: &str| -> String {
                    match word {
                        "yes" => "◉ yes".green().to_string(),
                        "no" => "○ no".dimmed().to_string(),
                        "challenge" => "◌ challenge".yellow().to_string(),
                        _ => "— unknown".dimmed().to_string(),
                    }
                };
                if !measured {
                    return format!(
                        "  {} privilege — {} (nothing measured)\n  └─ {}",
                        "🔐".yellow(),
                        "○ unmeasured".yellow(),
                        field("detail").dimmed(),
                    );
                }
                format!(
                    "  {} privilege (uid {}) — {}\n  ├─ network-control   {} activate / disconnect\n  ├─ settings.modify   {} add a new network\n  └─ {} {}",
                    "🔐".blue(),
                    field("uid"),
                    field("detail").dimmed(),
                    verdict(&field("network_control")),
                    verdict(&field("modify_connection")),
                    "measured",
                    "read-only (no write performed)".dimmed(),
                )
            }
            Some(W::Join { .. }) => {
                let Value::Object(pairs) = value else {
                    return String::new();
                };
                let field = |k: &str| -> String {
                    pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                };
                let performed = matches!(pairs.iter().find(|(k, _)| k == "performed").map(|(_, v)| v), Some(Value::Bool(true)));
                let needs = matches!(pairs.iter().find(|(k, _)| k == "needs_password").map(|(_, v)| v), Some(Value::Bool(true)));
                let mut out = format!(
                    "  {} wifi join — {}\n  ├─ SSID          {}\n  ├─ Security      {}\n",
                    "📷".blue().bold(),
                    field("ssid").green().bold(),
                    field("ssid"),
                    field("security"),
                );
                if needs {
                    out.push_str("  ├─ Password      missing — a phone will prompt; pass --password to embed it\n");
                }
                out.push_str(&format!("  ├─ Resolves to   {}\n", field("resolved_call").dimmed()));
                out.push_str(&format!(
                    "  └─ {} {}",
                    if performed { "✓ performed".green() } else { "○ dry-run (pass --force to perform)".dimmed() },
                    "",
                ));
                out
            }
            Some(W::Share { .. }) => {
                let Value::Object(pairs) = value else {
                    return String::new();
                };
                let field = |k: &str| -> String {
                    pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                };
                let ascii = matches!(pairs.iter().find(|(k, _)| k == "ascii").map(|(_, v)| v), Some(Value::Bool(true)));
                let ok = matches!(pairs.iter().find(|(k, _)| k == "round_trip_ok").map(|(_, v)| v), Some(Value::Bool(true)));
                let payload = field("payload");
                let mut out = format!(
                    "  {} wifi share — {}\n  ├─ Payload    {}\n  ├─ Round-trip {} scanned back to the exact payload\n",
                    "📲".blue().bold(),
                    field("ssid").green().bold(),
                    payload.dimmed(),
                    if ok { "◉".green().to_string() } else { "✗".red().to_string() },
                );
                // Render the QR; fall back to an honest error line if the
                // payload will not encode (rather than a blank screen).
                match super::qr::render_terminal(&payload, ascii) {
                    Ok(qr) => out.push_str(&qr),
                    Err(e) => out.push_str(&format!("  └─ (QR render failed: {e})\n")),
                }
                out.push_str("  └─ scan with a phone camera to join\n");
                out
            }
            Some(W::Connect { .. }) | Some(W::Disconnect) | Some(W::Forget { .. })
            | Some(W::On) | Some(W::Off) => {
                let Value::Object(pairs) = value else {
                    return String::new();
                };
                let verb = pairs.iter().find(|(k, _)| k == "verb").map(|(_, v)| v.display()).unwrap_or_default();
                let result = pairs.iter().find(|(k, _)| k == "result").map(|(_, v)| v.display()).unwrap_or_default();
                format!("  {} {} — {}", "✓".green(), verb.green(), result)
            }
        },
    }
}

/// The interface table, drawn from a list-of-records value.
pub fn interfaces_table(value: &Value) -> String {
    let Value::List(items) = value else {
        return String::new();
    };
    if items.is_empty() {
        return format!("     {}\n", "└─ (the engine enumerated no interfaces)".dimmed());
    }
    let mut table = Table::new(vec![
        "NAME".into(),
        "TYPE".into(),
        "LINK".into(),
        "CONNECTED".into(),
        "NETWORK".into(),
        "MANAGED".into(),
        "MTU".into(),
    ]);
    for item in items {
        if let Value::Object(pairs) = item {
            let f = |k: &str| -> String {
                pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
            };
            let fb = |k: &str| -> bool {
                matches!(pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v), Some(Value::Bool(true)))
            };
            table = table.row(vec![
                f("name"),
                f("type"),
                up_glyph(fb("up")).to_string(),
                if fb("connected") { "◉".to_string() } else { "○".to_string() },
                f("ssid"),
                if fb("managed") { "◉".to_string() } else { "○".to_string() },
                f("mtu"),
            ]);
        }
    }
    let mut out = table.render();
    out.push('\n');
    out
}

/// The radio table with hard/soft block states reported separately.
pub fn radios_table(value: &Value) -> String {
    let Value::List(items) = value else {
        return String::new();
    };
    if items.is_empty() {
        return format!("  {}\n", "└─ (the engine enumerated no radios)".dimmed());
    }
    let mut table = Table::new(vec![
        "RADIO".into(),
        "STATE".into(),
        "HARD-BLOCKED".into(),
        "SOFT-BLOCKED".into(),
    ]);
    for item in items {
        if let Value::Object(pairs) = item {
            let f = |k: &str| -> String {
                pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
            };
            let fb = |k: &str| -> bool {
                matches!(pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v), Some(Value::Bool(true)))
            };
            table = table.row(vec![
                f("name"),
                if fb("hard_blocked") {
                    "⛔ hard-blocked".to_string()
                } else if fb("soft_blocked") {
                    "⛔ soft-blocked".to_string()
                } else if fb("enabled") {
                    "◉ enabled".to_string()
                } else {
                    "○ disabled".to_string()
                },
                if fb("hard_blocked") { "yes".to_string() } else { "no".to_string() },
                if fb("soft_blocked") { "yes".to_string() } else { "no".to_string() },
            ]);
        }
    }
    let mut out = table.render();
    out.push('\n');
    out
}

/// The full `network status` view, rendered from the status value tree: the
/// host-networking master switch, the radios (hard and soft blocks reported
/// separately, never folded into enabled), and the interfaces with each
/// one's hostState — the four-state diagnostic that says WHY a silent device
/// is silent.
pub fn render_status_view(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "🌐".blue().bold(), "Network Status".blue().bold()));
    let networking = value.field_bool("networking");
    out.push_str(&format!(
        "  ├─ Networking  {}\n",
        if networking {
            "◉ enabled".green()
        } else {
            "○ disabled — every device is networking-disabled".red()
        }
    ));
    out.push_str("  ├─ Radios\n");
    if let Value::List(radios) = value.field("radios") {
        for (i, radio) in radios.iter().enumerate() {
            let branch = if i + 1 == radios.len() { "└─" } else { "├─" };
            let hard = radio.field_bool("hard_blocked");
            let soft = radio.field_bool("soft_blocked");
            let enabled = radio.field_bool("enabled");
            // The state is a (content, style) cell padded on its CONTENT to
            // the widest state there is. A `{:<9}` on the styled string would
            // count the escape bytes AND never fire (the shortest state is
            // already 9 columns of content), so a blocked radio's row pushed
            // the hard=/soft= columns out of the frame.
            let state = if hard {
                Cell::styled("⛔ hard-blocked", Sgr::Red)
            } else if soft {
                Cell::styled("⛔ soft-blocked", Sgr::Red)
            } else if enabled {
                Cell::styled("◉ enabled", Sgr::Green)
            } else {
                Cell::styled("○ disabled", Sgr::Dimmed)
            };
            out.push_str(&format!(
                "  │  {} {:<9} {} hard={} soft={}\n",
                branch,
                pad_to(&radio.field_str("name"), 9),
                pad_to(&state.to_string(), RADIO_STATE_COLUMN),
                if hard { "yes".red().to_string() } else { "no".dimmed().to_string() },
                if soft { "yes".red().to_string() } else { "no".dimmed().to_string() },
            ));
        }
    }
    out.push_str("  └─ Interfaces\n");
    if let Value::List(interfaces) = value.field("interfaces") {
        for (i, iface) in interfaces.iter().enumerate() {
            let branch = if i + 1 == interfaces.len() { "└─" } else { "├─" };
            // Padded on DISPLAY width, not on `chars().count()`: the name and
            // the type are host text, and an escape or a wide mark in either
            // would otherwise push the hostState column out of the frame.
            let ssid = iface.field_str("ssid");
            let ssid_cell = if ssid.is_empty() {
                String::new()
            } else {
                format!(" {}{}", "◉".green(), ssid.green())
            };
            out.push_str(&format!(
                "     {} {} {} {} hostState {} ({}){}\n",
                branch,
                pad_to(&iface.field_str("name"), 16),
                pad_to(&iface.field_str("type"), 9),
                if iface.field_bool("connected") { "◉".to_string() } else { "○".to_string() },
                iface.field_str("host_state"),
                iface.field_str("host_state_source").dimmed(),
                ssid_cell,
            ));
        }
    }
    out
}

