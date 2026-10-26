//! The pure formatting layer: numbers, times and text turned into the shortest
//! honest string.
//!
//! Every function here is deterministic and unit-tested, because these strings
//! are what the user actually reads. "1.4 KB" over a 1434-byte payload is
//! fine; "1.4 MB" would be a lie, and this module is where a lie like that
//! would be born.

use smart_clipboard_proto::{Item, ItemId, MimeFamily};

/// Bytes, in the shortest unit that does not lose the magnitude: `0 B`,
/// `42 B`, `1.4 KB`, `12 MB`. Decimal units (1000), because a clipboard's
/// sizes are quoted by what a payload *is*, not by what a disk allocates.
pub fn bytes(size: u64) -> String {
    if size < 1000 {
        return format!("{size} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = size as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < units.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{value:.0} {}", units[unit])
    }
}

/// A relative time, from two nanosecond timestamps.
///
/// The bands are chosen for scanning, not for precision: a clipboard is read
/// "just now / minutes / hours / days", and `2026-10-26 14:03:11` for
/// something copied nine seconds ago is noise.
pub fn relative_time(now_ns: u64, then_ns: u64) -> String {
    if then_ns == 0 {
        return "time unreadable".to_string();
    }
    if now_ns <= then_ns {
        return "just now".to_string();
    }
    let seconds = (now_ns - then_ns) / 1_000_000_000;
    match seconds {
        0..=9 => "just now".to_string(),
        10..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        86_400..=604_799 => format!("{}d ago", seconds / 86_400),
        _ => format!("{}w ago", seconds / 604_800),
    }
}

/// The absolute time, for the inspector where precision is the point.
///
/// Implemented against the Unix epoch with a civil-date conversion so the app
/// needs no date crate: the arithmetic is the standard days-to-civil
/// algorithm, and it is exact for every timestamp a clipboard will ever hold.
pub fn absolute_time(nanos: u64) -> String {
    if nanos == 0 {
        return "the service could not read the clock for this item".to_string();
    }
    let seconds = (nanos / 1_000_000_000) as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Howard Hinnant's `civil_from_days`: days since the epoch to a civil date.
/// Public domain algorithm, exact for the entire `i64` range we can reach.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Collapse whitespace and cut to `limit` characters on a char boundary.
///
/// A clipboard row must be one scannable line. Newlines, tabs and runs of
/// spaces are collapsed rather than trimmed only at the ends, because a code
/// snippet's indentation is what pushes the useful part off the row.
pub fn snippet(text: &str, limit: usize) -> String {
    let mut out = String::with_capacity(text.len().min(limit * 2));
    let mut last_was_space = true;
    let mut count = 0;
    for character in text.chars() {
        let is_space = character.is_whitespace();
        if is_space {
            if last_was_space {
                continue;
            }
            if count == limit {
                out.push('…');
                return out;
            }
            out.push(' ');
            count += 1;
            last_was_space = true;
            continue;
        }
        if count >= limit {
            out.push('…');
            return out;
        }
        out.push(character);
        count += 1;
        last_was_space = false;
    }
    out.trim_end().to_string()
}

/// The row title for an item: the family's own noun, so a user can tell at a
/// glance what kind of thing they are looking at before reading it.
pub fn family_title(family: MimeFamily) -> &'static str {
    match family {
        MimeFamily::Text => "Text",
        MimeFamily::Link => "Link",
        MimeFamily::Image => "Image",
        MimeFamily::File => "File",
        MimeFamily::Color => "Colour",
        MimeFamily::Other => "Payload",
    }
}

/// A `#rrggbb` / `#rgb` colour literal, when the whole payload is one.
///
/// Detected by content, not by MIME: nobody publishes `text/x-color`. The
/// check is strict (the trimmed payload must be exactly a literal) so a
/// paragraph that mentions a hex code is not rendered as a swatch.
pub fn color_literal(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    let hex = trimmed.strip_prefix('#')?;
    let value = match hex.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for character in hex.chars() {
                if !character.is_ascii_hexdigit() {
                    return None;
                }
                expanded.push(character);
                expanded.push(character);
            }
            u32::from_str_radix(&expanded, 16).ok()?
        }
        6 => {
            if !hex.chars().all(|character| character.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(hex, 16).ok()?
        }
        _ => return None,
    };
    Some(value)
}

/// A URL payload, when the whole payload is one (`http`, `https`, `mailto`,
/// `file`). Used only to *label* a row — never to auto-open anything.
pub fn url_literal(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let candidate = trimmed.split_whitespace().next()?;
    if candidate.len() != trimmed.len() {
        return None;
    }
    for scheme in ["https://", "http://", "mailto:", "file://"] {
        if candidate.starts_with(scheme) {
            return Some(candidate);
        }
    }
    None
}

/// The secondary line for a row: what it is, and how big, in one string.
///
/// Deliberately not a sentence. The row is 64 px tall and the eye needs a
/// stable pattern in the same place for every row; "image/png · 1.4 MB"
/// scans; "This is an image of 1.4 megabytes" does not.
pub fn row_meta(item: &Item, now_ns: u64) -> String {
    let mut parts = Vec::new();
    parts.push(item.mime_summary(2));
    parts.push(bytes(item.size_bytes));
    if let Some(nanos) = item.captured_at_ns {
        parts.push(relative_time(now_ns, nanos));
    } else {
        parts.push("time unreadable".to_string());
    }
    parts.join(" · ")
}

/// The one-line description of an item for the status bar and the palette's
/// footer, e.g. `text/plain · 1.4 KB · pinned`.
pub fn item_summary(item: &Item) -> String {
    let mut parts = vec![item.mime_summary(2), bytes(item.size_bytes)];
    if item.pinned {
        parts.push("pinned".to_string());
    }
    if !item.tags.is_empty() {
        parts.push(format!("tags: {}", item.tags.join(", ")));
    }
    format!("{} · {}", item.id, parts.join(" · "))
}

/// Whether a payload looks like a secret the user should be warned about.
///
/// This is a **local heuristic**, and every surface that shows it says so. It
/// never modifies, redacts or withholds anything: it adds a quiet warning
/// glyph so a password pasted by mistake is noticed before it is scrolled past
/// in a screen share. Deliberately conservative — a false positive is noise, a
/// false negative is the status quo.
pub fn looks_sensitive(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 8 || trimmed.len() > 512 {
        return false;
    }
    if trimmed.split_whitespace().count() > 3 {
        return false;
    }
    let starts_like_a_token = ["sk-", "ghp_", "gho_", "AKIA", "xoxb-", "eyJ"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix));
    if starts_like_a_token {
        return true;
    }
    let has_digit = trimmed.chars().any(|character| character.is_ascii_digit());
    let has_symbol = trimmed
        .chars()
        .any(|character| "!@#$%^&*_-+=~`|<>?/\\".contains(character));
    let has_upper = trimmed.chars().any(|character| character.is_ascii_uppercase());
    let has_lower = trimmed.chars().any(|character| character.is_ascii_lowercase());
    has_digit && has_symbol && has_upper && has_lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use smart_clipboard_proto::Item;

    #[test]
    fn bytes_use_the_shortest_honest_unit() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1000), "1.0 KB");
        assert_eq!(bytes(1434), "1.4 KB");
        assert_eq!(bytes(12_000_000), "12 MB");
    }

    #[test]
    fn relative_time_bands_are_scannable() {
        let second = 1_000_000_000;
        assert_eq!(relative_time(10 * second, 10 * second), "just now");
        assert_eq!(relative_time(20 * second, 10 * second), "10s ago");
        assert_eq!(relative_time(200 * second, 0 + second), "3m ago");
        assert_eq!(relative_time(7200 * second, 0 + second), "1h ago");
        assert_eq!(relative_time(90_000 * second, second), "1d ago");
        assert_eq!(relative_time(10, 0), "time unreadable");
    }

    #[test]
    fn absolute_time_is_exact_across_a_leap_day() {
        // 2024-02-29 12:00:00 UTC
        let nanos = 1_709_208_000_u64 * 1_000_000_000;
        assert_eq!(absolute_time(nanos), "2024-02-29 12:00:00 UTC");
        assert_eq!(absolute_time(0), "the service could not read the clock for this item");
    }

    #[test]
    fn snippets_collapse_whitespace_and_cut_on_a_char_boundary() {
        assert_eq!(snippet("a\n\n\tb   c", 20), "a b c");
        assert_eq!(snippet("abcdefghij", 4), "abcd…");
        assert_eq!(snippet("héllo wörld", 5), "héllo…");
        assert_eq!(snippet("   ", 5), "");
    }

    #[test]
    fn colour_literals_are_strict() {
        assert_eq!(color_literal("#fff"), Some(0xffffff));
        assert_eq!(color_literal("  #1F2937\n"), Some(0x1f2937));
        assert_eq!(color_literal("#12345"), None);
        assert_eq!(color_literal("the colour #fff is nice"), None);
        assert_eq!(color_literal("fff"), None);
    }

    #[test]
    fn url_literals_must_be_the_whole_payload() {
        assert_eq!(url_literal("https://example.com/a"), Some("https://example.com/a"));
        assert_eq!(url_literal("see https://example.com"), None);
        assert_eq!(url_literal("ftp://example.com"), None);
    }

    #[test]
    fn secret_heuristics_are_conservative() {
        assert!(looks_sensitive("sk-live-9f8a7b6c5d4e"));
        assert!(looks_sensitive("Abc123!@#def"));
        assert!(!looks_sensitive("hello world, this is a normal sentence with words"));
        assert!(!looks_sensitive("aaaa"));
        assert!(!looks_sensitive("just some text here"));
    }

    #[test]
    fn row_meta_is_a_stable_pattern() {
        let item = Item {
            id: smart_clipboard_proto::ItemId(7),
            captured_at_ns: Some(1_000_000_000),
            mimes: vec!["text/plain".to_string()],
            tags: vec![],
            pinned: false,
            size_bytes: 1434,
        };
        assert_eq!(
            row_meta(&item, 1_010_000_000),
            "text/plain · 1.4 KB · 10s ago"
        );
        assert_eq!(item_summary(&item), "#7 · text/plain · 1.4 KB");
    }
}
