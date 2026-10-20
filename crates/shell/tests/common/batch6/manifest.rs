//! The `manifest` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use smart_lib::core::{Error, Result};

use smart_lib::domains::manifest::types::{
    ManifestEntry, Page as ManifestPage, Pagination as ManifestPagination, ValidationReport,
};
use smart_lib::domains::manifest::ManifestBackend;

/// One scripted manifest.
pub fn manifest_entry(
    name: &str,
    display: &str,
    vendor: &str,
    depends: Vec<(&str, bool)>,
) -> ManifestEntry {
    ManifestEntry {
        name: name.to_string(),
        display_name: display.to_string(),
        description: format!("the {display} bundle"),
        vendor: vendor.to_string(),
        version: "1.2.3".to_string(),
        entries: vec!["main".to_string()],
        depends: depends
            .into_iter()
            .map(|(dep, optional)| {
                smart_lib::domains::manifest::types::ManifestDependency {
                    schema_id: 0,
                    name: dep.to_string(),
                    optional,
                }
            })
            .collect(),
        offers: vec!["text/plain".to_string()],
        shape: "executable".to_string(),
    }
}

/// A scripted manifest store, with the engine's own validation.
#[derive(Default)]
pub struct MockManifest {
    manifests: RefCell<HashMap<String, ManifestEntry>>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
}

impl MockManifest {
    /// A store holding one manifest that depends on a schema nothing offers.
    pub fn seeded() -> Self {
        let mut manifests = HashMap::new();
        manifests.insert(
            "com.example.app".to_string(),
            manifest_entry(
                "com.example.app",
                "Example App",
                "Example Inc",
                vec![("application/x-example", false)],
            ),
        );
        MockManifest {
            manifests: RefCell::new(manifests),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// An empty store.
    pub fn empty() -> Self {
        MockManifest::default()
    }

    fn validate(&self, name: &str) -> Result<ValidationReport> {
        let manifests = self.manifests.borrow();
        let manifest = manifests.get(name).ok_or_else(Error::not_found)?;
        let mut errors = Vec::new();
        for dependency in &manifest.depends {
            if dependency.optional {
                continue;
            }
            // The ENGINE's own rule: a dependency is satisfied by a manifest
            // that IS named by it, or that OFFERS it.
            let satisfied = manifests.values().any(|other| {
                other.name == dependency.name
                    || other.offers.iter().any(|offer| offer == &dependency.name)
            });
            if !satisfied {
                errors.push(format!(
                    "the required dependency {:?} is offered by no manifest in the store",
                    dependency.name
                ));
            }
        }
        Ok(ValidationReport {
            valid: errors.is_empty(),
            errors,
        })
    }
}

impl ManifestBackend for MockManifest {
    fn resolve(&self, name: &str) -> Result<ManifestEntry> {
        self.manifests
            .borrow()
            .get(name)
            .cloned()
            .ok_or_else(Error::not_found)
    }

    fn list(&self, page: ManifestPagination) -> Result<ManifestPage<ManifestEntry>> {
        let manifests = self.manifests.borrow();
        let mut rows: Vec<ManifestEntry> = manifests.values().cloned().collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let total = rows.len() as u64;
        let items = rows
            .into_iter()
            .skip(page.offset as usize)
            .take(page.effective_limit() as usize)
            .collect();
        Ok(ManifestPage { items, total })
    }

    fn validate(&self, name: &str) -> Result<ValidationReport> {
        self.validate(name)
    }

    fn validate_dependencies(&self, name: &str) -> Result<ValidationReport> {
        self.validate(name)
    }

    fn resolve_by_name(&self, issuer: &str, name: &str) -> Result<ManifestEntry> {
        self.manifests
            .borrow()
            .values()
            .find(|manifest| {
                manifest.display_name == name && (issuer.is_empty() || manifest.vendor == issuer)
            })
            .cloned()
            .ok_or_else(Error::not_found)
    }
}

/// The shell's store trait over the mock: the two write verbs the engine's own
/// cell exposes as inherent methods.
impl shell::domains::manifest::ManifestStore for MockManifest {
    fn register(&self, manifest: ManifestEntry) -> shell::error::SmartResult<()> {
        self.writes
            .borrow_mut()
            .push(format!("register {}", manifest.name));
        self.manifests
            .borrow_mut()
            .insert(manifest.name.clone(), manifest);
        Ok(())
    }

    fn unregister(&self, name: &str) -> shell::error::SmartResult<()> {
        self.writes.borrow_mut().push(format!("unregister {name}"));
        self.manifests
            .borrow_mut()
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| shell::error::SmartError::NotFound(format!("no manifest named {name}")))
    }
}
