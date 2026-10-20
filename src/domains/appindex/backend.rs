use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{AppEntry, AppSearchFilter, AppStats, Page, Pagination};

/// The appindex domain's backend surface.
///
/// The read half is served from the host's own desktop-entry corpus (the
/// freedesktop `.desktop` files under the XDG data directories); the write
/// half (`launch`, `pinApp`, …) is either host-mutating — which this library
/// never performs (law #8) — or has no host surface at all, and says so.
pub trait AppindexBackend {
    /// `listApps` — the applications the host publishes.
    fn list_apps(&self, _page: Pagination) -> Result<Page<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex listApps is not served on this backend",
        ))
    }

    /// `getApp` — one application by its id.
    fn get_app(&self, _app_id: &str) -> Result<AppEntry> {
        Err(Error::not_supported(
            "no-backend: appindex getApp is not served on this backend",
        ))
    }

    /// `launch` — start an application. Host-mutating; never performed here.
    fn launch(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex launch is not served on this backend",
        ))
    }

    /// `launchUrl` — hand a URL to an application. Host-mutating.
    fn launch_url(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex launchUrl is not served on this backend",
        ))
    }

    /// `getPinned` — the pinned applications.
    fn get_pinned(&self) -> Result<Vec<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex getPinned is not served on this backend",
        ))
    }

    /// `pinApp` — pin an application.
    fn pin_app(&self, _app_id: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex pinApp is not served on this backend",
        ))
    }

    /// `unpinApp` — unpin an application.
    fn unpin_app(&self, _app_id: &str) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex unpinApp is not served on this backend",
        ))
    }

    /// `searchApps` — the applications matching a filter.
    fn search_apps(&self, _filter: &AppSearchFilter, _page: Pagination) -> Result<Page<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex searchApps is not served on this backend",
        ))
    }

    /// `getRecentApps` — the recently used applications.
    fn get_recent_apps(&self) -> Result<Vec<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex getRecentApps is not served on this backend",
        ))
    }

    /// `clearRecentApps` — forget the recently used applications.
    fn clear_recent_apps(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex clearRecentApps is not served on this backend",
        ))
    }

    /// `subscribe` — watch the index for changes.
    fn subscribe(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex subscribe is not served on this backend",
        ))
    }

    /// `pull` — the buffered changes.
    fn pull(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex pull is not served on this backend",
        ))
    }

    /// `cancel` — end a watch.
    fn cancel(&self) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: appindex cancel is not served on this backend",
        ))
    }

    /// `getStats` — the index's statistics.
    fn get_stats(&self) -> Result<AppStats> {
        Err(Error::not_supported(
            "no-backend: appindex getStats is not served on this backend",
        ))
    }

    /// `listCapabilities` — what an application can do.
    fn list_capabilities(&self) -> Result<Vec<String>> {
        Err(Error::not_supported(
            "no-backend: appindex listCapabilities is not served on this backend",
        ))
    }

    /// `getDependents` — the applications that depend on one.
    fn get_dependents(&self) -> Result<Vec<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex getDependents is not served on this backend",
        ))
    }

    /// `batchGet` — several applications by their ids.
    fn batch_get(&self, _app_ids: &[String]) -> Result<Vec<AppEntry>> {
        Err(Error::not_supported(
            "no-backend: appindex batchGet is not served on this backend",
        ))
    }

    /// What this backend can do.
    fn appindex_caps(&self) -> Vec<&'static str> {
        alloc::vec!["appindex.xdg", "appindex.inproc"]
    }
}
