//! The clipboard domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `clipboard.capnp`'s `interface Clipboard` block (`@0`–`@16`) and its
//! `ClipboardChangeSubscription` interface (`@0`–`@1`). The dispatch is the
//! generated `ServerDispatch`; there is no ordinal, no method table and no
//! envelope of this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5). The clipboard domain's
//!   mutating verbs are `setContent`, `setOffer`, `clearContent`, the history
//!   mutators (`clearHistory`, `pinItem`, `unpinItem`, `deleteItem`) and
//!   `setSizeLimits`.
//!
//! # The payload is inline, and that is the contract's own choice
//!
//! `ClipboardContent.data` is a `T.DataHandle`: an `inline :Data` arm or a
//! `buffer :BufferId` arm. This service writes the inline arm — the bytes
//! travel in the message — and reads the inline arm back. A `buffer` arm
//! names shared memory this in-process service never allocated, so it reads
//! as the empty payload rather than as a fabricated buffer dereference.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::core::ids::ClipboardId;
use crate::core::ErrorCode;
use crate::domains::clipboard::availability;
use crate::domains::clipboard::backend::ClipboardBackend;
use crate::domains::clipboard::types::{
    ClipboardChange, ClipboardContent, ClipboardEventType, ClipboardItem, ClipboardSizeLimits,
    ContentOutcome, ConversionOutcome, Page, Pagination, PreviewOutcome, SubscriptionId,
    ValidationOutcome,
};
use crate::schema::clipboard_capnp::clipboard as clip;
use crate::schema::clipboard_capnp::clipboard_change_subscription as csub;
use crate::schema::clipboard_capnp::{clipboard_change, clipboard_content, clipboard_item};
use crate::schema::types_capnp;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The clipboard service: the domain's backend, and the class this connection
/// earned.
pub struct ClipboardService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for ClipboardService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> ClipboardService<B>
where
    B: ClipboardBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }

}

/// The class's answer for a mutating verb on a connection that did not earn
/// the right to change this host's selection (law #5). The verb is refused
/// *before* the backend is reached: a change that is refused afterwards has
/// already happened.
fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() {
        return None;
    }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!(
            "permissionDenied: the {verb} verb changes this host's clipboard, and this \
             connection holds the read-only class"
        ),
    ))
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then goes
/// to the backend. The answer comes BEFORE the class's gate: what the host
/// cannot do, no class can do.
fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Clipboard contract method"),
        )),
    }
}

/// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

/// The contract's pagination as the domain's.
fn pagination(reader: &types_capnp::pagination::Reader<'_>) -> Pagination {
    Pagination {
        offset: reader.get_offset(),
        limit: reader.get_limit(),
    }
}

/// Read a contract `T.ClipboardId` into the engine's own id.
fn clipboard_id(reader: &types_capnp::clipboard_id::Reader<'_>) -> ClipboardId {
    ClipboardId::new(reader.get_id())
}

/// Read a contract `T.MimeType` as the domain's `"type/subtype"` text.
fn mime_text(reader: &types_capnp::mime_type::Reader<'_>) -> String {
    let ty = match reader.get_type() {
        Ok(text) => super::text_of(text),
        Err(_) => String::new(),
    };
    let sub = match reader.get_subtype() {
        Ok(text) => super::text_of(text),
        Err(_) => String::new(),
    };
    if sub.is_empty() {
        ty
    } else {
        format!("{ty}/{sub}")
    }
}

/// Write a MIME type as the contract's own `T.MimeType` struct.
fn set_mime_text(builder: &mut types_capnp::mime_type::Builder<'_>, mime: &str) {
    let (ty, sub) = match mime.split_once('/') {
        Some((t, s)) => (t, s),
        None => (mime, ""),
    };
    builder.set_type(ty);
    builder.set_subtype(sub);
}

/// Read the payload bytes out of a contract `T.DataHandle`: only the `inline`
/// arm is materialised. A `buffer :BufferId` names shared memory this
/// in-process service never allocated, so it reads as the empty payload —
/// never a fabricated buffer dereference.
fn data_handle_bytes(reader: &types_capnp::data_handle::Reader<'_>) -> Vec<u8> {
    use types_capnp::data_handle::WhichReader;
    match reader.which() {
        Ok(WhichReader::Inline(Ok(bytes))) => bytes.to_vec(),
        _ => Vec::new(),
    }
}

/// Read the contract's `ClipboardContent` into the domain's own content.
fn read_content(reader: &clipboard_content::Reader<'_>) -> ClipboardContent {
    ClipboardContent {
        mime_type: reader
            .get_mime_type()
            .map(|m| mime_text(&m))
            .unwrap_or_default(),
        data: match reader.get_data() {
            Ok(handle) => data_handle_bytes(&handle),
            Err(_) => Vec::new(),
        },
    }
}

/// Write the domain's content into the contract's `ClipboardContent`.
fn write_content(content: &ClipboardContent, builder: &mut clipboard_content::Builder<'_>) {
    set_mime_text(&mut builder.reborrow().init_mime_type(), &content.mime_type);
    builder
        .reborrow()
        .init_data()
        .set_inline(content.data.as_slice());
}

/// Write the domain's history items into the contract's list.
fn write_items<'a>(
    items: &[ClipboardItem],
    list: &mut capnp::struct_list::Builder<'a, clipboard_item::Owned>,
) {
    for (index, item) in items.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.reborrow().init_id().set_id(item.id.get());
        entry.reborrow().init_timestamp().set_nanoseconds(item.timestamp);
        let mut mimes = entry.reborrow().init_mime_types(item.mime_types.len() as u32);
        for (position, mime) in item.mime_types.iter().enumerate() {
            set_mime_text(&mut mimes.reborrow().get(position as u32), mime);
        }
        let mut tags = entry.reborrow().init_tags(item.tags.len() as u32);
        for (position, tag) in item.tags.iter().enumerate() {
            tags.set(position as u32, tag);
        }
        entry.set_pinned(item.pinned);
        entry.set_size_bytes(item.size_bytes);
    }
}

/// Write the domain's change events into the contract's list.
fn write_changes<'a>(
    changes: &[ClipboardChange],
    list: &mut capnp::struct_list::Builder<'a, clipboard_change::Owned>,
) {
    for (index, change) in changes.iter().enumerate() {
        let mut entry = list.reborrow().get(index as u32);
        entry.reborrow().init_id().set_id(change.id.get());
        entry
            .reborrow()
            .init_timestamp()
            .set_nanoseconds(change.timestamp);
        entry
            .reborrow()
            .init_source_id()
            .set_id(change.source_id.get());
        let mut mimes = entry.reborrow().init_mime_types(change.mime_types.len() as u32);
        for (position, mime) in change.mime_types.iter().enumerate() {
            set_mime_text(&mut mimes.reborrow().get(position as u32), mime);
        }
        entry.set_event_type(match change.event_type {
            ClipboardEventType::Copied => crate::schema::clipboard_capnp::ClipboardEventType::Copied,
            ClipboardEventType::Pasted => crate::schema::clipboard_capnp::ClipboardEventType::Pasted,
            ClipboardEventType::Cleared => crate::schema::clipboard_capnp::ClipboardEventType::Cleared,
            ClipboardEventType::Expired => crate::schema::clipboard_capnp::ClipboardEventType::Expired,
            ClipboardEventType::Custom => crate::schema::clipboard_capnp::ClipboardEventType::Custom,
        });
    }
}

/// The clipboard change subscription the service hands back for
/// `subscribeChanges @5`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct ClipboardChangeSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SubscriptionId,
}

impl<B> Clone for ClipboardChangeSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> csub::Server for ClipboardChangeSubscriptionService<B>
where
    B: ClipboardBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: csub::PullParams,
        mut results: csub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_events();
            let changes = backend
                .pull_changes(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_changes(changes.len() as u32);
            write_changes(&changes, &mut list);
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: csub::CancelParams,
        mut results: csub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel_changes(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

impl<B> clip::Server for ClipboardService<B>
where
    B: ClipboardBackend + 'static,
{
    // ── The live selection ─────────────────────────────────────────────────

    fn get_content(
        self: Rc<Self>,
        params: clip::GetContentParams,
        mut results: clip::GetContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getContent") {
                return Err(refusal);
            }
            let params = params.get()?;
            let mime = mime_text(&params.get_mime_type()?);
            let outcome = backend.get_content(&mime)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut union = builder.init_result();
            match outcome {
                ContentOutcome::Content(content) => {
                    write_content(&content, &mut union.init_content());
                }
                ContentOutcome::Empty => {
                    union.set_empty(());
                }
            }
            Ok(())
        }
    }

    fn set_content(
        self: Rc<Self>,
        params: clip::SetContentParams,
        mut results: clip::SetContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setContent") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setContent") {
                return Err(refusal);
            }
            let params = params.get()?;
            let content = read_content(&params.get_content()?);
            let outcome = backend.set_content(content);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn clear_content(
        self: Rc<Self>,
        _params: clip::ClearContentParams,
        mut results: clip::ClearContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("clearContent") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "clearContent") {
                return Err(refusal);
            }
            let outcome = backend.clear_content();
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn get_supported_mime_types(
        self: Rc<Self>,
        _params: clip::GetSupportedMimeTypesParams,
        mut results: clip::GetSupportedMimeTypesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getSupportedMimeTypes") {
                return Err(refusal);
            }
            let mimes = backend
                .get_supported_mime_types()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_mime_types(mimes.len() as u32);
            for (index, mime) in mimes.iter().enumerate() {
                set_mime_text(&mut list.reborrow().get(index as u32), mime);
            }
            Ok(())
        }
    }

    // ── The change subscription: the contract's own capability ─────────────

    fn subscribe_changes(
        self: Rc<Self>,
        _params: clip::SubscribeChangesParams,
        mut results: clip::SubscribeChangesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("subscribeChanges") {
                return Err(refusal);
            }
            let subscription = backend
                .subscribe_changes()
                .map_err(|error| errors::to_capnp(&error))?;
            let server = ClipboardChangeSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: csub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    // ── The engine-side copy ledger ───────────────────────────────────────

    fn get_history(
        self: Rc<Self>,
        params: clip::GetHistoryParams,
        mut results: clip::GetHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getHistory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let page: Page<ClipboardItem> = backend
                .get_history(pagination(&params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_items(page.items.len() as u32);
            write_items(&page.items, &mut list);
            builder.set_total(page.total);
            Ok(())
        }
    }

    fn clear_history(
        self: Rc<Self>,
        _params: clip::ClearHistoryParams,
        mut results: clip::ClearHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("clearHistory") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "clearHistory") {
                return Err(refusal);
            }
            let outcome = backend.clear_history();
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn search_history(
        self: Rc<Self>,
        params: clip::SearchHistoryParams,
        mut results: clip::SearchHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("searchHistory") {
                return Err(refusal);
            }
            let params = params.get()?;
            let query = super::text_of(params.get_query()?);
            let page: Page<ClipboardItem> = backend
                .search_history(&query, pagination(&params.get_pagination()?))
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_items(page.items.len() as u32);
            write_items(&page.items, &mut list);
            builder.set_total(page.total);
            Ok(())
        }
    }

    fn pin_item(
        self: Rc<Self>,
        params: clip::PinItemParams,
        mut results: clip::PinItemResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("pinItem") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "pinItem") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let outcome = backend.pin_item(item);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn unpin_item(
        self: Rc<Self>,
        params: clip::UnpinItemParams,
        mut results: clip::UnpinItemResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("unpinItem") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "unpinItem") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let outcome = backend.unpin_item(item);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn delete_item(
        self: Rc<Self>,
        params: clip::DeleteItemParams,
        mut results: clip::DeleteItemResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("deleteItem") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "deleteItem") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let outcome = backend.delete_item(item);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    // ── Preview, validation, conversion ────────────────────────────────────

    fn get_preview(
        self: Rc<Self>,
        params: clip::GetPreviewParams,
        mut results: clip::GetPreviewResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getPreview") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let outcome = backend.get_preview(item)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut union = builder.init_result();
            match outcome {
                PreviewOutcome::Preview(preview) => {
                    let mut target = union.init_preview();
                    target.set_text_preview(&preview.text_preview);
                    target.set_available(preview.available);
                    target.set_original_size(preview.original_size);
                    set_mime_text(
                        &mut target.reborrow().init_preview_mime_type(),
                        &preview.preview_mime_type,
                    );
                }
                PreviewOutcome::Unavailable => {
                    union.set_unknown(());
                }
            }
            Ok(())
        }
    }

    fn validate_content(
        self: Rc<Self>,
        params: clip::ValidateContentParams,
        mut results: clip::ValidateContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("validateContent") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let outcome = backend.validate_content(item)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut union = builder.init_result();
            match outcome {
                ValidationOutcome::Valid => {
                    union.set_valid(());
                }
                ValidationOutcome::Invalid(reason) => {
                    let mut failure = union.init_invalid();
                    failure.set_code(crate::core::ErrorCode::InvalidArgument.into());
                    failure.set_message(&reason);
                }
                ValidationOutcome::Unknown => {
                    union.set_unknown(());
                }
            }
            Ok(())
        }
    }

    fn convert_content(
        self: Rc<Self>,
        params: clip::ConvertContentParams,
        mut results: clip::ConvertContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("convertContent") {
                return Err(refusal);
            }
            let params = params.get()?;
            let item = clipboard_id(&params.get_item_id()?);
            let target = mime_text(&params.get_target_mime_type()?);
            let outcome = backend.convert_content(item, &target)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut union = builder.init_result();
            match outcome {
                ConversionOutcome::Content(content) => {
                    write_content(&content, &mut union.init_content());
                }
                ConversionOutcome::Unsupported => {
                    union.set_unsupported(());
                }
            }
            Ok(())
        }
    }

    // ── The size limits: the backend's own policy ──────────────────────────

    fn get_size_limits(
        self: Rc<Self>,
        _params: clip::GetSizeLimitsParams,
        mut results: clip::GetSizeLimitsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("getSizeLimits") {
                return Err(refusal);
            }
            let limits: ClipboardSizeLimits = backend
                .get_size_limits()
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut target = builder.init_limits();
            target.set_max_item_size(limits.max_item_size);
            target.set_max_history_size(limits.max_history_size);
            target.set_max_history_count(limits.max_history_count);
            Ok(())
        }
    }

    fn set_size_limits(
        self: Rc<Self>,
        params: clip::SetSizeLimitsParams,
        mut results: clip::SetSizeLimitsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setSizeLimits") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setSizeLimits") {
                return Err(refusal);
            }
            let params = params.get()?;
            let reader = params.get_limits()?;
            let limits = ClipboardSizeLimits {
                max_item_size: reader.get_max_item_size(),
                max_history_size: reader.get_max_history_size(),
                max_history_count: reader.get_max_history_count(),
            };
            let outcome = backend.set_size_limits(limits);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_offer(
        self: Rc<Self>,
        params: clip::SetOfferParams,
        mut results: clip::SetOfferResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let class = self.class;
        async move {
            if let Some(refusal) = unavailable("setOffer") {
                return Err(refusal);
            }
            if let Some(refusal) = gate(class, "setOffer") {
                return Err(refusal);
            }
            let params = params.get()?;
            let mut offers: Vec<ClipboardContent> = Vec::new();
            if let Ok(list) = params.get_offers() {
                for offer in list.iter() {
                    offers.push(read_content(&offer));
                }
            }
            let outcome = backend.set_offer(&offers);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The clipboard domain, as a served domain: the contract interface it
/// serves, the descriptor discovery publishes for it, and the per-connection
/// server.
pub struct ClipboardDomain<B> {
    backend: Arc<B>,
}

impl<B> ClipboardDomain<B>
where
    B: ClipboardBackend + 'static,
{
    /// The clipboard domain over this backend. The backend is shared: the
    /// same host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for ClipboardDomain<B>
where
    B: ClipboardBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        // The contract's own interface id, from the generated code.
        clip::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::CLIPBOARD_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn category(&self) -> &'static str {
        "clipboard"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the clipboard domain answers its whole
    /// contract on this host, so it is `online`.
    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    /// The commands this domain publishes through the registry (cli.capnp).
    ///
    /// The paths are the PRODUCT's own words — what `smart clipboard …` types
    /// — and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "clipboard status",
                "Clipboard",
                "getContent",
                "the clipboard's current state",
                0,
                true,
            )
            .with_row(matrix_row("getContent")),
            DomainCommand::new(
                "clipboard get",
                "Clipboard",
                "getContent",
                "the clipboard's content for a MIME type",
                1,
                false,
            )
            .with_row(matrix_row("getContent")),
            DomainCommand::new(
                "clipboard set",
                "Clipboard",
                "setContent",
                "place text on the clipboard",
                2,
                false,
            )
            .with_row(matrix_row("setContent")),
            DomainCommand::new(
                "clipboard clear",
                "Clipboard",
                "clearContent",
                "empty the clipboard",
                3,
                true,
            )
            .with_row(matrix_row("clearContent")),
            DomainCommand::new(
                "clipboard history",
                "Clipboard",
                "getHistory",
                "the engine-side copy history",
                4,
                true,
            )
            .with_row(matrix_row("getHistory")),
            DomainCommand::new(
                "clipboard history search",
                "Clipboard",
                "searchHistory",
                "search the copy history",
                5,
                false,
            )
            .with_row(matrix_row("searchHistory")),
            DomainCommand::new(
                "clipboard history clear",
                "Clipboard",
                "clearHistory",
                "empty the copy history",
                6,
                true,
            )
            .with_row(matrix_row("clearHistory")),
            DomainCommand::new(
                "clipboard history pin",
                "Clipboard",
                "pinItem",
                "pin a copy against auto-expiry",
                7,
                false,
            )
            .with_row(matrix_row("pinItem")),
            DomainCommand::new(
                "clipboard history unpin",
                "Clipboard",
                "unpinItem",
                "unpin a copy",
                8,
                false,
            )
            .with_row(matrix_row("unpinItem")),
            DomainCommand::new(
                "clipboard history delete",
                "Clipboard",
                "deleteItem",
                "remove a copy from the history",
                9,
                false,
            )
            .with_row(matrix_row("deleteItem")),
            DomainCommand::new(
                "clipboard types",
                "Clipboard",
                "getSupportedMimeTypes",
                "the MIME types the selection offers",
                10,
                true,
            )
            .with_row(matrix_row("getSupportedMimeTypes")),
            DomainCommand::new(
                "clipboard preview",
                "Clipboard",
                "getPreview",
                "a low-resolution preview of a copy",
                11,
                false,
            )
            .with_row(matrix_row("getPreview")),
            DomainCommand::new(
                "clipboard validate",
                "Clipboard",
                "validateContent",
                "check that a copy matches its claimed MIME type",
                12,
                false,
            )
            .with_row(matrix_row("validateContent")),
            DomainCommand::new(
                "clipboard convert",
                "Clipboard",
                "convertContent",
                "convert a copy to another MIME type",
                13,
                false,
            )
            .with_row(matrix_row("convertContent")),
            DomainCommand::new(
                "clipboard limits",
                "Clipboard",
                "getSizeLimits",
                "the backend's declared size limits",
                14,
                true,
            )
            .with_row(matrix_row("getSizeLimits")),
            DomainCommand::new(
                "clipboard limits set",
                "Clipboard",
                "setSizeLimits",
                "declare the backend's size limits",
                15,
                false,
            )
            .with_row(matrix_row("setSizeLimits")),
            DomainCommand::new(
                "clipboard offer",
                "Clipboard",
                "setOffer",
                "offer several representations of one copy",
                16,
                false,
            )
            .with_row(matrix_row("setOffer")),
            DomainCommand::new(
                "clipboard watch",
                "Clipboard",
                "subscribeChanges",
                "subscribe to the clipboard's change events",
                17,
                true,
            )
            .with_row(matrix_row("subscribeChanges")),
            DomainCommand::new(
                "clipboard watch pull",
                "Clipboard",
                "pullChanges",
                "the next bounded batch of change events",
                18,
                true,
            )
            .with_row(matrix_row("pullChanges")),
            DomainCommand::new(
                "clipboard watch cancel",
                "Clipboard",
                "cancelSubscription",
                "retire a change subscription",
                19,
                true,
            )
            .with_row(matrix_row("cancelSubscription")),
        ]
    }

    /// The clipboard schema's node tree: what a client that never compiled
    /// `clipboard.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/clipboard.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = ClipboardService::new(Arc::clone(&self.backend), class);
        let client: clip::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
