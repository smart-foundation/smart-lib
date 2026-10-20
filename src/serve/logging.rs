//! The logging domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `logging.capnp`'s `interface Logging` block (`@0`–`@8`) and its
//! `LogSubscription` interface (`@0`–`@1`). The dispatch is the generated
//! `ServerDispatch`; there is no ordinal, no method table and no envelope of
//! this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5). The logging domain's
//!   mutating verbs are `setLevel`, `setSamplingRate`, `clear` and `log` —
//!   the last one writes the one stream this process owns, so it is gated
//!   with the rest.
//!
//! # The subscription is the contract's own capability
//!
//! `subscribe @2` hands back a `LogSubscription` **interface reference**, and
//! the reference IS the authority to pull: there is no subscription id to
//! present. The backend's id is the handle behind it.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::ErrorCode;
use crate::domains::logging::availability;
use crate::domains::logging::backend::LoggingBackend;
use crate::domains::logging::types::{Level, LogEntry, LogFilter, LogOutcome, Page, Pagination};
use crate::schema::logging_capnp::logging as lg;
use crate::schema::logging_capnp::log_subscription as sub;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The logging service: the domain's backend, and the class this connection
/// earned.
pub struct LoggingService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for LoggingService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> LoggingService<B>
where
    B: LoggingBackend + 'static,
{
    /// A service over this backend, at this class.
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }

    /// The class's answer for a mutating verb on a connection that did not
    /// earn the right to write (law #5). The verb is refused *before* the
    /// backend is reached: a write that is refused afterwards has already
    /// happened.
    fn gate(&self, verb: &str) -> Option<capnp::Error> {
        if self.class.is_control() {
            return None;
        }
        Some(errors::decline(
            ErrorCode::PermissionDenied,
            &format!(
                "permissionDenied: the {verb} verb writes this host's logging state, \
                 and this connection holds the read-only class"
            ),
        ))
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
////// Write the engine's `VoidResult` into the contract's `T.VoidResult`.
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

/// Write the domain's page of entries into the contract's list.
fn write_entries(
    entries: &[LogEntry],
    builder: &mut lg::get_entries_results::Builder<'_>,
) {
    let mut list = builder.reborrow().init_entries(entries.len() as u32);
    for (index, entry) in entries.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        entry.write_to(&mut item);
    }
}

/// Write the domain's `LogOutcome` into the contract's union.
///
/// The contract's `dropped` arm is a `Void` — its own comment promises an
/// open `reason`, but the struct carries none, so the drop reason cannot
/// cross the contract boundary. The engine keeps it (the domain's
/// `LogOutcome::Dropped` names it); the contract's arm is set, and the gap
/// is reported to the schema's owners.
fn write_outcome(
    outcome: LogOutcome,
    builder: &mut crate::schema::logging_capnp::log_outcome::Builder<'_>,
) {
    match outcome {
        LogOutcome::Accepted => {
            builder.set_accepted(());
        }
        LogOutcome::Sampled => {
            builder.set_sampled(());
        }
        LogOutcome::Dropped(_) => {
            builder.set_dropped(());
        }
    }
}

/// The log subscription the service hands back for `subscribe @2`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct LogSubscription<B> {
    backend: Arc<B>,
    subscription: crate::domains::logging::types::SubscriptionId,
}

impl<B> Clone for LogSubscription<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> sub::Server for LogSubscription<B>
where
    B: LoggingBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: sub::PullParams,
        mut results: sub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_entries();
            let entries = backend
                .pull(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_entries(entries.len() as u32);
            for (index, entry) in entries.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                entry.write_to(&mut item);
            }
            Ok(())
        }
    }

    fn cancel(
        self: Rc<Self>,
        _params: sub::CancelParams,
        mut results: sub::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let outcome = backend.cancel(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

impl<B> lg::Server for LoggingService<B>
where
    B: LoggingBackend + 'static,
{
    // ── The hot path ─────────────────────────────────────────────────────

    fn log(
        self: Rc<Self>,
        params: lg::LogParams,
        mut results: lg::LogResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("log") {
                return Err(refusal);
            }
            let params = params.get()?;
            let level = Level::from_generated(params.get_severity()?);
            let domain = super::text_of(params.get_domain()?);
            let message = super::text_of(params.get_message()?);
            let outcome = backend
                .log(level, &domain, &message)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            write_outcome(outcome, &mut builder.init_outcome());
            Ok(())
        }
    }

    // ── The reads ────────────────────────────────────────────────────────

    fn get_entries(
        self: Rc<Self>,
        params: lg::GetEntriesParams,
        mut results: lg::GetEntriesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = LogFilter::from_reader(&params.get_filter()?);
            let page = pagination(&params.get_page()?);
            let entries: Page<LogEntry> = backend
                .get_entries(&filter, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            write_entries(&entries.items, &mut builder);
            builder.set_total(entries.total);
            Ok(())
        }
    }

    fn get_level(
        self: Rc<Self>,
        params: lg::GetLevelParams,
        mut results: lg::GetLevelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let level = backend
                .get_level(&domain)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_level(level.to_generated());
            Ok(())
        }
    }

    fn list_domains(
        self: Rc<Self>,
        params: lg::ListDomainsParams,
        mut results: lg::ListDomainsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_page()?);
            let domains: Page<crate::domains::logging::types::LogDomain> = backend
                .list_domains(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_domains(domains.items.len() as u32);
            for (index, domain) in domains.items.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                domain.write_to(&mut item);
            }
            builder.set_total(domains.total);
            Ok(())
        }
    }

    fn export(
        self: Rc<Self>,
        params: lg::ExportParams,
        mut results: lg::ExportResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = LogFilter::from_reader(&params.get_filter()?);
            let mime = params.get_format()?;
            let format = format!(
                "{}/{}",
                super::text_of(mime.get_type()?),
                super::text_of(mime.get_subtype()?)
            );
            let data = backend
                .export(&filter, &format)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            builder.reborrow().init_data().set_inline(&data);
            Ok(())
        }
    }

    // ── The subscription ─────────────────────────────────────────────────

    fn subscribe(
        self: Rc<Self>,
        params: lg::SubscribeParams,
        mut results: lg::SubscribeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let filter = LogFilter::from_reader(&params.get_filter()?);
            let subscription = backend
                .subscribe(&filter)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = LogSubscription {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    // ── The writes ───────────────────────────────────────────────────────

    fn set_level(
        self: Rc<Self>,
        params: lg::SetLevelParams,
        mut results: lg::SetLevelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setLevel") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let level = Level::from_generated(params.get_level()?);
            let outcome = backend.set_level(&domain, level);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn clear(
        self: Rc<Self>,
        params: lg::ClearParams,
        mut results: lg::ClearResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("clear") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let outcome = backend.clear(&domain);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_sampling_rate(
        self: Rc<Self>,
        params: lg::SetSamplingRateParams,
        mut results: lg::SetSamplingRateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setSamplingRate") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let rate = params.get_rate();
            let outcome = backend.set_sampling_rate(&domain, rate);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The logging domain, as a served domain: the contract interface it serves,
/// the descriptor discovery publishes for it, and the per-connection server.
pub struct LoggingDomain<B> {
    backend: Arc<B>,
}

impl<B> LoggingDomain<B>
where
    B: LoggingBackend + 'static,
{
    /// The logging domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for LoggingDomain<B>
where
    B: LoggingBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        lg::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::LOGGING_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "logging"
    }

    fn category(&self) -> &'static str {
        "logging"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the logging domain answers its whole
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
    /// The paths are the PRODUCT's own words — what `smart logging …` types —
    /// and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "logging sinks",
                "Logging",
                "listDomains",
                "the sinks this host has, and each one's level",
                0,
                true,
            )
            .with_row(matrix_row("listDomains")),
            DomainCommand::composing(
                "logging domains",
                "Logging",
                "listDomains",
                "one domain's level and sampling rate, and every domain the backend knows",
                1,
                true,
                "getLevel",
            )
            .with_row(matrix_row("listDomains")),
            DomainCommand::new(
                "logging tail",
                "Logging",
                "getEntries",
                "read one sink's own lines",
                2,
                false,
            )
            .with_row(matrix_row("getEntries")),
            DomainCommand::new(
                "logging entries",
                "Logging",
                "getEntries",
                "this process's own retained entries, filtered",
                3,
                true,
            )
            .with_row(matrix_row("getEntries")),
            DomainCommand::composing(
                "logging follow",
                "Logging",
                "subscribe",
                "a live event stream, pulled in bounded rounds",
                4,
                true,
                "pull,cancel",
            )
            .with_row(matrix_row("subscribe")),
            DomainCommand::new(
                "logging emit",
                "Logging",
                "log",
                "write one entry through the contract's hot path",
                5,
                false,
            )
            .with_row(matrix_row("log")),
            DomainCommand::new(
                "logging level",
                "Logging",
                "getLevel",
                "read one domain's configured level",
                6,
                false,
            )
            .with_row(matrix_row("getLevel")),
            DomainCommand::new(
                "logging set-level",
                "Logging",
                "setLevel",
                "configure one domain's level",
                7,
                false,
            )
            .with_row(matrix_row("setLevel")),
            DomainCommand::new(
                "logging sampling",
                "Logging",
                "setSamplingRate",
                "declare a domain's sampling rate",
                8,
                false,
            )
            .with_row(matrix_row("setSamplingRate")),
            DomainCommand::new(
                "logging clear",
                "Logging",
                "clear",
                "clear one domain's retained entries",
                9,
                false,
            )
            .with_row(matrix_row("clear")),
            DomainCommand::new(
                "logging export",
                "Logging",
                "export",
                "export the retained entries in an open content type",
                10,
                true,
            )
            .with_row(matrix_row("export")),
            DomainCommand::new(
                "logging pull",
                "LogSubscription",
                "pull",
                "take the entries a log subscription has for you",
                11,
                true,
            )
            .with_row(matrix_row("pull")),
            DomainCommand::new(
                "logging cancel",
                "LogSubscription",
                "cancel",
                "retire a log subscription",
                12,
                false,
            )
            .with_row(matrix_row("cancel")),
        ]
    }

    /// The logging schema's node tree: what a client that never compiled
    /// `logging.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/logging.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = LoggingService::new(Arc::clone(&self.backend), class);
        let client: lg::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
