//! The search field's grammar: **one box, two jobs, honestly labelled.**
//!
//! # The problem this solves
//!
//! The contract's `searchHistory` is a text search. It is the service's search,
//! over the service's whole history, and it is the only way to find something
//! that is not in the loaded window. But a clipboard user asks questions the
//! contract cannot answer: "images from the last hour", "the pinned ones", "only
//! PDFs".
//!
//! Answering those by pretending the service did it would be the lie this product
//! exists to avoid. Answering them by refusing would make the app worse than a
//! `grep`. So the box takes both, and the split is explicit:
//!
//! ```text
//!   deploy kind:image newer:2h
//!   └──────┘ └─────────┘ └──────┘
//!   service   local        local     → the header says "kind:image newer:2h
//!   (searchHistory)                    applied to the 200 loaded rows"
//! ```
//!
//! # The grammar (deliberately tiny)
//!
//! | term | means | where |
//! |---|---|---|
//! | `kind:text\|link\|image\|file\|colour` | the item's first MIME family | local |
//! | `mime:image` / `mime:image/png` | any offered MIME starts with this | local |
//! | `is:pinned` / `is:secret` | pinned / the local secret heuristic fires | local |
//! | `newer:30m` `newer:2h` `newer:7d` | captured inside that window | local |
//! | `older:1h` | captured before that | local |
//! | anything else | text, passed to the service | service |
//!
//! Every term is AND-ed. An unrecognised `key:value` is **text**, not an error:
//! a user typing `https://example.com` must not be told their URL is malformed
//! search syntax, and a colon is common in exactly the things people copy.
//!
//! The module is pure — no gpui, no store, no clock of its own — so the grammar
//! is tested exhaustively without a window.

use smart_clipboard_proto::{Item, MimeFamily};

/// One local predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// The item's first MIME family.
    Kind(MimeFamily),
    /// Any offered MIME begins with this string.
    Mime(String),
    /// Pinned items only.
    Pinned,
    /// Items the local secret heuristic flags.
    Sensitive,
    /// Captured within this many nanoseconds of "now".
    Newer(u64),
    /// Captured before this many nanoseconds ago.
    Older(u64),
}

/// A parsed search field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The text the service is asked to search for (its own `searchHistory`).
    pub service: String,
    /// The predicates the app applies to the loaded window.
    pub terms: Vec<Term>,
    /// `key:value` tokens that looked like syntax but are not: shown as a hint so
    /// a user who typed `kind:imag` learns why nothing filtered.
    pub unknown: Vec<String>,
}

impl Parsed {
    /// Whether any local predicate is active (the header's "local" badge).
    pub fn is_local(&self) -> bool {
        !self.terms.is_empty()
    }

    /// A short human description of the local half, for the header chip.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for term in &self.terms {
            parts.push(match term {
                Term::Kind(family) => format!("kind:{}", family_word(*family)),
                Term::Mime(mime) => format!("mime:{mime}"),
                Term::Pinned => "is:pinned".to_string(),
                Term::Sensitive => "is:secret".to_string(),
                Term::Newer(ns) => format!("newer:{}", crate::query::duration_words(*ns)),
                Term::Older(ns) => format!("older:{}", crate::query::duration_words(*ns)),
            });
        }
        parts.join(" ")
    }

    /// Whether one loaded item satisfies every local predicate.
    ///
    /// `preview` is the service's preview text when it served one — the secret
    /// heuristic needs text and must not run on bytes it cannot read. `now_ns` is
    /// the clock the app is already keeping for relative times, so a filter and a
    /// timestamp can never disagree about what "2 minutes ago" means.
    pub fn matches(&self, item: &Item, preview: Option<&str>, now_ns: u64) -> bool {
        self.terms.iter().all(|term| match term {
            Term::Kind(family) => item.family() == *family,
            Term::Mime(prefix) => item.mimes.iter().any(|mime| {
                mime.to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            }),
            Term::Pinned => item.pinned,
            Term::Sensitive => preview.map(crate::format::looks_sensitive).unwrap_or(false),
            Term::Newer(window) => match item.captured_at_ns {
                // An item whose clock the service could not read is *not*
                // silently dropped from a time filter: it is unknown, and the
                // honest answer to "was this from the last hour?" is no. It stays
                // visible when the filter is removed, which is what the header's
                // hidden count is for.
                Some(captured) => now_ns.saturating_sub(captured) <= *window,
                None => false,
            },
            Term::Older(threshold) => match item.captured_at_ns {
                Some(captured) => now_ns.saturating_sub(captured) > *threshold,
                None => false,
            },
        })
    }
}

/// Parse a search field.
pub fn parse(input: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut service_words: Vec<&str> = Vec::new();
    for word in input.split_whitespace() {
        match classify(word) {
            Some(Ok(term)) => parsed.terms.push(term),
            Some(Err(token)) => {
                parsed.unknown.push(token.clone());
                // An unrecognised key:value is still text the user wants found.
                service_words.push(word);
            }
            None => service_words.push(word),
        }
    }
    parsed.service = service_words.join(" ");
    parsed
}

/// `None` = plain text; `Some(Ok(term))` = a predicate; `Some(Err(token))` = a
/// syntax-shaped token we do not understand.
fn classify(word: &str) -> Option<Result<Term, String>> {
    let (key, value) = word.split_once(':')?;
    // A URL, a time ("10:30"), a JSON fragment: a colon alone is not syntax.
    if value.is_empty() || value.contains("//") || value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let lower = key.to_ascii_lowercase();
    match lower.as_str() {
        "kind" | "type" => Some(match kind_of(value) {
            Some(family) => Ok(Term::Kind(family)),
            None => Err(word.to_string()),
        }),
        "mime" => Some(Ok(Term::Mime(value.to_string()))),
        "is" => match value.to_ascii_lowercase().as_str() {
            "pinned" | "pin" => Some(Ok(Term::Pinned)),
            "secret" | "sensitive" => Some(Ok(Term::Sensitive)),
            _ => Some(Err(word.to_string())),
        },
        "newer" | "since" | "within" => Some(match duration_ns(value) {
            Some(window) => Ok(Term::Newer(window)),
            None => Err(word.to_string()),
        }),
        "older" | "before" => Some(match duration_ns(value) {
            Some(window) => Ok(Term::Older(window)),
            None => Err(word.to_string()),
        }),
        // A first-class text term, so a bare prefix cannot swallow the word.
        _ => None,
    }
}

fn kind_of(value: &str) -> Option<MimeFamily> {
    match value.to_ascii_lowercase().as_str() {
        "text" | "txt" => Some(MimeFamily::Text),
        "link" | "url" => Some(MimeFamily::Link),
        "image" | "img" | "picture" | "screenshot" => Some(MimeFamily::Image),
        "file" | "files" => Some(MimeFamily::File),
        // Both spellings, because both are English.
        "colour" | "color" => Some(MimeFamily::Color),
        "other" | "unknown" => Some(MimeFamily::Other),
        _ => None,
    }
}

/// `30m`, `2h`, `7d`, `45s`, `1w` → nanoseconds. A missing unit is seconds, which
/// is the only reading that cannot surprise ("newer:30" means the last 30s).
pub fn duration_ns(value: &str) -> Option<u64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, unit) = value.split_at(split);
    let count: u64 = digits.parse().ok()?;
    let seconds = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        "w" | "week" | "weeks" => 7 * 24 * 60 * 60,
        _ => return None,
    };
    count.checked_mul(seconds)?.checked_mul(1_000_000_000)
}

/// A duration in the grammar's own spelling: `45s`, `30m`, `2h`, `7d`.
pub fn duration_words(ns: u64) -> String {
    let seconds = ns / 1_000_000_000;
    if seconds % (7 * 24 * 60 * 60) == 0 && seconds >= 7 * 24 * 60 * 60 {
        format!("{}w", seconds / (7 * 24 * 60 * 60))
    } else if seconds % (24 * 60 * 60) == 0 && seconds >= 24 * 60 * 60 {
        format!("{}d", seconds / (24 * 60 * 60))
    } else if seconds % (60 * 60) == 0 && seconds >= 60 * 60 {
        format!("{}h", seconds / (60 * 60))
    } else if seconds % 60 == 0 && seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn family_word(family: MimeFamily) -> &'static str {
    match family {
        MimeFamily::Text => "text",
        MimeFamily::Link => "link",
        MimeFamily::Image => "image",
        MimeFamily::File => "file",
        MimeFamily::Color => "colour",
        MimeFamily::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smart_clipboard_proto::ItemId;

    fn item(mimes: &[&str], pinned: bool, captured_at_ns: Option<u64>) -> Item {
        Item {
            id: ItemId(1),
            captured_at_ns,
            mimes: mimes.iter().map(|mime| mime.to_string()).collect(),
            tags: Vec::new(),
            pinned,
            size_bytes: 10,
        }
    }

    const NOW: u64 = 1_700_000_000_000_000_000;
    const MINUTE: u64 = 60 * 1_000_000_000;

    /// The two halves split cleanly: words go to the service, predicates stay.
    #[test]
    fn text_goes_to_the_service_and_predicates_stay_local() {
        let parsed = parse("deploy kind:image newer:2h");
        assert_eq!(parsed.service, "deploy");
        assert_eq!(parsed.terms.len(), 2);
        assert!(parsed.is_local());
        assert_eq!(parsed.describe(), "kind:image newer:2h");
        // Nothing local: the whole field is the service's, so the app does not
        // claim to have filtered anything.
        let plain = parse("just words");
        assert_eq!(plain.service, "just words");
        assert!(!plain.is_local());
        assert!(parse("").service.is_empty());
    }

    /// A colon is not syntax: URLs, clock times and empty values are text.
    #[test]
    fn colons_that_are_not_syntax_stay_text() {
        let url = parse("https://smart.foundation/spec");
        assert_eq!(url.service, "https://smart.foundation/spec");
        assert!(url.terms.is_empty());
        let clock = parse("10:30");
        assert_eq!(clock.service, "10:30");
        let trailing = parse("note:");
        assert_eq!(trailing.service, "note:");
    }

    /// An unknown key is remembered (for the hint) *and* searched as text, so a
    /// typo filters nothing and hides nothing.
    #[test]
    fn an_unknown_key_is_a_hint_and_a_search_word() {
        let parsed = parse("kind:imag");
        assert_eq!(parsed.unknown, vec!["kind:imag".to_string()]);
        assert_eq!(parsed.service, "kind:imag");
        assert!(parsed.terms.is_empty());
    }

    /// Durations parse in every documented spelling, and a bare number is seconds.
    #[test]
    fn durations_use_their_units() {
        assert_eq!(duration_ns("45s"), Some(45 * 1_000_000_000));
        assert_eq!(duration_ns("30m"), Some(30 * 60 * 1_000_000_000));
        assert_eq!(duration_ns("2h"), Some(2 * 3600 * 1_000_000_000));
        assert_eq!(duration_ns("7d"), Some(7 * 86_400 * 1_000_000_000));
        assert_eq!(duration_ns("1w"), Some(7 * 86_400 * 1_000_000_000));
        assert_eq!(duration_ns("30"), Some(30 * 1_000_000_000));
        assert_eq!(duration_ns("soon"), None);
        assert_eq!(duration_ns("2parsecs"), None);
        assert_eq!(duration_words(2 * 3600 * 1_000_000_000), "2h");
        assert_eq!(duration_words(30 * MINUTE), "30m");
    }

    /// Terms are AND-ed, and each one asks the item the question its name says.
    #[test]
    fn every_term_must_match() {
        let parsed = parse("kind:image is:pinned");
        let pinned_image = item(&["image/png"], true, Some(NOW));
        let loose_image = item(&["image/png"], false, Some(NOW));
        assert!(parsed.matches(&pinned_image, Some("a screenshot"), NOW));
        assert!(!parsed.matches(&loose_image, Some("a screenshot"), NOW));

        let mime = parse("mime:image/p");
        assert!(mime.matches(&item(&["image/png"], false, Some(NOW)), None, NOW));
        assert!(!mime.matches(&item(&["image/jpeg"], false, Some(NOW)), None, NOW));

        // A multi-representation copy is classified by its first MIME, so
        // `kind:text` finds an HTML copy and `mime:` can still name the image.
        let mixed = item(&["text/html", "image/png"], false, Some(NOW));
        assert!(parse("kind:text").matches(&mixed, None, NOW));
        assert!(parse("mime:image").matches(&mixed, None, NOW));
    }

    /// Time windows measure against the app's clock, and an unknown capture time
    /// never matches a window (it is unknown, not recent).
    #[test]
    fn time_windows_are_measured_from_now() {
        let recent = item(&["text/plain"], false, Some(NOW - 5 * MINUTE));
        let old = item(&["text/plain"], false, Some(NOW - 5 * 3600 * 1_000_000_000));
        let unknown = item(&["text/plain"], false, None);
        let newer = parse("newer:2h");
        assert!(newer.matches(&recent, None, NOW));
        assert!(!newer.matches(&old, None, NOW));
        assert!(!newer.matches(&unknown, None, NOW));
        let older = parse("older:2h");
        assert!(!older.matches(&recent, None, NOW));
        assert!(older.matches(&old, None, NOW));
    }

    /// The sensitive term uses the same heuristic the inspector shows, and only
    /// when the service served text to read.
    #[test]
    fn the_secret_term_needs_text_to_read() {
        let secret = parse("is:secret");
        let item = item(&["text/plain"], false, Some(NOW));
        assert!(secret.matches(&item, Some("ghp_0123456789abcdefghijklmnopqrstuvwx"), NOW));
        assert!(!secret.matches(&item, Some("the shopping list"), NOW));
        // No preview: the heuristic cannot fire, and it does not guess.
        assert!(!secret.matches(&item, None, NOW));
    }
}
