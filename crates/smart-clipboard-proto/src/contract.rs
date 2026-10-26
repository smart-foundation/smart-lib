//! The contract facts this product depends on, in one place.
//!
//! Two kinds of fact live here, and they are kept apart on purpose:
//!
//! * **Compiled facts** — interface ids and schema ids the generated code
//!   carries. These cannot be wrong: they are what this binary was built
//!   against.
//! * **The published schema id** — a number read from the canonical
//!   `clipboard.capnp` (`@0xa1b2c3d4e5f60012`) and mirrored by the engine's own
//!   availability matrix. It is here so a *drift* between the service's
//!   contract generation and this app's can be shown to the user instead of
//!   producing a confusing decode error later. A mismatch is a warning, never
//!   a hard failure: a newer service is allowed to exist.

use crate::schema::clipboard_capnp;

/// The clipboard interface id, from the generated code.
pub const CLIPBOARD_INTERFACE_ID: u64 = clipboard_capnp::clipboard::_private::TYPE_ID;

/// The change-subscription interface id.
pub const CLIPBOARD_SUBSCRIPTION_INTERFACE_ID: u64 =
    clipboard_capnp::clipboard_change_subscription::_private::TYPE_ID;

/// The schema file id the engine's matrix publishes for `clipboard.capnp`.
///
/// Mirrors `smart_lib::domains::clipboard::availability::CLIPBOARD_SCHEMA_ID`.
/// Duplicated here because the app does not link the engine: a client that
/// imported the server's constants to compare them would be checking a number
/// against itself.
pub const EXPECTED_CLIPBOARD_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60012;

/// The name discovery publishes for the clipboard service.
pub const DOMAIN_NAME: &str = "clipboard";

/// The clipboard contract's method names, in ordinal order (`@0`..`@16`).
///
/// The app keeps this list so its live matrix can show a verb it has not
/// called yet. It is a *compile-time* fact of the interface this binary was
/// built against — not a claim about what the service will answer. That is
/// what [`crate::model::Observation`] is for.
pub const CLIPBOARD_METHODS: &[&str] = &[
    "getContent",
    "setContent",
    "clearContent",
    "getHistory",
    "clearHistory",
    "subscribeChanges",
    "getSupportedMimeTypes",
    "searchHistory",
    "pinItem",
    "unpinItem",
    "deleteItem",
    "getPreview",
    "validateContent",
    "convertContent",
    "getSizeLimits",
    "setSizeLimits",
    "setOffer",
];

/// The service name this product asks for first. Discovery's `name` filter is
/// an exact match on the registered name; the engine registers the domain name
/// itself, so this is `clipboard`, not `smart-clipboard`.
pub const SERVICE_NAME: &str = "clipboard";

/// The directory the contract's schemas were compiled from, recorded by
/// `build.rs`. Shown in the About panel: two clients that disagree about the
/// contract should be able to see it in one glance.
pub const SCHEMA_DIR: &str = env!("SMART_CLIPBOARD_SCHEMA_DIR");

/// The ceiling this app puts on a single payload fetch.
///
/// It is a **client** bound, and every surface that mentions it says so. The
/// contract's own limits live in `getSizeLimits`; this one exists because a
/// GUI holds the bytes in memory to draw them, and a 2 GiB "copy" would take
/// the app down for no user benefit. 64 MiB is comfortably above any
/// screenshot and far below "the app dies".
pub const MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

/// The largest number of items the app asks for in one page.
///
/// The contract's pagination rule is `limit == 0` means one, so a page size
/// must always be explicit. 200 rows is a full screen and a half of the
/// densest view; the app fetches the next page when the user reaches the end
/// of this one.
pub const PAGE_SIZE: u32 = 200;

/// How long any single call may take before the UI is told it timed out.
///
/// A local unix-socket round trip is sub-millisecond; 10 s means "the service
/// is wedged", which is exactly when a user deserves words instead of a
/// spinner.
pub const CALL_TIMEOUT_MS: u64 = 10_000;

/// MIME spellings the app understands well enough to give them an icon and a
/// syntax-highlighted-ish treatment. The list is **not** a whitelist: anything
/// else is shown with its own name and the `generic` treatment, because the
/// contract's MIME vocabulary is open and a client that hides what it does not
/// recognise is lying about the clipboard.
pub const KNOWN_MIMES: &[&str] = &[
    "text/plain",
    "text/html",
    "text/uri-list",
    "text/x-url",
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
    "image/tiff",
    "image/svg+xml",
    "text/uri-list+x-file",
    "application/x-file-list",
    "application/json",
    "application/pdf",
];

/// The MIME this app prefers when it asks a service to "copy this item back"
/// and the item has a text representation.
pub const PREFERRED_TEXT_MIME: &str = "text/plain";

/// The MIME family a payload belongs to, for iconography and filtering.
///
/// The mapping is deliberately coarse and *open*: the family is a UI
/// convenience over an open vocabulary, so an unknown type maps to
/// [`MimeFamily::Other`] with its text preserved, never dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MimeFamily {
    /// `text/*` and friends.
    Text,
    /// A link list or a URL-shaped text.
    Link,
    /// `image/*`.
    Image,
    /// A file reference (`application/x-file-list`, `text/uri-list` of paths).
    File,
    /// A colour literal (`#rrggbb`), detected by content, not by mime.
    Color,
    /// Anything else, named rather than guessed.
    Other,
}

impl MimeFamily {
    /// The family of a MIME string.
    pub fn of(mime: &str) -> Self {
        let lower = mime.to_ascii_lowercase();
        if lower.starts_with("image/") {
            return MimeFamily::Image;
        }
        if lower == "text/uri-list" || lower == "text/x-url" {
            return MimeFamily::Link;
        }
        if lower == "application/x-file-list" || lower == "text/uri-list+x-file" {
            return MimeFamily::File;
        }
        if lower.starts_with("text/") || lower == "application/json" {
            return MimeFamily::Text;
        }
        MimeFamily::Other
    }

    /// The short token the filter chips use.
    pub fn token(self) -> &'static str {
        match self {
            MimeFamily::Text => "text",
            MimeFamily::Link => "link",
            MimeFamily::Image => "image",
            MimeFamily::File => "file",
            MimeFamily::Color => "color",
            MimeFamily::Other => "other",
        }
    }
}

/// Whether this build knows a MIME well enough to name it in the UI's hints.
/// Distinct from [`MimeFamily::of`], which classifies *everything*.
pub fn is_known_mime(mime: &str) -> bool {
    KNOWN_MIMES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_classify_the_open_vocabulary_without_dropping_anything() {
        assert_eq!(MimeFamily::of("text/plain"), MimeFamily::Text);
        assert_eq!(MimeFamily::of("IMAGE/PNG"), MimeFamily::Image);
        assert_eq!(MimeFamily::of("text/uri-list"), MimeFamily::Link);
        assert_eq!(MimeFamily::of("application/x-file-list"), MimeFamily::File);
        // Open vocabulary: an unknown type still classifies as something.
        assert_eq!(
            MimeFamily::of("application/x-nobody-named-this"),
            MimeFamily::Other
        );
    }

    #[test]
    fn an_unknown_mime_is_not_known_but_is_still_classified() {
        assert!(!is_known_mime("application/x-nobody-named-this"));
        assert!(is_known_mime("text/plain"));
    }
}
