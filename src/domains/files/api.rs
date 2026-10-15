//! The public files API: a thin, borrowing wrapper over [`FilesBackend`]
//! exposing the handful of calls a client actually touches, plus one combined
//! metadata view. Borrowed and zero-cost — it adds no state over `&B`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::core::Result;

use super::backend::{FilesBackend, WatchSubscription};
use super::types::{
    ComparisonResult, DirectoryEntry, DirectoryFilter, FileEvent, FileInfo, FileInfoResult, Page,
    Pagination, TextResult,
};

/// The combined view the CLI's `files` renders for one path: what the path is,
/// whether it exists, and how much room its filesystem has — three calls a
/// caller would otherwise repeat, and no extra concept to learn.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSummary {
    /// What the path is (`statFile`).
    pub info: FileInfo,
    /// Whether the path exists at all.
    pub exists: bool,
    /// The path's own resolved target (empty when it is not a symlink).
    pub resolved: String,
}

/// A borrowing handle to any [`FilesBackend`].
pub struct Files<'a, B: FilesBackend + ?Sized>(&'a B);

impl<'a, B: FilesBackend + ?Sized> Files<'a, B> {
    /// Wrap a backend (which the caller has already acquired authority to hold
    /// — law #5: the handle is the authority).
    pub fn new(backend: &'a B) -> Self {
        Files(backend)
    }

    /// The combined metadata view for one path.
    #[must_use = "the reading is the answer; ignoring it is never useful"]
    pub fn summary(&self, path: &str) -> Result<PathSummary> {
        let exists = self.0.file_exists(path)?;
        let info = match self.0.stat_file(path)? {
            FileInfoResult::Info(info) => info,
            FileInfoResult::Error(error) => return Err(error),
            FileInfoResult::Unknown => return Err(crate::core::Error::unknown(
                "statFile answered the contract's unknown union arm",
            )),
        };
        Ok(PathSummary {
            info,
            exists,
            resolved: String::new(),
        })
    }

    /// The first page of a directory listing under the caller's filter.
    #[must_use]
    pub fn list(&self, path: &str, filter: &DirectoryFilter) -> Result<Page<DirectoryEntry>> {
        self.0.list_directory(path, filter)
    }

    /// Stat one path.
    #[must_use]
    pub fn stat(&self, path: &str) -> Result<FileInfoResult> {
        self.0.stat_file(path)
    }

    /// Resolve a symlink to its own target.
    #[must_use]
    pub fn resolve(&self, path: &str) -> Result<TextResult> {
        self.0.resolve_symlink(path)
    }

    /// Compare two files (identity plus the structured difference).
    #[must_use]
    pub fn compare(&self, path1: &str, path2: &str) -> Result<ComparisonResult> {
        self.0.compare_files(path1, path2)
    }

    /// Watch a directory, pull the kernel's first batch, and return both the
    /// token and the events.
    #[must_use]
    pub fn watch(&self, path: &str) -> Result<(WatchSubscription, Vec<FileEvent>)> {
        let subscription = self.0.watch_directory(path)?;
        let first = self.0.pull_watch(subscription, 32)?;
        Ok((subscription, first))
    }

    /// Retire a watch subscription obtained from [`Files::watch`].
    pub fn close_watch(&self, subscription: WatchSubscription) -> Result<()> {
        self.0.cancel_watch(subscription)
    }

    /// The first page of a tree walk.
    #[must_use]
    pub fn walk(&self, path: &str, max_depth: u32, page: Pagination) -> Result<Page<DirectoryEntry>> {
        self.0.walk_tree(path, max_depth, page)
    }

    /// The backend's published capability tokens (law #6).
    #[must_use]
    pub fn caps(&self) -> alloc::vec::Vec<&'static str> {
        self.0.files_caps()
    }
}
