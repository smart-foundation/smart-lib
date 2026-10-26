//! The owned types the UI renders.
//!
//! Nothing capnp-shaped leaves this module. A reader borrowed from a response
//! message cannot outlive the response, so any design that hands readers to a
//! UI would either leak lifetimes into every view or hold a message alive
//! while it draws. Owning the values once, at the boundary, is the small
//! honest cost that keeps the rest of the app boring.
//!
//! ## What these types deliberately do not carry
//!
//! * **No preview text in [`Item`].** The contract's `ClipboardItem` is
//!   metadata: id, timestamp, MIME list, tags, pinned, size. The snippet a
//!   row shows comes from `getPreview`, one call per item, and so the type
//!   keeps preview and content in separate [`PreviewState`]/[`ContentState`]
//!   maps. A clipboard app that pretended the list carried text would have to
//!   invent it.
//! * **No source application name.** `ClipboardItem` has no app attribution;
//!   `ClipboardChange` has a `sourceId` whose vocabulary belongs to the
//!   backend. So the UI shows "source 1" and says the vocabulary is the
//!   backend's — never "from Firefox".
//! * **No timestamp zero-means-epoch.** `T.Timestamp` of 0 is the engine's
//!   "the clock could not be read". It becomes `None` here, so the UI cannot
//!   accidentally render 1970.

use std::collections::BTreeMap;

use crate::contract::{self, MimeFamily};
use crate::error::Refusal;

/// A stable item identity, as the contract spells it (`T.ClipboardId.id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u64);

impl std::fmt::Display for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One history entry, owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The contract id. Stable within one service session.
    pub id: ItemId,
    /// Nanoseconds since the epoch, or `None` when the service reported 0
    /// (the contract's "clock could not be read"). Never defaulted to 1970.
    pub captured_at_ns: Option<u64>,
    /// The MIME types this entry was offered with, in the service's order.
    pub mimes: Vec<String>,
    /// Audit tags the service attached. Empty is a real answer.
    pub tags: Vec<String>,
    /// Pinned items are exempt from the service's auto-expiry.
    pub pinned: bool,
    /// The measured payload size, in bytes.
    pub size_bytes: u64,
}

impl Item {
    /// The broadest family this item belongs to, for the row's icon.
    ///
    /// An item that offers several representations (a copy of a table is
    /// often `text/html` + `text/plain`) is classified by its *first* listed
    /// MIME, which is the service's own preference order.
    pub fn family(&self) -> MimeFamily {
        self.mimes
            .first()
            .map(|mime| MimeFamily::of(mime))
            .unwrap_or(MimeFamily::Other)
    }

    /// The MIME the app would ask for when copying this item back, when it
    /// has a text representation.
    pub fn preferred_text_mime(&self) -> Option<&str> {
        self.mimes
            .iter()
            .find(|mime| MimeFamily::of(mime) == MimeFamily::Text)
            .map(String::as_str)
    }

    /// The MIME the app would ask for when it wants a drawable payload.
    pub fn image_mime(&self) -> Option<&str> {
        self.mimes
            .iter()
            .find(|mime| MimeFamily::of(mime) == MimeFamily::Image)
            .map(String::as_str)
    }

    /// A short human list of the representations, for the row's chip row:
    /// at most `budget` names, with the remainder counted.
    pub fn mime_summary(&self, budget: usize) -> String {
        if self.mimes.is_empty() {
            return "no mime type published".to_string();
        }
        let shown: Vec<&str> = self.mimes.iter().take(budget).map(String::as_str).collect();
        let hidden = self.mimes.len().saturating_sub(shown.len());
        if hidden == 0 {
            shown.join(" · ")
        } else {
            format!("{} · +{hidden}", shown.join(" · "))
        }
    }
}

/// What the app knows about one item's preview.
///
/// Every arm is renderable. There is no "blank because we have not decided
/// yet" arm in the UI: [`PreviewState::NotRequested`] draws a skeleton that
/// says it is waiting, and [`PreviewState::Unavailable`] draws the *reason*,
/// which is usually the interesting part ("an image payload has no text
/// preview").
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum PreviewState {
    /// Scheduled but not yet asked.
    #[default]
    NotRequested,
    /// The call is in flight.
    Loading,
    /// The service answered with a preview.
    Ready {
        /// Preview text, already truncated by the service.
        text: String,
        /// Whether the service called the preview available.
        available: bool,
        /// The payload's real size, per the service.
        original_size: u64,
        /// The MIME of the preview itself.
        mime: String,
    },
    /// The service answered "no preview" — a first-class answer, with its own
    /// contract shape (`getPreview` returns `unknown`), not an error.
    Unavailable {
        /// Why, in words the app can stand behind.
        reason: String,
    },
    /// The service refused the call, and said why.
    Refused {
        /// The refusal, class included.
        refusal: Refusal,
    },
    /// The call failed on the client side (timeout, decode, disconnect).
    Failed {
        /// What went wrong, verbatim.
        message: String,
    },
}

impl PreviewState {
    /// The one-line text a row can show, if any.
    pub fn snippet(&self) -> Option<&str> {
        match self {
            PreviewState::Ready { text, available: true, .. } if !text.is_empty() => Some(text),
            _ => None,
        }
    }

    /// A short state label for the inspector's provenance strip.
    pub fn label(&self) -> &'static str {
        match self {
            PreviewState::NotRequested => "not requested",
            PreviewState::Loading => "loading",
            PreviewState::Ready { .. } => "served",
            PreviewState::Unavailable { .. } => "not served",
            PreviewState::Refused { .. } => "refused",
            PreviewState::Failed { .. } => "failed",
        }
    }
}

/// What the app knows about one item's bytes.
///
/// Bytes are only fetched when a view needs them: the thumbnail strip asks
/// for image payloads, the inspector asks for the selected item. A text item
/// usually never needs this at all, because its preview is enough to render.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ContentState {
    /// Not asked.
    #[default]
    NotRequested,
    /// In flight.
    Loading,
    /// The service handed over the representation.
    Ready {
        /// The MIME the bytes were converted to.
        mime: String,
        /// The bytes themselves.
        bytes: Vec<u8>,
    },
    /// The service can produce this item but not in the requested
    /// representation (`convertContent` answered `unsupported`).
    Unsupported {
        /// The representation that was asked for.
        target: String,
    },
    /// The service refused the call.
    Refused {
        /// The refusal, class included.
        refusal: Refusal,
    },
    /// The client refused to fetch: the payload is beyond
    /// [`contract::MAX_FETCH_BYTES`]. Labelled as a client policy in the UI.
    TooLarge {
        /// The size the service reported.
        bytes: u64,
        /// The client's ceiling.
        limit: u64,
    },
    /// The call failed on the client side.
    Failed {
        /// What went wrong, verbatim.
        message: String,
    },
}

impl Default for Item {
    fn default() -> Self {
        Item {
            id: ItemId(0),
            captured_at_ns: None,
            mimes: Vec::new(),
            tags: Vec::new(),
            pinned: false,
            size_bytes: 0,
        }
    }
}

/// The kind of change a subscription event describes (`ClipboardEventType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// `copied @0`.
    Copied,
    /// `pasted @1`.
    Pasted,
    /// `cleared @2`.
    Cleared,
    /// `expired @3`.
    Expired,
    /// `custom @4`.
    Custom,
}

impl ChangeKind {
    /// The contract token, for the event ticker.
    pub fn token(self) -> &'static str {
        match self {
            ChangeKind::Copied => "copied",
            ChangeKind::Pasted => "pasted",
            ChangeKind::Cleared => "cleared",
            ChangeKind::Expired => "expired",
            ChangeKind::Custom => "custom",
        }
    }
}

/// One change event delivered by the service's subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeEvent {
    /// The item the event refers to.
    pub id: ItemId,
    /// When it happened, per the service; `None` when the clock read failed.
    pub at_ns: Option<u64>,
    /// The backend's own source vocabulary. Shown as a number, never mapped
    /// to an app name the contract does not carry.
    pub source: u64,
    /// What kind of change it was.
    pub kind: ChangeKind,
    /// The MIME types involved.
    pub mimes: Vec<String>,
}

/// The authority a connection holds, as published by `capability.capnp`.
///
/// The app never *decides* whether it may write. It reads what the service
/// published, and it treats a `Write` operation in that list as permission to
/// offer a verb. Where the list is silent, the app still calls the verb and
/// renders the refusal — because the true answer is the one the service gives,
/// not the one the client guessed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Authority {
    /// Every capability row the service published for this connection.
    pub rows: Vec<CapabilityRow>,
    /// The union of the operations across those rows.
    pub operations: Vec<String>,
    /// The capability id of the connection's own class row, when one was
    /// published (`smart-lib:control` / `smart-lib:readonly`).
    pub class_id: Option<String>,
}

impl Authority {
    /// Whether the service published a write-capable operation for this
    /// connection. `None` means the service said nothing either way, which is
    /// different from "no" and is rendered differently.
    pub fn can_write(&self) -> Option<bool> {
        if self.rows.is_empty() {
            return None;
        }
        Some(self.operations.iter().any(|operation| operation == "write"))
    }

    /// A short class word for the status bar.
    pub fn class_label(&self) -> &'static str {
        match self.class_id.as_deref() {
            Some("smart-lib:control") => "control",
            Some("smart-lib:readonly") => "read-only",
            Some(_) => "capability",
            None if self.rows.is_empty() => "unpublished",
            None => "capability",
        }
    }
}

/// One published capability row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRow {
    /// The capability's opaque identifier, as text (the contract carries
    /// bytes; the engine publishes ASCII ids, and non-ASCII is hexed).
    pub id: String,
    /// What the capability covers.
    pub target: String,
    /// The operations it grants.
    pub operations: Vec<String>,
    /// Who issued it.
    pub issuer: String,
}

/// How one contract method has behaved in this session.
///
/// This is the app's live matrix, and it is deliberately *observed*, not
/// declared: the app has no way to ask a service "which methods do you
/// serve?" — the contract publishes no such list — so it records what
/// happened when it called them. An untested verb says `Untested`; a verb the
/// service refused says so with its class. Nothing here is guessed from
/// version numbers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Observation {
    /// Not called yet in this session.
    #[default]
    Untested,
    /// Answered with a value.
    Answered,
    /// Answered with a refusal.
    Refused(Refusal),
}

/// One row of the live matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedMethod {
    /// The contract method name.
    pub name: &'static str,
    /// The contract ordinal.
    pub ordinal: u16,
    /// What this session has seen so far.
    pub observation: Observation,
}

/// Everything the app knows about the service it is talking to.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ServiceInfo {
    /// The descriptor's name (`clipboard`).
    pub name: String,
    /// The descriptor's category.
    pub category: String,
    /// The descriptor's service id, which the contract defines as the schema
    /// file id.
    pub schema_id: u64,
    /// The descriptor's declared state, as its own token.
    pub state: String,
    /// The endpoints the service published, `transport://address`.
    pub endpoints: Vec<String>,
    /// The descriptor's open metadata (call model, carrier, …).
    pub metadata: BTreeMap<String, String>,
    /// Whether the published schema id matches the contract this app compiled.
    /// A mismatch is shown, never hidden, and never fatal.
    pub schema_matches_app: bool,
    /// The live per-method matrix.
    pub methods: Vec<ServedMethod>,
}

impl ServiceInfo {
    /// A fresh matrix for the clipboard contract, all rows untested.
    pub fn new_matrix() -> Vec<ServedMethod> {
        contract::CLIPBOARD_METHODS
            .iter()
            .enumerate()
            .map(|(ordinal, name)| ServedMethod {
                name,
                ordinal: ordinal as u16,
                observation: Observation::Untested,
            })
            .collect()
    }

    /// Record that a verb answered.
    pub fn note_answered(&mut self, verb: &str) {
        if let Some(row) = self.methods.iter_mut().find(|row| row.name == verb) {
            row.observation = Observation::Answered;
        }
    }

    /// Record that a verb was refused. A later success clears the refusal,
    /// because the matrix reports *this session's latest* answer.
    pub fn note_refused(&mut self, refusal: &Refusal) {
        if let Some(row) = self
            .methods
            .iter_mut()
            .find(|row| row.name == refusal.verb)
        {
            row.observation = Observation::Refused(refusal.clone());
        }
    }

    /// Count answered rows, for the status bar's "9/17 methods answered".
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut answered = 0;
        let mut refused = 0;
        for row in &self.methods {
            match row.observation {
                Observation::Answered => answered += 1,
                Observation::Refused(_) => refused += 1,
                Observation::Untested => {}
            }
        }
        (answered, refused, self.methods.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(mimes: &[&str]) -> Item {
        Item {
            mimes: mimes.iter().map(|mime| mime.to_string()).collect(),
            ..Item::default()
        }
    }

    #[test]
    fn a_timestamp_of_zero_is_unknown_not_the_epoch() {
        let item = Item {
            captured_at_ns: None,
            ..Item::default()
        };
        assert_eq!(item.captured_at_ns, None);
    }

    #[test]
    fn an_item_is_classified_by_its_first_representation() {
        assert_eq!(
            item(&["image/png", "text/plain"]).family(),
            MimeFamily::Image
        );
        assert_eq!(item(&["text/html", "text/plain"]).family(), MimeFamily::Text);
        assert_eq!(item(&[]).family(), MimeFamily::Other);
    }

    #[test]
    fn the_text_representation_is_found_wherever_it_sits() {
        let copied = item(&["image/png", "text/plain"]);
        assert_eq!(copied.preferred_text_mime(), Some("text/plain"));
        assert!(copied.image_mime().is_some());
        assert_eq!(item(&["image/png"]).preferred_text_mime(), None);
    }

    #[test]
    fn the_mime_summary_counts_what_it_hides() {
        let item = item(&["text/plain", "text/html", "text/rtf"]);
        assert_eq!(item.mime_summary(2), "text/plain · text/html · +1");
        assert_eq!(item.mime_summary(3), "text/plain · text/html · text/rtf");
        assert_eq!(Item::default().mime_summary(3), "no mime type published");
    }

    #[test]
    fn an_unpublished_authority_is_neither_a_yes_nor_a_no() {
        let authority = Authority::default();
        assert_eq!(authority.can_write(), None);
        assert_eq!(authority.class_label(), "unpublished");
    }

    #[test]
    fn a_read_only_class_publishes_no_write_operation() {
        let authority = Authority {
            rows: vec![CapabilityRow {
                id: "smart-lib:readonly".into(),
                target: "service".into(),
                operations: vec!["read".into(), "subscribe".into()],
                issuer: "smart-lib".into(),
            }],
            operations: vec!["read".into(), "subscribe".into()],
            class_id: Some("smart-lib:readonly".into()),
        };
        assert_eq!(authority.can_write(), Some(false));
        assert_eq!(authority.class_label(), "read-only");
    }

    #[test]
    fn the_matrix_records_the_latest_answer_per_verb() {
        let mut info = ServiceInfo {
            methods: ServiceInfo::new_matrix(),
            ..ServiceInfo::default()
        };
        info.note_answered("getHistory");
        let refusal = Refusal {
            verb: "setContent".into(),
            class: crate::error::RefusalClass::PermissionDenied,
            reason: "read-only class".into(),
            raw: "permissionDenied: read-only class".into(),
        };
        info.note_refused(&refusal);
        let (answered, refused, total) = info.counts();
        assert_eq!((answered, refused), (1, 1));
        assert_eq!(total, contract::CLIPBOARD_METHODS.len());
    }
}
