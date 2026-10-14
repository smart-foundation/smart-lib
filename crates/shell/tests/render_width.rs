//! The width model, locked: a table whose cells hold the shell's double-width
//! glyphs renders with every border on the same display column, asserted
//! against a hand-written width oracle rather than eyeballed. These tests never
//! touch the host (charter law #8): every render is over the scripted fixtures.

mod common;

use clap::Parser;
use common::{MockClipboard, MockService};
use shell::color::ColorExt;
use shell::domains::clipboard::ClipboardCmd;
use shell::domains::service::{evaluate, render, ServiceCmd, WriteGuard};
use shell::migrate;
use shell::widgets::{display_width, truncate_to, Cell, Sgr, Table};
use shell::{OutputFormat, Value};

// ── the test's own width oracle ─────────────────────────────────────────────
//
// Deliberately hand-written instead of calling back into the renderer: an
// assertion that measures with the code it is testing proves nothing. The
// widths below are the terminal's, and they are what the renderer must match.
//
// The rule, measured on this host (`TERM=xterm-256color`, kitty's terminfo,
// `LANG=en_ZA.UTF-8` with no `LC_ALL`/`LC_CTYPE` override):
//
//   East-Asian **W**ide / **F**ullwidth → **2** columns;
//   **everything else, INCLUDING the East-Asian Ambiguous class → 1**.
//
// The shell's meaning glyphs (`◉ ○ ● ◌ ◐`, `— …`, `↑ ↓ ↔`, `· •`) and its
// box-drawing borders (`│ ─ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`) are Ambiguous. A Western
// terminal draws every one of them a single column, so that is what a table is
// padded to. Only a terminal in a CJK locale (`zh_*`, `ja_*`, `ko_*`) draws
// them two, and that is a locale the renderer switches on
// (`widgets::ambiguous_width_for`) rather than a measurement a table gets
// wrong. The previous reading of this oracle counted the ambiguous glyphs two
// columns — the `width_cjk` rule — and every table it "locked" then disagreed
// with the terminal by one column per glyph: `smart service`'s header measured
// 76 against a `◉`-active row's 91 and a `○`-unknown row's 88.
/// Skip an escape sequence whose leading ESC was already taken: the CSI
/// introducer and its parameter/intermediate bytes, then its final byte. None of
/// it draws a column. (The shell emits CSI colour only; OSC is handled so the
/// oracle stays total.)
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            while let Some(c) = chars.next() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    return;
                }
            }
        }
        Some(']') => {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\u{07}' {
                    return;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    return;
                }
            }
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

fn columns(s: &str) -> usize {
    let mut w = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
            continue;
        }
        w += match c {
            // East-Asian Wide/Fullwidth: the CJK blocks always occupy two
            // columns, in every locale, in every terminal that can draw them.
            '日' | '本' | '語' => 2,
            // Emoji-presentation marks (the domain headers' 📋 🌐 🔗 🔎 📈 📜
            // 🔒 🏷 📐 🚧 🧭 📡 🔌 📶 and ⛔ in a blocked radio's state cell).
            // A terminal that drew these one column would clip them, not
            // narrow them.
            '📋' | '🌐' | '🔗' | '🔎' | '📈' | '📜' | '🔒' | '🏷' | '📐' | '🚧' | '🧭'
            | '📡' | '🔌' | '📶' => 2,
            // Everything else is ONE column on this terminal — ASCII, the
            // box-drawing borders, and the whole East-Asian Ambiguous class
            // the shell's meaning glyphs belong to:
            //   ◉ ○ ● ◌ ◐ (state dots)   ✗ ✓ ⚙ ❤ (marks)
            //   — … (null/ellipsis)        ↑ ↓ ↔ ← → (arrows)
            //   · • █ ░ (punctuation/meter)
            _ => 1,
        };
    }
    w
}

/// Strip the escapes so a border glyph can be located in the plain text.
fn plain(s: &str) -> String {
    let mut out = String::new();
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

/// The display column of every border glyph on a line.
fn border_columns(line: &str) -> Vec<usize> {
    let p = plain(line);
    let mut out = Vec::new();
    let mut w = 0usize;
    for c in p.chars() {
        if matches!(c, '│' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼') {
            out.push(w);
        }
        w += columns(&c.to_string());
    }
    out
}

/// Assert the whole rendering is pixel-perfect: every boxed line is the same
/// display width, and every border glyph sits at the same display column.
fn assert_borders_hold(rendered: &str) {
    let lines: Vec<&str> = rendered
        .lines()
        .filter(|l| l.contains('│') || l.contains('┌') || l.contains('└'))
        .collect();
    assert!(!lines.is_empty(), "the rendering must contain a table: {rendered}");
    let mut widths = Vec::new();
    let mut layouts = Vec::new();
    for line in &lines {
        widths.push(columns(line));
        layouts.push(border_columns(line));
    }
    let first_w = widths[0];
    for (i, w) in widths.iter().enumerate() {
        assert_eq!(
            *w, first_w,
            "line {i} is {w} columns wide, but the table is {first_w}: {}",
            lines[i]
        );
    }
    let first_l = &layouts[0];
    for (i, l) in layouts.iter().enumerate() {
        assert_eq!(
            l, first_l,
            "line {i}'s borders sit at {l:?}, not {first_l:?}: {}",
            lines[i]
        );
    }
}

/// The width model itself: a coloured cell measures what the terminal draws,
/// and the shell's ambiguous glyphs measure one column so the borders span
/// exactly the cells they close.
#[test]
fn display_width_counts_the_glyphs_the_shell_draws() {
    assert_eq!(display_width("NetworkManager"), 14);
    assert_eq!(display_width("◉"), 1, "the state dot is one column here");
    assert_eq!(display_width("○"), 1, "the void dot is one column here");
    assert_eq!(display_width("—"), 1, "the null mark is one column here");
    assert_eq!(display_width("↑↓"), 2, "the link arrows are one each");
    assert_eq!(display_width("─"), 1, "a border fill is one column");
    assert_eq!(display_width("│"), 1, "a border wall is one column");
    assert_eq!(display_width("日本語"), 6, "East-Asian wide marks are two");
    assert_eq!(display_width("📋"), 2, "an emoji-presentation mark is two");
    assert_eq!(display_width("⛔"), 2, "a blocked-radio mark is Wide (U+26D4), like every mark a blocked radio's state cell carries");
    // The emoji transport block — the media wave's glyphs — is Wide too: a
    // hand-rolled range list measured these ONE column and grew a row two
    // columns past its border.
    assert_eq!(display_width("⏩"), 2, "the transport marks are Wide");
    assert_eq!(display_width("⌚"), 2, "the watch mark is Wide");
    // Combining marks and joiners ride on their base: this is the ~12,000-mark
    // hole in the old list, which measured them one column each.
    assert_eq!(display_width("e\u{301}"), 1, "a combining accent is zero");
    assert_eq!(display_width("\u{093C}"), 0, "a Devanagari nukta is zero");
    assert_eq!(display_width("\u{1DC0}"), 0, "a supplemental combining mark is zero");
    assert_eq!(display_width("\u{fe0f}"), 0, "a variation selector is zero");
    // U+1F3F7 is Narrow in the UCD's EastAsianWidth: the old list drew the
    // clipboard's own label mark two columns.
    assert_eq!(display_width("\u{1F3F7}"), 1, "the label mark is Narrow");
    // A coloured cell measures its text, not its escapes — and a GREEN escape
    // is one byte longer than a DIM one, which is exactly the pair that used to
    // disagree with each other.
    assert_eq!(display_width("\u{1b}[32m◉\u{1b}[0m"), 1);
    assert_eq!(display_width("\u{1b}[2m○\u{1b}[0m"), 1);
    assert_eq!(display_width("\u{1b}[32m◉\u{1b}[0m"), display_width("\u{1b}[2m○\u{1b}[0m"));
    assert_eq!(display_width("\u{1b}[1m\u{1b}[34mServices\u{1b}[0m\u{1b}[0m"), 8);
    assert_eq!(display_width("◉".green().as_str()), 1);
    // Truncating marks the cut and never exceeds the bound. The ellipsis is
    // ONE column here (an East-Asian ambiguous mark), so 8 columns is 7
    // characters plus the mark — under the old CJK reading the ellipsis was
    // two columns and the bound held a column of text back.
    assert_eq!(truncate_to("abcdefgh", 8), "abcdefgh");
    assert_eq!(truncate_to("abcdefghij", 8), "abcdefg…");
    assert_eq!(display_width(&truncate_to("abcdefghij", 8)), 8);
    assert_eq!(display_width(&truncate_to("◉◉◉◉◉◉◉◉", 8)), 8);
}

/// THE lock on the architecture: a cell is CONTENT plus a STYLE, so the style
/// cannot reach a width computation. The escape exists only in what the table
/// prints, never in what it measures.
#[test]
fn a_cells_style_is_not_part_of_its_width() {
    let cell = Cell::styled("◉ active", Sgr::Green);
    // The content is plain text: no escape ever enters it.
    assert_eq!(cell.content(), "◉ active");
    assert!(!cell.content().contains('\u{1b}'), "{}", cell.content());
    // …so the cell measures exactly its content, and the painted form still
    // measures the same number of columns (the escapes draw nothing).
    assert_eq!(display_width(cell.content()), 8);
    assert_eq!(columns(&cell.to_string()), columns(cell.content()));
    assert_eq!(columns(&cell.to_string()), 8);
    // A plain cell carries no escape at all.
    assert_eq!(columns(&Cell::plain("wlp0s20f3").to_string()), 9);
    // The style is applied, not hidden: the painted cell opens with the
    // colour and ends with the reset.
    assert!(cell.to_string().starts_with("\u{1b}[32m"), "{}", cell.to_string());
    assert!(cell.to_string().ends_with("\u{1b}[0m"), "{}", cell.to_string());
}

/// The locale switch is the ONLY thing that moves an ambiguous glyph to two
/// columns, and it moves every ambiguous glyph together — so a CJK-locale
/// terminal still gets a table whose borders hold, one that is simply two
/// columns wider per glyph.
#[test]
fn the_ambiguous_rule_follows_the_locale() {
    use shell::widgets::{ambiguous_width_for, char_width};
    assert_eq!(ambiguous_width_for(None), 1, "an unread environment defaults Western");
    assert_eq!(ambiguous_width_for(Some("")), 1, "an empty value is not set");
    assert_eq!(ambiguous_width_for(Some("C")), 1);
    assert_eq!(ambiguous_width_for(Some("en_ZA.UTF-8")), 1, "this host");
    assert_eq!(ambiguous_width_for(Some("ja_JP.UTF-8")), 2);
    assert_eq!(ambiguous_width_for(Some("zh_Hant_TW.UTF-8")), 2);
    assert_eq!(ambiguous_width_for(Some("ko_KR.UTF-8")), 2);
    // The character rule tracks it: one host, one table.
    assert_eq!(char_width('◉'), ambiguous_width_for(Some("en_ZA.UTF-8")));
    assert_eq!(char_width('日'), 2, "a wide mark is two in every locale");
}

/// THE lock: a table rendered with known double-width content — a state dot, a
/// null dash, and a coloured cell — produces border-aligned output. Every row
/// holds the same borders, so the columns do not drift.
#[test]
fn a_table_with_double_width_cells_is_border_aligned() {
    let table = Table::new(vec![
        "NAME".into(),
        "STATE".into(),
        "SINCE".into(),
        "TEXT".into(),
    ])
    .row(vec![
        Cell::plain("NetworkManager"),
        Cell::plain("◉ active"),
        Cell::plain("1d"),
        Cell::plain("a wide cell"),
    ])
    .row(vec![
        Cell::plain("agetty-console"),
        Cell::plain("○ unknown"),
        Cell::plain("—"),
        Cell::plain("日本語"),
    ])
    .row(vec![
        Cell::plain("dhcpcd"),
        Cell::plain("✗ failed"),
        Cell::plain("—"),
        Cell::styled("◉", Sgr::Green),
    ]);
    let rendered = table.render();
    assert_borders_hold(&rendered);
    // The expectations are explicit, not just self-consistent: the STATE column
    // is `○ unknown` wide (9 display columns) plus its two cell gutters.
    assert!(
        rendered.contains("│ agetty-console │ ○ unknown │"),
        "{rendered}"
    );
    assert!(
        rendered.contains("◉ active  │"),
        "the wide glyph is followed by one pad column, not two: {rendered}"
    );
    assert!(
        rendered.contains("│ dhcpcd         │ ✗ failed  │"),
        "{rendered}"
    );
    assert!(rendered.contains("│ —     │"), "the null mark holds its column: {rendered}");
    assert!(rendered.contains("日本語      │"), "{rendered}");
    // The style is applied at render time, so the last cell paints after the
    // widths are fixed and its escape sits outside the measured content.
    assert!(
        rendered.contains("│ \u{1b}[32m◉\u{1b}[0m"),
        "the styled cell paints its glyph and resets: {rendered:?}"
    );
}

/// The real service table over the scripted supervisor — the shape the live
/// host renders — holds its borders with the glyphs mixed in.
#[test]
fn the_service_table_holds_its_borders() {
    let backend = MockService::default();
    let value = evaluate(
        &ServiceCmd::try_parse_from(["service"]).unwrap(),
        &backend,
        WriteGuard { allowed: false },
    )
    .unwrap();
    let rendered = render::render_action(
        &ServiceCmd::try_parse_from(["service"]).unwrap(),
        &value,
        OutputFormat::Human,
    );
    assert_borders_hold(&rendered);
    // Glyph and word, never a bare dot (colour never carries meaning alone).
    // The escapes are stripped first: the colour ends between the glyph and
    // its word, and the assertion is about what a reader sees.
    let seen = plain(&rendered);
    assert!(seen.contains("◉ active"), "{rendered}");
    assert!(seen.contains("○ inactive"), "{rendered}");
    assert!(seen.contains("✗ failed"), "{rendered}");
    // The null mark in SINCE is rendered, and the column holds its width.
    assert!(rendered.contains("—"), "{rendered}");
    // The measured summary line, not a single rounded count.
    assert!(
        rendered.contains("2 active · 4 supervised · 1 failed — 1 row(s) not supervised"),
        "{rendered}"
    );
}

/// The trailer names the ONE physically-privileged half with the command that
/// reads it — and it separates the two measured reasons a row could answer
/// `unknown` with: a credential wall (named as one) and a host that does not
/// supervise the service (named as that). The stale blanket denial is gone.
#[test]
fn the_trailer_names_the_privileged_half_instead_of_denying_the_read() {
    let cmd = ServiceCmd::try_parse_from(["service"]).unwrap();

    // A credential wall, built as the value tree the engine would hand over.
    let blocked = Value::List(vec![unknown_row("cold", "no-privilege: the supervisor's state files are not readable by this caller")]);
    let rendered = render::render_action(&cmd, &blocked, OutputFormat::Human);
    assert!(
        rendered.contains("the supervisor's run state is not readable by this caller"),
        "{rendered}"
    );
    assert!(
        rendered.contains("service-state-probe --state-root /run/runit"),
        "the privileged half points at its own reader: {rendered}"
    );

    // A host that simply does not supervise the service: every uid reads that
    // state, and the sentence says so instead of blaming the caller.
    let unsup = Value::List(vec![unknown_row("agetty-console", "not-found: /run/runit/supervise.agetty-console/stat: No such file or directory")]);
    let rendered = render::render_action(&cmd, &unsup, OutputFormat::Human);
    assert!(
        rendered.contains("real states for every uid"),
        "{rendered}"
    );
    assert!(
        rendered.contains("not supervised on this host"),
        "{rendered}"
    );
    assert!(
        rendered.contains("service-state-probe --state-root /run/runit"),
        "the privileged half points at its own reader in both cases: {rendered}"
    );
    assert!(
        !rendered.contains("not readable by this caller"),
        "the stale blanket denial must be gone: {rendered}"
    );
    assert_borders_hold(&rendered);
}

/// One `unknown` row, shaped exactly as the engine's own instance value.
fn unknown_row(name: &str, reason: &str) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("description".into(), Value::String(String::new())),
        ("state".into(), Value::String("unknown".to_string())),
        ("state_reason".into(), Value::String(reason.to_string())),
        ("pid".into(), Value::Number(0.0)),
        ("since_us".into(), Value::Number(0.0)),
        ("since_valid".into(), Value::Bool(false)),
        ("enabled".into(), Value::Bool(false)),
        ("on_demand".into(), Value::Bool(false)),
        ("supervised".into(), Value::Bool(false)),
        ("declared".into(), Value::Bool(true)),
    ])
}

/// The health table, the second table in this domain, holds its borders too.
#[test]
fn the_health_table_holds_its_borders() {
    let backend = MockService::default();
    let cmd = ServiceCmd::try_parse_from(["service", "health"]).unwrap();
    let value = evaluate(&cmd, &backend, WriteGuard { allowed: false }).unwrap();
    let rendered = render::render_action(&cmd, &value, OutputFormat::Human);
    assert_borders_hold(&rendered);
    assert!(plain(&rendered).contains("◉ online"), "{rendered}");
}

/// The clipboard ledger's TEXT column is bounded, so one long copy cannot blow
/// the table open — and the cut is marked rather than hidden.
#[test]
fn the_clipboard_ledger_text_column_is_bounded() {
    let backend = MockClipboard::seat();
    backend.history.borrow_mut().push(common::MockItem {
        id: 1,
        text: "x".repeat(400),
        pinned: false,
    });
    let cmd = ClipboardCmd::try_parse_from(["clipboard", "history"]).unwrap();
    let value = shell::domains::clipboard::evaluate(&cmd, &backend, shell::domains::clipboard::WriteGuard { allowed: false }).unwrap();
    let rendered = shell::domains::clipboard::render::render_action(
        &cmd,
        &value,
        OutputFormat::Human,
    );
    assert_borders_hold(&rendered);
    let widest = rendered
        .lines()
        .filter(|l| l.contains('│'))
        .map(columns)
        .max()
        .unwrap_or(0);
    assert!(
        widest < 90,
        "the ledger's table stays a panel, not a wall: {widest} columns"
    );
    assert!(rendered.contains('…'), "the cut is marked: {rendered}");
}

/// `network status` is a tree, not a box — but its columns are columns: the
/// state cell is padded to the widest state (a blocked radio's), so the
/// `hard=` / `soft=` boxes line up between an idle radio and a blocked one.
/// The old `{:<9}` counted the escape bytes AND never fired at all, because
/// even the shortest state is already 9 columns of CONTENT.
#[test]
fn the_status_view_aligns_a_blocked_radio_with_an_idle_one() {
    let value = Value::Object(vec![
        ("networking".into(), Value::Bool(true)),
        (
            "radios".into(),
            Value::List(vec![
                radio("wifi", false, false, true),
                radio("cellular", true, false, false),
                radio("gps", false, true, false),
            ]),
        ),
        (
            "interfaces".into(),
            Value::List(vec![iface("enp0s31f6", "ethernet", false)]),
        ),
    ]);
    let rendered = shell::domains::network::render::render_status_view(&value);
    // Every radio line puts `hard=` on the same display column: the state is
    // padded to the widest state, so a 14-column `⛔ hard-blocked` and a
    // 9-column `◉ enabled` end on the same column.
    let hard_at: Vec<usize> = rendered
        .lines()
        .filter(|l| l.contains("hard="))
        .map(|l| column_of(l, "hard=").expect("the radio line carries its boxes"))
        .collect();
    assert_eq!(hard_at.len(), 3, "{rendered}");
    assert!(
        hard_at.windows(2).all(|w| w[0] == w[1]),
        "the hard= column moved between radios: {hard_at:?}\n{rendered}"
    );
    assert!(plain(&rendered).contains("⛔ hard-blocked"), "{rendered}");
}

fn radio(name: &str, hard: bool, soft: bool, enabled: bool) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("hard_blocked".into(), Value::Bool(hard)),
        ("soft_blocked".into(), Value::Bool(soft)),
        ("enabled".into(), Value::Bool(enabled)),
    ])
}

fn iface(name: &str, kind: &str, connected: bool) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("type".into(), Value::String(kind.to_string())),
        ("connected".into(), Value::Bool(connected)),
        ("host_state".into(), Value::String("carrier-down".to_string())),
        ("host_state_source".into(), Value::String("kernel".to_string())),
    ])
}

/// The display column at which `needle` first appears on `line` (escapes
/// stripped first, so the answer is where the terminal puts it).
fn column_of(line: &str, needle: &str) -> Option<usize> {
    let p = plain(line);
    let byte = p.find(needle)?;
    let mut w = 0usize;
    for c in p[..byte].chars() {
        w += columns(&c.to_string());
    }
    Some(w)
}

/// The migration board is a table too: its every line holds its borders.
#[test]
fn the_migration_board_holds_its_borders() {
    let rendered = migrate::render_queue(&OutputFormat::Human);
    assert_borders_hold(&rendered);
    assert!(rendered.contains("clipboard"), "{rendered}");
}

/// The board's counts are the engine's own availability tables, tallied — so
/// the sentence `smart migrate` prints cannot drift from the matrix the engine
/// ships. This is the post-finish board: clipboard 5/15/0, service 7/12/10.
#[test]
fn the_boards_counts_are_the_engines_own_availability_tallies() {
    use smart_lib::domains::clipboard::availability::{
        AvailStatus as C, CLIPBOARD_AVAILABILITY,
    };
    use smart_lib::domains::service::availability::{
        AvailStatus as S, SERVICE_AVAILABILITY,
    };

    let mut clipboard = (0usize, 0usize, 0usize);
    for row in CLIPBOARD_AVAILABILITY {
        match row.status {
            C::Real => clipboard.0 += 1,
            C::Degraded => clipboard.1 += 1,
            C::Unavailable => clipboard.2 += 1,
        }
    }
    let mut service = (0usize, 0usize, 0usize);
    for row in SERVICE_AVAILABILITY {
        match row.status {
            S::Real => service.0 += 1,
            S::Degraded => service.1 += 1,
            S::Unavailable => service.2 += 1,
        }
    }
    assert_eq!(clipboard, (5, 15, 0), "clipboard's matrix: 5 REAL / 15 DEGRADED / 0 UNAVAILABLE");
    assert_eq!(service, (7, 12, 10), "service's matrix: 7 REAL / 12 DEGRADED / 10 UNAVAILABLE");

    let mut board = migrate::QUEUE.iter();
    let clipboard_row = board
        .find(|d| d.name == "clipboard")
        .expect("clipboard is on the board");
    assert_eq!(
        clipboard_row.count,
        format!(
            "native · {} REAL / {} DEGRADED / {} UNAVAILABLE",
            clipboard.0, clipboard.1, clipboard.2
        ),
        "the board states the engine's own tally"
    );
    assert_eq!(clipboard_row.status, migrate::Status::RealToday);

    let service_row = board
        .find(|d| d.name == "service")
        .expect("service is on the board");
    assert_eq!(
        service_row.count,
        format!(
            "native · {} REAL / {} DEGRADED / {} UNAVAILABLE",
            service.0, service.1, service.2
        )
    );
    assert_eq!(service_row.status, migrate::Status::RealToday);
    // The one privileged half is named on the board, not deferred.
    assert!(
        service_row.archive.contains("service-state-probe"),
        "{}",
        service_row.archive
    );
}
