use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{
    ExportRequest, ExportResult, FormatRequest, ImportRequest, ImportResult, SyncResult, VCard,
    VCardVersion,
};

/// The conflict resolution the contract's `resolveConflict` carries.
///
/// The canonical tree made `vcard.capnp` use **calendar's** `ConflictResolution`
/// instead of declaring a second enum with the same name, so this is the
/// generated calendar type — not a local copy.
pub type ConflictResolution = crate::schema::calendar_capnp::ConflictResolution;

/// The vCard domain's backend surface: the eight methods `vcard.capnp`'s
/// `VCardService` declares, one per method, each with its own honest default
/// (`Err(Error::not_supported(…))`) so a backend that cannot serve a method
/// says so instead of fabricating a value (law #6).
///
/// The trait is object-safe: it takes no generics and returns owned values,
/// so a platform backend is held as `&dyn VcardBackend`.
pub trait VcardBackend {
    /// `importVCard` — parse vCard data and import it as contacts.
    fn import_v_card(&self, _request: ImportRequest) -> Result<ImportResult> {
        Err(Error::not_supported(
            "no-backend: vcard importVCard is not served on this backend",
        ))
    }

    /// `exportVCard` — write contacts as vCard data.
    fn export_v_card(&self, _request: ExportRequest) -> Result<ExportResult> {
        Err(Error::not_supported(
            "no-backend: vcard exportVCard is not served on this backend",
        ))
    }

    /// `parseVCard` — parse vCard data without importing it.
    fn parse_v_card(&self, _data: &[u8], _version: VCardVersion) -> Result<Vec<VCard>> {
        Err(Error::not_supported(
            "no-backend: vcard parseVCard is not served on this backend",
        ))
    }

    /// `formatVCard` — write one contact as vCard data.
    fn format_v_card(&self, _request: FormatRequest) -> Result<Vec<u8>> {
        Err(Error::not_supported(
            "no-backend: vcard formatVCard is not served on this backend",
        ))
    }

    /// `getSupportedVersions` — the versions this backend can actually read
    /// and write.
    fn get_supported_versions(&self) -> Result<Vec<VCardVersion>> {
        Err(Error::not_supported(
            "no-backend: vcard getSupportedVersions is not served on this backend",
        ))
    }

    /// `getSyncToken` — a token that names the store's current state.
    fn get_sync_token(&self, _contact_ids: &[u64]) -> Result<String> {
        Err(Error::not_supported(
            "no-backend: vcard getSyncToken is not served on this backend",
        ))
    }

    /// `syncChanges` — the cards that changed since the caller's token.
    fn sync_changes(&self, _sync_token: &str) -> Result<SyncResult> {
        Err(Error::not_supported(
            "no-backend: vcard syncChanges is not served on this backend",
        ))
    }

    /// `resolveConflict` — resolve a detected sync conflict.
    fn resolve_conflict(&self, _conflict_id: &str, _resolution: ConflictResolution) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: vcard resolveConflict is not served on this backend",
        ))
    }

    /// What this backend can do — the caps the contract's `caps()` publishes.
    fn vcard_caps(&self) -> Vec<&'static str> {
        alloc::vec!["vcard.inproc"]
    }
}
