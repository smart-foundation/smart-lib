//! The public settings API: a thin, borrowing wrapper over [`SettingsBackend`].
//! Borrowed and zero-cost — it adds no state over `&B`.

use alloc::vec::Vec;

use crate::core::Result;

use super::backend::{SettingsBackend, SettingsSubscription};
use super::types::{
    Page, Pagination, SchemaInfo, SettingChange, SettingResult, SettingValue,
};

/// A borrowing handle to any [`SettingsBackend`].
pub struct Settings<'a, B: SettingsBackend + ?Sized>(&'a B);

impl<'a, B: SettingsBackend + ?Sized> Settings<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Settings(backend)
    }

    /// Read one key. The contract's own union, so a missing key is an answer.
    #[must_use = "the reading is the answer; ignoring it is never useful"]
    pub fn value(&self, domain: &str, key: &str) -> Result<SettingResult> {
        self.0.get(domain, key)
    }

    /// Read one key, resolving the contract's union into the value or the
    /// contract's own `notSet` marker. A consumer that wants to paint a default
    /// uses this; a consumer that wants to distinguish uses
    /// [`Settings::value`].
    #[must_use]
    pub fn value_or_not_set(&self, domain: &str, key: &str) -> Result<Option<SettingValue>> {
        Ok(self.0.get(domain, key)?.value().cloned())
    }

    /// The host's own schema for one domain: its keys, their types, defaults
    /// and ranges.
    #[must_use]
    pub fn describe(&self, domain: &str) -> Result<Option<SchemaInfo>> {
        self.0.describe(domain)
    }

    /// The host's whole schema source.
    #[must_use]
    pub fn describe_all(&self) -> Result<Vec<SchemaInfo>> {
        self.0.describe_all()
    }

    /// The first page of one domain's settings.
    #[must_use]
    pub fn all(&self, domain: &str, page: Pagination) -> Result<Page<super::types::SettingMapEntry>> {
        let map = self.0.get_all(domain, page)?;
        Ok(Page {
            total: map.entries.len() as u64,
            items: map.entries,
        })
    }

    /// Watch one key and pull the first batch of changes.
    #[must_use]
    pub fn watch(&self, domain: &str, key: &str) -> Result<(SettingsSubscription, Vec<SettingChange>)> {
        let subscription = self.0.watch(domain, key)?;
        let first = self.0.pull_settings(subscription, 16)?;
        Ok((subscription, first))
    }

    /// Retire a subscription obtained from [`Settings::watch`].
    pub fn close(&self, subscription: SettingsSubscription) -> Result<()> {
        self.0.cancel_settings(subscription)
    }

    /// The delta read: everything after one version, plus the version now.
    #[must_use]
    pub fn changes_since(&self, domain: &str, version: u64) -> Result<(Vec<SettingChange>, u64)> {
        self.0.get_settings_diff(domain, version)
    }

    /// The backend's published capability tokens (law #6).
    #[must_use]
    pub fn caps(&self) -> Vec<&'static str> {
        self.0.settings_caps()
    }
}
