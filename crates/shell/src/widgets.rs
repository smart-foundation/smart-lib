//! The rendering widgets the original shell's aesthetic is built from:
//! `make_bar` (the volume/level bar) and the box-drawing table every domain
//! module renders its rows with. Salvaged from the original's `cmd/mod.rs`
//! and its table style.

/// The 10-cell bar the original shell drew for a 0..=1 fraction.
pub fn make_bar(value: f32) -> String {
    let filled = (value * 10.0).round().min(10.0).max(0.0) as usize;
    let empty = 10usize.saturating_sub(filled);
    "█".repeat(filled) + &"░".repeat(empty)
}

/// A box-drawing table over named columns, in the shell's aesthetic:
///
/// ```text
///   ┌──────────────┬────────┐
///   │ NAME         │ TYPE   │
///   ├──────────────┼────────┤
///   │ wlp0s20f3    │ wifi   │
///   └──────────────┴────────┘
/// ```
///
/// Widths are display-width aware for the ASCII the domains emit; a cell
/// wider than its column (a long SSID) simply widens the column.
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Table { headers, rows: Vec::new() }
    }

    pub fn row(mut self, cells: Vec<String>) -> Self {
        self.rows.push(cells);
        self
    }

    fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| display_width(h)).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                let w = display_width(cell);
                if i < widths.len() && w > widths[i] {
                    widths[i] = w;
                }
            }
        }
        widths
    }

    pub fn render(&self) -> String {
        let widths = self.widths();
        let mut out = String::new();
        out.push_str("  ┌");
        out.push_str(&join_segments(&widths, "┬", "─"));
        out.push_str("┐\n");
        out.push_str("  │ ");
        out.push_str(&pad_row(&self.headers, &widths));
        out.push_str(" │\n");
        out.push_str("  ├");
        out.push_str(&join_segments(&widths, "┼", "─"));
        out.push_str("┤\n");
        for row in &self.rows {
            out.push_str("  │ ");
            out.push_str(&pad_row(row, &widths));
            out.push_str(" │\n");
        }
        out.push_str("  └");
        out.push_str(&join_segments(&widths, "┴", "─"));
        out.push('┘');
        out
    }
}

fn join_segments(widths: &[usize], sep: &str, fill: &str) -> String {
    widths
        .iter()
        .map(|w| fill.repeat(w + 2))
        .collect::<Vec<_>>()
        .join(sep)
}

fn pad_row(cells: &[String], widths: &[usize]) -> String {
    let mut padded = Vec::with_capacity(cells.len());
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        padded.push(format!("{}{}", cell, " ".repeat(w.saturating_sub(display_width(cell)))));
    }
    padded.join(" │ ")
}

/// Display width, counting the box-drawing glyphs the shell emits as one
/// column each (they are narrow marks, and the tables stay aligned beside
/// them only when they count as one).
pub fn display_width(s: &str) -> usize {
    s.chars().filter(|c| !is_wide(*c)).count()
}

fn is_wide(c: char) -> bool {
    // The shell's own aesthetic marks (◉ ○ ◌ ▲ ▼ ● ✗ ✓ ⟳) are narrow in every
    // terminal font that renders them; only CJK/fullwidth ranges count wide.
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF |
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF |
        0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F |
        0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x2FFFD | 0x30000..=0x3FFFD)
}
