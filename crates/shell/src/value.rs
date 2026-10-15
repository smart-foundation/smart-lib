//! Rendering a shell [`Value`]: the pretty form for humans and the JSON form
//! for agents and scripts.
//!
//! The original shell hand-rolled its JSON with `format!` — quoting-unsafe: a
//! value containing a quote or a newline produced invalid JSON. This one
//! serialises the value tree with real escaping, so machine output is
//! contract-shaped over the engine's owned types rather than a hope.

use crate::selector::Value;
use crate::widgets::{truncate_to, Cell, Table};

/// The widest one cell of a piped table may draw. The pipe's value is data, and
/// data a terminal cannot read is not a rendering: a command line 400 columns
/// wide is cut and the cut is marked, exactly as a domain table cuts. The full
/// value is one door away in `-o json`.
const PIPE_CELL_WIDTH: usize = 48;

/// The pretty rendering the REPL prints for a command's result value: a table
/// for a list of records, aligned lines for a record, one line per item for a
/// list of scalars.
pub fn render_pretty(value: &Value) -> String {
    match value {
        Value::List(items) => {
            if items.is_empty() {
                return "  (empty)".to_string();
            }
            if let Some(Value::Object(first)) = items.first() {
                let mut headers: Vec<String> = Vec::new();
                for (k, _) in first {
                    headers.push(k.clone());
                }
                let mut table = Table::new(headers);
                for item in items {
                    if let Value::Object(pairs) = item {
                        let row: Vec<Cell> = first
                            .iter()
                            .map(|(k, _)| {
                                let text = pairs
                                    .iter()
                                    .find(|(pk, _)| pk == k)
                                    .map(|(_, v)| v.display())
                                    .unwrap_or_default();
                                Cell::plain(truncate_to(&text, PIPE_CELL_WIDTH))
                            })
                            .collect();
                        table = table.row(row);
                    }
                }
                return table.render();
            }
            let lines: Vec<String> = items.iter().map(|v| format!("  {}", v.display())).collect();
            lines.join("\n")
        }
        Value::Object(pairs) => {
            let width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            pairs
                .iter()
                .map(|(k, v)| format!("  {:<width$}  {}", k, v.display(), width = width))
                .collect::<Vec<_>>()
                .join("\n")
        }
        scalar => format!("  {}", scalar.display()),
    }
}

/// Contract-shaped JSON over the value tree, with real string escaping.
pub fn to_json(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, &mut out);
    out
}

fn write_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if *n == (*n as i64) as f64 && n.is_finite() {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{n}"));
            }
        }
        Value::String(s) => write_json_string(s, out),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        Value::Object(pairs) => {
            out.push('{');
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                write_json(v, out);
            }
            out.push('}');
        }
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
