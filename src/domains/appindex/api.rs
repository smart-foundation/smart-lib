use alloc::string::String;
use alloc::vec::Vec;

use crate::core::Result;

use super::backend::AppindexBackend;
use super::types::{AppEntry, AppSearchFilter, AppStats, Page, Pagination};

/// The appindex domain's facade: the contract's methods, over one
/// object-safe [`AppindexBackend`].
pub struct Appindex<'a, B: AppindexBackend + ?Sized>(&'a B);

impl<'a, B: AppindexBackend + ?Sized> Appindex<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Appindex(backend)
    }

    /// What the backend can do.
    pub fn caps(&self) -> Vec<&'static str> {
        self.0.appindex_caps()
    }

    /// `listApps` — the applications the host publishes.
    pub fn list_apps(&self, page: Pagination) -> Result<Page<AppEntry>> {
        self.0.list_apps(page)
    }

    /// `getApp` — one application by its id.
    pub fn get_app(&self, app_id: &str) -> Result<AppEntry> {
        self.0.get_app(app_id)
    }

    /// `searchApps` — the applications matching a filter.
    pub fn search_apps(
        &self,
        filter: &AppSearchFilter,
        page: Pagination,
    ) -> Result<Page<AppEntry>> {
        self.0.search_apps(filter, page)
    }

    /// `getPinned` — the pinned applications.
    pub fn get_pinned(&self) -> Result<Vec<AppEntry>> {
        self.0.get_pinned()
    }

    /// `pinApp` — pin an application.
    pub fn pin_app(&self, app_id: &str) -> Result<()> {
        self.0.pin_app(app_id)
    }

    /// `unpinApp` — unpin an application.
    pub fn unpin_app(&self, app_id: &str) -> Result<()> {
        self.0.unpin_app(app_id)
    }

    /// `getStats` — the index's statistics.
    pub fn get_stats(&self) -> Result<AppStats> {
        self.0.get_stats()
    }

    /// `batchGet` — several applications by their ids.
    pub fn batch_get(&self, app_ids: &[String]) -> Result<Vec<AppEntry>> {
        self.0.batch_get(app_ids)
    }
}
