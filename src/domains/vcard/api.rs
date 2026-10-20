use alloc::string::String;
use alloc::vec::Vec;

use crate::core::Result;

use super::backend::VcardBackend;
use super::types::{ExportRequest, ExportResult, FormatRequest, ImportRequest, ImportResult, SyncResult, VCard, VCardVersion};

/// The vCard domain's facade: the contract's eight methods, over one
/// object-safe [`VcardBackend`].
///
/// The facade owns no state — it is a thin, typed view of the backend, so a
/// caller holds `Vcard<'_, dyn VcardBackend>` and never names a platform.
pub struct Vcard<'a, B: VcardBackend + ?Sized>(&'a B);

impl<'a, B: VcardBackend + ?Sized> Vcard<'a, B> {
    /// Wrap a backend.
    pub fn new(backend: &'a B) -> Self {
        Vcard(backend)
    }

    /// What the backend can do.
    pub fn caps(&self) -> Vec<&'static str> {
        self.0.vcard_caps()
    }

    /// `importVCard` — import vCard data as contacts.
    pub fn import(&self, request: ImportRequest) -> Result<ImportResult> {
        self.0.import_v_card(request)
    }

    /// `exportVCard` — write contacts as vCard data.
    pub fn export(&self, request: ExportRequest) -> Result<ExportResult> {
        self.0.export_v_card(request)
    }

    /// `parseVCard` — parse vCard data without importing it.
    pub fn parse(&self, data: &[u8], version: VCardVersion) -> Result<Vec<VCard>> {
        self.0.parse_v_card(data, version)
    }

    /// `formatVCard` — write one contact as vCard data.
    pub fn format(&self, request: FormatRequest) -> Result<Vec<u8>> {
        self.0.format_v_card(request)
    }

    /// `getSupportedVersions` — the versions this backend reads and writes.
    pub fn supported_versions(&self) -> Result<Vec<VCardVersion>> {
        self.0.get_supported_versions()
    }

    /// `getSyncToken` — a token naming the store's current state.
    pub fn sync_token(&self, contact_ids: &[u64]) -> Result<String> {
        self.0.get_sync_token(contact_ids)
    }

    /// `syncChanges` — the cards that changed since the caller's token.
    pub fn sync_changes(&self, sync_token: &str) -> Result<SyncResult> {
        self.0.sync_changes(sync_token)
    }

    /// `resolveConflict` — resolve a detected sync conflict.
    pub fn resolve_conflict(
        &self,
        conflict_id: &str,
        resolution: super::backend::ConflictResolution,
    ) -> Result<()> {
        self.0.resolve_conflict(conflict_id, resolution)
    }
}
