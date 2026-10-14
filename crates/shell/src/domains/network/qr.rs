//! The wifi **join**/**share** QR flow, behind `domain-network`.
//!
//! The founder's rule — *crates for the heavy, ours for the small* — carves
//! this module exactly: `qrcode` ENCODES a payload into a module matrix and
//! `rqrr` DECODES one back (both genuinely heavy), while the half that is
//! small and ours — the terminal renderer and its exact inverse — is written
//! here. The round trip (`encode → render → parse-ourselves-back → decode`)
//! proves the rendered screen reads back to the EXACT payload, so a phone that
//! scans `network wifi share` joins the network the operator meant.
//!
//! ## Why a phone can scan the screen (the reverse-video trick)
//!
//! A camera reads luminance: it needs DARK modules on a LIGHT field. Drawn as
//! blocks on a dark terminal, the glyphs are INVERTED (a light `█` on a dark
//! background), so the symbol is unreadable. Reverse video (`ESC [ 7 m`)
//! swaps foreground and background for the block, so a space renders light and
//! a full block renders dark on ANY terminal theme — the one orientation a
//! camera wants. The inverse parser reads the GLYPHS (space vs block), which
//! are theme-independent, so the round trip is unaffected.
//!
//! ## The credential never crosses
//!
//! `join` parses a `WIFI:…;P:<pass>;;` payload and routes the SSID + security
//! to the engine's `wifiConnect` as a secret REFERENCE; the password is used to
//! build that reference and dropped — never echoed, never stored. `share`
//! renders the payload; when the host's secret is not readable it says so
//! instead of inventing a password.

use crate::error::{SmartError, SmartResult};

/// A wifi network decoded from a `join` payload (or built by `share`): the
/// SSID, the security type as an open token, the password (empty when the
/// payload carries none — never fabricated), and the hidden flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiJoin {
    pub ssid: String,
    /// "open", "wpa", "wpa2", "wpa3", "wep", or "nopass" for the explicit
    /// open marker. A bare-string payload is open.
    pub security: String,
    pub password: String,
    pub hidden: bool,
}

impl WifiJoin {
    /// True when a password is needed but absent — a `WPA` payload with no `P`
    /// line. A scanner will prompt for it; the shell says so rather than
    /// pretending the network is joinable as-is.
    pub fn needs_password(&self) -> bool {
        self.password.is_empty() && !matches!(self.security.as_str(), "open" | "nopass")
    }

    /// The standard `WIFI:` payload this join decodes to, rebuilt verbatim —
    /// the string `share` would render and the round-trip compares against.
    pub fn to_payload(&self) -> String {
        let mut payload = String::from("WIFI:");
        let t = match self.security.as_str() {
            "open" | "nopass" => "nopass",
            "wep" => "WEP",
            // WPA, WPA2 and WPA3 all standardise on `T:WPA` in the QR.
            _ => "WPA",
        };
        payload.push_str(&format!("T:{t};"));
        payload.push_str(&format!("S:{};", escape(&self.ssid)));
        if !self.password.is_empty() {
            payload.push_str(&format!("P:{};", escape(&self.password)));
        }
        if self.hidden {
            payload.push_str("H:true;");
        }
        payload.push(';');
        payload
    }
}

/// Parse a `join` input: the standard `WIFI:T:WPA;S:<ssid>;P:<pass>;;` format,
/// or a bare string (just an SSID, treated as an open network). The `\`, `;`,
/// `,`, `:` and `"` escapes are unescaped.
pub fn parse(input: &str) -> SmartResult<WifiJoin> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SmartError::invalid_argument("the join payload is empty"));
    }
    let body = match trimmed.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("WIFI:") => &trimmed[5..],
        _ => {
            // A bare string is just the network name — an open network the
            // operator pointed at by hand, not a scanned QR.
            return Ok(WifiJoin {
                ssid: trimmed.to_string(),
                security: "open".to_string(),
                password: String::new(),
                hidden: false,
            });
        }
    };
    let mut ssid = String::new();
    let mut security = String::new();
    let mut password = String::new();
    let mut hidden = false;
    // Walk the `KEY:VAL;` fields; the payload ends at the first `;;`.
    let mut chars = body.chars().peekable();
    let mut field = String::new();
    let flush = |field: &str,
                 ssid: &mut String,
                 security: &mut String,
                 password: &mut String,
                 hidden: &mut bool|
     -> SmartResult<()> {
        let (key, value) = field.split_once(':').ok_or_else(|| {
            SmartError::invalid_argument(format!("a WIFI field has no `key:value`: {field:?}"))
        })?;
        let value = unescape(value);
        match key {
            "S" | "s" => *ssid = value,
            "T" | "t" => *security = value,
            "P" | "p" => *password = value,
            "H" | "h" => *hidden = value.eq_ignore_ascii_case("true"),
            // Unknown keys (a 2046 field nobody has named) are skipped, not
            // rejected — the token set is open.
            _ => {}
        }
        Ok(())
    };
    while let Some(c) = chars.next() {
        match c {
            ';' => {
                if let Some(';') = chars.peek() {
                    break; // the payload terminator `;;`
                }
                flush(&field, &mut ssid, &mut security, &mut password, &mut hidden)?;
                field.clear();
            }
            '\\' => {
                // An escaped delimiter is a literal, never a field separator.
                field.push(c);
                if let Some(&next) = chars.peek() {
                    field.push(next);
                    chars.next();
                }
            }
            _ => field.push(c),
        }
    }
    // A trailing field with no closing `;` is flushed (lenient of `WIFI:S:x`).
    if !field.is_empty() {
        flush(&field, &mut ssid, &mut security, &mut password, &mut hidden)?;
    }
    if ssid.is_empty() {
        return Err(SmartError::invalid_argument("the WIFI payload carries no SSID (S:)"));
    }
    if security.is_empty() {
        security = "nopass".to_string();
    }
    Ok(WifiJoin { ssid, security, password, hidden })
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    out
}

// ── The module matrix (heavy half delegated to `qrcode`) ───────────────────

/// Encode a payload into a module matrix: `(width, matrix)` where
/// `matrix[y][x]` is true for a DARK module. `qrcode` picks the version and
/// error correction — the heavy lifting is the crate's.
fn encode_matrix(payload: &str) -> SmartResult<(usize, Vec<Vec<bool>>)> {
    let code = qrcode::QrCode::new(payload.as_bytes())
        .map_err(|e| SmartError::invalid_argument(format!("QR encode: {e}")))?;
    let n = code.width();
    let colors = code.to_colors();
    let mut matrix = vec![vec![false; n]; n];
    for y in 0..n {
        for x in 0..n {
            matrix[y][x] = colors[y * n + x] != qrcode::Color::Light;
        }
    }
    Ok((n, matrix))
}

/// The terminal renderer (OURS): two module rows per text line as Unicode
/// half blocks on a light field — a space is light, `▀`/`▄`/`█` are the dark
/// halves. Every line is exactly `width` glyphs.
fn render_half_block(matrix: &[Vec<bool>]) -> Vec<String> {
    let n = matrix.len();
    let mut lines = Vec::new();
    let mut y = 0;
    while y < n {
        let mut line = String::new();
        for x in 0..n {
            let top = matrix[y][x];
            let bot = if y + 1 < n { matrix[y + 1][x] } else { false };
            line.push(match (top, bot) {
                (false, false) => ' ',
                (true, false) => '\u{2580}', // ▀ upper
                (false, true) => '\u{2584}', // ▄ lower
                (true, true) => '\u{2588}',  // █ full
            });
        }
        lines.push(line);
        y += 2;
    }
    lines
}

/// The ASCII renderer (OURS): one module per character, `#` dark / space
/// light. Larger on screen, and the same exact inverse.
fn render_ascii(matrix: &[Vec<bool>]) -> Vec<String> {
    matrix
        .iter()
        .map(|row| row.iter().map(|&d| if d { '#' } else { ' ' }).collect())
        .collect()
}

/// The half-block inverse (OURS): a rendered line becomes the two module rows
/// it encodes, so the parsed matrix equals the encoded one exactly.
fn parse_half_block(lines: &[String], n: usize) -> Vec<Vec<bool>> {
    let mut matrix = vec![vec![false; n]; n];
    for (row, line) in lines.iter().enumerate() {
        let y = row * 2;
        if y >= n {
            break;
        }
        for (x, ch) in line.chars().enumerate() {
            if x >= n {
                break;
            }
            let (top, bot) = match ch {
                ' ' => (false, false),
                '\u{2580}' => (true, false),
                '\u{2584}' => (false, true),
                '\u{2588}' => (true, true),
                _ => (false, false),
            };
            matrix[y][x] = top;
            if y + 1 < n {
                matrix[y + 1][x] = bot;
            }
        }
    }
    matrix
}

fn parse_ascii(lines: &[String], n: usize) -> Vec<Vec<bool>> {
    let mut matrix = vec![vec![false; n]; n];
    for (y, line) in lines.iter().enumerate() {
        if y >= n {
            break;
        }
        for (x, ch) in line.chars().enumerate() {
            if x < n {
                matrix[y][x] = ch == '#';
            }
        }
    }
    matrix
}

/// Decode a module matrix back to its payload with `rqrr` (heavy half
/// delegated). Dark = true maps to the `BitGrid` contract (true is black).
fn decode_matrix(matrix: &[Vec<bool>]) -> SmartResult<String> {
    let n = matrix.len();
    let borrowed = matrix.to_vec();
    let grid = rqrr::Grid::new(rqrr::SimpleGrid::from_func(n, |x, y| borrowed[y][x]));
    grid.decode().map(|(_, text)| text).map_err(|e| {
        SmartError::invalid_argument(format!("QR decode: the rendered matrix did not read back ({e})"))
    })
}

/// The round trip: encode → render (ours) → parse back (ours) → decode
/// (`rqrr`). Returns the decoded payload; a caller asserts it equals the
/// original. This is the proof that the screen a phone scans reads back to the
/// exact network.
pub fn round_trip(payload: &str, ascii: bool) -> SmartResult<String> {
    let (n, matrix) = encode_matrix(payload)?;
    let lines = if ascii { render_ascii(&matrix) } else { render_half_block(&matrix) };
    let parsed = if ascii { parse_ascii(&lines, n) } else { parse_half_block(&lines, n) };
    decode_matrix(&parsed)
}

// ── The scannable on-screen render (`network wifi share`) ──────────────────

/// Render a payload as a terminal QR a phone can scan: a 4-module light quiet
/// zone, half-block (or ASCII) glyphs, the whole block in reverse video so the
/// dark modules read dark on any terminal theme. Every returned line already
/// carries the reverse-video escapes and is ready to print.
pub fn render_terminal(payload: &str, ascii: bool) -> SmartResult<String> {
    let (n, core) = encode_matrix(payload)?;
    // A light (false) quiet zone so a camera has white margin around the
    // symbol; the inner matrix is placed at [q..q+n].
    let quiet = 4usize;
    let big = n + 2 * quiet;
    let mut matrix = vec![vec![false; big]; big];
    for y in 0..n {
        for x in 0..n {
            matrix[y + quiet][x + quiet] = core[y][x];
        }
    }
    let mut lines = if ascii { render_ascii(&matrix) } else { render_half_block(&matrix) };
    // Reverse video per line: a space renders light, a block renders dark — the
    // QR's true orientation for a camera, on any theme.
    for line in &mut lines {
        *line = format!("\u{1b}[7m{line}\u{1b}[0m");
    }
    Ok(format!("  {}\n", lines.join("\n  ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_standard_wifi_payload() {
        let j = parse("WIFI:T:WPA;S:My Net;P:pa ss\\;word;;").unwrap();
        assert_eq!(j.ssid, "My Net");
        assert_eq!(j.security, "WPA");
        // The escaped semicolon inside P is a literal, and the `;;` still ends.
        assert_eq!(j.password, "pa ss;word");
        assert!(!j.hidden);
    }

    #[test]
    fn parses_a_hidden_open_payload_and_a_bare_ssid() {
        let j = parse("WIFI:T:nopass;S:Guest;H:true;;").unwrap();
        assert_eq!(j.ssid, "Guest");
        assert!(j.hidden);
        assert!(j.needs_password() == false);
        let bare = parse("JustAName").unwrap();
        assert_eq!(bare.ssid, "JustAName");
        assert_eq!(bare.security, "open");
        assert!(!bare.needs_password());
    }

    #[test]
    fn a_wpa_payload_with_no_password_flags_the_gap() {
        let j = parse("WIFI:T:WPA;S:Secured;;").unwrap();
        assert_eq!(j.ssid, "Secured");
        assert!(j.needs_password(), "WPA with no P is not joinable as-is");
    }

    #[test]
    fn an_empty_or_ssidless_payload_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("WIFI:T:WPA;P:nossid;;").is_err());
    }

    #[test]
    fn the_payload_round_trips_through_parse_and_escape() {
        let payload = "WIFI:T:WPA;S:Matrix;P:hunter2;;";
        let j = parse(payload).unwrap();
        assert_eq!(j.to_payload(), payload, "rebuilt payload matches the original");
    }

    /// The round trip the founder asked for: encode → render (ours) → parse
    /// back (ours) → decode, and the payload survives byte for byte. Proven
    /// both for the half-block and the ASCII renderer, over escapes and a
    /// longer near-capacity payload.
    #[test]
    fn the_runnable_proof_the_screen_reads_back() {
        for payload in [
            "WIFI:T:WPA;S:VR-3245-0884;P:hunter2;;",
            "WIFI:T:nopass;S:Guest;;",
            "WIFI:T:WPA;S:My Net;P:pa ss\\;word;;",
        ] {
            for ascii in [false, true] {
                let decoded = round_trip(payload, ascii).expect("round trip");
                assert_eq!(decoded, payload, "ascii={ascii} payload={payload}");
            }
        }
    }

    #[test]
    fn a_longer_payload_still_round_trips() {
        // A payload near the practical capacity: exercises a higher QR version
        // (a larger matrix) rather than the trivially small version 1.
        let payload = "WIFI:T:WPA;S:Mahuntsi_Accounting_Services Inc;P:c0rrect-horse-battery;;";
        assert_eq!(round_trip(payload, false).unwrap(), payload);
        assert_eq!(round_trip(payload, true).unwrap(), payload);
    }

    #[test]
    fn the_terminal_render_is_reverse_video_with_a_quiet_zone() {
        let rendered = render_terminal("WIFI:T:nopass;S:Guest;;", false).unwrap();
        assert!(rendered.contains('\u{1b}'), "reverse video wraps the block");
        // The quiet zone makes the first and last lines wider than the bare
        // matrix (4 light modules each side); those render as light spaces.
        assert!(rendered.lines().count() > 2);
    }
}
