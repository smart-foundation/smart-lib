//! The queue domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads what the ENGINE handed back —
//! the queue names, the payloads' bytes, the engine's own availability matrix.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::Table;
use crate::OutputFormat;

use smart_lib::domains::queue::QueueBackend;

/// The widest the queue-name column may grow before the cut is marked.
const NAME_COLUMN: usize = 40;
/// The widest a payload column may grow before the cut is marked.
const PAYLOAD_COLUMN: usize = 60;

/// The queues the store holds, as the engine's own `listQueues` answers.
pub fn list_value(backend: &dyn QueueBackend) -> Result<Value, crate::error::SmartError> {
    let names = backend.list_queues()?;
    Ok(Value::List(
        names
            .into_iter()
            .map(|name| Value::Object(vec![("name".into(), Value::String(name))]))
            .collect(),
    ))
}

/// One payload read from a queue: the text when the bytes are UTF-8, the hex
/// when they are not, and an honest "the queue is empty" when the engine
/// answered `None`.
pub fn payload_record(
    name: &str,
    exists: bool,
    payload: Option<&[u8]>,
    removed: bool,
) -> Value {
    let (text, encoding, size) = match payload {
        Some(bytes) => {
            let (text, encoding) = match core::str::from_utf8(bytes) {
                Ok(text) => (text.to_string(), "utf-8"),
                Err(_) => (hex(bytes), "hex"),
            };
            (text, encoding, bytes.len())
        }
        None => (String::new(), "none", 0),
    };
    Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("exists".into(), Value::Bool(exists)),
        ("removed".into(), Value::Bool(removed)),
        ("payload".into(), Value::String(text)),
        ("encoding".into(), Value::String(encoding.to_string())),
        ("size".into(), Value::Number(size as f64)),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn QueueBackend) -> Value {
    Value::List(
        backend
            .queue_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it.
pub fn matrix_value() -> Value {
    use smart_lib::domains::queue::{AvailStatus, QUEUE_AVAILABILITY, QUEUE_SCHEMA_ID};
    let rows = QUEUE_AVAILABILITY
        .iter()
        .map(|row| {
            let status = match row.status {
                AvailStatus::Real => "real",
                AvailStatus::Degraded => "degraded",
                AvailStatus::Unavailable => "unavailable",
            };
            Value::Object(vec![
                ("ordinal".into(), Value::Number(row.ordinal as f64)),
                ("method".into(), Value::String(row.name.to_string())),
                ("status".into(), Value::String(status.to_string())),
                (
                    "reason_class".into(),
                    match row.reason_class {
                        Some(class) => Value::String(class.as_str().to_string()),
                        None => Value::Null,
                    },
                ),
                ("reason".into(), Value::String(row.reason.to_string())),
                (
                    "l3_test".into(),
                    match row.l3_test {
                        Some(test) => Value::String(test.to_string()),
                        None => Value::Null,
                    },
                ),
            ])
        })
        .collect();
    Value::Object(vec![
        ("schema_id".into(), Value::String(QUEUE_SCHEMA_ID.to_string())),
        ("rows".into(), Value::List(rows)),
    ])
}

/// The bytes as hex, the way a hex dump spells them.
fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Render one command's value in the requested format.
pub fn render_action(cmd: &super::QueueCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::QueueCmd, value: &Value) -> String {
    use super::QueueAction as Q;
    match &cmd.action {
        None | Some(Q::List) => render_list(value),
        Some(Q::Get { name }) => format!(
            "  {} {} — ◉ the store holds this queue",
            "📮".blue().bold(),
            name.blue().bold()
        ),
        Some(Q::Peek { name }) => render_payload(value, name, "head"),
        Some(Q::Dequeue { name }) => render_payload(value, name, "taken"),
        Some(Q::Enqueue { .. }) | Some(Q::Clear { .. }) | Some(Q::Delete { .. }) => {
            format!(
                "  {} {} — {}",
                "📮".blue().bold(),
                value.field_str("verb").green().bold(),
                value.field_str("name")
            )
        }
        Some(Q::Caps) => format!(
            "  {} {} — {}",
            "📮".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(Q::Matrix) => render_matrix(value),
    }
}

fn render_list(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📮".blue().bold(), "Queue".blue().bold()));
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!("  ├─ Queues  {}\n", items.len()));
            let mut table = Table::new(vec!["QUEUE".into()]).cap(0, NAME_COLUMN);
            for item in items {
                table = table.row(vec![item.field_str("name")]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's store holds no queue yet (an honest empty)\n",
                "○".dimmed()
            ));
        }
    }
    out
}

fn render_payload(value: &Value, name: &str, what: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "📮".blue().bold(),
        name.blue().bold(),
        what.dimmed()
    ));
    if !value.field_bool("exists") {
        out.push_str(&format!(
            "  └─ {} — the store holds no queue by that name\n",
            "○".dimmed()
        ));
        return out;
    }
    let encoding = value.field_str("encoding");
    if encoding == "none" {
        out.push_str(&format!(
            "  └─ {} — the queue is empty (the engine answered nothing at its head)\n",
            "○".dimmed()
        ));
        return out;
    }
    out.push_str(&format!("  ├─ Encoding  {encoding}\n"));
    out.push_str(&format!(
        "  ├─ Size      {} byte(s)\n",
        value.field("size").display()
    ));
    out.push_str(&format!(
        "  └─ Payload   {}\n",
        crate::widgets::truncate_to(&value.field_str("payload"), PAYLOAD_COLUMN)
    ));
    out
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "📮".blue().bold(),
        "Availability".blue().bold(),
        value.field_str("schema_id")
    ));
    out.push_str("  the contract's surface as the ENGINE's own matrix publishes it\n");
    if let Value::List(rows) = value.field("rows") {
        let mut table = Table::new(vec![
            "#".into(),
            "METHOD".into(),
            "STATUS".into(),
            "REASON".into(),
        ])
        .cap(3, 60);
        for row in rows {
            let status = row.field_str("status");
            let glyph = match status.as_str() {
                "real" => "◉".green().to_string(),
                "degraded" => "◐".yellow().to_string(),
                _ => "○".dimmed().to_string(),
            };
            // A REAL row carries no reason — it carries the L3 test that
            // PROVES it, and that proof is what the column shows.
            let reason = if row.field_str("reason").is_empty() {
                let proof = row.field_str("l3_test");
                if proof.is_empty() || proof == "null" {
                    "—".to_string()
                } else {
                    proof
                }
            } else {
                row.field_str("reason")
            };
            table = table.row(vec![
                row.field("ordinal").display(),
                row.field_str("method"),
                format!("{glyph} {status}"),
                reason,
            ]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    out
}
