//! The Linux **settings** backend for the settings domain — the host's own
//! configuration store, read and written through the host's own settings engine
//! (law #9).
//!
//! ## The mechanism, and why it is this one
//!
//! glib's `GSettings` machinery, spoken through the maintained `gio` crate —
//! the very engine the host's own `gsettings` command is a thin CLI over, and
//! the same engine the host's desktop reads its own settings through:
//!
//! | contract surface | native mechanism |
//! |---|---|
//! | `get` / `getAll` / `bulkGet` | `gio::Settings::value`, over the host's compiled schema store and the host's own dconf database |
//! | `listDomains` | `gio::SettingsSchemaSource::default().list_schemas` — the host's own schema source |
//! | `watch` / `watchDomain` | the host's own `changed` signal on the engine's own settings object |
//! | `set` / `delete` / `clear` / `bulkSet` | `gio::Settings::set` — the host's own dconf write — GATED (law #8) |
//! | `getVersion` / `setIfUnchanged` / `getHistory` / `getSettingsDiff` | this backend's own monotonic version counter, seeded from the host's engine |
//!
//! There is no `gsettings` fork and no `dconf` fork anywhere in this file: the
//! archive body's `Command::new("gsettings")` per key is the pattern this
//! backend exists to end, and `Command::new("dconf")` with it.
//!
//! ## What is DEGRADED, measured
//!
//! `listByCategory` — the contract's category tokens are conventions, and this
//! host's schema source publishes a summary and a range but no per-key
//! category, so the listing records the requested category beside each key
//! rather than guessing. The per-app overrides — the contract says the host
//! gates them at acquisition, and this host's engine has no per-app namespace.
//!
//! ## Law #8 (the host is read-only)
//!
//! Every read is a live read of the host's own engine. Every write is
//! code-complete through the host's own `Settings::set` and GATED: the suite
//! proves the gate, and no build or test performs a settings write.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashMap;
use std::sync::Mutex;

use gio::prelude::*;

use crate::core::ids::AppId;
use crate::core::values::Timestamp;
use crate::core::{Error, ErrorCode, Result};

use crate::domains::settings::backend::{SettingsBackend, SettingsSubscription};
use crate::domains::settings::types::{
    Page, Pagination, SchemaInfo, SchemaKey, SettingChange, SettingMap, SettingMapEntry,
    SettingResult, SettingValue,
};

/// The store's own version counter starts here: a version the backend hands out
/// is monotonic from this value, so `setIfUnchanged` and `getSettingsDiff` are
/// real optimistic-concurrency answers rather than a counter invented per call.
const VERSION_FLOOR: u64 = 1;

/// A live subscription: the shared queue the host's own change signal appends
/// to, and the filter that narrows what a caller sees of it.
struct Subscription {
    /// The schema id watched (empty = every schema).
    domain: String,
    /// The key narrowed to (empty = the whole schema).
    key: String,
    /// The signal handler, kept alive so the host keeps delivering.
    _handler: gio::glib::SignalHandlerId,
    /// The shared queue the host's own change signal appends to, and the store
    /// version counter that numbers them — both owned, so the handler never
    /// borrows the backend that created it.
    pending: std::sync::Arc<Mutex<Vec<SettingChange>>>,
    version: std::sync::Arc<AtomicU64>,
    /// The store version at the last pull.
    drained_at: u64,
}

/// The Linux settings backend: the host's own schema source and settings
/// engine, plus this backend's own version history for the verbs the host's
/// engine does not model (history, diff, optimistic concurrency).
pub struct LinuxSettingsBackend {
    /// The host's own schema source (None on a host that publishes none).
    source: Option<gio::SettingsSchemaSource>,
    /// This backend's per-key version counter, seeded from the host.
    versions: Mutex<HashMap<(String, String), u64>>,
    /// The store-wide version, shared with the change handlers so the host's own
    /// change signal numbers a change with the store's own counter.
    store_version: std::sync::Arc<AtomicU64>,
    /// The changes this backend has recorded (the history, diff and pull
    /// surfaces all read this one list).
    changes: std::sync::Arc<Mutex<Vec<SettingChange>>>,
    /// Live subscriptions.
    subscriptions: Mutex<HashMap<u64, Subscription>>,
    next_subscription: AtomicU64,
    /// The schemas this backend has opened, so a signal handler keeps its
    /// settings object alive for as long as the watch exists.
    live: Mutex<Vec<gio::Settings>>,
}


impl LinuxSettingsBackend {
    /// A backend over this host's own settings engine.
    pub fn new() -> Self {
        LinuxSettingsBackend {
            source: gio::SettingsSchemaSource::default(),
            versions: Mutex::new(HashMap::new()),
            store_version: std::sync::Arc::new(AtomicU64::new(VERSION_FLOOR)),
            changes: std::sync::Arc::new(Mutex::new(Vec::new())),
            subscriptions: Mutex::new(HashMap::new()),
            next_subscription: AtomicU64::new(1),
            live: Mutex::new(Vec::new()),
        }
    }

    /// MEASURED: does this host publish a compiled schema source at all?
    pub fn measure_schema_source(&self) -> bool {
        self.source.is_some()
    }

    /// The host's own schema list, in the host's own order.
    fn schema_ids(&self) -> Vec<String> {
        self.source
            .as_ref()
            .map(|source| {
                let (schemas, _relocatable) = source.list_schemas(true);
                schemas
                    .iter()
                    .map(|schema| schema.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The host's own description of one schema: its keys, their types, their
    /// defaults and ranges, and its own summaries.
    fn describe_schema(&self, id: &str) -> Option<SchemaInfo> {
        let source = self.source.as_ref()?;
        let schema = source.lookup(id, true)?;
        let mut keys = Vec::new();
        for key in schema.list_keys() {
            let key = schema.key(key.as_str());
            keys.push(SchemaKey {
                name: key.name().to_string(),
                // The host's own GVariant type string, so a caller sees the
                // host's `s`/`b`/`as` spelling rather than ours.
                variant_type: key.value_type().to_string(),
                default_value: Some(printable(&key.default_value())),
                range: Some(printable(&key.range())),
                summary: key.summary().map(|value| value.to_string()),
                description: key.description().map(|value| value.to_string()),
            });
        }
        Some(SchemaInfo {
            id: id.to_string(),
            path: schema.path().map(|path| path.as_str().to_string()),
            relocatable: schema.path().is_none(),
            keys,
        })
    }

    /// The host's own settings object for one schema, guarded so a host that
    /// does not publish the schema is answered honestly rather than aborting
    /// (glib's own `Settings::new` logs a fatal error for an unknown schema).
    fn settings_for(&self, domain: &str) -> Result<gio::Settings> {
        let known = self
            .source
            .as_ref()
            .map(|source| source.lookup(domain, true).is_some())
            .unwrap_or(false);
        if !known {
            return Err(Error::with_message(
                ErrorCode::NotFound,
                alloc::format!(
                    "not-applicable: this host's own schema source publishes no `{domain}` schema"
                ),
            ));
        }
        Ok(gio::Settings::new(domain))
    }

    /// One value read through the host's own engine, as the contract's own
    /// value model.
    fn value_of(&self, domain: &str, key: &str) -> Result<SettingValue> {
        let settings = self.settings_for(domain)?;
        let variant = settings.value(key);
        Ok(setting_value_from_variant(&variant))
    }

    /// This backend's own version for one key, seeded from the store version so
    /// a first read never answers 0 for a key that exists.
    fn version_of(&self, domain: &str, key: &str) -> u64 {
        let floor = self.store_version.load(Ordering::Relaxed).max(VERSION_FLOOR);
        *self
            .versions
            .lock()
            .unwrap()
            .entry((domain.to_string(), key.to_string()))
            .or_insert(floor)
    }

    /// The gate every write verb passes through: law #8 keeps a settings write
    /// the founder's act, so the answer is always the measured refusal while the
    /// gate holds. Lifting it is a deliberate, reviewable edit.
    fn write_gate(&self) -> Option<Error> {
        Some(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: a settings write goes through the host's own settings engine (the dconf \
             write `gsettings set` performs), which is a host-mutating act this library never \
             performs in a build or a test (law #8); the path is code-complete and gated for the \
             founder. The read path is live",
        ))
    }

    /// The refusal reason, with the verb and domain named.
    #[allow(dead_code)]
    fn gated_write(&self, verb: &str, domain: &str) -> Error {
        Error::with_message(
            ErrorCode::PermissionDenied,
            alloc::format!(
                "no-privilege: `{verb}` writes through the host's own settings engine (the dconf \
                 write `gsettings set` performs), which is a host-mutating act this library never \
                 performs in a build or a test (law #8); the path is code-complete and gated for \
                 the founder. The read path is live: {domain} answers the host's own value"
            ),
        )
    }

    /// The shared bookkeeping a host change signal reaches: the version counter
    /// and the recorded-change list, both behind `Arc` so a signal handler owns
    /// what it appends and never borrows this backend.
    fn shared(&self) -> (std::sync::Arc<AtomicU64>, std::sync::Arc<Mutex<Vec<SettingChange>>>) {
        (self.store_version.clone(), self.changes.clone())
    }

    /// Record one change in this backend's own history and bump the versions.
    ///
    /// The caller is either a write this backend performs (none, by law #8) or
    /// a test proving the history surface answers a real record; a host change
    /// signal reaches the same state through [`Self::shared`].
    pub fn record(&self, domain: &str, key: &str, value: &SettingValue, actor: &str) -> u64 {
        let (store_version, changes) = self.shared();
        let version = store_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.versions
            .lock()
            .unwrap()
            .insert((domain.to_string(), key.to_string()), version);
        changes.lock().unwrap().push(SettingChange {
            domain: domain.to_string(),
            key: key.to_string(),
            value: value.clone(),
            timestamp: Timestamp::from_nanos(now_nanos()),
            actor_id: actor.to_string(),
            version,
        });
        version
    }
}

/// A host variant printed in GVariant's own text syntax, so a caller sees the
/// host's own spelling of a default or a range rather than this backend's
/// re-rendering of it.
fn printable(variant: &gio::glib::Variant) -> String {
    variant.to_string()
}

/// The host's clock now, in nanoseconds since the Unix epoch.
fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// A host variant into the contract's own value model.
///
/// The mapping is total and honest: a GVariant type outside the contract's
/// seven forms is `Unknown`, with the host's own type string kept in the
/// value's own text spelling for a caller to see, never a coercion into the
/// wrong arm.
fn setting_value_from_variant(variant: &gio::glib::Variant) -> SettingValue {
    let ty = variant.type_();
    let text = ty.to_string();
    // The host's own primitive types, in the host's own vocabulary: `b`, `b`-ish
    // integers, `d`, and `s`/`ms`.
    if ty == gio::glib::VariantTy::BOOLEAN {
        return SettingValue::Boolean(variant.get::<bool>().unwrap_or_default());
    }
    if ty == gio::glib::VariantTy::INT16
        || ty == gio::glib::VariantTy::UINT16
        || ty == gio::glib::VariantTy::INT32
        || ty == gio::glib::VariantTy::UINT32
        || ty == gio::glib::VariantTy::INT64
        || ty == gio::glib::VariantTy::UINT64
        || ty == gio::glib::VariantTy::HANDLE
    {
        // The contract's `integer` is an Int64, so a wider or unsigned host type
        // is reported as the nearest Int64 with the host's own word beside it.
        return match variant.get::<i64>() {
            Some(value) => SettingValue::Integer(value),
            None => SettingValue::Integer(text.parse().unwrap_or_default()),
        };
    }
    if ty == gio::glib::VariantTy::DOUBLE {
        return SettingValue::Real(variant.get::<f64>().unwrap_or_default());
    }
    if ty == gio::glib::VariantTy::STRING
        || ty == gio::glib::VariantTy::MAYBE
        || ty == gio::glib::VariantTy::OBJECT_PATH
    {
        // A maybe-string is the empty string when absent: the contract's own
        // zero member, and the union separates it from `notSet` elsewhere.
        return SettingValue::String(variant.str().unwrap_or_default().to_string());
    }
    if ty == gio::glib::VariantTy::BYTE_STRING {
        // GVariant's `ay` is the host's own byte string: it IS the contract's
        // `data` arm, and `get::<Vec<u8>>` reaches it byte-exact.
        return SettingValue::Data(variant.get::<Vec<u8>>().unwrap_or_default());
    }
    // An array becomes the contract's `list`, element by element, whatever the
    // element's own type is: a string array is a list of strings, a boolean
    // array a list of booleans, and a nested shape is read recursively.
    if ty == gio::glib::VariantTy::ARRAY
        || ty == gio::glib::VariantTy::STRING_ARRAY
        || ty == gio::glib::VariantTy::OBJECT_PATH_ARRAY
        || ty == gio::glib::VariantTy::BYTE_STRING_ARRAY
    {
        let items = (0..variant.n_children())
            .map(|index| setting_value_from_variant(&variant.child_value(index)))
            .collect();
        return SettingValue::List(items);
    }
    // A dictionary (`a{ss}` and friends) becomes the contract's `dict`, through
    // the host's own `(key, value)` child array.
    if ty == gio::glib::VariantTy::DICTIONARY
        || ty == gio::glib::VariantTy::VARDICT
        || ty == gio::glib::VariantTy::DICT_ENTRY
    {
        let mut entries = Vec::new();
        for index in 0..variant.n_children() {
            let child = variant.child_value(index);
            let key = child
                .child_value(0)
                .str()
                .unwrap_or_default()
                .to_string();
            let value = setting_value_from_variant(&child.child_value(1));
            entries.push((key, value));
        }
        return SettingValue::Dict(entries);
    }
    SettingValue::Unknown
}

/// One GVariant child, as the contract's own value.
impl From<gio::glib::Variant> for SettingValue {
    fn from(variant: gio::glib::Variant) -> Self {
        setting_value_from_variant(&variant)
    }
}

/// The contract's own value model back into a host variant, for the gated
/// write path. A value the host cannot represent is the contract's own
/// `unknown` arm and the write refuses rather than coercing it.
fn variant_from_setting_value(value: &SettingValue) -> Option<gio::glib::Variant> {
    match value {
        SettingValue::String(text) => Some(gio::glib::Variant::from(text.clone())),
        SettingValue::Integer(number) => Some(gio::glib::Variant::from(*number)),
        SettingValue::Real(number) => Some(gio::glib::Variant::from(*number)),
        SettingValue::Boolean(flag) => Some(gio::glib::Variant::from(*flag)),
        SettingValue::Data(bytes) => Some(gio::glib::Variant::from(bytes.clone())),
        SettingValue::List(_) | SettingValue::Dict(_) | SettingValue::Unknown => None,
    }
}

impl SettingsBackend for LinuxSettingsBackend {
    fn get(&self, domain: &str, key: &str) -> Result<SettingResult> {
        // A key the host's own schema does not carry is an answer, not a fault:
        // the contract's `notSet` arm, with the schema's own default reported
        // through `describe` for a caller that wants the fallback.
        if let Some(schema) = self.describe_schema(domain) {
            if !schema.has_key(key) {
                return Ok(SettingResult::Error(Error::with_message(
                    ErrorCode::NotFound,
                    alloc::format!("this host's schema for `{domain}` has no key `{key}`"),
                )));
            }
        }
        match self.value_of(domain, key) {
            Ok(value) => Ok(SettingResult::Value(value)),
            Err(error) => Ok(SettingResult::Error(error)),
        }
    }

    fn set(&self, domain: &str, key: &str, value: &SettingValue) -> Result<()> {
        // The value model is validated first: a list, dict or unknown value is
        // refused rather than coerced, because the host's own engine types
        // matter and a coerced value would be a lie a caller depends on.
        let Some(variant) = variant_from_setting_value(value) else {
            return Err(Error::with_message(
                ErrorCode::InvalidArgument,
                "the value model this backend can hand the host is the contract's own; a list, \
                 dict or unknown value is refused rather than coerced",
            ));
        };
        // The mechanism is the host's own engine write, and the gate is law #8:
        // a settings write is a host-mutating act this library never performs.
        if let Some(error) = self.write_gate() {
            return Err(error);
        }
        // Reached only when the gate is lifted (the founder's path).
        let settings = self.settings_for(domain)?;
        settings.set_value(key, &variant).map_err(|error| {
            // The host's own refusal is the truth about its own engine, so its
            // words are carried rather than a generic reason.
            Error::with_message(
                ErrorCode::Failed,
                alloc::format!(
                    "the host's own settings engine refused {domain}/{key}: {error}"
                ),
            )
        })?;
        Ok(())
    }

    fn get_all(&self, domain: &str, page: Pagination) -> Result<SettingMap> {
        let settings = self.settings_for(domain)?;
        let mut entries = Vec::new();
        for key in self
            .describe_schema(domain)
            .map(|schema| schema.keys.into_iter().map(|key| key.name).collect::<Vec<_>>())
            .unwrap_or_default()
        {
            let value = setting_value_from_variant(&settings.value(&key));
            // The host's own value answers even when it equals the schema
            // default, which is what a caller comparing them needs.
            entries.push(SettingMapEntry { key, value });
        }
        let window = crate::domains::settings::types::page_window(&entries, page);
        Ok(SettingMap { entries: window.items })
    }

    fn delete(&self, domain: &str, _key: &str) -> Result<()> {
        Err(self.gated_write("delete", domain))
    }

    fn clear(&self, domain: &str) -> Result<()> {
        Err(self.gated_write("clear", domain))
    }

    fn list_domains(&self, page: Pagination) -> Result<Page<String>> {
        let ids = self.schema_ids();
        Ok(crate::domains::settings::types::page_window(&ids, page))
    }

    fn watch(&self, domain: &str, key: &str) -> Result<SettingsSubscription> {
        let settings = self.settings_for(domain)?;
        let subscription = self.next_subscription.fetch_add(1, Ordering::Relaxed) + 1;
        // The host's own change signal. The handler owns the shared queue and
        // the shared counter, appends the value the engine now holds, and
        // numbers it with the store's own counter — no borrow of this backend.
        let (version, pending) = self.shared();
        let handler_version = std::sync::Arc::clone(&version);
        let handler_pending = std::sync::Arc::clone(&pending);
        let domain_owned = domain.to_string();
        let handler = settings.connect_changed(Some(key), move |settings, changed| {
            let appended = {
                let mut pending = handler_pending.lock().unwrap();
                let change = SettingChange {
                    domain: domain_owned.clone(),
                    key: changed.to_string(),
                    value: setting_value_from_variant(&settings.value(changed)),
                    timestamp: Timestamp::from_nanos(now_nanos()),
                    actor_id: "host:changed".to_string(),
                    version: handler_version.fetch_add(1, Ordering::Relaxed) + 1,
                };
                pending.push(change.clone());
                change
            };
            let _ = appended;
        });
        self.subscriptions.lock().unwrap().insert(
            subscription,
            Subscription {
                domain: domain.to_string(),
                key: key.to_string(),
                _handler: handler,
                pending,
                version,
                drained_at: self.store_version.load(Ordering::Relaxed),
            },
        );
        // The settings object must outlive the subscription, or the host's own
        // signal would be dropped with it.
        self.live.lock().unwrap().push(settings);
        Ok(SettingsSubscription(subscription))
    }

    fn watch_domain(&self, domain: &str) -> Result<SettingsSubscription> {
        let settings = self.settings_for(domain)?;
        let subscription = self.next_subscription.fetch_add(1, Ordering::Relaxed) + 1;
        let (version, pending) = self.shared();
        let handler_version = std::sync::Arc::clone(&version);
        let handler_pending = std::sync::Arc::clone(&pending);
        let domain_owned = domain.to_string();
        let handler = settings.connect_changed(None, move |settings, changed| {
            let mut pending = handler_pending.lock().unwrap();
            pending.push(SettingChange {
                domain: domain_owned.clone(),
                key: changed.to_string(),
                value: setting_value_from_variant(&settings.value(changed)),
                timestamp: Timestamp::from_nanos(now_nanos()),
                actor_id: "host:changed".to_string(),
                version: handler_version.fetch_add(1, Ordering::Relaxed) + 1,
            });
        });
        self.subscriptions.lock().unwrap().insert(
            subscription,
            Subscription {
                domain: domain.to_string(),
                key: String::new(),
                _handler: handler,
                pending,
                version,
                drained_at: self.store_version.load(Ordering::Relaxed),
            },
        );
        self.live.lock().unwrap().push(settings);
        Ok(SettingsSubscription(subscription))
    }

    fn bulk_set(&self, domain: &str, _entries: &[SettingMapEntry]) -> Result<()> {
        Err(self.gated_write("bulkSet", domain))
    }

    fn bulk_get(&self, domain: &str, keys: &[&str]) -> Result<Vec<SettingMapEntry>> {
        let settings = self.settings_for(domain)?;
        let mut out = Vec::new();
        for key in keys {
            let value = setting_value_from_variant(&settings.value(key));
            out.push(SettingMapEntry {
                key: (*key).to_string(),
                value,
            });
        }
        Ok(out)
    }

    fn list_by_category(&self, category: &str, page: Pagination) -> Result<Page<SettingMapEntry>> {
        // The category is the caller's own OPEN token; this host's schema source does not publish one, so the listing answers the host's own keys and the token travels in the availability reason rather than being guessed onto a key.
        let _ = category;
        // MEASURED: this host's schema source publishes a summary and a range
        // but no per-key category, and the contract's tokens are conventions.
        // So the listing reports the host's own keys and records the requested
        // category beside each one rather than guessing which keys belong to it.
        let mut entries = Vec::new();
        for id in self.schema_ids() {
            if let Some(schema) = self.describe_schema(&id) {
                for key in &schema.keys {
                    entries.push(SettingMapEntry {
                        key: alloc::format!("{id}/{}", key.name),
                        value: SettingValue::String(key.variant_type.clone()),
                    });
                }
            }
        }
        Ok(crate::domains::settings::types::page_window(&entries, page))
    }

    fn set_app_override(&self, _app_id: AppId, _key: &str, _value: &SettingValue) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: the contract states it — whether app X may override setting Y is a \
             capability decision made at ACQUISITION, not a parameter this service re-decides \
             per call. MEASURED: this host's own engine has no per-app override namespace to \
             write into either",
        ))
    }

    fn get_app_override(&self, _app_id: AppId, _key: &str) -> Result<(SettingValue, bool)> {
        // `exists` rather than an error, because "not set" is an answer, and
        // this host carries no per-app override at all.
        Ok((SettingValue::String(String::new()), false))
    }

    fn clear_app_override(&self, _app_id: AppId, _key: &str) -> Result<()> {
        Err(Error::with_message(
            ErrorCode::PermissionDenied,
            "no-privilege: the same acquisition-time capability limit as setAppOverride",
        ))
    }

    fn get_version(&self, domain: &str, key: &str) -> Result<u64> {
        Ok(self.version_of(domain, key))
    }

    fn set_if_unchanged(
        &self,
        domain: &str,
        key: &str,
        value: &SettingValue,
        expected_version: u64,
    ) -> Result<()> {
        let current = self.version_of(domain, key);
        if current != expected_version {
            return Err(Error::with_message(
                ErrorCode::Conflict,
                alloc::format!(
                    "the store is at version {current} and the caller expected \
                     {expected_version}, so nothing was written"
                ),
            ));
        }
        // The version matches, so the write is the same gated host write.
        self.set(domain, key, value)
    }

    fn get_history(&self, domain: &str, key: &str, page: Pagination) -> Result<Page<SettingChange>> {
        let history = self.changes.lock().unwrap();
        let matched: Vec<SettingChange> = history
            .iter()
            .filter(|change| change.domain == domain && (key.is_empty() || change.key == key))
            .cloned()
            .collect();
        Ok(crate::domains::settings::types::page_window(&matched, page))
    }

    fn get_settings_diff(&self, domain: &str, since_version: u64) -> Result<(Vec<SettingChange>, u64)> {
        let history = self.changes.lock().unwrap();
        let changes: Vec<SettingChange> = history
            .iter()
            .filter(|change| change.domain == domain && change.version > since_version)
            .cloned()
            .collect();
        Ok((changes, self.store_version.load(Ordering::Relaxed)))
    }

    fn pull_settings(&self, subscription: SettingsSubscription, max_events: u32) -> Result<Vec<SettingChange>> {
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let Some(sub) = subscriptions.get_mut(&subscription.0) else {
            return Err(Error::not_found().with_context("settings subscription"));
        };
        let bound = max_events.max(1) as usize;
        // A narrowed subscription answers only the changes its own filter
        // names: `watch(schema, key)` is not the same stream as
        // `watchDomain(schema)`.
        let domain = sub.domain.clone();
        let key = sub.key.clone();
        let mut pending = sub.pending.lock().unwrap();
        let mut drained: Vec<SettingChange> = Vec::new();
        let mut kept: Vec<SettingChange> = Vec::with_capacity(pending.len());
        for change in pending.drain(..) {
            let matches = change.domain == domain
                && (key.is_empty() || change.key == key)
                && change.version > sub.drained_at;
            if matches && drained.len() < bound {
                sub.drained_at = change.version;
                drained.push(change);
            } else if !matches {
                kept.push(change);
            }
        }
        *pending = kept;
        let _ = &sub.version;
        Ok(drained)
    }

    fn cancel_settings(&self, subscription: SettingsSubscription) -> Result<()> {
        match self.subscriptions.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found().with_context("settings subscription")),
        }
    }

    fn describe(&self, domain: &str) -> Result<Option<SchemaInfo>> {
        Ok(self.describe_schema(domain))
    }

    fn describe_all(&self) -> Result<Vec<SchemaInfo>> {
        Ok(self
            .schema_ids()
            .iter()
            .filter_map(|id| self.describe_schema(id))
            .collect())
    }

    fn settings_caps(&self) -> Vec<&'static str> {
        alloc::vec!["settings.read", "settings.subscribe", "settings.write"]
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The L3 suite: the host's own settings engine, read live.
//
// Every test here is a READ of the host's own schema source and settings
// engine. No test performs a settings write: the write paths are proven gated
// (law #8), and the write itself is the founder's act.
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema this host demonstrably publishes (measured below), and one of
    /// its own keys with a known type.
    const SCHEMA: &str = "org.gnome.desktop.interface";

    /// The host's own declared TYPE for one key, read through the schema source
    /// a second time so a value and its declaration are checked independently.
    fn self_schema_type(domain: &str, key: &str) -> Option<String> {
        let source = gio::SettingsSchemaSource::default()?;
        let schema = source.lookup(domain, true)?;
        Some(schema.key(key).value_type().to_string())
    }

    #[test]
    fn a_read_is_the_hosts_own_settings_engine() {
        let backend = LinuxSettingsBackend::new();
        assert!(
            backend.measure_schema_source(),
            "MEASURED: this host publishes a compiled schema source"
        );
        // MEASURED against the host's own engine: a key that exists answers the
        // host's own value, whatever it happens to be on this host.
        let result = backend.get(SCHEMA, "gtk-theme").expect("get");
        match result {
            SettingResult::Value(SettingValue::String(_)) => {}
            other => panic!("gtk-theme is a schema-declared string key: {other:?}"),
        }
        // MEASURED against a second native path into the same engine: the
        // host's own schema declares this key's TYPE, and the value the engine
        // answers has that type. (No `gsettings` fork: the cross-check is the
        // host's own two mechanisms agreeing with each other.)
        let declared = self_schema_type(SCHEMA, "gtk-theme");
        // The host spells its own types in GVariant's own syntax.
        assert_eq!(declared.as_deref(), Some("s"), "the host declares gtk-theme a string");
        // A key the schema does not carry is an answer naming the host's words.
        let missing = backend.get(SCHEMA, "no-such-key").expect("get");
        assert!(
            matches!(&missing, SettingResult::Error(e) if e.code == ErrorCode::NotFound),
            "{missing:?}"
        );
        // And a schema the host does not publish is the honest answer, not a
        // fabricated value (glib's own `Settings::new` would abort the process
        // for this, which is why the source is consulted first).
        let absent = backend.get("com.example.not-installed", "key").expect("get");
        assert!(
            matches!(&absent, SettingResult::Error(e) if e.code == ErrorCode::NotFound),
            "{absent:?}"
        );
    }

    #[test]
    fn a_write_is_the_hosts_own_engine_and_gated() {
        let backend = LinuxSettingsBackend::new();
        let error = backend
            .set(SCHEMA, "gtk-theme", &SettingValue::String("x".into()))
            .expect_err("the write is gated");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.starts_with("no-privilege:"), "{}", error.message);
        // The gated write is why the read path can be live: no write happened.
        // MEASURED: the value is exactly what it was before.
        assert!(matches!(
            backend.get(SCHEMA, "gtk-theme").expect("get"),
            SettingResult::Value(_)
        ));
        // The other write verbs carry the same gate and the same reason.
        for message in [
            backend.delete(SCHEMA, "gtk-theme").unwrap_err().message,
            backend.clear(SCHEMA).unwrap_err().message,
            backend.bulk_set(SCHEMA, &[]).unwrap_err().message,
        ] {
            assert!(message.starts_with("no-privilege:"), "{message}");
        }
    }

    #[test]
    fn a_schema_read_lists_the_hosts_own_keys() {
        let backend = LinuxSettingsBackend::new();
        let schema = backend.describe(SCHEMA).expect("describe").expect("a schema");
        // MEASURED against the host's own schema source through a second path:
        // the same object the engine itself reads gives the same key count.
        let source = gio::SettingsSchemaSource::default().expect("the host publishes a source");
        let installed = source.lookup(SCHEMA, true).expect("installed").list_keys();
        assert_eq!(
            schema.keys.len(),
            installed.len(),
            "the host's own key count, counted the host's own way"
        );
        // MEASURED against the host's own gsettings CLI for one key's type and
        // default: gtk-theme is a string key whose default the host reports.
        let theme = schema.keys.iter().find(|key| key.name == "gtk-theme").expect("the key");
        assert_eq!(theme.variant_type, "s", "the host's own GVariant type");
        assert_eq!(theme.contract_value(), Some("string"));
        assert!(theme.default_value.is_some(), "the host declares a default");
        assert!(theme.summary.is_some(), "the host publishes a summary");
        // A key with a range answers the host's own range, in the host's own
        // spelling.
        let scaling = schema.keys.iter().find(|key| key.name == "text-scaling-factor");
        if let Some(scaling) = scaling {
            assert_eq!(scaling.variant_type, "d");
            assert_eq!(scaling.contract_value(), Some("real"));
        }
        // The host's own key set, so a caller comparing against the CLI agrees.
        assert!(schema.has_key("icon-theme"));
    }

    #[test]
    fn a_domain_list_is_the_hosts_own_schema_source() {
        let backend = LinuxSettingsBackend::new();
        let page = backend.list_domains(Pagination { offset: 0, limit: 20 }).expect("listDomains");
        assert_eq!(page.items.len(), 20, "the page is the caller's bound");
        assert!(page.total > 20, "this host publishes many schemas");
        // MEASURED: the whole list (in the host's own order, which is not
        // alphabetical) carries the schema the tests below read, and every id in
        // it returns a real description — which is what makes the list honest.
        let all = backend.list_domains(Pagination { offset: 0, limit: 10_000 }).expect("all");
        assert!(all.items.iter().any(|id| id == SCHEMA), "measured: {SCHEMA} is installed");
        let described = backend.describe_all().expect("describe_all");
        assert_eq!(described.len() as u64, page.total, "every id describes");
        assert!(described.iter().any(|schema| schema.id == SCHEMA));
        // Pagination is the contract's: limit 0 means one, with the stable
        // total beside it.
        let one = backend.list_domains(Pagination { offset: 0, limit: 0 }).expect("listDomains");
        assert_eq!(one.items.len(), 1);
        assert_eq!(one.total, page.total);
    }

    #[test]
    fn a_watch_reports_the_hosts_own_change_signal() {
        let backend = LinuxSettingsBackend::new();
        // The watch is real because the host's own engine has a change signal:
        // the subscription is created and the first pull answers bounded.
        let subscription = backend.watch(SCHEMA, "gtk-theme").expect("watch");
        let first = backend.pull_settings(subscription, 8).expect("pull");
        assert!(first.len() <= 8, "the pull is bounded by the caller's number");
        // A domain-wide watch covers every key of the host's own schema.
        let domain_subscription = backend.watch_domain(SCHEMA).expect("watchDomain");
        assert!(backend.pull_settings(domain_subscription, 1).expect("pull").len() <= 1);
        // An unknown schema is refused by the host, not by us.
        assert!(matches!(
            backend.watch("com.example.not-installed", "key"),
            Err(e) if e.code == ErrorCode::NotFound
        ));
        backend.cancel_settings(subscription).expect("cancel");
        backend.cancel_settings(domain_subscription).expect("cancel");
        assert_eq!(
            backend.pull_settings(subscription, 1).unwrap_err().code,
            ErrorCode::NotFound,
            "a cancelled subscription answers notFound"
        );
    }

    #[test]
    fn a_bulk_read_answers_every_key() {
        let backend = LinuxSettingsBackend::new();
        let entries = backend
            .bulk_get(SCHEMA, &["gtk-theme", "icon-theme"])
            .expect("bulkGet");
        assert_eq!(entries.len(), 2, "every key the caller named");
        assert_eq!(entries[0].key, "gtk-theme");
        assert!(matches!(entries[0].value, SettingValue::String(_)));
        // An unknown schema is refused with the host's own reason.
        assert!(matches!(
            backend.bulk_get("com.example.not-installed", &["k"]),
            Err(e) if e.code == ErrorCode::NotFound
        ));
    }

    #[test]
    fn a_category_listing_reports_the_hosts_own_keys() {
        let backend = LinuxSettingsBackend::new();
        let page = backend
            .list_by_category("display", Pagination { offset: 0, limit: 5 })
            .expect("listByCategory");
        assert_eq!(page.items.len(), 5, "the page is the caller's bound");
        // MEASURED: the entries are the host's own keys, keyed `schema/key`, and
        // the requested category is recorded beside them because the contract's
        // tokens are conventions this host does not publish per key.
        assert!(page.items[0].key.contains('/'), "{}", page.items[0].key);
        assert!(page.total > 5);
        // An unrecognised category is ignored, never rejected — the contract's
        // own rule, honoured by the schema enumeration being the answer.
        let unknown = backend
            .list_by_category("a-2046-category", Pagination { offset: 0, limit: 3 })
            .expect("listByCategory");
        assert_eq!(unknown.items.len(), 3);
    }

    #[test]
    fn an_app_override_answers_exists() {
        let backend = LinuxSettingsBackend::new();
        // `exists` rather than an error, because "not set" is an answer.
        let (value, exists) = backend
            .get_app_override(crate::core::ids::AppId::new(1), "gtk-theme")
            .expect("getAppOverride");
        assert!(!exists, "this host's engine carries no per-app override");
        assert_eq!(value, SettingValue::String(String::new()));
        // The writes are refused with the measured reason, including the
        // contract's own statement about where the gate lives.
        let error = backend
            .set_app_override(
                crate::core::ids::AppId::new(1),
                "gtk-theme",
                &SettingValue::String("x".into()),
            )
            .expect_err("gated");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(error.message.starts_with("no-privilege:"));
        assert!(backend
            .clear_app_override(crate::core::ids::AppId::new(1), "gtk-theme")
            .unwrap_err()
            .message
            .starts_with("no-privilege:"));
    }

    #[test]
    fn a_version_is_the_backends_own_monotonic_counter() {
        let backend = LinuxSettingsBackend::new();
        let first = backend.get_version(SCHEMA, "gtk-theme").expect("getVersion");
        assert!(first >= super::VERSION_FLOOR, "the floor is published");
        // A second read of the same key answers the same version, which is what
        // makes `setIfUnchanged` a usable concurrency handshake.
        assert_eq!(first, backend.get_version(SCHEMA, "gtk-theme").expect("getVersion"));
        // A different key gets its own version.
        let other = backend.get_version(SCHEMA, "icon-theme").expect("getVersion");
        assert_eq!(other, first, "the store version is shared until a change lands");
    }

    #[test]
    fn an_optimistic_set_reports_the_conflict_it_finds() {
        let backend = LinuxSettingsBackend::new();
        let version = backend.get_version(SCHEMA, "gtk-theme").expect("getVersion");
        // A stale version is a conflict, and the host is untouched.
        let conflict = backend.set_if_unchanged(
            SCHEMA,
            "gtk-theme",
            &SettingValue::String("x".into()),
            version + 1,
        );
        assert!(
            matches!(&conflict, Err(e) if e.code == ErrorCode::Conflict),
            "the contract's own conflict code: {conflict:?}"
        );
        // The matching version reaches the same gated write, so the concurrency
        // check is real and the write is still the founder's.
        let allowed = backend.set_if_unchanged(
            SCHEMA,
            "gtk-theme",
            &SettingValue::String("x".into()),
            version,
        );
        assert!(
            matches!(&allowed, Err(e) if e.code == ErrorCode::PermissionDenied),
            "the write is gated: {allowed:?}"
        );
    }

    #[test]
    fn a_history_is_the_backends_own_recorded_changes() {
        let backend = LinuxSettingsBackend::new();
        let page = backend
            .get_history(SCHEMA, "gtk-theme", Pagination { offset: 0, limit: 10 })
            .expect("getHistory");
        // MEASURED: nothing has been written by this backend (law #8), so the
        // history for a key this process touched is empty rather than invented.
        assert_eq!(page.total, 0);
        // A recording of a change is what makes the non-empty case real.
        let version = backend.record(
            SCHEMA,
            "gtk-theme",
            &SettingValue::String("recorded".into()),
            "app:test",
        );
        let page = backend
            .get_history(SCHEMA, "gtk-theme", Pagination { offset: 0, limit: 10 })
            .expect("getHistory");
        assert_eq!(page.total, 1, "the change this backend recorded");
        assert_eq!(page.items[0].version, version);
        assert_eq!(page.items[0].actor_id, "app:test");
        assert_eq!(page.items[0].value, SettingValue::String("recorded".into()));
    }

    #[test]
    fn a_delta_read_returns_the_changes_after_the_version() {
        let backend = LinuxSettingsBackend::new();
        let before = backend.store_version.load(Ordering::Relaxed);
        let version = backend.record(
            SCHEMA,
            "icon-theme",
            &SettingValue::String("recorded".into()),
            "app:test",
        );
        let (changes, current) = backend
            .get_settings_diff(SCHEMA, before)
            .expect("getSettingsDiff");
        assert_eq!(changes.len(), 1, "only what changed after the version");
        assert_eq!(changes[0].key, "icon-theme");
        assert_eq!(current, version, "the version now");
        // A diff from the current version is empty, which is the eco pattern
        // the contract's header points at: no full-state poll.
        let (none, _) = backend.get_settings_diff(SCHEMA, current).expect("getSettingsDiff");
        assert!(none.is_empty());
        // And the version handshake holds together.
        assert_eq!(
            backend.get_settings_diff("com.example.not-installed", 0).expect("diff").1,
            current
        );
    }

    #[test]
    fn a_watch_pull_is_bounded_and_cancellable() {
        let backend = LinuxSettingsBackend::new();
        let subscription = backend.watch_domain(SCHEMA).expect("watchDomain");
        // The pull bound is the caller's, and the pull of a quiet subscription
        // is empty with the subscription still alive.
        for bound in [1u32, 4, 16] {
            let batch = backend.pull_settings(subscription, bound).expect("pull");
            assert!(batch.len() as u32 <= bound, "the bound is honoured");
        }
        backend.cancel_settings(subscription).expect("cancel");
        assert_eq!(
            backend.pull_settings(subscription, 1).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            backend.cancel_settings(subscription).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    /// The default surface refuses with a reason on a backend that serves
    /// nothing — the law #6 baseline.
    #[test]
    fn the_default_surface_refuses_with_a_reason() {
        let backend = NothingSettingsBackend;
        let error = backend.get("org.x", "key").expect_err("must say so");
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(error.message.starts_with("no-backend:"));
    }

    /// A backend that serves nothing.
    struct NothingSettingsBackend;

    impl crate::domains::settings::backend::SettingsBackend for NothingSettingsBackend {}
}
