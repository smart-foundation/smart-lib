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

/// The pipe-ready PLAIN TEXT form — what `-o raw` prints.
///
/// Raw is the door to the engine's real pipe seam: one line per record with
/// tab-separated fields (a header line first, so `column -t`, `awk` and
/// `sort -k` all work), one line per scalar in a list of scalars, and
/// `key<TAB>value` lines for a record. Nothing is truncated here — the cut is
/// the human renderer's business, never the machine's — and an empty answer
/// prints its shape (the header, or nothing for a scalar list) rather than
/// zero bytes. The rot this replaces: `-o raw` printed **0 bytes for all nine
/// domains**, because every `run()` skipped rendering for the Raw format and
/// the binary dropped the returned value on the floor.
pub fn to_raw(value: &Value) -> String {
    let mut out = String::new();
    write_raw(value, &mut out);
    // A record's lines carry their own newline discipline; a scalar's does not.
    out
}

fn write_raw(value: &Value, out: &mut String) {
    match value {
        Value::List(items) => {
            // A list of records is a table: the first record's keys are the
            // header, every record a row, a missing field an empty cell.
            if let Some(Value::Object(first)) = items.first() {
                let headers: Vec<&str> = first.iter().map(|(k, _)| k.as_str()).collect();
                out.push_str(&headers.join("\t"));
                out.push('\n');
                for item in items {
                    if let Value::Object(pairs) = item {
                        let row: Vec<String> = headers
                            .iter()
                            .map(|h| {
                                pairs
                                    .iter()
                                    .find(|(k, _)| k == h)
                                    .map(|(_, v)| raw_scalar(v))
                                    .unwrap_or_default()
                            })
                            .collect();
                        out.push_str(&row.join("\t"));
                        out.push('\n');
                    }
                }
                return;
            }
            for item in items {
                out.push_str(&raw_scalar(item));
                out.push('\n');
            }
        }
        Value::Object(pairs) => {
            for (k, v) in pairs {
                out.push_str(k);
                out.push('\t');
                out.push_str(&raw_scalar(v));
                out.push('\n');
            }
        }
        scalar => {
            out.push_str(&raw_scalar(scalar));
            out.push('\n');
        }
    }
}

/// One scalar as its pipe-ready text: a nested value keeps its JSON shape
/// (a machine consumer parses it), a string travels verbatim.
fn raw_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::List(_) | Value::Object(_) => {
            let mut out = String::new();
            write_json(value, &mut out);
            out
        }
        scalar => scalar.display(),
    }
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
