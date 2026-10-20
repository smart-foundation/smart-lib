//! The manifest domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the engine's OWNED types
//! (`ManifestEntry`, `ValidationReport`) — the rows are the manifests the
//! engine's own store holds.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::Table;
use crate::OutputFormat;

use smart_lib::domains::manifest::types::{ManifestEntry, Pagination, ValidationReport};
use smart_lib::domains::manifest::ManifestBackend;

/// The widest the NAME column may grow before the cut is marked.
const NAME_COLUMN: usize = 28;
/// The widest the DESCRIPTION column may grow before the cut is marked.
const DESCRIPTION_COLUMN: usize = 44;
/// The widest the REASON column may grow before the cut is marked.
const REASON_COLUMN: usize = 64;

/// One manifest, as the engine's own entry.
pub fn manifest_value(manifest: &ManifestEntry) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(manifest.name.clone())),
        ("display_name".into(), Value::String(manifest.display_name.clone())),
        ("description".into(), Value::String(manifest.description.clone())),
        ("vendor".into(), Value::String(manifest.vendor.clone())),
        ("version".into(), Value::String(manifest.version.clone())),
        (
            "entries".into(),
            Value::List(
                manifest
                    .entries
                    .iter()
                    .map(|e| Value::String(e.clone()))
                    .collect(),
            ),
        ),
        (
            "depends".into(),
            Value::List(
                manifest
                    .depends
                    .iter()
                    .map(|d| {
                        Value::Object(vec![
                            ("name".into(), Value::String(d.name.clone())),
                            ("optional".into(), Value::Bool(d.optional)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "offers".into(),
            Value::List(
                manifest
                    .offers
                    .iter()
                    .map(|o| Value::String(o.clone()))
                    .collect(),
            ),
        ),
        ("shape".into(), Value::String(manifest.shape.clone())),
    ])
}

/// The store's rows, with the store's own total beside them.
pub fn list_value(
    backend: &dyn ManifestBackend,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let listing = backend.list(page)?;
    Ok(Value::Object(vec![
        ("total".into(), Value::Number(listing.total as f64)),
        (
            "manifests".into(),
            Value::List(listing.items.iter().map(manifest_value).collect()),
        ),
    ]))
}

/// A validation report, with the engine's own errors verbatim.
pub fn validation_value(report: &ValidationReport, name: &str, verb: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("name".into(), Value::String(name.to_string())),
        ("valid".into(), Value::Bool(report.valid)),
        (
            "errors".into(),
            Value::List(
                report
                    .errors
                    .iter()
                    .map(|e| Value::String(e.clone()))
                    .collect(),
            ),
        ),
    ])
}

/// A write verb's echo.
pub fn verb_value(verb: &str, name: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("name".into(), Value::String(name.to_string())),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn ManifestBackend) -> Value {
    Value::List(
        backend
            .manifest_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it.
pub fn matrix_value() -> Value {
    use smart_lib::domains::manifest::{AvailStatus, MANIFEST_AVAILABILITY, MANIFEST_SCHEMA_ID};
    let rows = MANIFEST_AVAILABILITY
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
        ("schema_id".into(), Value::String(MANIFEST_SCHEMA_ID.to_string())),
        ("rows".into(), Value::List(rows)),
    ])
}

/// Render one command's value in the requested format.
pub fn render_action(
    cmd: &super::ManifestCmd,
    value: &Value,
    format: OutputFormat,
) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::ManifestCmd, value: &Value) -> String {
    use super::ManifestAction as M;
    match &cmd.action {
        None | Some(M::List { .. }) => render_list(value),
        Some(M::Resolve { name }) => render_manifest(value, name),
        Some(M::ByName { name, .. }) => render_manifest(value, name),
        Some(M::Validate { .. }) | Some(M::ValidateDeps { .. }) => render_validation(value),
        Some(M::Register { .. }) | Some(M::Unregister { .. }) => format!(
            "  {} {} — {}",
            "📦".blue().bold(),
            value.field_str("verb").green().bold(),
            value.field_str("name")
        ),
        Some(M::Caps) => format!(
            "  {} {} — {}",
            "📦".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(M::Matrix) => render_matrix(value),
    }
}

fn render_list(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📦".blue().bold(), "Manifests".blue().bold()));
    match value.field("manifests") {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!(
                "  ├─ Manifests  {} of {} in the engine's store\n",
                items.len(),
                value.field("total").display()
            ));
            let mut table = Table::new(vec![
                "NAME".into(),
                "DISPLAY".into(),
                "VENDOR".into(),
                "VERSION".into(),
                "DEPENDS".into(),
            ])
            .cap(0, NAME_COLUMN);
            for item in items {
                let depends = match item.field("depends") {
                    Value::List(list) => list
                        .iter()
                        .map(|d| {
                            let mark = if d.field_bool("optional") {
                                "?"
                            } else {
                                ""
                            };
                            format!("{mark}{}", d.field_str("name"))
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                    _ => String::new(),
                };
                table = table.row(vec![
                    item.field_str("name"),
                    item.field_str("display_name"),
                    item.field_str("vendor"),
                    item.field_str("version"),
                    depends,
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's store holds no manifest yet (an honest empty;\n",
                "○".dimmed()
            ));
            out.push_str("      `manifest register` puts one there)\n");
        }
    }
    out
}

fn render_manifest(value: &Value, name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "📦".blue().bold(),
        value.field_str("display_name").bold(),
        name.dimmed()
    ));
    out.push_str(&format!("  ├─ Name        {}\n", value.field_str("name")));
    out.push_str(&format!(
        "  ├─ Description {}\n",
        crate::widgets::truncate_to(&value.field_str("description"), DESCRIPTION_COLUMN)
    ));
    out.push_str(&format!("  ├─ Vendor      {}\n", value.field_str("vendor")));
    out.push_str(&format!("  ├─ Version     {}\n", value.field_str("version")));
    let entries = match value.field("entries") {
        Value::List(list) => list
            .iter()
            .map(|e| e.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    out.push_str(&format!("  ├─ Entries     {entries}\n"));
    let depends = match value.field("depends") {
        Value::List(list) if !list.is_empty() => list
            .iter()
            .map(|d| {
                let mark = if d.field_bool("optional") {
                    "optional "
                } else {
                    "required "
                };
                format!("{mark}{}", d.field_str("name"))
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => "none".to_string(),
    };
    out.push_str(&format!("  ├─ Depends     {depends}\n"));
    let offers = match value.field("offers") {
        Value::List(list) => list
            .iter()
            .map(|o| o.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    out.push_str(&format!("  ├─ Offers      {offers}\n"));
    out.push_str(&format!("  └─ Shape       {}\n", value.field_str("shape")));
    out
}

fn render_validation(value: &Value) -> String {
    let valid = value.field_bool("valid");
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "📦".blue().bold(),
        value.field_str("verb").bold(),
        value.field_str("name")
    ));
    out.push_str(&format!(
        "  ├─ Verdict  {}\n",
        if valid {
            "◉ valid".green().to_string()
        } else {
            "✗ invalid".red().to_string()
        }
    ));
    match value.field("errors") {
        Value::List(errors) if !errors.is_empty() => {
            out.push_str(&format!("  └─ Errors  {}\n", errors.len()));
            for error in errors {
                out.push_str(&format!(
                    "     ├─ {}\n",
                    crate::widgets::truncate_to(&error.display(), REASON_COLUMN)
                ));
            }
        }
        _ => {
            out.push_str("  └─ Errors  none\n");
        }
    }
    out
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "📦".blue().bold(),
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
