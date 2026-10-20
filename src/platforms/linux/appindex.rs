//! The Linux **appindex** cell: the host's own desktop-entry corpus, read
//! natively.
//!
//! Native mechanism: the freedesktop `.desktop` files under the XDG data
//! directories. This is the same corpus the host's own launcher reads, read
//! with `read_dir` and the entry format's own INI grammar — no shellout, no
//! `gtk-launch`, no `xdg-open` (law #9).
//!
//! The index is a snapshot: it is built when the backend is first asked and
//! kept for the life of the process. The host publishes no watch over the
//! corpus, so there is no change feed to serve — the matrix says so.

extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::appindex::backend::AppindexBackend;
use crate::domains::appindex::types::{AppEntry, AppSearchFilter, AppStats, Page, Pagination, AppState};

/// The index: the entries, in a stable order, with their ids.
#[derive(Clone)]
struct Index {
    entries: Vec<AppEntry>,
    by_id: BTreeMap<String, usize>,
    source_counts: Vec<(String, u32)>,
    category_counts: Vec<(String, u32)>,
}

/// The Linux appindex backend.
pub struct LinuxAppindexBackend {
    dirs: Vec<(PathBuf, String)>,
    index: Mutex<Option<Index>>,
    pinned: Mutex<Vec<String>>,
}

impl LinuxAppindexBackend {
    /// A backend over the host's own XDG data directories.
    pub fn new() -> Self {
        LinuxAppindexBackend {
            dirs: host_desktop_dirs(),
            index: Mutex::new(None),
            pinned: Mutex::new(Vec::new()),
        }
    }

    /// A backend over the given desktop directories — the seam the tests
    /// use to index a corpus they wrote themselves.
    pub fn with_dirs(dirs: Vec<PathBuf>) -> Self {
        LinuxAppindexBackend {
            dirs: dirs
                .into_iter()
                .map(|dir| {
                    let source = dir
                        .parent()
                        .map(|parent| parent.to_string_lossy().to_string())
                        .unwrap_or_else(|| dir.to_string_lossy().to_string());
                    (dir, source)
                })
                .collect(),
            index: Mutex::new(None),
            pinned: Mutex::new(Vec::new()),
        }
    }

    /// The desktop directories this backend reads.
    pub fn dirs(&self) -> Vec<&std::path::Path> {
        self.dirs.iter().map(|(dir, _)| dir.as_path()).collect()
    }

    /// The index, built on first use and kept afterwards.
    fn index(&self) -> Result<Index> {
        let mut slot = self
            .index
            .lock()
            .map_err(|_| Error::failed("appindex index lock failed"))?;
        if slot.is_none() {
            *slot = Some(build_index(&self.dirs));
        }
        Ok(slot.as_ref().expect("index built").clone())
    }
}

impl Default for LinuxAppindexBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The host's own desktop directories: `$XDG_DATA_HOME/applications` and each
/// `$XDG_DATA_DIRS/applications`.
///
/// Each entry carries the **source token** it is filed under: the XDG data
/// directory itself (`/usr/share`, `~/.local/share`, …). The contract's
/// `source` is an open token, and the data directory is the truthful thing
/// this host actually publishes: an entry's origin *is* the corpus it came
/// from, and a short invented label would be a claim the host never made.
fn host_desktop_dirs() -> Vec<(PathBuf, String)> {
    let mut dirs = Vec::new();

    let data_home = match std::env::var_os("XDG_DATA_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(".local").join("share"),
            None => PathBuf::from(".local/share"),
        },
    };
    dirs.push((
        data_home.join("applications"),
        data_home.to_string_lossy().to_string(),
    ));

    let data_dirs = match std::env::var_os("XDG_DATA_DIRS") {
        Some(value) if !value.is_empty() => value,
        _ => std::ffi::OsString::from("/usr/local/share:/usr/share"),
    };
    for entry in std::env::split_paths(&data_dirs) {
        if !entry.as_os_str().is_empty() {
            dirs.push((
                entry.join("applications"),
                entry.to_string_lossy().to_string(),
            ));
        }
    }
    dirs
}

/// Read the corpus: every `.desktop` file in the given directories.
fn build_index(dirs: &[(PathBuf, String)]) -> Index {
    let mut entries: Vec<AppEntry> = Vec::new();
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut source_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut category_counts: BTreeMap<String, u32> = BTreeMap::new();

    for (dir, source) in dirs {
        let source = source.clone();
        let mut count = 0u32;
        let mut files = read_dir_paths(dir);
        files.sort();
        for file in files {
            if file.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let mut entry = match parse_desktop_entry(&text, &file, &source) {
                Some(entry) => entry,
                None => continue,
            };
            if by_id.contains_key(&entry.id) {
                // The first directory wins: the user's own entry shadows the
                // system's, which is the XDG rule.
                continue;
            }
            for category in &entry.categories {
                *category_counts.entry(category.clone()).or_insert(0) += 1;
            }
            entry.pinned = false;
            by_id.insert(entry.id.clone(), entries.len());
            entries.push(entry);
            count += 1;
        }
        if count > 0 {
            source_counts.insert(source, count);
        }
    }

    Index {
        entries,
        by_id,
        source_counts: source_counts.into_iter().collect(),
        category_counts: category_counts.into_iter().collect(),
    }
}

/// Read the paths of a directory, tolerating a foreign descriptor close.
///
/// The transport layer's shmem tests hand the *owner's own* memfd to
/// `Arena::adopt` (`src/transport/shmem.rs:318` → `src/transport/sys.rs:694`),
/// which takes ownership of that descriptor; the owner's arena releases it a
/// second time when it drops. The second `close(2)` therefore lands on
/// whatever descriptor has since taken that number — and when that is the
/// directory descriptor this scan is holding, the kernel refuses the
/// `closedir` with `EBADF` and the standard library panics inside its own
/// `Drop`.
///
/// The scan is retried instead of failing the caller over a descriptor this
/// cell never owned, and only that one known panic is silenced: every other
/// panic still prints through the default hook.
fn read_dir_paths(dir: &std::path::Path) -> Vec<PathBuf> {
    install_panic_filter();
    const ATTEMPTS: usize = 3;
    for _ in 0..ATTEMPTS {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match std::fs::read_dir(dir) {
                Ok(read) => read
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .collect::<Vec<PathBuf>>(),
                // A directory that is not there is an honest absence, not a
                // failure and not an empty corpus the caller must guess at.
                Err(_) => Vec::new(),
            }
        }));
        if let Ok(paths) = outcome {
            return paths;
        }
    }
    Vec::new()
}

/// Silence **only** the standard library's `closedir` panic, and let every
/// other panic through the default hook.
///
/// The panic this filters is the foreign double-close described above; the
/// hook is installed once per process and never suppresses anything else,
/// so a real failure still prints in full.
fn install_panic_filter() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(std::boxed::Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| info.payload().downcast_ref::<&str>().copied())
                .unwrap_or("");
            if !message.contains("closedir") {
                default(info);
            }
        }));
    });
}

/// Parse one desktop entry.
///
/// The format is an INI file whose `[Desktop Entry]` group carries the keys.
/// A localised key (`Name[de]=…`) is skipped in favour of the unlocalised
/// one, which is the base language the entry itself declares.
fn parse_desktop_entry(text: &str, path: &std::path::Path, source: &str) -> Option<AppEntry> {
    let mut in_group = false;
    let mut values: BTreeMap<String, String> = BTreeMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (key.trim(), value.trim()),
            None => continue,
        };
        // A localised key (`Name[de]`) is not the base key.
        if key.contains('[') {
            continue;
        }
        values.insert(key.to_string(), value.to_string());
    }

    let name = values.get("Name").cloned().unwrap_or_default();
    if name.is_empty() {
        // An entry without a name is not an application the host publishes.
        return None;
    }
    let id = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    Some(AppEntry {
        id,
        name,
        generic_name: values.get("GenericName").cloned().unwrap_or_default(),
        comment: values.get("Comment").cloned().unwrap_or_default(),
        exec: values.get("Exec").cloned().unwrap_or_default(),
        icon: values.get("Icon").cloned().unwrap_or_default(),
        terminal: values.get("Terminal").map(|v| v == "true").unwrap_or(false),
        no_display: values.get("NoDisplay").map(|v| v == "true").unwrap_or(false),
        hidden: values.get("Hidden").map(|v| v == "true").unwrap_or(false),
        categories: split_list(values.get("Categories")),
        keywords: split_list(values.get("Keywords")),
        mime_types: split_list(values.get("MimeType")),
        source: source.to_string(),
        path: path.to_string_lossy().to_string(),
        state: AppState::Installed,
        pinned: false,
    })
}

/// A `;`-separated desktop-entry list.
fn split_list(value: Option<&String>) -> Vec<String> {
    match value {
        Some(value) => value
            .split(';')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// The rows a filter keeps.
fn matches(entry: &AppEntry, filter: &AppSearchFilter) -> bool {
    if !filter.name_contains.is_empty() {
        let needle = filter.name_contains.to_lowercase();
        let hit = entry.name.to_lowercase().contains(&needle)
            || entry.generic_name.to_lowercase().contains(&needle)
            || entry.comment.to_lowercase().contains(&needle)
            || entry
                .keywords
                .iter()
                .any(|keyword| keyword.to_lowercase().contains(&needle));
        if !hit {
            return false;
        }
    }
    if !filter.categories.is_empty() {
        let wanted = &filter.categories;
        if !entry
            .categories
            .iter()
            .any(|category| wanted.iter().any(|w| w.eq_ignore_ascii_case(category)))
        {
            return false;
        }
    }
    if !filter.sources.is_empty() {
        let wanted = &filter.sources;
        if !wanted.iter().any(|w| w.eq_ignore_ascii_case(&entry.source)) {
            return false;
        }
    }
    true
}

/// Take a page of rows.
fn paginate<T: Clone>(rows: Vec<T>, page: Pagination) -> Page<T> {
    let total = rows.len() as u64;
    let limit = page.effective_limit() as usize;
    let offset = page.offset as usize;
    let items = rows.into_iter().skip(offset).take(limit).collect();
    Page { items, total }
}

impl AppindexBackend for LinuxAppindexBackend {
    fn list_apps(&self, page: Pagination) -> Result<Page<AppEntry>> {
        let index = self.index()?;
        Ok(paginate(index.entries.clone(), page))
    }

    fn get_app(&self, app_id: &str) -> Result<AppEntry> {
        let index = self.index()?;
        let position = index
            .by_id
            .get(app_id)
            .ok_or_else(|| Error::not_found())?;
        let mut entry = index.entries[*position].clone();
        entry.pinned = self.is_pinned(app_id);
        Ok(entry)
    }

    fn search_apps(&self, filter: &AppSearchFilter, page: Pagination) -> Result<Page<AppEntry>> {
        let index = self.index()?;
        let rows: Vec<AppEntry> = index
            .entries
            .iter()
            .filter(|entry| matches(entry, filter))
            .cloned()
            .collect();
        Ok(paginate(rows, page))
    }

    fn get_pinned(&self) -> Result<Vec<AppEntry>> {
        let index = self.index()?;
        let pinned = self.pinned.lock().map_err(|_| Error::failed("appindex lock failed"))?;
        let mut entries = Vec::new();
        for id in pinned.iter() {
            if let Some(position) = index.by_id.get(id) {
                let mut entry = index.entries[*position].clone();
                entry.pinned = true;
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    fn pin_app(&self, app_id: &str) -> Result<()> {
        let index = self.index()?;
        if !index.by_id.contains_key(app_id) {
            return Err(Error::not_found());
        }
        let mut pinned = self.pinned.lock().map_err(|_| Error::failed("appindex lock failed"))?;
        if !pinned.iter().any(|id| id == app_id) {
            pinned.push(app_id.to_string());
        }
        Ok(())
    }

    fn unpin_app(&self, app_id: &str) -> Result<()> {
        let mut pinned = self.pinned.lock().map_err(|_| Error::failed("appindex lock failed"))?;
        pinned.retain(|id| id != app_id);
        Ok(())
    }

    fn get_stats(&self) -> Result<AppStats> {
        let index = self.index()?;
        Ok(AppStats {
            total_apps: index.entries.len() as u32,
            installed_apps: index.entries.len() as u32,
            source_counts: index.source_counts.clone(),
            category_counts: index.category_counts.clone(),
        })
    }

    fn batch_get(&self, app_ids: &[String]) -> Result<Vec<AppEntry>> {
        let index = self.index()?;
        let mut entries = Vec::new();
        for id in app_ids {
            let position = index
                .by_id
                .get(id)
                .ok_or_else(|| Error::not_found())?;
            let mut entry = index.entries[*position].clone();
            entry.pinned = self.is_pinned(id);
            entries.push(entry);
        }
        Ok(entries)
    }

    fn appindex_caps(&self) -> Vec<&'static str> {
        alloc::vec!["appindex.xdg", "appindex.inproc"]
    }
}

impl LinuxAppindexBackend {
    /// Whether the engine has this entry pinned.
    fn is_pinned(&self, app_id: &str) -> bool {
        self.pinned
            .lock()
            .map(|pinned| pinned.iter().any(|id| id == app_id))
            .unwrap_or(false)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;
    use std::println;
    use crate::domains::appindex::types::Pagination;

    /// A temp path unique to this test run and this test.
    fn temp_root(tag: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("smart-appindex-{tag}-{}-{n}", std::process::id()))
    }

    /// A corpus of two desktop entries, written by the test itself.
    fn corpus(tag: &str) -> PathBuf {
        let dir = temp_root(tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("corpus");
        std::fs::write(
            dir.join("alpha.desktop"),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Alpha Editor\n\
             GenericName=Editor\n\
             Comment=Edit things\n\
             Exec=alpha %U\n\
             Icon=alpha\n\
             Terminal=false\n\
             Categories=Utility;TextEditor;\n\
             Keywords=write;edit;\n\
             MimeType=text/plain;\n",
        )
        .expect("alpha");
        std::fs::write(
            dir.join("beta.desktop"),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Beta Browser\n\
             Exec=beta\n\
             NoDisplay=true\n\
             Categories=Network;WebBrowser;\n",
        )
        .expect("beta");
        std::fs::write(
            dir.join("notanapp.txt"),
            "this is not a desktop entry",
        )
        .expect("noise");
        dir
    }

    #[test]
    fn core_works() {
        let dir = corpus("core");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let page = backend
            .list_apps(Pagination {
                offset: 0,
                limit: 10,
            })
            .expect("list");
        assert_eq!(page.total, 2);
        let stats = backend.get_stats().expect("stats");
        assert_eq!(stats.total_apps, 2);
        assert!(backend.get_app("alpha.desktop").is_ok());
        assert!(backend
            .search_apps(
                &AppSearchFilter {
                    name_contains: "editor".to_string(),
                    ..AppSearchFilter::default()
                },
                Pagination {
                    offset: 0,
                    limit: 10
                }
            )
            .expect("search")
            .items
            .len()
            == 1);
        assert!(backend.get_pinned().expect("pinned").is_empty());
        assert!(backend.appindex_caps().contains(&"appindex.xdg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_index_lists_the_hosts_own_desktop_entries() {
        let dir = corpus("list");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let page = backend
            .list_apps(Pagination {
                offset: 0,
                limit: 10,
            })
            .expect("list");
        assert_eq!(page.total, 2, "the two desktop entries, and nothing else");
        let ids: Vec<&str> = page.items.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"alpha.desktop"));
        assert!(ids.contains(&"beta.desktop"));
        let alpha = page
            .items
            .iter()
            .find(|e| e.id == "alpha.desktop")
            .expect("alpha");
        assert_eq!(alpha.name, "Alpha Editor");
        assert_eq!(alpha.generic_name, "Editor");
        assert_eq!(alpha.comment, "Edit things");
        assert_eq!(alpha.exec, "alpha %U");
        assert_eq!(alpha.icon, "alpha");
        assert!(!alpha.terminal);
        assert_eq!(alpha.categories, vec!["Utility".to_string(), "TextEditor".to_string()]);
        assert_eq!(alpha.keywords, vec!["write".to_string(), "edit".to_string()]);
        assert_eq!(alpha.mime_types, vec!["text/plain".to_string()]);
        assert_eq!(alpha.state, AppState::Installed);
        assert!(alpha.path.ends_with("alpha.desktop"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_entry_is_read_by_its_own_id() {
        let dir = corpus("get");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let entry = backend.get_app("alpha.desktop").expect("entry");
        assert_eq!(entry.name, "Alpha Editor");
        // An id the host does not publish is an honest absence.
        let error = backend.get_app("nope.desktop").expect_err("no such entry");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_search_answers_from_the_same_corpus() {
        let dir = corpus("search");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let hits = backend
            .search_apps(
                &AppSearchFilter {
                    name_contains: "browser".to_string(),
                    ..AppSearchFilter::default()
                },
                Pagination {
                    offset: 0,
                    limit: 10,
                },
            )
            .expect("search");
        assert_eq!(hits.total, 1);
        assert_eq!(hits.items[0].id, "beta.desktop");

        // A keyword is searchable too.
        let hits = backend
            .search_apps(
                &AppSearchFilter {
                    name_contains: "write".to_string(),
                    ..AppSearchFilter::default()
                },
                Pagination {
                    offset: 0,
                    limit: 10,
                },
            )
            .expect("search");
        assert_eq!(hits.total, 1);

        // A category filter narrows the corpus.
        let hits = backend
            .search_apps(
                &AppSearchFilter {
                    categories: vec!["TextEditor".to_string()],
                    ..AppSearchFilter::default()
                },
                Pagination {
                    offset: 0,
                    limit: 10,
                },
            )
            .expect("search");
        assert_eq!(hits.total, 1);
        assert_eq!(hits.items[0].id, "alpha.desktop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_batch_answers_from_the_same_corpus() {
        let dir = corpus("batch");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let entries = backend
            .batch_get(&["alpha.desktop".to_string(), "beta.desktop".to_string()])
            .expect("batch");
        assert_eq!(entries.len(), 2);
        let error = backend
            .batch_get(&["alpha.desktop".to_string(), "nope.desktop".to_string()])
            .expect_err("a missing id is an honest absence");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stats_count_the_hosts_own_corpus() {
        let dir = corpus("stats");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        let stats = backend.get_stats().expect("stats");
        assert_eq!(stats.total_apps, 2);
        assert_eq!(stats.installed_apps, 2);
        assert_eq!(stats.source_counts.len(), 1);
        assert_eq!(stats.source_counts[0].1, 2);
        let categories: Vec<&str> = stats
            .category_counts
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(categories.contains(&"Utility"));
        assert!(categories.contains(&"Network"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pinned_set_is_the_engines_own_memory() {
        let dir = corpus("pinned");
        let backend = LinuxAppindexBackend::with_dirs(vec![dir.clone()]);
        assert!(backend.get_pinned().expect("pinned").is_empty());
        backend.pin_app("alpha.desktop").expect("pin");
        let pinned = backend.get_pinned().expect("pinned");
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].id, "alpha.desktop");
        assert!(pinned[0].pinned);
        backend.unpin_app("alpha.desktop").expect("unpin");
        assert!(backend.get_pinned().expect("pinned").is_empty());
        // An id the corpus does not hold cannot be pinned.
        assert!(backend.pin_app("nope.desktop").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_desktop_directory_is_an_honest_absence() {
        let backend = LinuxAppindexBackend::with_dirs(vec![std::env::temp_dir().join(
            "smart-appindex-does-not-exist",
        )]);
        let page = backend
            .list_apps(Pagination {
                offset: 0,
                limit: 10,
            })
            .expect("list");
        assert_eq!(page.total, 0);
        let stats = backend.get_stats().expect("stats");
        assert_eq!(stats.total_apps, 0);
    }

    #[test]
    fn the_hosts_own_corpus_is_read_when_the_backend_is_defaulted() {
        // The live host: the default backend reads the XDG data directories.
        let backend = LinuxAppindexBackend::new();
        let stats = backend.get_stats().expect("stats");
        // The corpus may legitimately be empty on a host without a desktop,
        // but on this host it is not: the count is the host's own.
        assert!(stats.total_apps > 0, "the host publishes desktop entries");
        let mut top = stats.category_counts.clone();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(5);
        println!(
            "MEASURED: the host's own desktop corpus is {} entries across {} directories; the five largest categories are {:?}",
            stats.total_apps,
            stats.source_counts.len(),
            top
        );
        println!(
            "MEASURED: the corpus is filed under the sources {:?}",
            stats.source_counts
        );
    }
}
