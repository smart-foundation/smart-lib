//! The `appindex` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use smart_lib::core::Result;

use smart_lib::domains::appindex::types::{
    AppEntry, AppSearchFilter, AppState, AppStats, Page, Pagination,
};
use smart_lib::domains::appindex::AppindexBackend;

/// One scripted desktop entry: the fields a `.desktop` file really carries.
pub fn app_entry(id: &str, name: &str, generic: &str, source: &str) -> AppEntry {
    AppEntry {
        id: id.to_string(),
        name: name.to_string(),
        generic_name: generic.to_string(),
        comment: format!("the {name} application"),
        exec: format!("/usr/bin/{id}"),
        icon: format!("{id}-icon"),
        terminal: false,
        no_display: false,
        hidden: false,
        categories: vec!["Utility".to_string()],
        keywords: vec!["demo".to_string()],
        mime_types: vec!["application/octet-stream".to_string()],
        source: source.to_string(),
        path: format!("/usr/share/applications/{id}"),
        state: AppState::Installed,
        pinned: false,
    }
}

/// A scripted corpus of desktop entries and the engine's own pinned set.
#[derive(Default)]
pub struct MockAppindex {
    entries: RefCell<Vec<AppEntry>>,
    pinned: RefCell<Vec<String>>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
}

impl MockAppindex {
    /// A corpus of three entries, the way a host's XDG directory holds them.
    pub fn seeded() -> Self {
        MockAppindex {
            entries: RefCell::new(vec![
                app_entry(
                    "alpha.desktop",
                    "Alpha",
                    "Editor",
                    "/usr/share/applications",
                ),
                app_entry(
                    "beta.desktop",
                    "Beta",
                    "Browser",
                    "/usr/share/applications",
                ),
                app_entry(
                    "gamma.desktop",
                    "Gamma",
                    "Editor",
                    "/usr/local/share/applications",
                ),
            ]),
            pinned: RefCell::new(Vec::new()),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// An empty corpus.
    pub fn empty() -> Self {
        MockAppindex::default()
    }

    fn find(&self, id: &str) -> Option<AppEntry> {
        let mut entry = self
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.id == id)
            .cloned()?;
        entry.pinned = self.pinned.borrow().iter().any(|pinned| pinned == id);
        Some(entry)
    }
}

impl AppindexBackend for MockAppindex {
    fn list_apps(&self, page: Pagination) -> Result<Page<AppEntry>> {
        let entries = self.entries.borrow();
        let total = entries.len() as u64;
        let items: Vec<AppEntry> = entries
            .iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .map(|entry| {
                let mut entry = entry.clone();
                entry.pinned = self.pinned.borrow().iter().any(|pinned| pinned == &entry.id);
                entry
            })
            .collect();
        Ok(Page { items, total })
    }

    fn get_app(&self, app_id: &str) -> Result<AppEntry> {
        self.find(app_id).ok_or_else(smart_lib::core::Error::not_found)
    }

    fn get_pinned(&self) -> Result<Vec<AppEntry>> {
        let pinned = self.pinned.borrow();
        Ok(pinned.iter().filter_map(|id| self.find(id)).collect())
    }

    fn pin_app(&self, app_id: &str) -> Result<()> {
        self.writes.borrow_mut().push(format!("pin {app_id}"));
        if self.find(app_id).is_none() {
            return Err(smart_lib::core::Error::not_found());
        }
        let mut pinned = self.pinned.borrow_mut();
        if !pinned.iter().any(|id| id == app_id) {
            pinned.push(app_id.to_string());
        }
        Ok(())
    }

    fn unpin_app(&self, app_id: &str) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("unpin {app_id}"));
        self.pinned.borrow_mut().retain(|id| id != app_id);
        Ok(())
    }

    fn search_apps(&self, filter: &AppSearchFilter, page: Pagination) -> Result<Page<AppEntry>> {
        let entries = self.entries.borrow();
        let rows: Vec<AppEntry> = entries
            .iter()
            .filter(|entry| {
                if !filter.name_contains.is_empty() {
                    let needle = filter.name_contains.to_lowercase();
                    let hit = entry.name.to_lowercase().contains(&needle)
                        || entry.generic_name.to_lowercase().contains(&needle)
                        || entry.comment.to_lowercase().contains(&needle);
                    if !hit {
                        return false;
                    }
                }
                if !filter.categories.is_empty()
                    && !entry.categories.iter().any(|category| {
                        filter
                            .categories
                            .iter()
                            .any(|w| w.eq_ignore_ascii_case(category))
                    })
                {
                    return false;
                }
                if !filter.sources.is_empty()
                    && !filter
                        .sources
                        .iter()
                        .any(|source| source.eq_ignore_ascii_case(&entry.source))
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        let total = rows.len() as u64;
        let items = rows
            .into_iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .collect();
        Ok(Page { items, total })
    }

    fn get_stats(&self) -> Result<AppStats> {
        let entries = self.entries.borrow();
        let mut source_counts: HashMap<String, u32> = HashMap::new();
        let mut category_counts: HashMap<String, u32> = HashMap::new();
        for entry in entries.iter() {
            *source_counts.entry(entry.source.clone()).or_default() += 1;
            for category in &entry.categories {
                *category_counts.entry(category.clone()).or_default() += 1;
            }
        }
        let mut source_counts: Vec<(String, u32)> = source_counts.into_iter().collect();
        source_counts.sort();
        let mut category_counts: Vec<(String, u32)> = category_counts.into_iter().collect();
        category_counts.sort();
        Ok(AppStats {
            total_apps: entries.len() as u32,
            installed_apps: entries.len() as u32,
            source_counts,
            category_counts,
        })
    }

    fn batch_get(&self, app_ids: &[String]) -> Result<Vec<AppEntry>> {
        app_ids
            .iter()
            .map(|id| self.find(id).ok_or_else(smart_lib::core::Error::not_found))
            .collect()
    }
}
