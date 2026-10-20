//! The settings domain, served through the contract's **generated** `Server`
//! trait.
//!
//! # What this file is
//!
//! One method per contract method, typed params and typed results, straight
//! from `settings.capnp`'s `interface Settings` block (`@0`–`@17`) and its
//! `SettingsSubscription` interface (`@0`–`@1`). The dispatch is the
//! generated `ServerDispatch`; there is no ordinal, no method table and no
//! envelope of this layer's own anywhere on the path.
//!
//! # The three answers a method can give
//!
//! * **the backend's answer**, for a method this backend serves;
//! * **the matrix's answer**, for a method the availability matrix marks
//!   unavailable: the row's `reasonClass` token prefixes the row's reason;
//! * **the class's answer**, for a mutating verb on a read-only connection:
//!   refused *before* the backend is reached (law #5). The settings domain's
//!   mutating verbs are `set`, `delete`, `clear`, `bulkSet`,
//!   `setAppOverride`, `clearAppOverride` and `setIfUnchanged`.
//!
//! # The gate is ONCE, and it is the host's
//!
//! The contract says so itself: "Writes are gated ONCE, by the capability
//! held when the caller acquired this service. The store does not check
//! per-key permissions itself." So this layer gates the verb by the
//! connection's class, and the backend's own host-side gate (the dconf
//! path) decides the rest. Two authorities, one decision each — never two
//! answers.

extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use std::rc::Rc;
use std::sync::Arc;

use crate::core::ids::AppId;
use crate::core::ErrorCode;
use crate::domains::settings::availability;
use crate::domains::settings::backend::{SettingsBackend, SettingsSubscription};
use crate::domains::settings::types::{
    Page, Pagination, SettingChange, SettingMapEntry, SettingResult, SettingValue,
};
use crate::schema::settings_capnp::settings as st;
use crate::schema::settings_capnp::settings_subscription as sub;
use crate::schema::types_capnp;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;

use super::availability::matrix_reader;

matrix_reader!(availability);

/// The settings service: the domain's backend, and the class this connection
/// earned.
pub struct SettingsService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for SettingsService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            class: self.class,
        }
    }
}

impl<B> SettingsService<B>
where
    B: SettingsBackend + 'static,
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
                "permissionDenied: the {verb} verb writes this host's settings, and this \
                 connection holds the read-only class"
            ),
        ))
    }
}

/// The availability answer for a method the matrix marks unavailable: the
/// row's `reasonClass` token prefixes the row's reason, read as data.
///
/// `None` when the matrix says the method IS served here — the arm then
/// goes to the backend. The answer comes BEFORE the class's gate: what the
/// host cannot do, no class can do.
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
            &format!("invalidArgument: {name} is not a Settings contract method"),
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

/// Write a list of map entries into a generated list builder.
fn write_entries_list<'a>(
    entries: &[SettingMapEntry],
    list: &mut capnp::struct_list::Builder<'a, crate::schema::settings_capnp::setting_map_entry::Owned>,
) {
    for (index, entry) in entries.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        item.set_key(&entry.key);
        entry.value.write_to(item.reborrow().init_value());
    }
}

/// Write the domain's changes into the contract's list.
fn write_changes<'a>(
    changes: &[SettingChange],
    list: &mut capnp::struct_list::Builder<'a, crate::schema::settings_capnp::setting_change::Owned>,
) {
    for (index, change) in changes.iter().enumerate() {
        let mut item = list.reborrow().get(index as u32);
        item.set_domain(&change.domain);
        item.set_key(&change.key);
        change.value.write_to(item.reborrow().init_value());
        item.reborrow().init_timestamp().set_nanoseconds(change.timestamp.as_nanos());
        item.set_actor_id(&change.actor_id);
        item.set_version(change.version);
    }
}

/// The settings subscription the service hands back for `watch @6` and
/// `watchDomain @7`.
///
/// Holding the reference IS the authority to pull — the contract's own rule.
pub struct SettingsSubscriptionService<B> {
    backend: Arc<B>,
    subscription: SettingsSubscription,
}

impl<B> Clone for SettingsSubscriptionService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            subscription: self.subscription,
        }
    }
}

impl<B> sub::Server for SettingsSubscriptionService<B>
where
    B: SettingsBackend + 'static,
{
    fn pull(
        self: Rc<Self>,
        params: sub::PullParams,
        mut results: sub::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        let subscription = self.subscription;
        async move {
            let max = params.get()?.get_max_events();
            let changes = backend
                .pull_settings(subscription, max)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_changes(changes.len() as u32);
            write_changes(&changes, &mut list);
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
            let outcome = backend.cancel_settings(subscription);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

impl<B> st::Server for SettingsService<B>
where
    B: SettingsBackend + 'static,
{
    // ── The reads ────────────────────────────────────────────────────────

    fn get(
        self: Rc<Self>,
        params: st::GetParams,
        mut results: st::GetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let result = backend
                .get(&domain, &key)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut union = builder.init_result();
            match result {
                SettingResult::Value(value) => {
                    value.write_to(union.init_value());
                }
                SettingResult::NotSet => {
                    union.set_not_set(());
                }
                SettingResult::Error(error) => {
                    let mut failure = union.init_error();
                    errors::write_t_error(&error, &mut failure);
                }
                SettingResult::Unknown => {
                    union.set_unknown(());
                }
            }
            Ok(())
        }
    }

    fn get_all(
        self: Rc<Self>,
        params: st::GetAllParams,
        mut results: st::GetAllResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let page = pagination(&params.get_pagination()?);
            let map = backend
                .get_all(&domain, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut settings = builder.reborrow().init_settings();
            let mut list = settings.reborrow().init_entries(map.entries.len() as u32);
            write_entries_list(&map.entries, &mut list);
            // The engine's map carries no separate total: the count of what
            // the store returned IS the total it knows.
            builder.set_total(map.entries.len() as u64);
            Ok(())
        }
    }

    fn list_domains(
        self: Rc<Self>,
        params: st::ListDomainsParams,
        mut results: st::ListDomainsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let page = pagination(&params.get_pagination()?);
            let domains: Page<String> = backend
                .list_domains(page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_domains(domains.items.len() as u32);
            for (index, domain) in domains.items.iter().enumerate() {
                list.reborrow().set(index as u32, domain);
            }
            builder.set_total(domains.total);
            Ok(())
        }
    }

    fn bulk_get(
        self: Rc<Self>,
        params: st::BulkGetParams,
        mut results: st::BulkGetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let mut keys: Vec<String> = Vec::new();
            for key in params.get_keys()?.iter() {
                keys.push(super::text_of(key?));
            }
            let borrowed: Vec<&str> = keys.iter().map(|key| key.as_str()).collect();
            let entries = backend
                .bulk_get(&domain, &borrowed)
                .map_err(|error| errors::to_capnp(&error))?;
            let builder = results.get();
            let mut list = builder.init_entries(entries.len() as u32);
            write_entries_list(&entries, &mut list);
            Ok(())
        }
    }

    fn list_by_category(
        self: Rc<Self>,
        params: st::ListByCategoryParams,
        mut results: st::ListByCategoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let category = super::text_of(params.get_category()?);
            let page = pagination(&params.get_pagination()?);
            let entries: Page<SettingMapEntry> = backend
                .list_by_category(&category, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_entries(entries.items.len() as u32);
            write_entries_list(&entries.items, &mut list);
            builder.set_total(entries.total);
            Ok(())
        }
    }

    fn get_app_override(
        self: Rc<Self>,
        params: st::GetAppOverrideParams,
        mut results: st::GetAppOverrideResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let app_id = AppId::new(params.get_app_id()?.get_id());
            let key = super::text_of(params.get_key()?);
            let (value, exists) = backend
                .get_app_override(app_id, &key)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            value.write_to(builder.reborrow().init_value());
            builder.set_exists(exists);
            Ok(())
        }
    }

    fn get_version(
        self: Rc<Self>,
        params: st::GetVersionParams,
        mut results: st::GetVersionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let version = backend
                .get_version(&domain, &key)
                .map_err(|error| errors::to_capnp(&error))?;
            results.get().set_version(version);
            Ok(())
        }
    }

    fn get_history(
        self: Rc<Self>,
        params: st::GetHistoryParams,
        mut results: st::GetHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let page = pagination(&params.get_pagination()?);
            let changes: Page<SettingChange> = backend
                .get_history(&domain, &key, page)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_changes(changes.items.len() as u32);
            write_changes(&changes.items, &mut list);
            builder.set_total(changes.total);
            Ok(())
        }
    }

    fn get_settings_diff(
        self: Rc<Self>,
        params: st::GetSettingsDiffParams,
        mut results: st::GetSettingsDiffResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let since = params.get_since_version();
            let (changes, current) = backend
                .get_settings_diff(&domain, since)
                .map_err(|error| errors::to_capnp(&error))?;
            let mut builder = results.get();
            let mut list = builder.reborrow().init_changes(changes.len() as u32);
            write_changes(&changes, &mut list);
            builder.set_current_version(current);
            Ok(())
        }
    }

    // ── The subscriptions ─────────────────────────────────────────────────

    fn watch(
        self: Rc<Self>,
        params: st::WatchParams,
        mut results: st::WatchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let subscription = backend
                .watch(&domain, &key)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = SettingsSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    fn watch_domain(
        self: Rc<Self>,
        params: st::WatchDomainParams,
        mut results: st::WatchDomainResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let subscription = backend
                .watch_domain(&domain)
                .map_err(|error| errors::to_capnp(&error))?;
            let server = SettingsSubscriptionService {
                backend: Arc::clone(&backend),
                subscription,
            };
            let client: sub::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    // ── The writes ───────────────────────────────────────────────────────

    fn set(
        self: Rc<Self>,
        params: st::SetParams,
        mut results: st::SetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("set") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let value = SettingValue::from_reader(&params.get_value()?);
            let outcome = backend.set(&domain, &key, &value);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn delete(
        self: Rc<Self>,
        params: st::DeleteParams,
        mut results: st::DeleteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("delete") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let outcome = backend.delete(&domain, &key);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn clear(
        self: Rc<Self>,
        params: st::ClearParams,
        mut results: st::ClearResults,
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

    fn bulk_set(
        self: Rc<Self>,
        params: st::BulkSetParams,
        mut results: st::BulkSetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("bulkSet") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let mut entries: Vec<SettingMapEntry> = Vec::new();
            for entry in params.get_entries()?.iter() {
                entries.push(SettingMapEntry {
                    key: super::text_of(entry.get_key()?),
                    value: SettingValue::from_reader(&entry.get_value()?),
                });
            }
            let outcome = backend.bulk_set(&domain, &entries);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_app_override(
        self: Rc<Self>,
        params: st::SetAppOverrideParams,
        mut results: st::SetAppOverrideResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("setAppOverride") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("setAppOverride") {
                return Err(refusal);
            }
            let params = params.get()?;
            let app_id = AppId::new(params.get_app_id()?.get_id());
            let key = super::text_of(params.get_key()?);
            let value = SettingValue::from_reader(&params.get_value()?);
            let outcome = backend.set_app_override(app_id, &key, &value);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn clear_app_override(
        self: Rc<Self>,
        params: st::ClearAppOverrideParams,
        mut results: st::ClearAppOverrideResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = unavailable("clearAppOverride") {
                return Err(refusal);
            }
            if let Some(refusal) = self.gate("clearAppOverride") {
                return Err(refusal);
            }
            let params = params.get()?;
            let app_id = AppId::new(params.get_app_id()?.get_id());
            let key = super::text_of(params.get_key()?);
            let outcome = backend.clear_app_override(app_id, &key);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }

    fn set_if_unchanged(
        self: Rc<Self>,
        params: st::SetIfUnchangedParams,
        mut results: st::SetIfUnchangedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let backend = self.backend.clone();
        async move {
            if let Some(refusal) = self.gate("setIfUnchanged") {
                return Err(refusal);
            }
            let params = params.get()?;
            let domain = super::text_of(params.get_domain()?);
            let key = super::text_of(params.get_key()?);
            let value = SettingValue::from_reader(&params.get_value()?);
            let expected = params.get_expected_version();
            let outcome = backend.set_if_unchanged(&domain, &key, &value, expected);
            let mut builder = results.get();
            void_result(outcome, &mut builder);
            Ok(())
        }
    }
}

/// The settings domain, as a served domain: the contract interface it
/// serves, the descriptor discovery publishes for it, and the per-connection
/// server.
pub struct SettingsDomain<B> {
    backend: Arc<B>,
}

impl<B> SettingsDomain<B>
where
    B: SettingsBackend + 'static,
{
    /// The settings domain over this backend. The backend is shared: the same
    /// host state answers every connection.
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for SettingsDomain<B>
where
    B: SettingsBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        st::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::SETTINGS_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "settings"
    }

    fn category(&self) -> &'static str {
        "settings"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    /// The service's own state: the settings domain cannot answer two
    /// methods of its contract on this host (the app-override pair, which
    /// is an acquisition-time capability decision), so the service is
    /// `degraded` and the missing methods are named in the registry.
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
    /// The paths are the PRODUCT's own words — what `smart settings …` types
    /// — and each one names the contract method that answers it. A verb that
    /// needs several methods publishes the one that produces its value and
    /// lists the others under the stable `composes` key.
    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "settings domains",
                "Settings",
                "listDomains",
                "the domains this host's settings store publishes",
                0,
                true,
            )
            .with_row(matrix_row("listDomains")),
            DomainCommand::new(
                "settings list",
                "Settings",
                "getAll",
                "one domain's live values",
                1,
                false,
            )
            .with_row(matrix_row("getAll")),
            DomainCommand::new(
                "settings get",
                "Settings",
                "get",
                "read one key, as the contract's own union",
                2,
                false,
            )
            .with_row(matrix_row("get")),
            DomainCommand::new(
                "settings set",
                "Settings",
                "set",
                "write one key",
                3,
                false,
            )
            .with_row(matrix_row("set")),
            DomainCommand::new(
                "settings delete",
                "Settings",
                "delete",
                "remove one key",
                4,
                false,
            )
            .with_row(matrix_row("delete")),
            DomainCommand::new(
                "settings clear",
                "Settings",
                "clear",
                "remove every key of one domain",
                5,
                false,
            )
            .with_row(matrix_row("clear")),
            DomainCommand::composing(
                "settings watch",
                "Settings",
                "watch",
                "the host's own change signal, delivered live",
                6,
                false,
                "watchDomain,pull,cancel",
            )
            .with_row(matrix_row("watch")),
            DomainCommand::new(
                "settings watch-domain",
                "Settings",
                "watchDomain",
                "watch every key of one domain",
                7,
                false,
            )
            .with_row(matrix_row("watchDomain")),
            DomainCommand::new(
                "settings bulk-set",
                "Settings",
                "bulkSet",
                "write several keys at once",
                8,
                false,
            )
            .with_row(matrix_row("bulkSet")),
            DomainCommand::new(
                "settings bulk-get",
                "Settings",
                "bulkGet",
                "read several keys at once",
                9,
                false,
            )
            .with_row(matrix_row("bulkGet")),
            DomainCommand::new(
                "settings category",
                "Settings",
                "listByCategory",
                "the keys of one category (an open token)",
                10,
                false,
            )
            .with_row(matrix_row("listByCategory")),
            DomainCommand::new(
                "settings set-app-override",
                "Settings",
                "setAppOverride",
                "set one app's override of one key",
                11,
                false,
            )
            .with_row(matrix_row("setAppOverride")),
            DomainCommand::new(
                "settings get-app-override",
                "Settings",
                "getAppOverride",
                "read one app's override of one key",
                12,
                false,
            )
            .with_row(matrix_row("getAppOverride")),
            DomainCommand::new(
                "settings clear-app-override",
                "Settings",
                "clearAppOverride",
                "remove one app's override of one key",
                13,
                false,
            )
            .with_row(matrix_row("clearAppOverride")),
            DomainCommand::new(
                "settings version",
                "Settings",
                "getVersion",
                "the store's own version of one key",
                14,
                false,
            )
            .with_row(matrix_row("getVersion")),
            DomainCommand::new(
                "settings set-if-unchanged",
                "Settings",
                "setIfUnchanged",
                "write one key only if its version is unchanged",
                15,
                false,
            )
            .with_row(matrix_row("setIfUnchanged")),
            DomainCommand::new(
                "settings history",
                "Settings",
                "getHistory",
                "one key's recorded change history",
                16,
                false,
            )
            .with_row(matrix_row("getHistory")),
            DomainCommand::new(
                "settings diff",
                "Settings",
                "getSettingsDiff",
                "the delta read instead of a full-state poll",
                17,
                false,
            )
            .with_row(matrix_row("getSettingsDiff")),
            DomainCommand::new(
                "settings pull",
                "SettingsSubscription",
                "pull",
                "take the changes a settings subscription has for you",
                18,
                false,
            )
            .with_row(matrix_row("pull")),
            DomainCommand::new(
                "settings cancel",
                "SettingsSubscription",
                "cancel",
                "retire a settings subscription",
                19,
                false,
            )
            .with_row(matrix_row("cancel")),
        ]
    }

    /// The settings schema's node tree: what a client that never compiled
    /// `settings.capnp` needs in order to address these methods.
    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(env!("OUT_DIR"), "/schemas/settings.schema.bin")))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = SettingsService::new(Arc::clone(&self.backend), class);
        let client: st::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
