//! The **vcard** domain: vCard import/export for contacts interoperability
//! (RFC 6350) — over one object-safe [`VcardBackend`] trait.
//!
//! Native mechanism: the engine's own in-process RFC 6350 parser and
//! formatter. There is no `libvcard` on this host and no shellout (law #9):
//! the conversion is the engine's own code, and the contact store it imports
//! into is the engine's own memory.
//!
//! The contract's normative mapping table — the one in `vcard.capnp`'s header,
//! which declares what `VCardContact` becomes in a `Contact` — is implemented
//! row by row in `crate::platforms::linux::vcard`, and the rows that the
//! canonical tree cannot carry (the PO-box and extended-address components of
//! `ADR`, which the table does not name, and `VCardContact.title`, whose
//! destination field `Contact.organization.title` does not exist in
//! `T.OrganizationName`) are **reported as warnings**, never dropped
//! silently.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Vcard;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, MethodAvailability,
    ReasonClass, VCARD_AVAILABILITY, VCARD_SCHEMA_ID,
};
pub use backend::{ConflictResolution, VcardBackend};
pub use types::{
    Contact, ContactAddress, ContactDate, ContactEmail, ContactOrganization, ContactPhone,
    ContactPhoto, DuplicateHandling, ExportRequest, ExportResult, FormatRequest, ImportError,
    ImportRequest, ImportResult, ImportedContact, OutputFormat, Page, Pagination, PersonName,
    SubscriptionId, SyncConflict, SyncResult, VCard, VCardParameter, VCardProperty, VCardValue,
    VCardVersion,
};
