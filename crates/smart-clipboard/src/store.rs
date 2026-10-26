//! The UI's state, and the reducer that moves it.
//!
//! This module has no gpui types in it at all. That is on purpose: every
//! interesting decision the app makes — what a refusal does to the banner,
//! whether a search replaces or appends, when a preview is worth asking for,
//! what "the newest copy arrived" should cost — is a pure function over data,
//! unit-tested without a window. The views then become what they should be: a
//! projection of this struct onto pixels.
//!
//! ## The two rules the reducer enforces
//!
//! 1. **The service is the list.** Nothing is prepended locally when a copy
//!    event arrives: a change event triggers a `Refresh`, so the row that
//!    appears is a row the service actually returns (and carries the id, the
//!    MIME list and the size the service recorded, not a guess from a string
//!    that happened to arrive).
//! 2. **Every wait has a visible state.** `PreviewState::NotRequested` is a
//!    skeleton, `Loading` is a progress line, `Refused` is a reason. No row is
//!    ever blank "because we have not thought about it yet".

use std::collections::HashMap;
use std::time::SystemTime;

use crate::query::{self, Parsed};
use smart_clipboard_proto::{
    Authority, BackendEvent, ChangeEvent, ChangeKind, Command, ContentState, CopyTarget, Item,
    ItemId, LiveContent, MimeFamily, Page, PreviewState, Query, Refusal, ServiceInfo,
};

/// How many rows' previews the store asks for when a page lands. A screen of
/// rows plus a little lookahead: enough that scrolling does not stutter, few
/// enough that opening the app is not 200 round trips.
pub const PREVIEW_BUDGET: usize = 48;

/// How many change events the ticker keeps.
const CHANGE_LOG: usize = 64;

/// How many toasts may stack.
const TOASTS: usize = 4;

/// Where the connection stands, in the app's own words.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Connection {
    /// Before the worker has said anything.
    #[default]
    Idle,
    /// Dialling one endpoint.
    Dialling {
        /// The endpoint being attempted.
        endpoint: String,
    },
    /// Handshake complete.
    Connected {
        /// The endpoint the session is on.
        endpoint: String,
    },
    /// The connection died.
    Lost {
        /// Why.
        reason: String,
        /// Whether the worker will retry on its own.
        retrying: bool,
    },
    /// Every candidate failed.
    Failed {
        /// The headline reason.
        reason: String,
    },
}

impl Connection {
    /// The short label the status bar shows.
    pub fn label(&self) -> String {
        match self {
            Connection::Idle => "idle".to_string(),
            Connection::Dialling { endpoint } => format!("dialling {endpoint}"),
            Connection::Connected { endpoint } => endpoint.clone(),
            Connection::Lost { retrying, .. } => {
                if *retrying {
                    "connection lost · retrying".to_string()
                } else {
                    "connection lost".to_string()
                }
            }
            Connection::Failed { .. } => "no service".to_string(),
        }
    }

    /// Whether the app should behave as "connected".
    pub fn is_connected(&self) -> bool {
        matches!(self, Connection::Connected { .. })
    }
}

/// One entry in the connection panel's attempt log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialRecord {
    /// The endpoint tried.
    pub endpoint: String,
    /// Where the candidate came from (`--socket`, env, …).
    pub source: String,
    /// Why it failed, or `None` while it is being dialled.
    pub failure: Option<String>,
}

/// The rail's facets. Every one of them is a **client-side** filter over the
/// loaded window: the contract's search is text only, so a filter that claimed
/// to search the whole history would be a lie. The rail's counts therefore
/// show loaded items, and the header says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Facet {
    /// Everything the window holds.
    #[default]
    All,
    /// `text/*`.
    Text,
    /// A URL payload.
    Link,
    /// `image/*` — thumbnails live here.
    Image,
    /// A file reference.
    File,
    /// Pinned by the service.
    Pinned,
    /// The app's own secret heuristic, labelled as such in the UI.
    Sensitive,
}

impl Facet {
    /// The rail label.
    pub fn label(self) -> &'static str {
        match self {
            Facet::All => "All",
            Facet::Text => "Text",
            Facet::Link => "Links",
            Facet::Image => "Images",
            Facet::File => "Files",
            Facet::Pinned => "Pinned",
            Facet::Sensitive => "Sensitive",
        }
    }

    /// The rail order.
    pub const ORDER: [Facet; 7] = [
        Facet::All,
        Facet::Text,
        Facet::Link,
        Facet::Image,
        Facet::File,
        Facet::Pinned,
        Facet::Sensitive,
    ];

    /// Whether an item passes this facet, given its preview text when the
    /// store has one (the Sensitive facet needs text; without it the row is
    /// simply not claimed either way).
    pub fn admits(self, item: &Item, preview_text: Option<&str>) -> bool {
        match self {
            Facet::All => true,
            Facet::Text => item.family() == MimeFamily::Text,
            Facet::Link => {
                item.family() == MimeFamily::Link
                    || preview_text
                        .map(|text| crate::format::url_literal(text).is_some())
                        .unwrap_or(false)
            }
            Facet::Image => item.family() == MimeFamily::Image,
            Facet::File => item.family() == MimeFamily::File,
            Facet::Pinned => item.pinned,
            Facet::Sensitive => preview_text
                .map(crate::format::looks_sensitive)
                .unwrap_or(false),
        }
    }
}

/// The order the loaded window is shown in. All of these are local: the
/// contract has no sort parameter, and the header says "loaded".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sort {
    /// The service's own order, which is newest-first on every backend that
    /// serves the domain today.
    #[default]
    ServiceOrder,
    /// Oldest first.
    Oldest,
    /// Largest payload first.
    Largest,
    /// Pinned items first, then the service's order.
    PinnedFirst,
}

impl Sort {
    /// The control's label.
    pub fn label(self) -> &'static str {
        match self {
            Sort::ServiceOrder => "service order",
            Sort::Oldest => "oldest first",
            Sort::Largest => "largest first",
            Sort::PinnedFirst => "pinned first",
        }
    }

    /// The next sort in the cycle, for the one-key toggle.
    pub fn next(self) -> Self {
        match self {
            Sort::ServiceOrder => Sort::Oldest,
            Sort::Oldest => Sort::Largest,
            Sort::Largest => Sort::PinnedFirst,
            Sort::PinnedFirst => Sort::ServiceOrder,
        }
    }
}

/// The two layouts the library offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Layout {
    /// Dense rows: one line of content plus metadata. The default, because
    /// scanning text is what a clipboard is mostly used for.
    #[default]
    List,
    /// A thumbnail grid, for the "find that screenshot" task where a picture
    /// is faster to recognise than any description.
    Gallery,
}

/// A transient message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// The title, one line.
    pub title: String,
    /// The detail, one or two lines, non-empty when it carries a reason.
    pub detail: String,
    /// The severity.
    pub kind: ToastKind,
    /// When it was raised, for auto-dismiss.
    pub raised: SystemTime,
}

/// Toast severity. Named by effect, not by colour: the colour is derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Neutral information.
    Info,
    /// Something worked.
    Good,
    /// Something needs attention but is not broken.
    Warn,
    /// Something failed.
    Bad,
}

/// Everything the app renders.
#[derive(Default)]
pub struct Store {
    /// Dark mode.
    pub theme_dark: bool,
    /// The connection state.
    pub connection: Connection,
    /// Every endpoint attempt this session, in order.
    pub dials: Vec<DialRecord>,
    /// What discovery said.
    pub service: Option<ServiceInfo>,
    /// What capability said.
    pub authority: Authority,
    /// Other services the host publishes, for the "what else is here" list.
    pub published: Vec<String>,
    /// The loaded window.
    pub items: Vec<Item>,
    /// The service's total for the current query.
    pub total: u64,
    /// Whether the window has reached the total.
    pub exhausted: bool,
    /// Previews by item id.
    pub previews: HashMap<u64, PreviewState>,
    /// Payload states by item id.
    pub contents: HashMap<u64, ContentState>,
    /// The selected item.
    pub selected: Option<ItemId>,
    /// The text in the search field, verbatim (what the user typed).
    pub query: String,
    /// The search field, parsed: the service's half and the local predicates.
    ///
    /// Kept as data rather than re-parsed on every frame: the header draws its
    /// description, the list filters by it, and both must agree.
    pub parsed: Parsed,
    /// The clock the app is currently using for relative times and time filters.
    /// Set from every event, so "2 minutes ago" and `newer:2m` cannot disagree.
    pub now_ns: u64,
    /// The text the current window was fetched for (what the header reports).
    pub applied_query: String,
    /// The facet filter.
    pub facet: Facet,
    /// The sort.
    pub sort: Sort,
    /// The layout.
    pub layout: Layout,
    /// Recent change events, newest first.
    pub changes: Vec<ChangeEvent>,
    /// The live selection, when the app has read it.
    pub live: Option<Result<LiveContent, Refusal>>,
    /// Transient messages, oldest first.
    pub toasts: Vec<Toast>,
    /// Session notices (the worker's own log line), newest first.
    pub notices: Vec<String>,
    /// The last refusal the session saw, for the inspector's status block.
    pub last_refusal: Option<Refusal>,
    /// Whether a page is in flight.
    pub loading: bool,
}

impl Store {
    /// A store before anything has happened.
    pub fn new(theme_dark: bool) -> Self {
        Store {
            theme_dark,
            connection: Connection::Idle,
            ..Store::default()
        }
    }

    // ── derived views ──────────────────────────────────────────────────────

    /// The preview text of an item, when the service served one.
    pub fn preview_text(&self, id: ItemId) -> Option<&str> {
        self.previews.get(&id.0).and_then(|state| state.snippet())
    }

    /// The rows the list should draw: facet filter, then sort, both local.
    pub fn visible(&self) -> Vec<Item> {
        let mut rows: Vec<Item> = self
            .items
            .iter()
            .filter(|item| {
                self.facet.admits(item, self.preview_text(item.id))
                    // The second filter is the search field's local half. Both
                    // are local, and the header says so.
                    && self
                        .parsed
                        .matches(item, self.preview_text(item.id), self.now_ns)
            })
            .cloned()
            .collect();
        match self.sort {
            Sort::ServiceOrder => {}
            Sort::Oldest => rows.reverse(),
            Sort::Largest => rows.sort_by(|left, right| right.size_bytes.cmp(&left.size_bytes)),
            Sort::PinnedFirst => {
                rows.sort_by_key(|item| !item.pinned);
            }
        }
        rows
    }

    /// How many loaded items the current facet hides. Shown in the header so
    /// a filter never silently swallows rows.
    pub fn hidden_by_facet(&self) -> usize {
        self.items.len() - self.visible().len()
    }

    /// The selected item, if it is still in the window.
    pub fn selected_item(&self) -> Option<&Item> {
        let id = self.selected?;
        self.items.iter().find(|item| item.id == id)
    }

    /// The index of the selection within [`Store::visible`], for keyboard
    /// movement.
    pub fn selected_visible_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.visible().iter().position(|item| item.id == id)
    }

    /// Whether the app may *offer* a write. `None` is "the service did not
    /// say", which the UI renders as a hint, never as a block: the verb is
    /// still callable and its refusal is the truth.
    pub fn can_write(&self) -> Option<bool> {
        self.authority.can_write()
    }

    /// A one-line provenance string for the status bar.
    pub fn provenance(&self) -> String {
        match &self.service {
            Some(service) => format!(
                "{} · {} · schema 0x{:016x}{}",
                service.name,
                service.state,
                service.schema_id,
                if service.schema_matches_app {
                    ""
                } else {
                    " · drift"
                }
            ),
            None => "no service".to_string(),
        }
    }

    /// The capture stream's state, in words.
    pub fn capture_label(&self) -> &'static str {
        if !self.connection.is_connected() {
            return "capture: no service";
        }
        if self.changes.is_empty() && self.items.is_empty() {
            return "capture: waiting for the first copy";
        }
        "capture: live"
    }

    // ── reducers ───────────────────────────────────────────────────────────

    /// Apply one worker event, returning the commands the event implies.
    ///
    /// Returning commands (rather than reaching for the worker) is what keeps
    /// this module pure: the app forwards them, and a test can assert on them.
    pub fn apply(&mut self, event: BackendEvent, now_ns: u64) -> Vec<Command> {
        let mut effects = Vec::new();
        // One clock for timestamps and time filters, advanced by every event.
        self.now_ns = now_ns;
        match event {
            BackendEvent::Dialling { endpoint, source } => {
                self.connection = Connection::Dialling {
                    endpoint: endpoint.clone(),
                };
                if let Some(record) = self.dials.iter_mut().find(|r| r.endpoint == endpoint) {
                    record.failure = None;
                } else {
                    self.dials.push(DialRecord {
                        endpoint,
                        source: source.to_string(),
                        failure: None,
                    });
                }
            }
            BackendEvent::DialFailed { endpoint, reason } => {
                if let Some(record) = self.dials.iter_mut().find(|r| r.endpoint == endpoint) {
                    record.failure = Some(reason);
                } else {
                    self.dials.push(DialRecord {
                        endpoint,
                        source: "unknown".to_string(),
                        failure: Some(reason),
                    });
                }
            }
            BackendEvent::Connected {
                endpoint,
                service,
                authority,
                published,
            } => {
                self.connection = Connection::Connected { endpoint };
                self.service = Some(service);
                self.authority = authority;
                self.published = published;
                self.toast(
                    ToastKind::Good,
                    "connected",
                    format!("{} serves the clipboard contract", self.provenance()),
                );
            }
            BackendEvent::ConnectFailed { attempts, reason } => {
                self.connection = Connection::Failed {
                    reason: reason.clone(),
                };
                for (endpoint, failure) in attempts {
                    if let Some(record) = self.dials.iter_mut().find(|r| r.endpoint == endpoint) {
                        record.failure = Some(failure);
                    }
                }
            }
            BackendEvent::Disconnected { reason, retrying } => {
                self.connection = Connection::Lost { reason, retrying };
                self.loading = false;
            }
            BackendEvent::Page { page, append } => {
                self.loading = false;
                if !append {
                    // A late page for a query the user has already replaced
                    // must not overwrite the newer window.
                    if page.query.text != self.applied_query
                        && !(page.query.text.is_empty() && self.applied_query.is_empty())
                    {
                        return effects;
                    }
                    self.items = page.items;
                } else {
                    self.items.extend(page.items);
                }
                self.total = page.total;
                self.exhausted = self.items.len() as u64 >= page.total;
                if self.selected.is_none() {
                    if let Some(first) = self.visible().first().map(|item| item.id) {
                        effects.extend(self.select(first));
                    }
                }
                effects.extend(self.request_missing_previews());
            }
            BackendEvent::Preview { id, state } => {
                self.previews.insert(id.0, state);
            }
            BackendEvent::Content { id, state } => {
                self.contents.insert(id.0, state);
            }
            BackendEvent::Copied { id, report } => {
                self.toast(
                    ToastKind::Good,
                    "on the clipboard",
                    format!(
                        "{} · {} · {} representation{}",
                        report.mime,
                        crate::format::bytes(report.bytes),
                        report.offered,
                        if report.offered == 1 { "" } else { "s" }
                    ),
                );
                if let Some(id) = id {
                    // The service may have recorded this as a new item; ask it
                    // rather than assuming.
                    self.toast(
                        ToastKind::Info,
                        "paste it",
                        format!("{id} is held by the service; this app never sends keys for you"),
                    );
                }
                let _ = id;
            }
            BackendEvent::Changes { events } => {
                let copied = events
                    .iter()
                    .filter(|event| event.kind == ChangeKind::Copied)
                    .count();
                for event in events.into_iter().rev() {
                    self.changes.insert(0, event);
                }
                self.changes.truncate(CHANGE_LOG);
                if copied > 0 {
                    // The service is the list: a capture triggers a re-read so
                    // the new row carries the service's own id and metadata.
                    self.loading = true;
                    effects.push(Command::Refresh(Query::first(self.applied_query.clone())));
                }
            }
            BackendEvent::Live { content } => {
                self.live = Some(content);
            }
            BackendEvent::Refused { verb, refusal } => {
                if let Some(service) = self.service.as_mut() {
                    service.note_refused(&refusal);
                }
                self.last_refusal = Some(refusal.clone());
                let hint = if refusal.class.is_authority() {
                    " · reconnect with a control-capable connection"
                } else {
                    ""
                };
                self.toast(
                    ToastKind::Warn,
                    format!("{verb} refused"),
                    format!("{}{hint}", refusal.headline()),
                );
            }
            BackendEvent::Failed { verb, message } => {
                self.toast(ToastKind::Bad, format!("{verb} failed"), message);
            }
            BackendEvent::Notice { message } => {
                self.notices.insert(0, message);
                self.notices.truncate(32);
            }
            BackendEvent::Stopped => {
                self.connection = Connection::Failed {
                    reason: "the protocol worker stopped".to_string(),
                };
            }
        }
        let _ = now_ns;
        effects
    }

    /// Replace the query. The caller owns debouncing; this records the intent
    /// and returns the fetch.
    pub fn set_query(&mut self, text: String) -> Vec<Command> {
        self.query = text;
        // One box, two jobs: the predicates stay here, the words go to the
        // service's own search. Sending `kind:image` to `searchHistory` would
        // find nothing, which is why the split has to happen before the call.
        self.parsed = query::parse(&self.query);
        self.applied_query = self.parsed.service.clone();
        self.selected = None;
        self.loading = true;
        vec![Command::Refresh(Query::first(self.applied_query.clone()))]
    }

    /// The local half of the search field, described for the header. `None` when
    /// the app is filtering by nothing but the service's own text search.
    pub fn local_filter_label(&self) -> Option<String> {
        if self.parsed.is_local() {
            Some(format!(
                "{} applied to the {} loaded rows",
                self.parsed.describe(),
                self.items.len()
            ))
        } else {
            None
        }
    }

    /// Re-run the current query.
    pub fn refresh(&mut self) -> Vec<Command> {
        self.loading = true;
        vec![Command::Refresh(Query::first(self.applied_query.clone()))]
    }

    /// Fetch the next page.
    pub fn load_more(&mut self) -> Vec<Command> {
        if self.exhausted || !self.connection.is_connected() {
            return Vec::new();
        }
        self.loading = true;
        vec![Command::LoadMore]
    }

    /// Select an item, and ask for the two things its inspector needs.
    pub fn select(&mut self, id: ItemId) -> Vec<Command> {
        self.selected = Some(id);
        let mut effects = Vec::new();
        if !matches!(
            self.previews.get(&id.0),
            Some(PreviewState::Ready { .. }) | Some(PreviewState::Unavailable { .. })
        ) {
            self.previews.insert(id.0, PreviewState::Loading);
            effects.push(Command::Preview(vec![id]));
        }
        if let Some(item) = self.items.iter().find(|item| item.id == id) {
            if let Some(mime) = item.image_mime() {
                if !matches!(self.contents.get(&id.0), Some(ContentState::Ready { .. })) {
                    effects.push(Command::Content {
                        id,
                        mime: mime.to_string(),
                    });
                }
            }
        }
        effects
    }

    /// Move the selection by `delta` rows within the visible list.
    pub fn move_selection(&mut self, delta: isize) -> Vec<Command> {
        let rows = self.visible();
        if rows.is_empty() {
            return Vec::new();
        }
        let current = self.selected_visible_index().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
        let id = rows[next].id;
        if self.selected == Some(id) {
            return Vec::new();
        }
        self.select(id)
    }

    /// Select the first row.
    pub fn select_first(&mut self) -> Vec<Command> {
        match self.visible().first().map(|item| item.id) {
            Some(id) => self.select(id),
            None => Vec::new(),
        }
    }

    /// Copy the selection back through the service.
    pub fn copy_selected(&mut self, all_representations: bool) -> Vec<Command> {
        let Some(item) = self.selected_item().cloned() else {
            return Vec::new();
        };
        if all_representations {
            return vec![Command::Copy(CopyTarget::AllRepresentations {
                id: item.id,
            })];
        }
        let mime = item
            .mimes
            .first()
            .cloned()
            .unwrap_or_else(|| "text/plain".to_string());
        vec![Command::Copy(CopyTarget::Representation {
            id: item.id,
            mime,
        })]
    }

    /// Copy one specific representation (the inspector's "copy as" list).
    pub fn copy_representation(&mut self, id: ItemId, mime: String) -> Vec<Command> {
        vec![Command::Copy(CopyTarget::Representation { id, mime })]
    }

    /// Ask the service to pin or unpin the selection.
    pub fn pin_selected(&mut self) -> Vec<Command> {
        let Some(item) = self.selected_item().cloned() else {
            return Vec::new();
        };
        vec![Command::SetPinned {
            id: item.id,
            pinned: !item.pinned,
        }]
    }

    /// Ask the service to delete the selection. The caller confirms first:
    /// this is a destructive verb and the app is not its owner.
    pub fn delete_selected(&mut self) -> Vec<Command> {
        let Some(item) = self.selected_item().cloned() else {
            return Vec::new();
        };
        vec![Command::Delete(item.id)]
    }

    /// Re-read the live selection.
    pub fn read_live(&mut self) -> Vec<Command> {
        vec![Command::ReadLive {
            mime: "text/plain".to_string(),
        }]
    }

    /// Retry the connection from the top of the candidate list.
    pub fn reconnect(&mut self) -> Vec<Command> {
        self.dials.clear();
        self.connection = Connection::Idle;
        self.items.clear();
        self.previews.clear();
        self.contents.clear();
        self.selected = None;
        self.changes.clear();
        vec![Command::Connect]
    }

    /// Ask for the previews of the visible window's first rows that have none.
    pub fn request_missing_previews(&mut self) -> Vec<Command> {
        let mut wanted = Vec::new();
        for item in self.visible().into_iter().take(PREVIEW_BUDGET) {
            let known = matches!(
                self.previews.get(&item.id.0),
                Some(PreviewState::Ready { .. })
                    | Some(PreviewState::Unavailable { .. })
                    | Some(PreviewState::Refused { .. })
                    | Some(PreviewState::Loading)
            );
            if !known {
                self.previews.insert(item.id.0, PreviewState::Loading);
                wanted.push(item.id);
            }
        }
        if wanted.is_empty() {
            Vec::new()
        } else {
            vec![Command::Preview(wanted)]
        }
    }

    /// Ask for the image payloads of the first visible image rows, so the
    /// grid and the row thumbnails can draw.
    pub fn request_missing_thumbnails(&mut self, budget: usize) -> Vec<Command> {
        // The candidates are collected first: the filter needs to read
        // `self.contents`, and the body must write it, so holding a borrowing
        // iterator across the mutation would not compile — and would be the
        // wrong shape anyway (a filter that changes what it filters).
        let candidates: Vec<Item> = self
            .visible()
            .into_iter()
            .filter(|item| item.family() == MimeFamily::Image)
            .collect();
        let mut wanted = Vec::new();
        for item in candidates {
            if wanted.len() >= budget {
                break;
            }
            if matches!(
                self.contents.get(&item.id.0),
                Some(ContentState::Ready { .. }) | Some(ContentState::Loading)
            ) {
                continue;
            }
            let Some(mime) = item.image_mime().map(|mime| mime.to_string()) else {
                continue;
            };
            self.contents.insert(item.id.0, ContentState::Loading);
            wanted.push(Command::Content { id: item.id, mime });
        }
        wanted
    }

    /// Raise a toast.
    pub fn toast(&mut self, kind: ToastKind, title: impl Into<String>, detail: impl Into<String>) {
        let toast = Toast {
            title: title.into(),
            detail: detail.into(),
            kind,
            raised: SystemTime::now(),
        };
        // A repeated identical toast would otherwise stack four deep.
        if self
            .toasts
            .last()
            .map(|last| last.title == toast.title && last.detail == toast.detail)
            .unwrap_or(false)
        {
            return;
        }
        self.toasts.push(toast);
        if self.toasts.len() > TOASTS {
            let overflow = self.toasts.len() - TOASTS;
            self.toasts.drain(0..overflow);
        }
    }

    /// Drop toasts older than `ttl`, so the app does not need a timer per
    /// toast.
    pub fn expire_toasts(&mut self, ttl: std::time::Duration) {
        let now = SystemTime::now();
        self.toasts.retain(|toast| {
            now.duration_since(toast.raised)
                .map(|age| age < ttl)
                .unwrap_or(true)
        });
    }

    /// A page's item count, for the header's "showing N of M".
    pub fn window_label(&self) -> String {
        let visible = self.visible().len();
        let hidden = self.hidden_by_facet();
        if hidden > 0 {
            format!(
                "{visible} shown · {hidden} hidden by filter · {} on the service",
                self.total
            )
        } else {
            format!("{visible} of {} on the service", self.total)
        }
    }
}

/// The page's items, without the store: a helper for views that only need to
/// know whether a page is empty.
pub fn page_is_empty(page: &Page) -> bool {
    page.items.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smart_clipboard_proto::{ChangeEvent, ChangeKind, RefusalClass};

    fn item(id: u64, mime: &str, size: u64, pinned: bool) -> Item {
        Item {
            id: ItemId(id),
            captured_at_ns: Some(id * 1_000_000_000),
            mimes: vec![mime.to_string()],
            tags: Vec::new(),
            pinned,
            size_bytes: size,
        }
    }

    fn page(items: Vec<Item>) -> BackendEvent {
        BackendEvent::Page {
            page: Page {
                total: items.len() as u64,
                items,
                query: Query::first(""),
            },
            append: false,
        }
    }

    #[test]
    fn a_page_selects_the_first_row_and_asks_for_its_preview() {
        let mut store = Store::new(true);
        let effects = store.apply(page(vec![item(1, "text/plain", 10, false)]), 0);
        assert_eq!(store.selected, Some(ItemId(1)));
        assert!(effects
            .iter()
            .any(|command| matches!(command, Command::Preview(ids) if ids.contains(&ItemId(1)))));
    }

    #[test]
    fn a_captured_copy_triggers_a_re_read_instead_of_a_local_insert() {
        let mut store = Store::new(true);
        store.connection = Connection::Connected {
            endpoint: "unix:///tmp/x.sock".to_string(),
        };
        let effects = store.apply(
            BackendEvent::Changes {
                events: vec![ChangeEvent {
                    id: ItemId(9),
                    at_ns: Some(1),
                    source: 1,
                    kind: ChangeKind::Copied,
                    mimes: vec!["text/plain".to_string()],
                }],
            },
            0,
        );
        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], Command::Refresh(_)));
        assert!(store.items.is_empty(), "the store invents no rows");
        assert_eq!(store.changes.len(), 1);
    }

    #[test]
    fn a_paste_event_does_not_trigger_a_refresh() {
        let mut store = Store::new(true);
        let effects = store.apply(
            BackendEvent::Changes {
                events: vec![ChangeEvent {
                    id: ItemId(9),
                    at_ns: Some(1),
                    source: 1,
                    kind: ChangeKind::Pasted,
                    mimes: vec![],
                }],
            },
            0,
        );
        assert!(effects.is_empty());
    }

    #[test]
    fn a_stale_page_for_an_old_query_is_dropped() {
        let mut store = Store::new(true);
        store.applied_query = "new".to_string();
        let event = BackendEvent::Page {
            page: Page {
                total: 1,
                items: vec![item(1, "text/plain", 1, false)],
                query: Query::first("old"),
            },
            append: false,
        };
        let effects = store.apply(event, 0);
        assert!(effects.is_empty());
        assert!(store.items.is_empty());
    }

    #[test]
    fn facets_filter_locally_and_report_what_they_hide() {
        let mut store = Store::new(true);
        store.apply(
            page(vec![
                item(1, "text/plain", 10, false),
                item(2, "image/png", 20, true),
            ]),
            0,
        );
        assert_eq!(store.visible().len(), 2);
        store.facet = Facet::Image;
        assert_eq!(store.visible().len(), 1);
        assert_eq!(store.hidden_by_facet(), 1);
        assert!(store.window_label().contains("1 hidden by filter"));
    }

    #[test]
    fn sorts_are_local_and_stable_enough_to_be_predictable() {
        let mut store = Store::new(true);
        store.apply(
            page(vec![
                item(1, "text/plain", 10, false),
                item(2, "text/plain", 30, true),
                item(3, "text/plain", 20, false),
            ]),
            0,
        );
        store.sort = Sort::Largest;
        let sizes: Vec<u64> = store.visible().iter().map(|item| item.size_bytes).collect();
        assert_eq!(sizes, vec![30, 20, 10]);
        store.sort = Sort::PinnedFirst;
        assert_eq!(store.visible()[0].id, ItemId(2));
    }

    #[test]
    fn keyboard_movement_clamps_at_both_ends() {
        let mut store = Store::new(true);
        store.apply(
            page(vec![
                item(1, "text/plain", 1, false),
                item(2, "text/plain", 2, false),
            ]),
            0,
        );
        store.select(ItemId(1));
        store.move_selection(-5);
        assert_eq!(store.selected, Some(ItemId(1)));
        store.move_selection(1);
        assert_eq!(store.selected, Some(ItemId(2)));
        store.move_selection(5);
        assert_eq!(store.selected, Some(ItemId(2)));
    }

    #[test]
    fn a_refusal_is_recorded_on_the_matrix_and_toasted_with_its_class() {
        let mut store = Store::new(true);
        store.service = Some(ServiceInfo {
            methods: ServiceInfo::new_matrix(),
            ..ServiceInfo::default()
        });
        store.apply(
            BackendEvent::Refused {
                verb: "setContent".to_string(),
                refusal: Refusal {
                    verb: "setContent".to_string(),
                    class: RefusalClass::PermissionDenied,
                    reason: "read-only connection".to_string(),
                    raw: "permissionDenied: read-only connection".to_string(),
                },
            },
            0,
        );
        assert!(store
            .service
            .as_ref()
            .unwrap()
            .methods
            .iter()
            .any(|row| row.name == "setContent"
                && !matches!(
                    row.observation,
                    smart_clipboard_proto::Observation::Untested
                )));
        let toast = store.toasts.last().expect("a toast");
        assert_eq!(toast.kind, ToastKind::Warn);
        assert!(toast.detail.contains("permissionDenied"));
        assert!(toast.detail.contains("control-capable"));
    }

    #[test]
    fn duplicate_toasts_collapse() {
        let mut store = Store::new(true);
        store.toast(ToastKind::Info, "hi", "there");
        store.toast(ToastKind::Info, "hi", "there");
        assert_eq!(store.toasts.len(), 1);
    }

    #[test]
    fn copy_uses_the_items_first_representation() {
        let mut store = Store::new(true);
        store.apply(page(vec![item(1, "image/png", 10, false)]), 0);
        let effects = store.copy_selected(false);
        assert!(matches!(
            effects.as_slice(),
            [Command::Copy(CopyTarget::Representation { mime, .. })] if mime == "image/png"
        ));
    }

    #[test]
    fn a_disconnect_keeps_the_rows_but_stops_the_loading_state() {
        let mut store = Store::new(true);
        store.apply(page(vec![item(1, "text/plain", 1, false)]), 0);
        store.apply(
            BackendEvent::Disconnected {
                reason: "socket closed".to_string(),
                retrying: true,
            },
            0,
        );
        assert_eq!(store.items.len(), 1);
        assert!(!store.loading);
        assert_eq!(store.connection.label(), "connection lost · retrying");
    }
}
