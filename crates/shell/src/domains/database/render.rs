//! The database domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads what the ENGINE handed back —
//! the bytes of a value, the key and namespace lists, the engine's own
//! availability matrix — so nothing here can drift from what the engine's own
//! store holds.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::Table;
use crate::OutputFormat;

use smart_lib::domains::database::types::Pagination;
use smart_lib::domains::database::DatabaseBackend;

/// The widest the namespace column may grow before the cut is marked.
const NAMESPACE_COLUMN: usize = 40;

/// The namespaces the store holds, each with the number of keys it holds —
/// the engine's own `listNamespaces` and `count` reads.
pub fn namespaces_value(
    backend: &dyn DatabaseBackend,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let (namespaces, total) = backend.list_namespaces(page)?;
    let mut rows = Vec::with_capacity(namespaces.len());
    for namespace in &namespaces {
        let count = backend.count(namespace)?;
        rows.push(Value::Object(vec![
            ("namespace".into(), Value::String(namespace.clone())),
            ("keys".into(), Value::Number(count as f64)),
        ]));
    }
    Ok(Value::Object(vec![
        ("total".into(), Value::Number(total as f64)),
        ("namespaces".into(), Value::List(rows)),
    ]))
}

/// The keys of one namespace, with the store's own total beside them.
pub fn keys_value(
    backend: &dyn DatabaseBackend,
    namespace: &str,
    prefix: &str,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let (keys, total) = backend.list_keys(namespace, prefix, page)?;
    let count = backend.count(namespace)?;
    Ok(Value::Object(vec![
        ("namespace".into(), Value::String(namespace.to_string())),
        ("prefix".into(), Value::String(prefix.to_string())),
        ("total".into(), Value::Number(total as f64)),
        ("keys_in_namespace".into(), Value::Number(count as f64)),
        (
            "keys".into(),
            Value::List(
                keys.iter()
                    .map(|key| Value::Object(vec![("key".into(), Value::String(key.clone()))]))
                    .collect(),
            ),
        ),
    ]))
}

/// One value, as the engine's bytes: the text when the payload is UTF-8, the
/// hex when it is not (a lossy replacement would be a lie about the bytes).
pub fn value_record(namespace: &str, key: &str, data: &[u8]) -> Value {
    let (text, encoding) = match core::str::from_utf8(data) {
        Ok(text) => (text.to_string(), "utf-8"),
        Err(_) => (hex(data), "hex"),
    };
    Value::Object(vec![
        ("namespace".into(), Value::String(namespace.to_string())),
        ("key".into(), Value::String(key.to_string())),
        ("value".into(), Value::String(text)),
        ("encoding".into(), Value::String(encoding.to_string())),
        ("size".into(), Value::Number(data.len() as f64)),
    ])
}

/// A write verb's echo: what the engine acknowledged.
pub fn verb_value(verb: &str, namespace: &str, key: &str, size: usize) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("namespace".into(), Value::String(namespace.to_string())),
        ("key".into(), Value::String(key.to_string())),
        ("size".into(), Value::Number(size as f64)),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn DatabaseBackend) -> Value {
    Value::List(
        backend
            .database_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it —
/// the engine's own per-method claim, not the shell's verb list.
pub fn matrix_value() -> Value {
    use smart_lib::domains::database::{AvailStatus, DATABASE_AVAILABILITY, DATABASE_SCHEMA_ID};
    let rows = DATABASE_AVAILABILITY
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
        ("schema_id".into(), Value::String(DATABASE_SCHEMA_ID.to_string())),
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
pub fn render_action(
    cmd: &super::DatabaseCmd,
    value: &Value,
    format: OutputFormat,
) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::DatabaseCmd, value: &Value) -> String {
    use super::DatabaseAction as D;
    match &cmd.action {
        None | Some(D::Namespaces { .. }) => render_namespaces(value),
        Some(D::Keys { namespace, .. }) => render_keys(value, namespace),
        Some(D::Get { namespace, key }) => render_value(value, namespace, key),
        Some(D::Has { namespace, key }) => {
            let present = value.field_bool("present");
            format!(
                "  {} {}/{} — {}",
                "🗄".blue().bold(),
                namespace,
                key,
                if present {
                    "◉ present".green().to_string()
                } else {
                    "○ absent".dimmed().to_string()
                }
            )
        }
        Some(D::Count { namespace }) => format!(
            "  {} {} — {} key(s)",
            "🗄".blue().bold(),
            namespace,
            value.field("count").display()
        ),
        Some(D::Set { .. }) | Some(D::Delete { .. }) | Some(D::Clear { .. }) => render_verb(value),
        Some(D::Caps) => format!(
            "  {} {} — {}",
            "🗄".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(D::Matrix) => render_matrix(value),
    }
}

fn render_namespaces(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "🗄".blue().bold(), "Database".blue().bold()));
    let total = value.field("total").display();
    match value.field("namespaces") {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!(
                "  ├─ Namespaces  {} ({} in the store)\n",
                items.len(),
                total
            ));
            let mut table = Table::new(vec!["NAMESPACE".into(), "KEYS".into()]).cap(0, NAMESPACE_COLUMN);
            for item in items {
                table = table.row(vec![item.field_str("namespace"), item.field("keys").display()]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's store holds no namespace yet (an honest empty)\n",
                "○".dimmed()
            ));
        }
    }
    out
}

fn render_keys(value: &Value, namespace: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "🗄".blue().bold(),
        namespace.blue().bold(),
        "keys".dimmed()
    ));
    let prefix = value.field_str("prefix");
    if !prefix.is_empty() {
        out.push_str(&format!("  ├─ Prefix     {prefix}\n"));
    }
    match value.field("keys") {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!(
                "  ├─ Keys       {} of {} in the namespace\n",
                items.len(),
                value.field("keys_in_namespace").display()
            ));
            let mut table = Table::new(vec!["KEY".into()]).cap(0, NAMESPACE_COLUMN);
            for item in items {
                table = table.row(vec![item.field_str("key")]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the namespace holds no key under that prefix (an honest empty)\n",
                "○".dimmed()
            ));
        }
    }
    out
}

fn render_value(value: &Value, namespace: &str, key: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}/{} — {}\n",
        "🗄".blue().bold(),
        namespace.blue().bold(),
        key.blue().bold(),
        format!("{} byte(s)", value.field("size").display()).dimmed()
    ));
    out.push_str(&format!(
        "  ├─ Encoding   {}\n",
        value.field_str("encoding")
    ));
    out.push_str(&format!(
        "  └─ Value      {}\n",
        value.field_str("value")
    ));
    out
}

fn render_verb(value: &Value) -> String {
    format!(
        "  {} {} — {} {}\n",
        "🗄".blue().bold(),
        value.field_str("verb").green().bold(),
        value.field_str("namespace"),
        value.field_str("key")
    )
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "🗄".blue().bold(),
        "Availability".blue().bold(),
        value.field_str("schema_id")
    ));
    out.push_str("  the contract's surface as the ENGINE's own matrix publishes it\n");
    match value.field("rows") {
        Value::List(rows) if !rows.is_empty() => {
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
                let reason = if row.field_str("reason").is_empty() {
                    // A REAL row carries no reason — it carries the L3 test
                    // that PROVES it, and that proof is what the column shows.
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
        _ => {}
    }
    out
}
