use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{ManifestEntry, Page, Pagination, ValidationReport};

pub trait ManifestBackend {
    fn resolve(&self, _name: &str) -> Result<ManifestEntry> {
        Err(Error::not_supported(
            "no-backend: manifest resolve is not served on this backend",
        ))
    }
    fn list(&self, _page: Pagination) -> Result<Page<ManifestEntry>> {
        Err(Error::not_supported(
            "no-backend: manifest list is not served on this backend",
        ))
    }
    fn subscribe(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest subscribe is not served on this backend"))
    }
    fn pull(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest pull is not served on this backend"))
    }
    fn compose(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest compose is not served on this backend"))
    }
    fn diff(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest diff is not served on this backend"))
    }
    fn validate(&self, _name: &str) -> Result<ValidationReport> {
        Err(Error::not_supported(
            "no-backend: manifest validate is not served on this backend",
        ))
    }
    fn sign(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest sign is not served on this backend"))
    }
    fn verify(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest verify is not served on this backend"))
    }
    fn migrate(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest migrate is not served on this backend"))
    }
    fn create_template(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest createTemplate is not served on this backend"))
    }
    fn compose_with_config(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: manifest composeWithConfig is not served on this backend"))
    }
    fn validate_dependencies(&self, _name: &str) -> Result<ValidationReport> {
        Err(Error::not_supported(
            "no-backend: manifest validateDependencies is not served on this backend",
        ))
    }
    fn resolve_by_name(&self, _issuer: &str, _name: &str) -> Result<ManifestEntry> {
        Err(Error::not_supported(
            "no-backend: manifest resolveByName is not served on this backend",
        ))
    }
    fn manifest_caps(&self) -> Vec<&'static str> { alloc::vec!["manifest.inproc"] }
}
