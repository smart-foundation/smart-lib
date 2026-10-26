//! The typed calls: one function per contract method this product uses, with
//! the wire's unions and lists turned into owned values.
//!
//! ## The shape of every call
//!
//! ```text
//!   build params ──▶ send ──▶ [timeout] ──▶ results ──▶ owned value
//!                       │                     │
//!                       └─ capnp::Error       └─ reader error
//!                          → Refusal             → Decode
//!                          (class preserved)
//! ```
//!
//! A capnp error is not "some error": the engine answers a served-but-refused
//! verb through `notSupported`/`permissionDenied` with its reason class in the
//! message, so [`crate::error::Refusal`] parses the class out and the UI can
//! say *why*. A `Disconnected` kind is different again — that is the transport
//! dying, and it drives the reconnect banner rather than a per-row error.
//!
//! ## What this client will not do
//!
//! * It will not write to the clipboard except through `setContent`/`setOffer`
//!   on a service that serves them. There is no fallback path, no `xclip`, no
//!   `wl-copy`: the app has no clipboard backend and must never grow one.
//! * It will not synthesise a paste. Even a successful copy leaves the paste
//!   keystroke to the user, because injecting input is a different domain's
//!   capability and inventing it here would be a privilege grab.
//! * It will not fetch a payload bigger than [`contract::MAX_FETCH_BYTES`]
//!   without saying so; the ceiling is labelled a client policy wherever it
//!   appears.

use std::future::Future;

use crate::contract;
use crate::error::{ClientError, Refusal};
use crate::model::{ChangeEvent, ChangeKind, ContentState, Item, ItemId, PreviewState};
use crate::schema::clipboard_capnp::clipboard_change_subscription as csub;
use crate::schema::clipboard_capnp::clipboard_content;
use crate::schema::types_capnp;
use crate::session::{text_of, Session};

/// One page request: the contract's `T.Pagination` plus the query text when
/// the call is a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The search text. Empty means "list everything" (`getHistory`).
    pub text: String,
    /// The contract's offset.
    pub offset: u32,
    /// The contract's limit. Never 0 here: the contract defines 0 as "one",
    /// so the app always states its page size explicitly.
    pub limit: u32,
}

impl Query {
    /// The first page for a query text.
    pub fn first(text: impl Into<String>) -> Self {
        Query {
            text: text.into(),
            offset: 0,
            limit: contract::PAGE_SIZE,
        }
    }

    /// The next page after a page of `items`.
    pub fn next(&self, items: usize) -> Self {
        Query {
            text: self.text.clone(),
            offset: self.offset + items as u32,
            limit: self.limit,
        }
    }

    /// Whether this query asks for the unfiltered history.
    pub fn is_unfiltered(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// One page of results, with the total the contract promises is stable across
/// two identical calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The items in this window.
    pub items: Vec<Item>,
    /// The service's total count for the query, not for the window.
    pub total: u64,
    /// The query that produced this page, echoed back so a late response for
    /// a stale query can be dropped by the UI rather than appended.
    pub query: Query,
}

/// What to copy back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyTarget {
    /// One representation of one stored item: `convertContent` then
    /// `setContent`.
    Representation {
        /// The item.
        id: ItemId,
        /// The representation to ask for.
        mime: String,
    },
    /// Every representation the service can produce for one item, published
    /// in one `setOffer` so a pasting app can choose (`setOffer @16`).
    AllRepresentations {
        /// The item.
        id: ItemId,
    },
}

/// What a copy did, in the numbers the UI reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyReport {
    /// The MIME that was offered first (the primary representation).
    pub mime: String,
    /// How many bytes the primary representation carried.
    pub bytes: u64,
    /// How many representations were offered in total.
    pub offered: usize,
}

/// A live clipboard read (`getContent @0`), owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveContent {
    /// The clipboard holds this representation.
    Content {
        /// Its MIME.
        mime: String,
        /// Its size, measured from the bytes.
        bytes: u64,
        /// A bounded text view when the payload is UTF-8 text.
        text: Option<String>,
    },
    /// The clipboard holds nothing. A first-class answer, not a fault.
    Empty,
}

/// The outcome of asking for one representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Representation {
    /// The service produced it.
    Content {
        /// The MIME it answered with (the service's spelling, not ours).
        mime: String,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// The service cannot produce that representation (`unsupported @1`).
    Unsupported {
        /// The representation that was asked for.
        target: String,
    },
}

/// The typed client over one [`Session`].
///
/// It borrows the session mutably so it can keep the session's live method
/// matrix up to date: an observation is only worth recording if it is
/// recorded where the call happened.
pub struct ClipboardClient<'a> {
    session: &'a mut Session,
}

impl<'a> ClipboardClient<'a> {
    /// Wrap a session.
    pub fn new(session: &'a mut Session) -> Self {
        ClipboardClient { session }
    }

    /// The session, for a caller that needs the discovery or capability
    /// handles.
    pub fn session(&mut self) -> &mut Session {
        self.session
    }

    /// `getHistory @3` / `searchHistory @7`, whichever the query asks for.
    pub async fn page(&mut self, query: &Query) -> Result<Page, ClientError> {
        if query.is_unfiltered() {
            self.history(query).await
        } else {
            self.search(query).await
        }
    }

    /// `getHistory @3`.
    async fn history(&mut self, query: &Query) -> Result<Page, ClientError> {
        let mut request = self.session.clipboard.get_history_request();
        {
            let mut params = request.get();
            let mut pagination = params.reborrow().init_pagination();
            pagination.set_offset(query.offset);
            pagination.set_limit(query.limit);
        }
        let response = timed("getHistory", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("getHistory results: {error}")))?;
        let total = results.get_total();
        let items = read_items(results.get_items())?;
        self.session.service.note_answered("getHistory");
        Ok(Page {
            items,
            total,
            query: query.clone(),
        })
    }

    /// `searchHistory @7`.
    async fn search(&mut self, query: &Query) -> Result<Page, ClientError> {
        let mut request = self.session.clipboard.search_history_request();
        {
            let mut params = request.get();
            params.set_query(&query.text);
            let mut pagination = params.reborrow().init_pagination();
            pagination.set_offset(query.offset);
            pagination.set_limit(query.limit);
        }
        let response = timed("searchHistory", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("searchHistory results: {error}")))?;
        let total = results.get_total();
        let items = read_items(results.get_items())?;
        self.session.service.note_answered("searchHistory");
        Ok(Page {
            items,
            total,
            query: query.clone(),
        })
    }

    /// `getPreview @11`.
    ///
    /// The contract's three answers become three states, and the "no preview"
    /// arm keeps a sentence that says *why* the app is showing a placeholder.
    pub async fn preview(&mut self, id: ItemId) -> PreviewState {
        let mut request = self.session.clipboard.get_preview_request();
        {
            let mut params = request.get();
            let mut item_id = params.reborrow().init_item_id();
            item_id.set_id(id.0);
        }
        let response = match timed("getPreview", request.send().promise).await {
            Ok(response) => response,
            Err(error) => return PreviewState::from(error),
        };
        let results = match response.get() {
            Ok(results) => results,
            Err(error) => {
                return PreviewState::Failed {
                    message: format!("getPreview results: {error}"),
                }
            }
        };
        let union = match results.get_result() {
            Ok(union) => union,
            Err(error) => {
                return PreviewState::Failed {
                    message: format!("getPreview result: {error}"),
                }
            }
        };
        use crate::schema::clipboard_capnp::preview_result::WhichReader;
        match union.which() {
            Ok(WhichReader::Preview(reader)) => match reader {
                // The union accessor itself can fail: the pointer is there but
                // the struct is not readable. That is a decode fault, and it is
                // reported as one rather than shown as "no preview".
                Ok(reader) => {
                    self.session.service.note_answered("getPreview");
                    PreviewState::Ready {
                        text: text_of(reader.get_text_preview()),
                        available: reader.get_available(),
                        original_size: reader.get_original_size(),
                        mime: reader
                            .get_preview_mime_type()
                            .map(|mime| mime_text(&mime))
                            .unwrap_or_default(),
                    }
                }
                Err(error) => PreviewState::Failed {
                    message: format!("getPreview preview: {error}"),
                },
            },
            Ok(WhichReader::Unknown(())) => {
                self.session.service.note_answered("getPreview");
                PreviewState::Unavailable {
                    reason: "the service answered `unknown`: it keeps no preview for this \
                             payload (an image or a binary has no text view)"
                        .to_string(),
                }
            }
            Ok(WhichReader::Error(error)) => PreviewState::Failed {
                message: text_of(error.and_then(|error| error.get_message())),
            },
            Err(error) => PreviewState::Failed {
                message: format!("getPreview union: {error}"),
            },
        }
    }

    /// `convertContent @13`, returning the bytes for one representation.
    ///
    /// `Err(ClientError::Refused)` for a real refusal, and the dedicated
    /// [`Representation::Unsupported`] outcome for the contract's own "I
    /// cannot convert to that" answer — two different facts, so two different
    /// shapes.
    pub async fn representation(
        &mut self,
        id: ItemId,
        mime: &str,
    ) -> Result<Representation, ClientError> {
        let mut request = self.session.clipboard.convert_content_request();
        {
            let mut params = request.get();
            let mut item_id = params.reborrow().init_item_id();
            item_id.set_id(id.0);
            let mut target = params.reborrow().init_target_mime_type();
            set_mime(&mut target, mime);
        }
        let response = timed("convertContent", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("convertContent results: {error}")))?;
        self.session.service.note_answered("convertContent");
        let union = results
            .get_result()
            .map_err(|error| ClientError::Decode(format!("convertContent result: {error}")))?;
        use crate::schema::clipboard_capnp::conversion_result::WhichReader;
        match union.which() {
            Ok(WhichReader::Content(reader)) => {
                let Ok(reader) = reader else {
                    return Err(ClientError::Decode(
                        "convertContent content: the payload is not readable".to_string(),
                    ));
                };
                let (mime, bytes) = read_content(&reader);
                Ok(Representation::Content { mime, bytes })
            }
            Ok(WhichReader::Unsupported(())) => Ok(Representation::Unsupported {
                target: mime.to_string(),
            }),
            Ok(WhichReader::Error(error)) => Err(ClientError::Refused(Refusal::from_reason_text(
                "convertContent",
                &text_of(error.and_then(|error| error.get_message())),
            ))),
            Ok(WhichReader::Unknown(())) => Err(ClientError::Decode(
                "convertContent: the service answered `unknown`".to_string(),
            )),
            Err(error) => Err(ClientError::Decode(format!(
                "convertContent union: {error}"
            ))),
        }
    }

    /// `setContent @1`, asked to hold one representation.
    pub async fn set_content(&mut self, mime: &str, bytes: &[u8]) -> Result<(), ClientError> {
        let mut request = self.session.clipboard.set_content_request();
        {
            let mut params = request.get();
            let mut content = params.reborrow().init_content();
            write_content(&mut content, mime, bytes);
        }
        let response = timed("setContent", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("setContent results: {error}")))?;
        let outcome = check_void("setContent", results.get_result());
        self.note_outcome("setContent", &outcome);
        outcome
    }

    /// `setOffer @16`, asked to hold several representations of one copy at
    /// once — the contract's own multi-representation verb.
    pub async fn set_offer(
        &mut self,
        representatives: &[(String, Vec<u8>)],
    ) -> Result<(), ClientError> {
        let mut request = self.session.clipboard.set_offer_request();
        {
            let mut params = request.get();
            let mut offers = params.reborrow().init_offers(representatives.len() as u32);
            for (index, (mime, bytes)) in representatives.iter().enumerate() {
                let mut content = offers.reborrow().get(index as u32);
                write_content(&mut content, mime, bytes);
            }
        }
        let response = timed("setOffer", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("setOffer results: {error}")))?;
        let outcome = check_void("setOffer", results.get_result());
        self.note_outcome("setOffer", &outcome);
        outcome
    }

    /// `pinItem @8` / `unpinItem @9`.
    pub async fn set_pinned(&mut self, id: ItemId, pinned: bool) -> Result<(), ClientError> {
        if pinned {
            let mut request = self.session.clipboard.pin_item_request();
            {
                let mut params = request.get();
                let mut item_id = params.reborrow().init_item_id();
                item_id.set_id(id.0);
            }
            let response = timed("pinItem", request.send().promise).await?;
            let results = response
                .get()
                .map_err(|error| ClientError::Decode(format!("pinItem results: {error}")))?;
            let outcome = check_void("pinItem", results.get_result());
            self.note_outcome("pinItem", &outcome);
            outcome
        } else {
            let mut request = self.session.clipboard.unpin_item_request();
            {
                let mut params = request.get();
                let mut item_id = params.reborrow().init_item_id();
                item_id.set_id(id.0);
            }
            let response = timed("unpinItem", request.send().promise).await?;
            let results = response
                .get()
                .map_err(|error| ClientError::Decode(format!("unpinItem results: {error}")))?;
            let outcome = check_void("unpinItem", results.get_result());
            self.note_outcome("unpinItem", &outcome);
            outcome
        }
    }

    /// `deleteItem @10`.
    pub async fn delete(&mut self, id: ItemId) -> Result<(), ClientError> {
        let mut request = self.session.clipboard.delete_item_request();
        {
            let mut params = request.get();
            let mut item_id = params.reborrow().init_item_id();
            item_id.set_id(id.0);
        }
        let response = timed("deleteItem", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("deleteItem results: {error}")))?;
        let outcome = check_void("deleteItem", results.get_result());
        self.note_outcome("deleteItem", &outcome);
        outcome
    }

    /// `clearHistory @4`.
    pub async fn clear_history(&mut self) -> Result<(), ClientError> {
        let request = self.session.clipboard.clear_history_request();
        let response = timed("clearHistory", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("clearHistory results: {error}")))?;
        let outcome = check_void("clearHistory", results.get_result());
        self.note_outcome("clearHistory", &outcome);
        outcome
    }

    /// `getSupportedMimeTypes @6`.
    pub async fn supported_mime_types(&mut self) -> Result<Vec<String>, ClientError> {
        let request = self.session.clipboard.get_supported_mime_types_request();
        let response = timed("getSupportedMimeTypes", request.send().promise).await?;
        let results = response.get().map_err(|error| {
            ClientError::Decode(format!("getSupportedMimeTypes results: {error}"))
        })?;
        let list = results
            .get_mime_types()
            .map_err(|error| ClientError::Decode(format!("mime list: {error}")))?;
        let mut mimes = Vec::new();
        for entry in list.iter() {
            mimes.push(mime_text(&entry));
        }
        self.session.service.note_answered("getSupportedMimeTypes");
        Ok(mimes)
    }

    /// `getContent @0`, the live selection.
    pub async fn live_content(&mut self, mime: &str) -> Result<LiveContent, ClientError> {
        let mut request = self.session.clipboard.get_content_request();
        {
            let mut params = request.get();
            let mut wanted = params.reborrow().init_mime_type();
            set_mime(&mut wanted, mime);
        }
        let response = timed("getContent", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("getContent results: {error}")))?;
        let union = results
            .get_result()
            .map_err(|error| ClientError::Decode(format!("getContent result: {error}")))?;
        self.session.service.note_answered("getContent");
        use crate::schema::clipboard_capnp::content_result::WhichReader;
        match union.which() {
            Ok(WhichReader::Content(reader)) => {
                let Ok(reader) = reader else {
                    return Err(ClientError::Decode(
                        "getContent content: the payload is not readable".to_string(),
                    ));
                };
                let (mime, bytes) = read_content(&reader);
                let size = bytes.len() as u64;
                let text = String::from_utf8(bytes).ok();
                Ok(LiveContent::Content {
                    mime,
                    bytes: size,
                    text,
                })
            }
            Ok(WhichReader::Empty(())) => Ok(LiveContent::Empty),
            Ok(WhichReader::Error(error)) => Err(ClientError::Refused(Refusal::from_reason_text(
                "getContent",
                &text_of(error.and_then(|error| error.get_message())),
            ))),
            Ok(WhichReader::Unknown(())) => Err(ClientError::Decode(
                "getContent: the service answered `unknown`".to_string(),
            )),
            Err(error) => Err(ClientError::Decode(format!(
                "getContent union: {error}"
            ))),
        }
    }

    /// `subscribeChanges @5`.
    pub async fn subscribe(&mut self) -> Result<csub::Client, ClientError> {
        let request = self.session.clipboard.subscribe_changes_request();
        let response = timed("subscribeChanges", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("subscribeChanges results: {error}")))?;
        let subscription = results
            .get_subscription()
            .map_err(|error| ClientError::Decode(format!("subscription: {error}")))?;
        self.session.service.note_answered("subscribeChanges");
        Ok(subscription)
    }

    /// `ClipboardChangeSubscription.pull @0`.
    pub async fn pull(
        subscription: &csub::Client,
        max_events: u32,
    ) -> Result<Vec<ChangeEvent>, ClientError> {
        let mut request = subscription.pull_request();
        {
            let mut params = request.get();
            params.set_max_events(max_events);
        }
        let response = timed("pullChanges", request.send().promise).await?;
        let results = response
            .get()
            .map_err(|error| ClientError::Decode(format!("pullChanges results: {error}")))?;
        let changes = results
            .get_changes()
            .map_err(|error| ClientError::Decode(format!("changes: {error}")))?;
        let mut events = Vec::new();
        for change in changes.iter() {
            events.push(ChangeEvent {
                id: ItemId(change.get_id().map(|id| id.get_id()).unwrap_or_default()),
                at_ns: match change.get_timestamp().map(|ts| ts.get_nanoseconds()) {
                    Ok(0) | Err(_) => None,
                    Ok(nanos) => Some(nanos),
                },
                source: change
                    .get_source_id()
                    .map(|source| source.get_id())
                    .unwrap_or_default(),
                kind: match change.get_event_type().unwrap_or_default() {
                    crate::schema::clipboard_capnp::ClipboardEventType::Copied => ChangeKind::Copied,
                    crate::schema::clipboard_capnp::ClipboardEventType::Pasted => ChangeKind::Pasted,
                    crate::schema::clipboard_capnp::ClipboardEventType::Cleared => {
                        ChangeKind::Cleared
                    }
                    crate::schema::clipboard_capnp::ClipboardEventType::Expired => {
                        ChangeKind::Expired
                    }
                    crate::schema::clipboard_capnp::ClipboardEventType::Custom => ChangeKind::Custom,
                },
                mimes: read_mimes(change.get_mime_types()),
            });
        }
        Ok(events)
    }

    // ── matrix bookkeeping ────────────────────────────────────────────────

    fn note_outcome(&mut self, verb: &str, outcome: &Result<(), ClientError>) {
        match outcome {
            Ok(()) => self.session.service.note_answered(verb),
            Err(ClientError::Refused(refusal)) => self.session.service.note_refused(refusal),
            Err(_) => {}
        }
    }
}

impl From<ClientError> for PreviewState {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Refused(refusal) => PreviewState::Refused { refusal },
            other => PreviewState::Failed {
                message: other.to_string(),
            },
        }
    }
}

impl From<ClientError> for ContentState {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Refused(refusal) => ContentState::Refused { refusal },
            ClientError::PayloadTooLarge { bytes, limit } => ContentState::TooLarge { bytes, limit },
            other => ContentState::Failed {
                message: other.to_string(),
            },
        }
    }
}

/// Await a call under the app's patience budget.
async fn timed<F, T>(verb: &str, future: F) -> Result<T, ClientError>
where
    F: Future<Output = Result<T, capnp::Error>>,
{
    match tokio::time::timeout(
        std::time::Duration::from_millis(contract::CALL_TIMEOUT_MS),
        future,
    )
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(capnp_error(verb, error)),
        Err(_) => Err(ClientError::Timeout {
            verb: verb.to_string(),
            millis: contract::CALL_TIMEOUT_MS,
        }),
    }
}

/// Map a capnp error to the client's vocabulary.
fn capnp_error(verb: &str, error: capnp::Error) -> ClientError {
    if error.kind == capnp::ErrorKind::Disconnected {
        ClientError::Transport(format!(
            "the service disconnected during {verb}: {}",
            error.extra
        ))
    } else {
        ClientError::Refused(Refusal::from_error(verb, &error))
    }
}

/// Check a contract `T.VoidResult`.
fn check_void(
    verb: &str,
    result: capnp::Result<types_capnp::void_result::Reader<'_>>,
) -> Result<(), ClientError> {
    use types_capnp::void_result::WhichReader;
    let reader = result.map_err(|error| ClientError::Decode(format!("{verb} result: {error}")))?;
    match reader.which() {
        Ok(WhichReader::Success(())) => Ok(()),
        Ok(WhichReader::Error(error)) => {
            // The engine's `T.Error.message` already carries the reason-class
            // prefix; `Refusal::from_reason_text` reads it, so a
            // `permissionDenied` here keeps its class instead of flattening to
            // "failed".
            let message = text_of(error.and_then(|error| error.get_message()));
            Err(ClientError::Refused(Refusal::from_reason_text(verb, &message)))
        }
        Err(error) => Err(ClientError::Decode(format!("{verb} union: {error}"))),
    }
}

/// Read the item list of a page.
fn read_items(
    list: capnp::Result<
        capnp::struct_list::Reader<'_, crate::schema::clipboard_capnp::clipboard_item::Owned>,
    >,
) -> Result<Vec<Item>, ClientError> {
    let list = list.map_err(|error| ClientError::Decode(format!("item list: {error}")))?;
    let mut items = Vec::with_capacity(list.len() as usize);
    for entry in list.iter() {
        let mimes = read_mimes(entry.get_mime_types());
        let mut tags = Vec::new();
        if let Ok(tag_list) = entry.get_tags() {
            for tag in tag_list.iter() {
                tags.push(text_of(tag));
            }
        }
        items.push(Item {
            id: ItemId(entry.get_id().map(|id| id.get_id()).unwrap_or_default()),
            captured_at_ns: match entry.get_timestamp().map(|ts| ts.get_nanoseconds()) {
                // 0 is the contract's "the clock could not be read".
                Ok(0) | Err(_) => None,
                Ok(nanos) => Some(nanos),
            },
            mimes,
            tags,
            pinned: entry.get_pinned(),
            size_bytes: entry.get_size_bytes(),
        });
    }
    Ok(items)
}

/// Read a `List(T.MimeType)`.
fn read_mimes(
    list: capnp::Result<capnp::struct_list::Reader<'_, types_capnp::mime_type::Owned>>,
) -> Vec<String> {
    let Ok(list) = list else {
        return Vec::new();
    };
    list.iter().map(|entry| mime_text(&entry)).collect()
}

/// Spell a `T.MimeType` as `"type/subtype"` (or the bare type).
fn mime_text(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let ty = text_of(reader.get_type());
    let subtype = text_of(reader.get_subtype());
    if subtype.is_empty() {
        ty
    } else {
        format!("{ty}/{subtype}")
    }
}

/// Write a `T.MimeType` from `"type/subtype"` text.
fn set_mime(builder: &mut types_capnp::mime_type::Builder<'_>, mime: &str) {
    let (ty, subtype) = mime.split_once('/').unwrap_or((mime, ""));
    builder.set_type(ty);
    builder.set_subtype(subtype);
}

/// Write a `ClipboardContent` with its payload in the `inline` arm.
///
/// The `buffer :BufferId` arm is not used: it names shared memory that this
/// client never allocated, and a fabricated handle would be exactly the kind
/// of lie the charter forbids.
fn write_content(builder: &mut clipboard_content::Builder<'_>, mime: &str, bytes: &[u8]) {
    set_mime(&mut builder.reborrow().init_mime_type(), mime);
    builder.reborrow().init_data().set_inline(bytes);
}

/// Read a `ClipboardContent` into `(mime, bytes)`.
fn read_content(reader: &clipboard_content::Reader<'_>) -> (String, Vec<u8>) {
    let mime = reader
        .get_mime_type()
        .map(|mime| mime_text(&mime))
        .unwrap_or_default();
    use types_capnp::data_handle::WhichReader;
    let bytes = match reader.get_data().map(|handle| handle.which()) {
        Ok(Ok(WhichReader::Inline(Ok(bytes)))) => bytes.to_vec(),
        _ => Vec::new(),
    };
    (mime, bytes)
}
