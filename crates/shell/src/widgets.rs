//! The rendering widgets the original shell's aesthetic is built from:
//! `make_bar` (the volume/level bar) and the box-drawing table every domain
//! module renders its rows with. Salvaged from the original's `cmd/mod.rs`
//! and its table style.
//!
//! ## The width model (why every table is pixel-perfect)
//!
//! The first shell measured a cell by its **byte** length, so a `◉` (three
//! bytes, one column) and a `—` (three bytes, one column) made each row's
//! padding one column short, and the borders drifted apart row by row.
//! Every width in this crate is now a **display width** computed by
//! [`display_width`]:
//!
//! * the glyph table is [`unicode-width`]'s — the maintained, per-Unicode-
//!   release table the whole Rust ecosystem measures with (`console`,
//!   `tabled`, `ratatui`, `clap` all reach for it) — not a hand-rolled range
//!   list that goes stale silently. The hand-rolled list this crate shipped
//!   was wrong in BOTH directions, measured over every scalar: it drew the
//!   emoji transport/symbol marks (`⏩ ⏪ ⏫ ⏬ ⌚ ⏰ ⚡ ⚽ ✅ ✨ ⚓ ⛵ ⛺ ⛲ ⛳ ☔ ☕`
//!   — the media wave's glyphs) **one** column where a terminal draws **two**;
//!   it drew `🏷` (U+1F3F7, Narrow in the UCD's EastAsianWidth) **two** columns
//!   where a terminal draws **one**; and it counted **~12,000 combining and
//!   default-ignorable marks** (`U+093C`, `U+094D`, `U+0E31..U+0E3A`,
//!   `U+1DC0..U+1DFF`, …) **one** column where they ride on their base at
//!   **zero**. Any of those in a cell drifts that row off its own border.
//! * ANSI escapes inside a cell are skipped before the glyphs are measured
//!   (the order `console::measure_text_width` established: strip, then
//!   measure), so a coloured cell measures exactly what the terminal draws;
//! * **East-Asian Ambiguous** — the shell's meaning glyphs: the circle state
//!   family `◉ ○ ● ◌ ◐`, the em dash and ellipsis `— …`, the arrows
//!   `↑ ↓ ↔ ← →`, the `·` and `•` it punctuates with, and the box-drawing
//!   borders themselves — measure **one**, because that is what this terminal
//!   draws them as; only a CJK-locale terminal draws them two, and the
//!   renderer follows its environment so it aligns on both
//!   ([`ambiguous_width`]).
//!
//! The first rule was measured by the CJK oracle (`width_cjk`) and was wrong
//! for this host: it counted every ambiguous mark two columns, so a live table's
//! header came out four columns wider than each of its rows (`smart network`
//! measured 63 against 59). A terminal in a Western locale (`LANG=en_ZA.UTF-8`
//! here, no `LC_ALL`/`LC_CTYPE` override) is what `unicode-width`'s DEFAULT
//! `width()` models: Wide/Fullwidth → 2, Ambiguous → 1.
//!
//! ## Why a cell is content and a style, never a styled string
//!
//! A table cell built as a PRE-STYLED string (`"◉".green()` is
//! `\x1b[32m◉\x1b[0m`) makes width a *guessing* problem: the measure has to
//! find the escapes inside the caller's `String`, and every width function
//! becomes a second parser that can disagree with the padding it feeds. This
//! is the failure the service table shipped with — three widths in one table
//! (76 for the header, 91 for a `◉`-active row, 88 for a `○`-unknown row) —
//! because `\x1b[32m` is one byte longer than `\x1b[2m` and the two were
//! measured inconsistently.
//!
//! So the table never sees a styled string. [`Cell`] is a **content** (what
//! the terminal draws, and the only thing any width is measured on) plus a
//! **style** ([`Sgr`]) that the table applies *after* the column widths are
//! known. The style is therefore structurally incapable of participating in
//! a width computation. The escape-skipping [`display_width`] stays, but as
//! the backstop for the non-table surfaces (the tree views, which format
//! styled text inline), not as the table's width model.
//!
//! [`pad_to`] and [`truncate_to`] are built on the same measure, so padding
//! and ellipsising cannot disagree with it.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// The 10-cell bar the original shell drew for a 0..=1 fraction.
pub fn make_bar(value: f32) -> String {
    let filled = (value * 10.0).round().min(10.0).max(0.0) as usize;
    let empty = 10usize.saturating_sub(filled);
    "█".repeat(filled) + &"░".repeat(empty)
}

/// The style a [`Cell`] wears.
///
/// One style per cell, applied by the renderer once the column widths are
/// fixed — which is why a cell's width is always its content's width, and why
/// no width function ever has to parse an escape. `Sgr::Plain` emits no
/// escape at all, so an unstyled cell is byte-identical to the original's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sgr {
    /// No style: the cell prints exactly its content.
    #[default]
    Plain,
    Red,
    Green,
    Yellow,
    Blue,
    Cyan,
    Bold,
    Dimmed,
}

impl Sgr {
    /// The sequence that ends every styled run.
    pub const RESET: &'static str = "\u{1b}[0m";

    /// The sequence that opens a styled run. `Plain` has none.
    pub fn open(self) -> &'static str {
        match self {
            Sgr::Plain => "",
            Sgr::Red => "\u{1b}[31m",
            Sgr::Green => "\u{1b}[32m",
            Sgr::Yellow => "\u{1b}[33m",
            Sgr::Blue => "\u{1b}[34m",
            Sgr::Cyan => "\u{1b}[36m",
            Sgr::Bold => "\u{1b}[1m",
            Sgr::Dimmed => "\u{1b}[2m",
        }
    }
}

/// One table cell: its **content** — the text the terminal draws, and the only
/// thing any width is measured on — and the **style** it wears, applied after
/// the column widths are known.
///
/// ```
/// use shell::widgets::{Cell, Sgr};
/// // The style is not part of the content, so it is not part of a width:
/// assert_eq!(Cell::styled("◉ active", Sgr::Green).content(), "◉ active");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    content: String,
    style: Sgr,
}

impl Cell {
    /// A cell that prints its content with no style.
    pub fn plain(content: impl Into<String>) -> Cell {
        Cell {
            content: content.into(),
            style: Sgr::Plain,
        }
    }

    /// A cell that prints its content in `style`. The content is still plain
    /// text: the colour is the table's business, applied at render time.
    pub fn styled(content: impl Into<String>, style: Sgr) -> Cell {
        Cell {
            content: content.into(),
            style,
        }
    }

    /// The text the terminal draws — what every width is measured on.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The style the cell wears when the table prints it.
    pub fn style(&self) -> Sgr {
        self.style
    }

    /// Append to the content (the style still covers the whole cell). The
    /// shell's state cells are a glyph and its word in one colour, the way the
    /// network tree view already prints `◉ Online` green.
    pub fn push_text(&mut self, text: &str) {
        self.content.push_str(text);
    }

    /// The builder form of [`Cell::push_text`].
    pub fn with_text(mut self, text: &str) -> Cell {
        self.push_text(text);
        self
    }

    /// The cell as the terminal should draw it: the style opens, the content
    /// prints, the style ends where the text ends, and the pad follows the
    /// reset (so a coloured cell's padding is never coloured).
    fn print(&self, width: usize) -> String {
        let pad = width.saturating_sub(display_width(&self.content));
        match self.style {
            Sgr::Plain => format!("{}{}", self.content, " ".repeat(pad)),
            style => format!(
                "{}{}{}{}",
                style.open(),
                self.content,
                Sgr::RESET,
                " ".repeat(pad)
            ),
        }
    }
}

impl std::fmt::Display for Cell {
    /// The styled cell with no padding — the form a tree view formats inline.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.style {
            Sgr::Plain => f.write_str(&self.content),
            style => write!(f, "{}{}{}", style.open(), self.content, Sgr::RESET),
        }
    }
}

impl From<&str> for Cell {
    fn from(content: &str) -> Cell {
        Cell::plain(content)
    }
}

impl From<String> for Cell {
    fn from(content: String) -> Cell {
        Cell::plain(content)
    }
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
/// Every width is a display width (see the module docs) measured on cell
/// CONTENT, so a cell holding `◉ active` or `—` occupies the same number of
/// terminal columns as the ASCII that shares its column, and the borders hold
/// their alignment.
pub struct Table {
    pub headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Table {
            headers,
            rows: Vec::new(),
        }
    }

    /// Add a row. Anything that becomes a [`Cell`] — plain `String`s, `&str`s,
    /// and [`Cell::styled`] — is accepted, so a caller cannot hand the table a
    /// pre-styled string even by accident.
    pub fn row<I>(mut self, cells: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Cell>,
    {
        self.rows.push(cells.into_iter().map(Into::into).collect());
        self
    }

    fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| display_width(h)).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                let w = display_width(cell.content());
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
        out.push_str(
            &self
                .headers
                .iter()
                .enumerate()
                .map(|(i, h)| pad_to(h, widths.get(i).copied().unwrap_or(0)))
                .collect::<Vec<_>>()
                .join(" │ "),
        );
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

fn pad_row(cells: &[Cell], widths: &[usize]) -> String {
    let mut padded = Vec::with_capacity(cells.len());
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        padded.push(cell.print(w));
    }
    padded.join(" │ ")
}

/// Pad a cell to `width` display columns with trailing spaces. The width is
/// measured on content, so a styled cell keeps its escapes and gains the
/// padding after them: the colour ends where the text ends.
pub fn pad_to(cell: &str, width: usize) -> String {
    let w = display_width(cell);
    if w >= width {
        return cell.to_string();
    }
    format!("{}{}", cell, " ".repeat(width - w))
}

/// Ellipsise a cell to `width` display columns, keeping ANSI escapes intact and
/// marking the cut with `…` (the shell never wraps a table row, and never
/// hides that it truncated: §8.6).
pub fn truncate_to(cell: &str, width: usize) -> String {
    if display_width(cell) <= width {
        return cell.to_string();
    }
    // One column is reserved for the ellipsis, which is one column wide in a
    // Western-terminal locale (an East-Asian ambiguous mark).
    let budget = width.saturating_sub(display_width("…"));
    let mut out = String::new();
    let mut used = 0usize;
    let mut chars = cell.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // The style survives the cut: an escape is copied through, not
            // measured, and never counted against the budget.
            out.push(c);
            copy_escape(&mut chars, &mut out);
            continue;
        }
        let cw = char_width(c);
        if used + cw > budget {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// The display width of a string: the number of terminal columns it occupies.
/// ANSI escapes count zero, and the shell's ambiguous-width glyphs count what
/// this terminal draws them as ([`ambiguous_width`]).
///
/// The measure is the ecosystem's own, in the ecosystem's own order: the
/// escapes come out first (`console::measure_text_width`'s `strip then
/// measure`), then the remaining text goes through [`unicode-width`], which is
/// what the width is for and which carries the per-release Unicode tables a
/// hand-rolled range list cannot.
pub fn display_width(s: &str) -> usize {
    if !s.contains('\u{1b}') {
        return width_str(s);
    }
    let stripped = strip_ansi(s);
    width_str(&stripped)
}

/// Consume an ANSI escape sequence whose leading ESC was already taken: CSI
/// (`ESC [ … final`) runs to its final byte in `0x40..=0x7E`; OSC (`ESC ] …`)
/// runs to BEL or ST; any other two-character escape takes its next char. The
/// bytes carry no width.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            chars.next();
            for c in chars.by_ref() {
                if c == '\u{07}' {
                    break;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
            }
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

/// The same consumption, copying the sequence through so a style survives a
/// truncation.
fn copy_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    match chars.peek().copied() {
        Some('[') => {
            out.push(chars.next().unwrap());
            for c in chars.by_ref() {
                out.push(c);
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            out.push(chars.next().unwrap());
            for c in chars.by_ref() {
                out.push(c);
                if c == '\u{07}' {
                    break;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        out.push(chars.next().unwrap());
                    }
                    break;
                }
            }
        }
        Some(_) => {
            out.push(chars.next().unwrap());
        }
        None => {}
    }
}

/// The text of a cell with its escapes removed: what the terminal draws.
fn strip_ansi(s: &str) -> String {
    if !s.contains('\u{1b}') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
            continue;
        }
        out.push(c);
    }
    out
}

/// The width of already-escape-free text, under this terminal's ambiguous
/// rule.
fn width_str(s: &str) -> usize {
    if ambiguous_width() == 2 {
        s.width_cjk()
    } else {
        s.width()
    }
}

/// One character's display width: the number of columns this terminal draws it
/// as.
///
/// This is the standard [`unicode-width`] rule — East-Asian **Wide**/Fullwidth
/// and emoji-presentation marks two, control characters and combining marks
/// zero, everything else one — not the CJK rule (`width_cjk`) that counts
/// **Ambiguous** marks two as well. A terminal in a Western locale, which is
/// this host's (`LANG=en_ZA.UTF-8`, no `LC_ALL`/`LC_CTYPE` override), draws an
/// ambiguous mark ONE column, so one is what the renderer counts. Under a CJK
/// locale the same shell renders the same glyph two columns, because that is
/// what that terminal draws: [`ambiguous_width`].
pub fn char_width(c: char) -> usize {
    if ambiguous_width() == 2 {
        c.width_cjk()
    } else {
        c.width()
    }
    .unwrap_or(0)
}

/// The width a terminal gives an East-Asian **Ambiguous** character — the
/// class the shell's meaning glyphs and its box-drawing borders belong to.
///
/// The two rules a terminal can play by, and why the renderer picks:
///
/// * **1 column** — the Western rule, what a terminal outside a CJK locale
///   draws (`xterm`, `alacritty`, `kitty`, `foot` in `en_ZA.UTF-8`, this host).
///   It is also `unicode-width`'s DEFAULT `width()`, which is why the previous
///   CJK reading was wrong HERE: it measured `smart network`'s header 63
///   columns against every row's 59.
/// * **2 columns** — the CJK rule, what a terminal in `zh_*` / `ja_*` / `ko_*`
///   renders them as (`width_cjk`). It is not ASCII-era legacy only: a
///   CJK-locale terminal on the same host genuinely draws `◉` two columns.
///
/// The shell reads the locale it is actually running under, so one binary
/// aligns on both hosts instead of guessing. The environment is read once: a
/// caller can never change the width underneath an already-rendered table.
///
/// ```
/// assert_eq!(shell::widgets::ambiguous_width_for(None), 1);
/// assert_eq!(shell::widgets::ambiguous_width_for(Some("en_ZA.UTF-8")), 1);
/// assert_eq!(shell::widgets::ambiguous_width_for(Some("ja_JP.UTF-8")), 2);
/// assert_eq!(shell::widgets::ambiguous_width_for(Some("zh_Hant_TW.UTF-8")), 2);
/// assert_eq!(shell::widgets::ambiguous_width_for(Some("ko_KR.UTF-8")), 2);
/// ```
pub fn ambiguous_width() -> usize {
    static WIDTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WIDTH.get_or_init(|| ambiguous_width_for(ctype_locale().as_deref()))
}

/// The same rule over a locale value rather than the process environment, so
/// the mapping itself can be proven — and so the default is a decision, not an
/// accident of whoever ran the suite.
pub fn ambiguous_width_for(locale: Option<&str>) -> usize {
    match locale {
        Some(l) if is_cjk_locale(l) => 2,
        _ => 1,
    }
}

/// The locale this process is running under, read the way a terminal reads it:
/// `LC_ALL` wins, then `LC_CTYPE`, then `LANG`; an empty value is "not set".
fn ctype_locale() -> Option<String> {
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// A CJK locale: the language whose terminals render an ambiguous mark two
/// columns wide (`zh_Hant_TW`, `ja_JP.UTF-8`, `ko_KR`, …).
fn is_cjk_locale(locale: &str) -> bool {
    let language = locale
        .split(['.', '_', '@', '-'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        language.as_str(),
        "zh" | "ja" | "jp" | "ko" | "yue" | "wuu" | "nan" | "cmn"
    )
}
