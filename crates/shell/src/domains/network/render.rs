//! The network domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, `↑↓`, aligned tables. Every renderer reads the ENGINE'S OWNED
//! TYPES (`InterfaceSnapshot`, `ConnStatus`, `RadioState`, `KnownWifiNetwork`,
//! `WifiNetwork`) — there is no capnp reader here, so nothing can leak and
//! nothing can desynchronise from what the engine measured.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::Table;
use crate::OutputFormat;

use smart_lib::domains::network::types::{
    ConnStatus, InterfaceSnapshot, KnownWifiNetwork, RadioState, WifiNetwork,
};

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

/// One remembered network. The credential is not here and can never be here:
/// the engine's type has no field for one.
pub fn known_value(network: &KnownWifiNetwork) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(network.ssid.clone())),
        ("auto_connect".into(), Value::Bool(network.auto_connect)),
        ("security".into(), Value::String(network.security.clone())),
    ])
}

/// One scanned access point.
pub fn wifi_value(network: &WifiNetwork) -> Value {
    Value::Object(vec![
        ("ssid".into(), Value::String(network.ssid.clone())),
        ("bssid".into(), Value::String(network.bssid.0.clone())),
        ("strength_dbm".into(), Value::Number(network.strength_dbm as f64)),
        ("security".into(), Value::String(network.security.clone())),
    ])
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
            out.push_str(&format!("  {} {}\n", "🔌".cyan(), name.cyan().bold()));
            out.push_str(&format!("  ├─ Type          {}\n", field("type")));
            out.push_str(&format!("  ├─ Connected     {}\n", yes_no(value.field_bool("connected"))));
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
                    "📶".cyan(),
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
                let mut table = Table::new(vec!["SSID".into(), "SECURITY".into(), "AUTOCONNECT".into()]);
                for item in items {
                    if let Value::Object(pairs) = item {
                        let f = |k: &str| -> String {
                            pairs.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.display()).unwrap_or_default()
                        };
                        table = table.row(vec![f("ssid"), f("security"), f("auto_connect")]);
                    }
                }
                out.push_str(&table.render());
                out.push('\n');
                out
            }
            Some(W::Scan) | Some(W::Connect { .. }) | Some(W::Disconnect) | Some(W::Forget { .. })
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
            let state = if hard {
                "⛔ hard-blocked".red()
            } else if soft {
                "⛔ soft-blocked".red()
            } else if enabled {
                "◉ enabled".green()
            } else {
                "○ disabled".dimmed()
            };
            out.push_str(&format!(
                "  │  {} {:<9} {} hard={} soft={}\n",
                branch,
                radio.field_str("name"),
                state,
                if hard { "yes".red().to_string() } else { "no".dimmed().to_string() },
                if soft { "yes".red().to_string() } else { "no".dimmed().to_string() },
            ));
        }
    }
    out.push_str("  └─ Interfaces\n");
    if let Value::List(interfaces) = value.field("interfaces") {
        for (i, iface) in interfaces.iter().enumerate() {
            let branch = if i + 1 == interfaces.len() { "└─" } else { "├─" };
            out.push_str(&format!(
                "     {} {:<16} {:<9} {} hostState {} ({})\n",
                branch,
                iface.field_str("name"),
                iface.field_str("type"),
                if iface.field_bool("connected") { "◉".to_string() } else { "○".to_string() },
                iface.field_str("host_state"),
                iface.field_str("host_state_source").dimmed(),
            ));
        }
    }
    out
}

