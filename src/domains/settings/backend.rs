//! The `SettingsBackend` trait: one object-safe method per `Settings` contract
//! method (`get @0` through `getSettingsDiff @17`), plus the
//! `SettingsSubscription` interface.
//!
//! ## Capability story (law #5)
//!
//! Writes are gated ONCE, by the capability held when the caller acquired this
//! service. The store does not check per-key permissions itself: a backend that
//! also enforces a rule inside `set` has two authorities that must agree
//! forever. Writing `system.*` requires a platform capability; an app holding
//! none may still read public domains and write its own.
//!
//! ## The honest default surface
//!
//! Each method has a default body answering `notSupported` **with a
//! `reasonClass`-prefixed reason** — the law #6 baseline a platform backend
//! overrides exactly where it serves something.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::ids::AppId;
use crate::core::{Error, Result};

use super::types::{
    Page, Pagination, SchemaInfo, SettingChange, SettingMap, SettingMapEntry, SettingResult,
    SettingValue,
};

/// A settings subscription. Holding the returned id IS the authority to pull.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SettingsSubscription(pub u64);

/// A protocol-compliant settings backend.
pub trait SettingsBackend {
    /// `get @0`. The union is the contract's own: a value, `notSet`, an error,
    /// or the unknown arm.
    fn get(&self, _domain: &str, _key: &str) -> Result<SettingResult> {
        Err(Error::not_supported(
            "no-backend: the settings read is not served on this backend",
        ))
    }

    /// `set @1` — the gated write.
    fn set(&self, _domain: &str, _key: &str, _value: &SettingValue) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing a setting is a host-mutating act this backend never performs",
        ))
    }

    /// `getAll @2`.
    fn get_all(&self, _domain: &str, _page: Pagination) -> Result<SettingMap> {
        Err(Error::not_supported(
            "no-backend: the settings read is not served on this backend",
        ))
    }

    /// `delete @3` — the gated write.
    fn delete(&self, _domain: &str, _key: &str) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing a setting is a host-mutating act this backend never performs",
        ))
    }

    /// `clear @4` — the gated write.
    fn clear(&self, _domain: &str) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing a setting is a host-mutating act this backend never performs",
        ))
    }

    /// `listDomains @5`.
    fn list_domains(&self, _page: Pagination) -> Result<Page<String>> {
        Err(Error::not_supported(
            "no-backend: the domain list is not served on this backend",
        ))
    }

    /// `watch @6` — one key.
    fn watch(&self, _domain: &str, _key: &str) -> Result<SettingsSubscription> {
        Err(Error::not_supported(
            "no-backend: settings subscriptions are not served on this backend",
        ))
    }

    /// `watchDomain @7` — a whole schema.
    fn watch_domain(&self, _domain: &str) -> Result<SettingsSubscription> {
        Err(Error::not_supported(
            "no-backend: settings subscriptions are not served on this backend",
        ))
    }

    /// `bulkSet @8` — the gated write.
    fn bulk_set(&self, _domain: &str, _entries: &[SettingMapEntry]) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: writing a setting is a host-mutating act this backend never performs",
        ))
    }

    /// `bulkGet @9`.
    fn bulk_get(&self, _domain: &str, _keys: &[&str]) -> Result<Vec<SettingMapEntry>> {
        Err(Error::not_supported(
            "no-backend: the settings read is not served on this backend",
        ))
    }

    /// `listByCategory @10` — the category is an OPEN token.
    fn list_by_category(
        &self,
        _category: &str,
        _page: Pagination,
    ) -> Result<Page<SettingMapEntry>> {
        Err(Error::not_supported(
            "no-backend: the category listing is not served on this backend",
        ))
    }

    /// `setAppOverride @11`.
    fn set_app_override(
        &self,
        _app_id: AppId,
        _key: &str,
        _value: &SettingValue,
    ) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: whether app X may override setting Y is an acquisition-time capability \
             decision, not a parameter this service re-decides per call",
        ))
    }

    /// `getAppOverride @12` — `exists`, because "not set" is an answer.
    fn get_app_override(&self, _app_id: AppId, _key: &str) -> Result<(SettingValue, bool)> {
        Err(Error::not_supported(
            "no-backend: the app-override read is not served on this backend",
        ))
    }

    /// `clearAppOverride @13`.
    fn clear_app_override(&self, _app_id: AppId, _key: &str) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: the same acquisition-time capability limit as setAppOverride",
        ))
    }

    /// `getVersion @14`.
    fn get_version(&self, _domain: &str, _key: &str) -> Result<u64> {
        Err(Error::not_supported(
            "no-backend: the settings version is not served on this backend",
        ))
    }

    /// `setIfUnchanged @15` — optimistic concurrency.
    fn set_if_unchanged(
        &self,
        _domain: &str,
        _key: &str,
        _value: &SettingValue,
        _expected_version: u64,
    ) -> Result<()> {
        Err(Error::with_message(
            crate::core::ErrorCode::NotSupported,
            "no-privilege: the same gated write as @set",
        ))
    }

    /// `getHistory @16`.
    fn get_history(&self, _domain: &str, _key: &str, _page: Pagination) -> Result<Page<SettingChange>> {
        Err(Error::not_supported(
            "no-backend: the settings history is not served on this backend",
        ))
    }

    /// `getSettingsDiff @17` — the delta read instead of a full-state poll.
    fn get_settings_diff(&self, _domain: &str, _since_version: u64) -> Result<(Vec<SettingChange>, u64)> {
        Err(Error::not_supported(
            "no-backend: the settings diff is not served on this backend",
        ))
    }

    /// `SettingsSubscription.pull @0`.
    fn pull_settings(
        &self,
        _subscription: SettingsSubscription,
        _max_events: u32,
    ) -> Result<Vec<SettingChange>> {
        Err(Error::not_supported(
            "no-backend: settings subscriptions are not served on this backend",
        ))
    }

    /// `SettingsSubscription.cancel @1`.
    fn cancel_settings(&self, _subscription: SettingsSubscription) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: settings subscriptions are not served on this backend",
        ))
    }

    /// The host's own schema table for one domain — the read that makes every
    /// value answer honest about its type, default and range.
    fn describe(&self, _domain: &str) -> Result<Option<SchemaInfo>> {
        Err(Error::not_supported(
            "no-backend: the schema read is not served on this backend",
        ))
    }

    /// The host's own schema source as a domain list (the read behind
    /// `listDomains`).
    fn describe_all(&self) -> Result<Vec<SchemaInfo>> {
        Err(Error::not_supported(
            "no-backend: the schema source read is not served on this backend",
        ))
    }

    /// Publish this backend's honest settings capability tokens (law #6).
    fn settings_caps(&self) -> Vec<&'static str> {
        alloc::vec!["settings.read", "settings.subscribe", "settings.write"]
    }
}

/// A `SettingValue`'s own string rendering, for a CLI.
pub fn render_value(value: &SettingValue) -> String {
    match value {
        SettingValue::String(text) => text.clone(),
        SettingValue::Integer(number) => alloc::format!("{number}"),
        SettingValue::Real(number) => alloc::format!("{number}"),
        SettingValue::Boolean(flag) => alloc::format!("{flag}"),
        SettingValue::Data(bytes) => alloc::format!("<{} bytes>", bytes.len()),
        SettingValue::List(items) => alloc::format!("[{} items]", items.len()),
        SettingValue::Dict(entries) => alloc::format!("{{{} entries}}", entries.len()),
        SettingValue::Unknown => "<unknown>".to_string(),
    }
}
