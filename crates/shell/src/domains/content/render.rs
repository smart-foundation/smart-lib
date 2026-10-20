//! The content domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the engine's OWNED types
//! (`ContentItem`, `ContentFilter`) — the rows are the items the engine's own
//! store holds, with the store's own versions and timestamps.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::{human_bytes, truncate_to, Table};
use crate::OutputFormat;

use smart_lib::domains::content::types::{ContentFilter, ContentItem, Pagination};
use smart_lib::domains::content::ContentBackend;

/// The widest the TITLE column may grow before the cut is marked.
const TITLE_COLUMN: usize = 34;
/// The widest the SUMMARY column may grow before the cut is marked.
const SUMMARY_COLUMN: usize = 40;
/// The widest the REASON column may grow before the cut is marked.
const REASON_COLUMN: usize = 60;

/// One item, as the engine's own `ContentItem`.
pub fn item_value(item: &ContentItem) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(item.id as f64)),
        ("title".into(), Value::String(item.title.clone())),
        ("summary".into(), Value::String(item.summary.clone())),
        ("uri".into(), Value::String(item.uri.clone())),
        ("mime_type".into(), Value::String(item.mime_type.clone())),
        ("size".into(), Value::Number(item.size() as f64)),
        (
            "tags".into(),
            Value::List(item.tags.iter().map(|t| Value::String(t.clone())).collect()),
        ),
        ("version".into(), Value::Number(item.version as f64)),
        ("created_at".into(), Value::Number(item.created_at as f64)),
        ("modified_at".into(), Value::Number(item.modified_at as f64)),
        ("trashed".into(), Value::Bool(item.trashed)),
        ("published".into(), Value::Bool(item.published)),
        (
            "body".into(),
            Value::String(String::from_utf8_lossy(&item.body).to_string()),
        ),
    ])
}

/// The query's rows, with the store's own total beside them.
pub fn list_value(
    backend: &dyn ContentBackend,
    filter: &ContentFilter,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let listing = backend.query(filter, page)?;
    Ok(Value::Object(vec![
        ("total".into(), Value::Number(listing.total as f64)),
        ("query".into(), Value::String(filter.query.clone())),
        ("mime".into(), Value::String(filter.mime_type_prefix.clone())),
        (
            "tags".into(),
            Value::List(filter.tags.iter().map(|t| Value::String(t.clone())).collect()),
        ),
        (
            "include_trashed".into(),
            Value::Bool(filter.include_trashed),
        ),
        (
            "items".into(),
            Value::List(listing.items.iter().map(item_value).collect()),
        ),
    ]))
}

/// A write verb's echo.
pub fn verb_value(verb: &str, id: u64, title: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("id".into(), Value::Number(id as f64)),
        ("title".into(), Value::String(title.to_string())),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn ContentBackend) -> Value {
    Value::List(
        backend
            .content_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it.
pub fn matrix_value() -> Value {
    use smart_lib::domains::content::{AvailStatus, CONTENT_AVAILABILITY, CONTENT_SCHEMA_ID};
    let rows = CONTENT_AVAILABILITY
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
        ("schema_id".into(), Value::String(CONTENT_SCHEMA_ID.to_string())),
        ("rows".into(), Value::List(rows)),
    ])
}

/// Render one command's value in the requested format.
pub fn render_action(cmd: &super::ContentCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::ContentCmd, value: &Value) -> String {
    use super::ContentAction as C;
    match &cmd.action {
        None | Some(C::List { .. }) => render_list(value),
        Some(C::Get { id }) => render_item(value, *id),
        Some(C::Publish { .. }) => render_verb(value),
        Some(C::PublishMany { .. }) => format!(
            "  {} {} — {} item(s): {}",
            "📄".blue().bold(),
            value.field_str("verb").green().bold(),
            value.field("count").display(),
            match value.field("ids") {
                Value::List(ids) => ids
                    .iter()
                    .map(|id| id.display())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => String::new(),
            }
        ),
        Some(C::Update { .. }) => format!(
            "  {} {} — item {} is now at version {}",
            "📄".blue().bold(),
            value.field_str("verb").green().bold(),
            value.field("id").display(),
            value.field("version").display()
        ),
        Some(C::Unpublish { .. })
        | Some(C::Delete { .. })
        | Some(C::Trash { .. })
        | Some(C::Restore { .. }) => render_verb(value),
        Some(C::EmptyTrash) => format!(
            "  {} {} — {} item(s) removed",
            "📄".blue().bold(),
            value.field_str("verb").green().bold(),
            value.field("emptied").display()
        ),
        Some(C::Caps) => format!(
            "  {} {} — {}",
            "📄".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(C::Matrix) => render_matrix(value),
    }
}

fn render_list(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📄".blue().bold(), "Content".blue().bold()));
    let query = value.field_str("query");
    if !query.is_empty() {
        out.push_str(&format!("  ├─ Query    {}\n", query.bold()));
    }
    let mime = value.field_str("mime");
    if !mime.is_empty() {
        out.push_str(&format!("  ├─ MIME     {mime}\n"));
    }
    let tags = match value.field("tags") {
        Value::List(list) if !list.is_empty() => list
            .iter()
            .map(|t| t.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    if !tags.is_empty() {
        out.push_str(&format!("  ├─ Tags     {tags}\n"));
    }
    match value.field("items") {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!(
                "  ├─ Items    {} of {} in the engine's store\n",
                items.len(),
                value.field("total").display()
            ));
            let mut table = Table::new(vec![
                "ID".into(),
                "TITLE".into(),
                "MIME".into(),
                "SIZE".into(),
                "VER".into(),
                "TAGS".into(),
            ])
            .cap(1, TITLE_COLUMN);
            for item in items {
                let tags = match item.field("tags") {
                    Value::List(list) => list
                        .iter()
                        .map(|t| t.display())
                        .collect::<Vec<_>>()
                        .join(","),
                    _ => String::new(),
                };
                table = table.row(vec![
                    item.field("id").display(),
                    item.field_str("title"),
                    item.field_str("mime_type"),
                    human_bytes(item.field("size").as_f64().unwrap_or(0.0) as u64),
                    item.field("version").display(),
                    tags,
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's store holds no item (an honest empty;\n",
                "○".dimmed()
            ));
            out.push_str("      `content publish` puts one there)\n");
        }
    }
    out
}

fn render_item(value: &Value, id: u64) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "📄".blue().bold(),
        value.field_str("title").bold(),
        id.to_string().dimmed()
    ));
    out.push_str(&format!("  ├─ ID         {}\n", value.field("id").display()));
    out.push_str(&format!(
        "  ├─ Summary    {}\n",
        truncate_to(&value.field_str("summary"), SUMMARY_COLUMN)
    ));
    out.push_str(&format!("  ├─ URI        {}\n", value.field_str("uri")));
    out.push_str(&format!("  ├─ MIME       {}\n", value.field_str("mime_type")));
    out.push_str(&format!(
        "  ├─ Size       {}\n",
        human_bytes(value.field("size").as_f64().unwrap_or(0.0) as u64)
    ));
    let tags = match value.field("tags") {
        Value::List(list) => list
            .iter()
            .map(|t| t.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    out.push_str(&format!("  ├─ Tags       {tags}\n"));
    out.push_str(&format!(
        "  ├─ Version    {}\n",
        value.field("version").display()
    ));
    out.push_str(&format!(
        "  ├─ Trashed    {}\n",
        if value.field_bool("trashed") { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "  ├─ Published  {}\n",
        if value.field_bool("published") { "yes" } else { "no" }
    ));
    out.push_str("  └─ Body\n");
    for line in value.field_str("body").lines() {
        out.push_str(&format!("     {line}\n"));
    }
    out
}

fn render_verb(value: &Value) -> String {
    format!(
        "  {} {} — item {}",
        "📄".blue().bold(),
        value.field_str("verb").green().bold(),
        value.field("id").display()
    )
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "📄".blue().bold(),
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
        .cap(3, REASON_COLUMN);
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
