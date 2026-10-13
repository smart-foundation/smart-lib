//! The clipboard domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the ENGINE'S OWNED TYPES
//! (`ContentOutcome`, `ClipboardItem`, `ClipboardPreview`, `ClipboardSizeLimits`)
//! — there is no capnp reader here, so nothing can leak and nothing can
//! desynchronise from what the engine read off the live selection.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::Table;
use crate::OutputFormat;

use smart_lib::domains::clipboard::types::{ClipboardSizeLimits, Pagination};
use smart_lib::domains::clipboard::ClipboardBackend;

/// The data half of `clipboard` / `clipboard status`: the transport, whether the
/// live selection can be read, the current text content, the offered MIME types
/// and the history ledger — the tree `@clipboard/*` selectors resolve from.
pub fn status_value(backend: &dyn ClipboardBackend) -> Result<Value, crate::error::SmartError> {
    use smart_lib::domains::clipboard::types::ContentOutcome;
    let (transport, can_read, can_write) = backend.clipboard_caps();

    let content = match backend.get_content("text/plain") {
        Ok(ContentOutcome::Content(c)) => content_value(&c.mime_type, &c.data),
        Ok(ContentOutcome::Empty) => Value::Null,
        // An honest unavailable read keeps status answering the rest of the tree.
        Err(_) => Value::Null,
    };

    let mimes = match backend.get_supported_mime_types() {
        Ok(list) => Value::List(list.into_iter().map(Value::String).collect()),
        Err(_) => Value::List(Vec::new()),
    };

    let history = history_value(backend, Pagination { offset: 0, limit: u32::MAX })?;

    Ok(Value::Object(vec![
        ("transport".into(), Value::String(transport)),
        ("available".into(), Value::Bool(can_read)),
        ("writable".into(), Value::Bool(can_write)),
        ("content".into(), content),
        ("mimes".into(), mimes),
        ("history".into(), history),
    ]))
}

/// One content value (a `get`/`convert`/`content` result): the MIME type, the
/// measured size, and the payload as text when it is UTF-8 (a binary payload
/// reports an empty text rather than a lossy replacement).
pub fn content_value(mime_type: &str, data: &[u8]) -> Value {
    let text = core::str::from_utf8(data).unwrap_or("").to_string();
    Value::Object(vec![
        ("mime_type".into(), Value::String(mime_type.to_string())),
        ("size_bytes".into(), Value::Number(data.len() as f64)),
        ("text".into(), Value::String(text)),
    ])
}

/// The history ledger as a list of records.
pub fn history_value(
    backend: &dyn ClipboardBackend,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let history = backend.get_history(page)?;
    Ok(Value::List(history.items.iter().map(item_value).collect()))
}

/// A history search as a list of records.
pub fn search_value(
    backend: &dyn ClipboardBackend,
    query: &str,
    page: Pagination,
) -> Result<Value, crate::error::SmartError> {
    let hits = backend.search_history(query, page)?;
    Ok(Value::List(hits.items.iter().map(item_value).collect()))
}

/// The offered MIME types as a list.
pub fn mimes_value(backend: &dyn ClipboardBackend) -> Result<Value, crate::error::SmartError> {
    let mimes = backend.get_supported_mime_types()?;
    Ok(Value::List(mimes.into_iter().map(Value::String).collect()))
}

/// A preview verb's value.
pub fn preview_value(backend: &dyn ClipboardBackend, id: u64) -> Result<Value, crate::error::SmartError> {
    use smart_lib::core::ids::ClipboardId;
    use smart_lib::domains::clipboard::types::PreviewOutcome;
    match backend.get_preview(ClipboardId::new(id))? {
        PreviewOutcome::Preview(p) => Ok(Value::Object(vec![
            ("text_preview".into(), Value::String(p.text_preview.clone())),
            ("available".into(), Value::Bool(p.available)),
            ("original_size".into(), Value::Number(p.original_size as f64)),
            ("preview_mime_type".into(), Value::String(p.preview_mime_type.clone())),
        ])),
        PreviewOutcome::Unavailable => Ok(Value::Null),
    }
}

/// A size-limits verb's value.
pub fn limits_value(backend: &dyn ClipboardBackend) -> Result<Value, crate::error::SmartError> {
    let l: ClipboardSizeLimits = backend.get_size_limits()?;
    Ok(Value::Object(vec![
        ("max_item_size".into(), Value::Number(l.max_item_size as f64)),
        ("max_history_size".into(), Value::Number(l.max_history_size as f64)),
        ("max_history_count".into(), Value::Number(l.max_history_count as f64)),
    ]))
}

/// A mutating verb's echo (its rendering is human-only; a pipe sees the record).
pub fn verb_value(verb: &str, result: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("result".into(), Value::String(result.to_string())),
    ])
}

fn item_value(item: &smart_lib::domains::clipboard::types::ClipboardItem) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(item.id.get() as f64)),
        ("timestamp".into(), Value::Number(item.timestamp as f64)),
        (
            "mime_types".into(),
            Value::List(item.mime_types.iter().cloned().map(Value::String).collect()),
        ),
        ("pinned".into(), Value::Bool(item.pinned)),
        ("size_bytes".into(), Value::Number(item.size_bytes as f64)),
        ("text".into(), Value::String(item.text.clone())),
    ])
}

fn transport_state(transport: &str, can_read: bool) -> String {
    if can_read {
        format!("◉ {}", transport.green())
    } else if transport == "none" {
        "○ none — no native selection transport".dimmed()
    } else {
        format!("◉ {} (read-only from this backend)", transport.green())
    }
}

/// Render one command's value in the requested format.
pub fn render_action(cmd: &super::ClipboardCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::ClipboardCmd, value: &Value) -> String {
    use super::ClipboardAction as A;
    match &cmd.action {
        None | Some(A::Status) => render_status_view(value),
        Some(A::Get { .. }) => render_content(value, "Clipboard"),
        Some(A::Set { .. }) | Some(A::Clear) => render_verb(value),
        Some(A::History { .. }) => render_history(value),
        Some(A::Types) => render_types(value),
        Some(A::Preview { .. }) => render_content(value, "Preview"),
        Some(A::Validate { .. }) => render_verb(value),
        Some(A::Convert { .. }) => render_content(value, "Converted"),
        Some(A::Limits) => render_limits(value),
    }
}

fn render_status_view(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📋".blue().bold(), "Clipboard".blue().bold()));
    out.push_str(&format!(
        "  ├─ Transport   {}\n",
        transport_state(value.field_str("transport").as_str(), value.field_bool("available"))
    ));
    out.push_str(&format!(
        "  ├─ Readable    {}\n",
        if value.field_bool("available") {
            "◉ yes".green()
        } else {
            "○ no — honest Unavailable (no native selection transport on this host)"
                .dimmed()
        }
    ));
    match value.field("content") {
        Value::Null => out.push_str(&format!("  └─ Content     {}\n", "○ empty (no selection owner)".dimmed())),
        content => {
            out.push_str(&format!("  └─ Content     {}\n", "◉ present".green()));
            out.push_str(&render_content_block(content, "     "));
        }
    }
    // Trim the trailing newline of the last block for a tidy tree.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn render_content(value: &Value, title: &str) -> String {
    match value {
        Value::Null => format!("  {} {} — ○ empty (the clipboard holds nothing)", "📋".cyan(), title),
        obj => format!("  {} {}\n{}", "📋".cyan(), title, render_content_block(obj, "  ├─ ")),
    }
}

fn render_content_block(content: &Value, indent: &str) -> String {
    let mime = content.field_str("mime_type");
    let size = content.field_str("size_bytes");
    let text = content.field_str("text");
    let head: String = text.chars().take(60).collect();
    let shown = if text.chars().count() > 60 { format!("{head}…") } else { head };
    out_block(indent, &format!("mime {mime}"), &format!("{size} bytes"))
        + &out_line(indent, &format!("text {shown}"))
}

fn out_block(indent: &str, first: &str, second: &str) -> String {
    format!("{indent}{first}\n{indent}{second}\n")
}

fn out_line(indent: &str, line: &str) -> String {
    format!("{indent}{line}\n")
}

fn render_history(value: &Value) -> String {
    let Value::List(items) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📋".blue().bold(), "Copy History".blue().bold()));
    if items.is_empty() {
        out.push_str(&format!(
            "  └─ {} this process has made no copy yet (the ledger is per-process, not the seat's)\n",
            "○".dimmed()
        ));
        return out;
    }
    let mut table = Table::new(vec![
        "ID".into(),
        "SIZE".into(),
        "PINNED".into(),
        "MIME".into(),
        "TEXT".into(),
    ]);
    for item in items {
        let f = |k: &str| item.field_str(k);
        let mimes = match item.field("mime_types") {
            Value::List(list) => list
                .iter()
                .map(|v| v.display())
                .collect::<Vec<_>>()
                .join(","),
            _ => String::new(),
        };
        table = table.row(vec![
            f("id"),
            f("size_bytes"),
            if item.field_bool("pinned") { "◉".to_string() } else { "○".to_string() },
            mimes,
            f("text"),
        ]);
    }
    out.push_str(&table.render());
    out.push('\n');
    out
}

fn render_types(value: &Value) -> String {
    match value {
        Value::List(items) if items.is_empty() => format!(
            "  {} no MIME targets to report (no selection owner, and this process has made no copy)",
            "○".dimmed()
        ),
        Value::List(items) => {
            let mut out = String::new();
            out.push_str(&format!("  {} {}\n", "🏷️".cyan(), "Offered MIME types".blue().bold()));
            for item in items {
                out.push_str(&format!("  ├─ {}\n", item.display()));
            }
            while out.ends_with('\n') {
                out.pop();
            }
            out
        }
        _ => String::new(),
    }
}

fn render_limits(value: &Value) -> String {
    let f = |k: &str| value.field_str(k);
    format!(
        "  {} Size Limits (this backend's own policy — DEGRADED, not a host fact)\n  ├─ max item     {}\n  ├─ max history  {}\n  └─ max count    {}",
        "📐".cyan(),
        f("max_item_size"),
        f("max_history_size"),
        f("max_history_count"),
    )
}

fn render_verb(value: &Value) -> String {
    let verb = value.field_str("verb");
    let result = value.field_str("result");
    format!("  {} {} — {}", "✓".green(), verb.green(), result)
}
