//! The appindex domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the engine's OWNED types
//! (`AppEntry`, `AppStats`) — the rows are the host's own desktop entries, so
//! a name on screen is a name the host published.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::{truncate_to, Table};
use crate::OutputFormat;

use smart_lib::domains::appindex::types::{AppEntry, AppSearchFilter, AppStats, Pagination};
use smart_lib::domains::appindex::AppindexBackend;

/// The widest the NAME column may grow before the cut is marked.
const NAME_COLUMN: usize = 34;
/// The widest the SOURCE column may grow before the cut is marked.
const SOURCE_COLUMN: usize = 28;
/// The widest the CATEGORY column may grow before the cut is marked.
const CATEGORY_COLUMN: usize = 26;

/// One application, as the engine's own entry.
pub fn entry_value(entry: &AppEntry) -> Value {
    Value::Object(vec![
        ("id".into(), Value::String(entry.id.clone())),
        ("name".into(), Value::String(entry.name.clone())),
        ("generic_name".into(), Value::String(entry.generic_name.clone())),
        ("comment".into(), Value::String(entry.comment.clone())),
        ("exec".into(), Value::String(entry.exec.clone())),
        ("icon".into(), Value::String(entry.icon.clone())),
        ("terminal".into(), Value::Bool(entry.terminal)),
        ("no_display".into(), Value::Bool(entry.no_display)),
        ("hidden".into(), Value::Bool(entry.hidden)),
        (
            "categories".into(),
            Value::List(
                entry
                    .categories
                    .iter()
                    .map(|c| Value::String(c.clone()))
                    .collect(),
            ),
        ),
        (
            "keywords".into(),
            Value::List(entry.keywords.iter().map(|k| Value::String(k.clone())).collect()),
        ),
        ("source".into(), Value::String(entry.source.clone())),
        ("path".into(), Value::String(entry.path.clone())),
        ("pinned".into(), Value::Bool(entry.pinned)),
    ])
}

/// The index's rows, with the corpus's own total beside them.
pub fn list_value(
    backend: &dyn AppindexBackend,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let listing = backend.list_apps(page)?;
    Ok(Value::Object(vec![
        ("total".into(), Value::Number(listing.total as f64)),
        (
            "apps".into(),
            Value::List(listing.items.iter().map(entry_value).collect()),
        ),
    ]))
}

/// The search's rows, with the filter that produced them beside them.
pub fn search_value(
    backend: &dyn AppindexBackend,
    filter: &AppSearchFilter,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let hits = backend.search_apps(filter, page)?;
    Ok(Value::Object(vec![
        ("total".into(), Value::Number(hits.total as f64)),
        ("query".into(), Value::String(filter.name_contains.clone())),
        (
            "categories".into(),
            Value::List(
                filter
                    .categories
                    .iter()
                    .map(|c| Value::String(c.clone()))
                    .collect(),
            ),
        ),
        (
            "sources".into(),
            Value::List(filter.sources.iter().map(|s| Value::String(s.clone())).collect()),
        ),
        (
            "apps".into(),
            Value::List(hits.items.iter().map(entry_value).collect()),
        ),
    ]))
}

/// The index's own statistics.
pub fn stats_value(stats: &AppStats) -> Value {
    Value::Object(vec![
        ("total_apps".into(), Value::Number(stats.total_apps as f64)),
        (
            "installed_apps".into(),
            Value::Number(stats.installed_apps as f64),
        ),
        (
            "source_counts".into(),
            Value::List(
                stats
                    .source_counts
                    .iter()
                    .map(|(source, count)| {
                        Value::Object(vec![
                            ("source".into(), Value::String(source.clone())),
                            ("count".into(), Value::Number(*count as f64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "category_counts".into(),
            Value::List(
                stats
                    .category_counts
                    .iter()
                    .map(|(category, count)| {
                        Value::Object(vec![
                            ("category".into(), Value::String(category.clone())),
                            ("count".into(), Value::Number(*count as f64)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// A write verb's echo.
pub fn verb_value(verb: &str, id: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("id".into(), Value::String(id.to_string())),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn AppindexBackend) -> Value {
    Value::List(
        backend
            .appindex_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it.
pub fn matrix_value() -> Value {
    use smart_lib::domains::appindex::{AvailStatus, APPINDEX_AVAILABILITY, APPINDEX_SCHEMA_ID};
    let rows = APPINDEX_AVAILABILITY
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
        ("schema_id".into(), Value::String(APPINDEX_SCHEMA_ID.to_string())),
        ("rows".into(), Value::List(rows)),
    ])
}

/// Render one command's value in the requested format.
pub fn render_action(
    cmd: &super::AppindexCmd,
    value: &Value,
    format: OutputFormat,
) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::AppindexCmd, value: &Value) -> String {
    use super::AppindexAction as A;
    match &cmd.action {
        None | Some(A::List { .. }) => render_list(value),
        Some(A::Get { id }) => render_entry(value, id),
        Some(A::Search { query, .. }) => render_search(value, query),
        Some(A::Stats) => render_stats(value),
        Some(A::Pinned) => render_pinned(value),
        Some(A::Batch { ids }) => render_batch(value, ids),
        Some(A::Pin { .. }) | Some(A::Unpin { .. }) => format!(
            "  {} {} — {} {}",
            "🧭".blue().bold(),
            value.field_str("verb").green().bold(),
            value.field_str("id"),
            "(the engine's own pinned set)".dimmed()
        ),
        Some(A::Caps) => format!(
            "  {} {} — {}",
            "🧭".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(A::Matrix) => render_matrix(value),
    }
}

/// The apps table: the name the host published, the generic name, the
/// categories and the source directory the entry came from.
fn apps_table(value: &Value) -> String {
    let mut table = Table::new(vec![
        "ID".into(),
        "NAME".into(),
        "GENERIC".into(),
        "CATEGORIES".into(),
        "SOURCE".into(),
        "PIN".into(),
    ])
    .cap(1, NAME_COLUMN)
    .cap(3, CATEGORY_COLUMN)
    .cap(4, SOURCE_COLUMN);
    if let Value::List(items) = value.field("apps") {
        for item in items {
            let categories = match item.field("categories") {
                Value::List(list) => list
                    .iter()
                    .map(|c| c.display())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => String::new(),
            };
            table = table.row(vec![
                item.field_str("id"),
                item.field_str("name"),
                item.field_str("generic_name"),
                categories,
                item.field_str("source"),
                if item.field_bool("pinned") {
                    "◉".to_string()
                } else {
                    "○".to_string()
                },
            ]);
        }
    }
    table.render()
}

fn render_list(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "🧭".blue().bold(),
        "App Index".blue().bold()
    ));
    let total = value.field("total").display();
    match value.field("apps") {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!(
                "  ├─ Entries  {} of {} in the host's corpus\n",
                items.len(),
                total
            ));
            out.push_str(&apps_table(value));
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the host publishes no desktop entry (an honest empty)\n",
                "○".dimmed()
            ));
        }
    }
    out
}

fn render_search(value: &Value, query: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "🧭".blue().bold(),
        "Search".blue().bold(),
        query.bold()
    ));
    out.push_str(&format!(
        "  ├─ Matches  {} of {} in the host's corpus\n",
        match value.field("apps") {
            Value::List(items) => items.len(),
            _ => 0,
        },
        value.field("total").display()
    ));
    let categories = match value.field("categories") {
        Value::List(list) if !list.is_empty() => list
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    if !categories.is_empty() {
        out.push_str(&format!("  ├─ Category {categories}\n"));
    }
    let sources = match value.field("sources") {
        Value::List(list) if !list.is_empty() => list
            .iter()
            .map(|s| s.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    if !sources.is_empty() {
        out.push_str(&format!("  ├─ Source   {sources}\n"));
    }
    out.push_str(&apps_table(value));
    out.push('\n');
    out
}

fn render_entry(value: &Value, id: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "🧭".blue().bold(),
        value.field_str("name").bold(),
        id.dimmed()
    ));
    out.push_str(&format!("  ├─ ID           {}\n", value.field_str("id")));
    out.push_str(&format!(
        "  ├─ Generic      {}\n",
        value.field_str("generic_name")
    ));
    out.push_str(&format!("  ├─ Comment      {}\n", value.field_str("comment")));
    out.push_str(&format!("  ├─ Exec         {}\n", value.field_str("exec")));
    out.push_str(&format!("  ├─ Icon         {}\n", value.field_str("icon")));
    let categories = match value.field("categories") {
        Value::List(list) => list
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    out.push_str(&format!("  ├─ Categories   {categories}\n"));
    let keywords = match value.field("keywords") {
        Value::List(list) => list
            .iter()
            .map(|k| k.display())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    out.push_str(&format!("  ├─ Keywords     {keywords}\n"));
    out.push_str(&format!("  ├─ Source       {}\n", value.field_str("source")));
    out.push_str(&format!("  ├─ Path         {}\n", value.field_str("path")));
    out.push_str(&format!(
        "  └─ Pinned       {} (the engine's own set)\n",
        if value.field_bool("pinned") {
            "◉ yes".green().to_string()
        } else {
            "○ no".dimmed().to_string()
        }
    ));
    out
}

fn render_stats(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "🧭".blue().bold(),
        "App Index Stats".blue().bold()
    ));
    out.push_str(&format!(
        "  ├─ Entries     {}\n",
        value.field("total_apps").display()
    ));
    out.push_str(&format!(
        "  ├─ Installed   {}\n",
        value.field("installed_apps").display()
    ));
    out.push_str("  ├─ Sources\n");
    if let Value::List(items) = value.field("source_counts") {
        let mut table = Table::new(vec!["SOURCE".into(), "ENTRIES".into()]).cap(0, SOURCE_COLUMN);
        for item in items {
            table = table.row(vec![item.field_str("source"), item.field("count").display()]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    out.push_str("  └─ Categories\n");
    if let Value::List(items) = value.field("category_counts") {
        let mut table = Table::new(vec!["CATEGORY".into(), "ENTRIES".into()]).cap(0, CATEGORY_COLUMN);
        for item in items {
            table = table.row(vec![item.field_str("category"), item.field("count").display()]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    out
}

fn render_pinned(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "🧭".blue().bold(),
        "Pinned".blue().bold()
    ));
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!("  ├─ {} application(s)\n", items.len()));
            let mut table = Table::new(vec!["ID".into(), "NAME".into(), "SOURCE".into()])
                .cap(1, NAME_COLUMN)
                .cap(2, SOURCE_COLUMN);
            for item in items {
                table = table.row(vec![
                    item.field_str("id"),
                    item.field_str("name"),
                    item.field_str("source"),
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's own pinned set is empty (the desktop shell's own\n",
                "○".dimmed()
            ));
            out.push_str("      pinned list lives in the desktop shell's store, which this\n");
            out.push_str("      backend neither reads nor writes)\n");
        }
    }
    out
}

fn render_batch(value: &Value, ids: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "🧭".blue().bold(),
        "Batch".blue().bold(),
        ids.join(", ")
    ));
    match value {
        Value::List(items) => {
            out.push_str(&format!("  ├─ {} of {} answered\n", items.len(), ids.len()));
            let mut table = Table::new(vec!["ID".into(), "NAME".into(), "SOURCE".into()])
                .cap(1, NAME_COLUMN)
                .cap(2, SOURCE_COLUMN);
            for item in items {
                table = table.row(vec![
                    item.field_str("id"),
                    item.field_str("name"),
                    item.field_str("source"),
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {}
    }
    out
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "🧭".blue().bold(),
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
                truncate_to(&reason, 60),
            ]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    out
}
