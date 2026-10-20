use alloc::vec::Vec;

use crate::core::Result;

use super::backend::ManifestBackend;
use super::types::{ManifestEntry, Page, Pagination, ValidationReport};

/// The manifest domain's facade: the contract's methods, over one
/// object-safe [`ManifestBackend`].
pub struct Manifest<'a, B: ManifestBackend + ?Sized>(&'a B);

impl<'a, B: ManifestBackend + ?Sized> Manifest<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Manifest(backend)
    }

    /// What the backend can do.
    pub fn caps(&self) -> Vec<&'static str> {
        self.0.manifest_caps()
    }

    /// `resolve` — the manifest with that name.
    pub fn resolve(&self, name: &str) -> Result<ManifestEntry> {
        self.0.resolve(name)
    }

    /// `list` — the manifests the store holds.
    pub fn list(&self, page: Pagination) -> Result<Page<ManifestEntry>> {
        self.0.list(page)
    }

    /// `validate` — the manifest's own validation.
    pub fn validate(&self, name: &str) -> Result<ValidationReport> {
        self.0.validate(name)
    }

    /// `validateDependencies` — the manifest's dependencies, checked.
    pub fn validate_dependencies(&self, name: &str) -> Result<ValidationReport> {
        self.0.validate_dependencies(name)
    }

    /// `resolveByName` — the manifest with that human spelling.
    pub fn resolve_by_name(&self, issuer: &str, name: &str) -> Result<ManifestEntry> {
        self.0.resolve_by_name(issuer, name)
    }
}
