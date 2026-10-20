//! The `vcard` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;

use smart_lib::core::Result;

use smart_lib::domains::vcard::types::{
    Contact, ExportRequest, ExportResult, FormatRequest, ImportRequest, ImportResult, SyncResult,
    VCard, VCardVersion,
};
use smart_lib::domains::vcard::VcardBackend;

/// One scripted contact.
pub fn contact(id: u64, name: &str, email: &str) -> Contact {
    Contact {
        id,
        name: smart_lib::domains::vcard::types::PersonName {
            given: name.to_string(),
            family: String::new(),
            middle: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            display: name.to_string(),
        },
        emails: vec![smart_lib::domains::vcard::types::ContactEmail {
            type_: "home".to_string(),
            value: email.to_string(),
            primary: true,
        }],
        ..Contact::default()
    }
}

/// A scripted vCard store: contacts, the parse answer and the format answer.
#[derive(Default)]
pub struct MockVcard {
    contacts: RefCell<Vec<Contact>>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
}

impl MockVcard {
    /// A store holding one contact.
    pub fn seeded() -> Self {
        MockVcard {
            contacts: RefCell::new(vec![contact(1, "Ada Lovelace", "ada@example.org")]),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// An empty store.
    pub fn empty() -> Self {
        MockVcard::default()
    }
}

impl VcardBackend for MockVcard {
    fn import_v_card(&self, _request: ImportRequest) -> Result<ImportResult> {
        self.writes.borrow_mut().push("import".to_string());
        let mut contacts = self.contacts.borrow_mut();
        let contact_id = contacts.len() as u64 + 1;
        contacts.push(contact(contact_id, "Imported Contact", "imported@example.org"));
        Ok(ImportResult {
            contacts: vec![smart_lib::domains::vcard::types::ImportedContact {
                contact_id,
                vcard_uid: "imported-uid".to_string(),
                created: true,
                warnings: Vec::new(),
            }],
            total_processed: 1,
            imported: 1,
            skipped: 0,
            failed: 0,
            warnings: Vec::new(),
            errors: Vec::new(),
        })
    }

    fn export_v_card(&self, _request: ExportRequest) -> Result<ExportResult> {
        let contacts = self.contacts.borrow();
        let mut data = String::new();
        for contact in contacts.iter() {
            data.push_str("BEGIN:VCARD\n");
            data.push_str("VERSION:4.0\n");
            data.push_str(&format!("FN:{}\n", contact.name.display));
            data.push_str("END:VCARD\n");
        }
        Ok(ExportResult {
            data: data.into_bytes(),
            exported: contacts.len() as u32,
            warnings: Vec::new(),
            errors: Vec::new(),
        })
    }

    fn parse_v_card(&self, data: &[u8], _version: VCardVersion) -> Result<Vec<VCard>> {
        let text = String::from_utf8_lossy(data);
        let mut properties = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("BEGIN:") || line.starts_with("END:") {
                continue;
            }
            let (name, value) = line.split_once(':').unwrap_or((line, ""));
            properties.push(smart_lib::domains::vcard::types::VCardProperty {
                name: name.to_uppercase(),
                group: String::new(),
                parameters: Vec::new(),
                values: vec![smart_lib::domains::vcard::types::VCardValue::Text(
                    value.to_string(),
                )],
            });
        }
        Ok(vec![VCard {
            version: VCardVersion::V40,
            properties,
            raw_lines: text.lines().map(str::to_string).collect(),
        }])
    }

    fn format_v_card(&self, request: FormatRequest) -> Result<Vec<u8>> {
        let mut out = String::new();
        out.push_str("BEGIN:VCARD\n");
        out.push_str("VERSION:4.0\n");
        out.push_str(&format!("FN:{}\n", request.contact.name.display));
        for email in &request.contact.emails {
            out.push_str(&format!("EMAIL:{}", email.value));
        }
        out.push_str("END:VCARD\n");
        Ok(out.into_bytes())
    }

    fn get_supported_versions(&self) -> Result<Vec<VCardVersion>> {
        Ok(vec![
            VCardVersion::V21,
            VCardVersion::V30,
            VCardVersion::V40,
        ])
    }

    fn get_sync_token(&self, contact_ids: &[u64]) -> Result<String> {
        Ok(format!("token:{}", contact_ids.len()))
    }

    fn sync_changes(&self, sync_token: &str) -> Result<SyncResult> {
        Ok(SyncResult {
            new_sync_token: format!("{sync_token}:next"),
            changed: Vec::new(),
            deleted: Vec::new(),
            has_more: false,
            conflicts: Vec::new(),
        })
    }
}

/// The shell's store trait over the mock: the engine's own contact read.
impl shell::domains::vcard::VcardStore for MockVcard {
    fn contacts(&self) -> shell::error::SmartResult<Vec<Contact>> {
        Ok(self.contacts.borrow().clone())
    }
}
