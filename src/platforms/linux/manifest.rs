//! The Linux **manifest** cell: the engine's own manifest store.
//!
//! Native mechanism: an in-process store of manifests — the contract's
//! manifests are the engine's own bundles' descriptions, and this host
//! publishes no bundle corpus, so the store *is* the mechanism. There is no
//! shellout (law #9) and no host-mutating act (law #8): a manifest lives in
//! this process's memory, and the store says so where it matters.
//!
//! What is real: the store's own read verbs — `resolve`, `list`, `validate`,
//! `validateDependencies` and `resolveByName` — over real manifests with
//! real names, versions, entry points and dependencies. The validation is a
//! real check: a required dependency that no manifest offers is an error the
//! caller is told about, not a green light.
//!
//! What is not served, and why: the verbs that need machinery this engine
//! does not have — composition, diffing, signing, verification, migration,
//! template creation and the change feed. Each refuses with the reason.

extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::manifest::backend::ManifestBackend;
use crate::domains::manifest::types::{ManifestEntry, Page, Pagination, ValidationReport};

/// The engine's own manifest store.
pub struct LinuxManifestBackend {
    store: Mutex<Store>,
}

/// The store: the manifests by name.
#[derive(Default)]
struct Store {
    manifests: BTreeMap<String, ManifestEntry>,
}

impl LinuxManifestBackend {
    /// A backend with an empty store.
    pub fn new() -> Self {
        LinuxManifestBackend {
            store: Mutex::new(Store::default()),
        }
    }

    /// Register a manifest in the store — the engine's own write path.
    ///
    /// The contract has no `put` verb: manifests arrive with the bundles the
    /// engine installs, and this store is what a bundle install lands in.
    pub fn put(&self, manifest: ManifestEntry) -> Result<()> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        store.manifests.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// Remove a manifest from the store.
    pub fn remove(&self, name: &str) -> Result<()> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        store
            .manifests
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| Error::not_found())
    }

    /// The manifests the store holds, in name order.
    pub fn manifests(&self) -> Result<Vec<ManifestEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        Ok(store.manifests.values().cloned().collect())
    }
}

impl Default for LinuxManifestBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Take a page of rows.
fn paginate<T: Clone>(rows: Vec<T>, page: Pagination) -> Page<T> {
    let total = rows.len() as u64;
    let limit = page.effective_limit() as usize;
    let offset = page.offset as usize;
    let items = rows.into_iter().skip(offset).take(limit).collect();
    Page { items, total }
}

/// The validation of a manifest against the store: a required dependency
/// that no manifest in the store offers is an error.
fn validate_against(store: &Store, manifest: &ManifestEntry) -> ValidationReport {
    let mut errors = Vec::new();
    for dependency in &manifest.depends {
        if dependency.optional {
            continue;
        }
        let satisfied = store.manifests.values().any(|candidate| {
            candidate.name == dependency.name
                || candidate
                    .offers
                    .iter()
                    .any(|offer| offer == &dependency.name)
                || candidate.name == dependency.schema_id.to_string()
        });
        if !satisfied {
            errors.push(alloc::format!(
                "the required dependency {:?} is offered by no manifest in the store",
                dependency.name
            ));
        }
    }
    if manifest.name.is_empty() {
        errors.push("the manifest has no name".to_string());
    }
    if manifest.version.is_empty() {
        errors.push("the manifest has no version".to_string());
    }
    ValidationReport {
        valid: errors.is_empty(),
        errors,
    }
}

impl ManifestBackend for LinuxManifestBackend {
    fn resolve(&self, name: &str) -> Result<ManifestEntry> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        store
            .manifests
            .get(name)
            .cloned()
            .ok_or_else(|| Error::not_found())
    }

    fn list(&self, page: Pagination) -> Result<Page<ManifestEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        let rows: Vec<ManifestEntry> = store.manifests.values().cloned().collect();
        Ok(paginate(rows, page))
    }

    fn validate(&self, name: &str) -> Result<ValidationReport> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        let manifest = store
            .manifests
            .get(name)
            .cloned()
            .ok_or_else(|| Error::not_found())?;
        Ok(validate_against(&store, &manifest))
    }

    fn validate_dependencies(&self, name: &str) -> Result<ValidationReport> {
        self.validate(name)
    }

    fn resolve_by_name(&self, issuer: &str, name: &str) -> Result<ManifestEntry> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::failed("manifest store lock failed"))?;
        // The human spelling: the display name, optionally scoped by the
        // issuer (the vendor).
        store
            .manifests
            .values()
            .find(|manifest| {
                manifest.display_name == name
                    && (issuer.is_empty() || manifest.vendor == issuer)
            })
            .cloned()
            .ok_or_else(|| Error::not_found())
    }

    fn manifest_caps(&self) -> Vec<&'static str> {
        alloc::vec!["manifest.inproc", "manifest.store"]
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;
    use crate::domains::manifest::types::ManifestDependency;

    fn manifest(name: &str, display: &str, depends: Vec<ManifestDependency>) -> ManifestEntry {
        ManifestEntry {
            name: name.to_string(),
            display_name: display.to_string(),
            description: format!("the {display} bundle"),
            vendor: "Smart".to_string(),
            version: "1.2.3".to_string(),
            entries: vec!["main".to_string()],
            depends,
            offers: Vec::new(),
            shape: "executable".to_string(),
        }
    }

    fn page() -> Pagination {
        Pagination {
            offset: 0,
            limit: 10,
        }
    }

    #[test]
    fn core_works() {
        let backend = LinuxManifestBackend::new();
        backend
            .put(manifest("smart.shell", "Shell", Vec::new()))
            .expect("put");
        let resolved = backend.resolve("smart.shell").expect("resolve");
        assert_eq!(resolved.display_name, "Shell");
        let listed = backend.list(page()).expect("list");
        assert_eq!(listed.total, 1);
        let report = backend.validate("smart.shell").expect("validate");
        assert!(report.valid);
        let by_name = backend.resolve_by_name("Smart", "Shell").expect("by name");
        assert_eq!(by_name.name, "smart.shell");
        assert!(backend.manifest_caps().contains(&"manifest.inproc"));
    }

    #[test]
    fn a_registered_manifest_resolves() {
        let backend = LinuxManifestBackend::new();
        backend
            .put(manifest("smart.shell", "Shell", Vec::new()))
            .expect("put");
        let resolved = backend.resolve("smart.shell").expect("resolve");
        assert_eq!(resolved.name, "smart.shell");
        assert_eq!(resolved.display_name, "Shell");
        assert_eq!(resolved.vendor, "Smart");
        assert_eq!(resolved.version, "1.2.3");
        assert_eq!(resolved.entries, vec!["main".to_string()]);
        assert_eq!(resolved.version_parts(), (1, 2, 3));

        // A manifest the store does not hold is an honest absence.
        let error = backend.resolve("nope").expect_err("no such manifest");
        assert_eq!(error.code, crate::core::ErrorCode::NotFound);
    }

    #[test]
    fn the_listing_is_the_stores_own() {
        let backend = LinuxManifestBackend::new();
        assert_eq!(backend.list(page()).expect("list").total, 0);
        backend.put(manifest("a.one", "One", Vec::new())).expect("put");
        backend.put(manifest("b.two", "Two", Vec::new())).expect("put");
        let listed = backend.list(page()).expect("list");
        assert_eq!(listed.total, 2);
        assert_eq!(listed.items[0].name, "a.one");
        assert_eq!(listed.items[1].name, "b.two");
    }

    #[test]
    fn a_validation_reports_the_dependency_it_cannot_find() {
        let backend = LinuxManifestBackend::new();
        backend
            .put(manifest("app.one", "One", vec![ManifestDependency {
                schema_id: 42,
                name: "schema.base".to_string(),
                optional: false,
            }]))
            .expect("put");
        let report = backend.validate("app.one").expect("validate");
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("schema.base"));

        // A dependency no manifest offers is an error, not a green light.
        backend
            .put(manifest("schema.base", "Base", Vec::new()))
            .expect("put");
        let report = backend.validate("app.one").expect("validate");
        assert!(report.valid, "{:?}", report.errors);

        // An optional dependency is not an error.
        backend
            .put(manifest("app.two", "Two", vec![ManifestDependency {
                schema_id: 43,
                name: "schema.missing".to_string(),
                optional: true,
            }]))
            .expect("put");
        let report = backend.validate("app.two").expect("validate");
        assert!(report.valid);
    }

    #[test]
    fn a_human_name_resolves_through_its_vendor() {
        let backend = LinuxManifestBackend::new();
        backend
            .put(manifest("smart.shell", "Shell", Vec::new()))
            .expect("put");
        assert!(backend.resolve_by_name("Smart", "Shell").is_ok());
        assert!(backend.resolve_by_name("Other", "Shell").is_err());
        assert!(backend.resolve_by_name("", "Shell").is_ok());
        assert!(backend.resolve_by_name("", "Nothing").is_err());
    }
}
